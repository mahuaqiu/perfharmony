//! P0 采样循环。
//!
//! 采集内核只负责 HDC、解析和同构 Sample，不负责 HTTP 上报。Harmony PC 已
//! 冻结批量 top 与进程内存表格式；Mobile 在获得真机证据前仍由调用方关闭 TopN。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use regex::Regex;

use crate::collector::{
    optional_command, split_optional_output, CollectContext, Collector, CpuFreqCollector,
    DiskIoCounters, GpuCollector, NetworkCollector, OptionalState, PowerCollector,
    ThermalCollector,
};
use crate::data::{
    AggregatedProcessInfo, ProcessFilter, ProcessInfo, Sample, SensorValue, SystemMetrics,
};
use crate::hdc::HdcClient;
use crate::parse::{
    diff_cpu_percent, parse_hidumper_cpuusage, parse_hidumper_mem_processes,
    parse_hidumper_mem_system, parse_proc_meminfo, parse_proc_stat, parse_ps_ef, CpuStatSnapshot,
    ProcessMemSnapshot,
};
use crate::ring_buffer::RingBuffer;

const SAMPLE_BUFFER_CAPACITY: usize = 1024;
const SHELL_TIMEOUT_SECS: u64 = 15;

pub(crate) trait ShellClient: Send + Sync {
    fn shell_with_cancel(
        &self,
        udid: &str,
        command: &str,
        timeout_secs: u64,
        cancel: &AtomicBool,
    ) -> Result<String, crate::error::Error>;
}

impl ShellClient for HdcClient {
    fn shell_with_cancel(
        &self,
        udid: &str,
        command: &str,
        timeout_secs: u64,
        cancel: &AtomicBool,
    ) -> Result<String, crate::error::Error> {
        HdcClient::shell_with_cancel(self, udid, command, timeout_secs, cancel)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct MonitorConfigOwned {
    pub udid: String,
    pub hdc_path: Option<String>,
    pub interval: f64,
    pub duration: Option<f64>,
    pub process_filter: Option<ProcessFilter>,
    pub top_n_cpu: Option<usize>,
    pub top_n_gpu: Option<usize>,
    pub enable_aggregation: bool,
    pub enable_system_cpu: bool,
    pub enable_system_mem: bool,
    pub enable_process: bool,
    pub enable_gpu: bool,
    pub enable_network: bool,
    pub enable_thermal_power: bool,
    pub enable_cpu_freq: bool,
    pub enable_disk_io: bool,
}

#[derive(Debug)]
struct MonitorState {
    running: bool,
    sequence: u64,
    samples: RingBuffer<Sample>,
    last_error: Option<String>,
}

#[derive(Debug)]
pub(crate) struct MonitorInner {
    stop: AtomicBool,
    state: Mutex<MonitorState>,
}

#[derive(Debug, Clone)]
pub(crate) struct MonitorResultInner {
    pub samples: Vec<Sample>,
}

impl MonitorInner {
    pub(crate) fn new() -> Self {
        Self {
            stop: AtomicBool::new(false),
            state: Mutex::new(MonitorState {
                running: false,
                sequence: 0,
                samples: RingBuffer::new(SAMPLE_BUFFER_CAPACITY),
                last_error: None,
            }),
        }
    }

    pub(crate) fn reset(&self) {
        self.stop.store(false, Ordering::Relaxed);
        let mut state = self.state.lock().expect("Monitor state mutex poisoned");
        state.running = false;
        state.sequence = 0;
        state.samples = RingBuffer::new(SAMPLE_BUFFER_CAPACITY);
        state.last_error = None;
    }

    pub(crate) fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let mut state = self.state.lock().expect("Monitor state mutex poisoned");
        state.running = false;
    }

    pub(crate) fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    pub(crate) fn is_running(&self) -> bool {
        self.state
            .lock()
            .expect("Monitor state mutex poisoned")
            .running
    }

    pub(crate) fn mark_running(&self) {
        self.state
            .lock()
            .expect("Monitor state mutex poisoned")
            .running = true;
    }

    pub(crate) fn finish(&self) {
        self.state
            .lock()
            .expect("Monitor state mutex poisoned")
            .running = false;
    }

    pub(crate) fn push(&self, sample: Sample) {
        let mut state = self.state.lock().expect("Monitor state mutex poisoned");
        state.sequence = sample.sequence;
        state.samples.push(sample);
    }

    pub(crate) fn next_sequence(&self) -> u64 {
        self.state
            .lock()
            .expect("Monitor state mutex poisoned")
            .sequence
            + 1
    }

    pub(crate) fn set_error(&self, message: String) {
        self.state
            .lock()
            .expect("Monitor state mutex poisoned")
            .last_error = Some(message);
    }

    pub(crate) fn last_error(&self) -> Option<String> {
        self.state
            .lock()
            .expect("Monitor state mutex poisoned")
            .last_error
            .clone()
    }

    pub(crate) fn buffer_len(&self) -> usize {
        self.state
            .lock()
            .expect("Monitor state mutex poisoned")
            .samples
            .len()
    }

    pub(crate) fn drain(&self) -> MonitorResultInner {
        MonitorResultInner {
            samples: self
                .state
                .lock()
                .expect("Monitor state mutex poisoned")
                .samples
                .drain(),
        }
    }
}

pub(crate) fn run_monitor<C: ShellClient>(
    config: MonitorConfigOwned,
    client: C,
    inner: Arc<MonitorInner>,
) {
    inner.mark_running();
    let started = Instant::now();
    let interval = Duration::from_secs_f64(config.interval);
    let mut previous_cpu: Option<CpuStatSnapshot> = None;
    let mut optional_state = OptionalState::default();
    let mut previous_disk: HashMap<u32, DiskIoCounters> = HashMap::new();
    let mut consecutive_empty_p0 = 0u32;

    while !inner.is_stopped() {
        if config
            .duration
            .is_some_and(|duration| started.elapsed().as_secs_f64() >= duration)
        {
            break;
        }

        let round_started = Instant::now();
        match collect_sample_with_optional(
            &config,
            &client,
            &inner,
            &mut previous_cpu,
            &mut optional_state,
            &mut previous_disk,
            started.elapsed(),
        ) {
            Ok(Some(sample)) => {
                consecutive_empty_p0 = 0;
                inner.push(sample);
            }
            Ok(None) => {
                consecutive_empty_p0 += 1;
                if consecutive_empty_p0 >= 3 {
                    inner.set_error("连续 3 轮未取得任何 P0 指标".to_string());
                    break;
                }
            }
            Err(error) => {
                if !inner.is_stopped() {
                    consecutive_empty_p0 += 1;
                    inner.set_error(error.clone());
                    if consecutive_empty_p0 >= 3 {
                        break;
                    }
                }
            }
        }

        if inner.is_stopped() {
            break;
        }
        let cost = round_started.elapsed();
        if cost < interval {
            let remaining = interval - cost;
            let deadline = Instant::now() + remaining;
            while Instant::now() < deadline && !inner.is_stopped() {
                thread::sleep(
                    Duration::from_millis(20)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
        }
    }
    inner.finish();
}

fn collect_sample<C: ShellClient>(
    config: &MonitorConfigOwned,
    client: &C,
    inner: &MonitorInner,
    previous_cpu: &mut Option<CpuStatSnapshot>,
    elapsed: Duration,
) -> Result<Option<Sample>, String> {
    let mut optional_state = OptionalState::default();
    let mut previous_disk = HashMap::new();
    collect_sample_with_optional(
        config,
        client,
        inner,
        previous_cpu,
        &mut optional_state,
        &mut previous_disk,
        elapsed,
    )
}

fn collect_sample_with_optional<C: ShellClient>(
    config: &MonitorConfigOwned,
    client: &C,
    inner: &MonitorInner,
    previous_cpu: &mut Option<CpuStatSnapshot>,
    optional_state: &mut OptionalState,
    previous_disk: &mut HashMap<u32, DiskIoCounters>,
    elapsed: Duration,
) -> Result<Option<Sample>, String> {
    let mut system = SystemMetrics::default();
    let mut raw = HashMap::new();
    let mut p0_count = 0usize;
    let mut cpu_text = None;
    let mut mem_text = None;
    let mut cpu_fallback_percent = None;
    let needs_process_snapshot =
        config.enable_process && (config.process_filter.is_some() || config.top_n_cpu.is_some());

    // 真实 hidumper 输出冻结前先保留 /proc fallback；单轮正常路径最多 4 次 shell。
    if config.enable_system_cpu {
        match client.shell_with_cancel(
            &config.udid,
            "hidumper --cpuusage",
            SHELL_TIMEOUT_SECS,
            &inner.stop,
        ) {
            Ok(text) => cpu_text = Some(text),
            Err(error) if !matches!(error, crate::error::Error::Cancelled(_)) => {
                match client.shell_with_cancel(
                    &config.udid,
                    "cat /proc/stat",
                    SHELL_TIMEOUT_SECS,
                    &inner.stop,
                ) {
                    Ok(text) => {
                        if let Ok(current) = parse_proc_stat(&text) {
                            if let Some(previous) = previous_cpu {
                                cpu_fallback_percent =
                                    diff_cpu_percent(previous, &current).map(clamp_percent);
                                system.cpu_percent = cpu_fallback_percent;
                            }
                            *previous_cpu = Some(current);
                        }
                    }
                    Err(fallback_error)
                        if !matches!(fallback_error, crate::error::Error::Cancelled(_)) =>
                    {
                        log::debug!("系统 CPU 采集失败: {fallback_error}");
                    }
                    Err(_) => return Ok(None),
                }
            }
            Err(_) => return Ok(None),
        }
        if let Some(text) = cpu_text {
            if let Ok(cpu) = parse_hidumper_cpuusage(&text) {
                system.cpu_percent = Some(clamp_percent(cpu.total_percent));
                add_sensor(&mut raw, "Harmony CPU Usage", cpu.total_percent, "%");
                if let Some(value) = cpu.user_percent {
                    add_sensor(&mut raw, "Harmony CPU User", clamp_percent(value), "%");
                }
                if let Some(value) = cpu.system_percent {
                    add_sensor(&mut raw, "Harmony CPU System", clamp_percent(value), "%");
                }
                if let Some(value) = cpu.idle_percent {
                    add_sensor(&mut raw, "Harmony CPU Idle", clamp_percent(value), "%");
                }
                p0_count += 1;
            } else if let Ok(text) = client.shell_with_cancel(
                &config.udid,
                "cat /proc/stat",
                SHELL_TIMEOUT_SECS,
                &inner.stop,
            ) {
                cpu_fallback_percent = update_proc_cpu(&text, previous_cpu, &mut system);
            }
        }
        if let Some(value) = cpu_fallback_percent {
            add_sensor(&mut raw, "Harmony CPU Usage", value, "%");
            p0_count += 1;
        }
    }

    if config.enable_system_mem || needs_process_snapshot {
        match client.shell_with_cancel(
            &config.udid,
            "hidumper --mem",
            SHELL_TIMEOUT_SECS,
            &inner.stop,
        ) {
            Ok(text) => mem_text = Some(text),
            Err(error) if !matches!(error, crate::error::Error::Cancelled(_)) => {
                if let Ok(text) = client.shell_with_cancel(
                    &config.udid,
                    "cat /proc/meminfo",
                    SHELL_TIMEOUT_SECS,
                    &inner.stop,
                ) {
                    mem_text = Some(text);
                } else {
                    log::debug!("系统内存采集失败: {error}");
                }
            }
            Err(_) => return Ok(None),
        }
        if config.enable_system_mem {
            if let Some(text) = mem_text.as_deref() {
                let parsed = parse_hidumper_mem_system(text)
                    .map(|value| (value.total_kb, value.used_kb, value.available_kb))
                    .or_else(|_| {
                        parse_proc_meminfo(text).map(|value| {
                            (
                                value.total_kb,
                                value.total_kb.saturating_sub(value.available_kb),
                                Some(value.available_kb),
                            )
                        })
                    })
                    .or_else(|_| {
                        client
                            .shell_with_cancel(
                                &config.udid,
                                "cat /proc/meminfo",
                                SHELL_TIMEOUT_SECS,
                                &inner.stop,
                            )
                            .map_err(|error| error.to_string())
                            .and_then(|text| {
                                parse_proc_meminfo(&text)
                                    .map_err(|error| error.to_string())
                                    .map(|value| {
                                        (
                                            value.total_kb,
                                            value.total_kb.saturating_sub(value.available_kb),
                                            Some(value.available_kb),
                                        )
                                    })
                            })
                    });
                if let Ok((total_kb, used_kb, available_kb)) = parsed {
                    add_sensor(
                        &mut raw,
                        "Harmony Mem Total",
                        total_kb as f64 / 1024.0,
                        "MB",
                    );
                    add_sensor(&mut raw, "Harmony Mem Used", used_kb as f64 / 1024.0, "MB");
                    if let Some(value) = available_kb {
                        add_sensor(
                            &mut raw,
                            "Harmony Mem Available",
                            value as f64 / 1024.0,
                            "MB",
                        );
                    }
                    p0_count += 1;
                }
            }
        }
    }

    let mut processes = None;
    let mut top_n_cpu = None;
    if needs_process_snapshot {
        // 进程 shell 失败只降级进程字段，不能把已经采到的系统 CPU/内存整轮丢掉。
        match client.shell_with_cancel(&config.udid, "ps -ef", SHELL_TIMEOUT_SECS, &inner.stop) {
            Ok(ps_text) => {
                let process_list = parse_ps_ef(&ps_text);
                match client.shell_with_cancel(
                    &config.udid,
                    "top -n 1 -b",
                    SHELL_TIMEOUT_SECS,
                    &inner.stop,
                ) {
                    Ok(top_text) => {
                        let cpu_snapshots = parse_top_processes(&top_text);
                        let mem_snapshots = mem_text
                            .as_deref()
                            .map(parse_hidumper_mem_processes)
                            .unwrap_or_default();
                        let all_details =
                            process_details(&process_list, &cpu_snapshots, &mem_snapshots);

                        if let Some(filter) = &config.process_filter {
                            let candidates = filtered_processes(filter, &process_list)
                                .into_iter()
                                .map(|(pid, _)| pid)
                                .collect::<Vec<_>>();
                            let details = all_details
                                .iter()
                                .filter(|item| candidates.contains(&item.pid))
                                .cloned()
                                .collect::<Vec<_>>();
                            if !details.is_empty() {
                                p0_count += 1;
                                processes = Some(details);
                            }
                        }

                        if let Some(limit) = config.top_n_cpu {
                            let mut values = aggregate_processes(&all_details);
                            values.sort_by(|left, right| {
                                right
                                    .cpu_percent_total
                                    .total_cmp(&left.cpu_percent_total)
                                    .then_with(|| left.name.cmp(&right.name))
                            });
                            values.truncate(limit);
                            if !values.is_empty() {
                                p0_count += 1;
                                top_n_cpu = Some(values);
                            }
                        }
                    }
                    Err(error) if matches!(error, crate::error::Error::Cancelled(_)) => {
                        return Ok(None);
                    }
                    Err(error) => {
                        log::debug!("进程 top 快照不可用，本轮仅保留系统指标: {error}");
                    }
                }
            }
            Err(error) if matches!(error, crate::error::Error::Cancelled(_)) => {
                return Ok(None);
            }
            Err(error) => {
                log::debug!("进程列表不可用，本轮仅保留系统指标: {error}");
            }
        }
    }

    if p0_count == 0 {
        return Ok(None);
    }

    let aggregated = processes
        .as_ref()
        .filter(|_| config.enable_aggregation)
        .map(|values| aggregate_processes(values));
    let mut sample = Sample {
        sequence: inner.next_sequence(),
        elapsed_ms: elapsed.as_millis() as u64,
        timestamp: Utc::now(),
        system,
        hwinfo_raw: raw,
        processes,
        aggregated,
        top_n_cpu,
        top_n_gpu: None,
    };

    // 可选能力共用一次 shell。它们的设备端格式和权限仍需真机冻结，故解析失败只降级
    // 当前能力，不影响已经完成的 P0 样本。
    if let Some(command) = optional_command(
        config.enable_network,
        config.enable_thermal_power,
        config.enable_thermal_power,
        config.enable_gpu,
        config.enable_cpu_freq,
    ) {
        match client.shell_with_cancel(&config.udid, &command, SHELL_TIMEOUT_SECS, &inner.stop) {
            Ok(text) => {
                let sections = split_optional_output(&text);
                let mut collectors: Vec<Box<dyn Collector>> = Vec::new();
                if config.enable_network {
                    collectors.push(Box::new(NetworkCollector));
                }
                if config.enable_thermal_power {
                    collectors.push(Box::new(PowerCollector));
                    collectors.push(Box::new(ThermalCollector));
                }
                if config.enable_gpu {
                    collectors.push(Box::new(GpuCollector));
                }
                if config.enable_cpu_freq {
                    collectors.push(Box::new(CpuFreqCollector));
                }

                let mut context = CollectContext {
                    sample: &mut sample,
                    optional_state,
                    interval_secs: config.interval,
                    network_text: sections.get("network").map(String::as_str),
                    power_text: sections.get("power").map(String::as_str),
                    thermal_text: sections.get("thermal").map(String::as_str),
                    gpu_text: sections.get("gpu").map(String::as_str),
                    cpufreq_text: sections.get("cpufreq").map(String::as_str),
                };
                for collector in &mut collectors {
                    if let Err(error) = collector.collect(&mut context) {
                        log::debug!("{} 指标不可用: {error}", collector.name());
                    }
                }
            }
            Err(error) if !matches!(error, crate::error::Error::Cancelled(_)) => {
                log::debug!("可选指标 shell 不可用: {error}");
            }
            Err(_) => return Ok(None),
        }
    }

    // 磁盘 IO 只能逐 pid 经 hidumper --storage 代读，与其它可选指标无法共享一条
    // 静态命令，单独一次 shell；仅对目标进程采集，失败只降级磁盘指标。
    if config.enable_disk_io {
        let pids: Vec<u32> = sample
            .processes
            .as_ref()
            .map(|values| values.iter().map(|process| process.pid).collect())
            .unwrap_or_default();
        if !pids.is_empty() {
            let command = crate::collector::disk_io_command(&pids);
            match client.shell_with_cancel(&config.udid, &command, SHELL_TIMEOUT_SECS, &inner.stop)
            {
                Ok(text) => {
                    if let Err(error) = crate::collector::collect_disk_io(
                        &text,
                        previous_disk,
                        config.interval,
                        &mut sample.hwinfo_raw,
                    ) {
                        log::debug!("磁盘 IO 指标不可用: {error}");
                    }
                }
                Err(error) if !matches!(error, crate::error::Error::Cancelled(_)) => {
                    log::debug!("磁盘 IO shell 不可用: {error}");
                }
                Err(_) => return Ok(None),
            }
        }
    }

    Ok(Some(sample))
}

fn update_proc_cpu(
    text: &str,
    previous_cpu: &mut Option<CpuStatSnapshot>,
    system: &mut SystemMetrics,
) -> Option<f64> {
    let Ok(current) = parse_proc_stat(text) else {
        return None;
    };
    let value = previous_cpu
        .as_ref()
        .and_then(|previous| diff_cpu_percent(previous, &current).map(clamp_percent));
    system.cpu_percent = value;
    *previous_cpu = Some(current);
    value
}

#[derive(Debug, Clone, Copy)]
struct TopProcessSnapshot {
    cpu_percent: f64,
    memory_mb: f64,
}

fn process_details(
    process_list: &[(u32, String)],
    cpu_snapshots: &HashMap<u32, TopProcessSnapshot>,
    mem_snapshots: &HashMap<u32, ProcessMemSnapshot>,
) -> Vec<ProcessInfo> {
    process_list
        .iter()
        .filter_map(|(pid, name)| {
            let cpu = cpu_snapshots.get(pid)?;
            let mem = mem_snapshots.get(pid)?;
            Some(ProcessInfo {
                pid: *pid,
                name: name.clone(),
                cpu_percent: clamp_percent(cpu.cpu_percent),
                working_set_mb: mem.rss_kb as f64 / 1024.0,
                committed_memory_mb: mem.vss_kb as f64 / 1024.0,
                gpu_percent: 0.0,
                gpu_memory_mb: 0.0,
                handle_count: 0,
            })
        })
        .collect()
}

fn parse_top_processes(text: &str) -> HashMap<u32, TopProcessSnapshot> {
    let mut result = HashMap::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 10 {
            // 常见 top: PID USER PR NI VIRT RES SHR S %CPU %MEM TIME COMMAND。
            let (Ok(pid), Ok(cpu), Ok(mem)) = (
                fields[0].parse::<u32>(),
                fields[8].parse::<f64>(),
                memory_to_mb(fields[5]),
            ) else {
                continue;
            };
            result.insert(
                pid,
                TopProcessSnapshot {
                    cpu_percent: cpu,
                    memory_mb: mem,
                },
            );
        } else if fields.len() >= 3 {
            // 测试桩/不同版本 top 的最小格式: PID %CPU RES COMMAND。
            let (Ok(pid), Ok(cpu), Ok(mem)) = (
                fields[0].parse::<u32>(),
                fields[1].parse::<f64>(),
                memory_to_mb(fields[2]),
            ) else {
                continue;
            };
            result.insert(
                pid,
                TopProcessSnapshot {
                    cpu_percent: cpu,
                    memory_mb: mem,
                },
            );
        }
    }
    result
}

fn memory_to_mb(value: &str) -> Result<f64, std::num::ParseFloatError> {
    let lower = value.to_ascii_lowercase();
    let (number, factor) = if let Some(number) = lower.strip_suffix('g') {
        (number, 1024.0)
    } else if let Some(number) = lower.strip_suffix('m') {
        (number, 1.0)
    } else if let Some(number) = lower.strip_suffix('k') {
        (number, 1.0 / 1024.0)
    } else {
        (lower.as_str(), 1.0 / 1024.0)
    };
    number.parse::<f64>().map(|value| value * factor)
}

fn filtered_processes(filter: &ProcessFilter, values: &[(u32, String)]) -> Vec<(u32, String)> {
    let mut result = Vec::new();
    for (pid, name) in values {
        let matched = match filter {
            ProcessFilter::Pids(pids) => pids.contains(pid),
            ProcessFilter::Name(expected) => name_matches(expected, name),
            ProcessFilter::Names(names) => {
                names.iter().any(|expected| name_matches(expected, name))
            }
            ProcessFilter::NameRegex(pattern) => {
                Regex::new(pattern).is_ok_and(|regex| regex.is_match(name))
            }
        };
        if matched {
            result.push((*pid, name.clone()));
        }
    }
    result
}

/// 按名匹配需覆盖鸿蒙应用子进程：appspawn 以 `包名:子进程名` 命名子进程
/// （真机实证 com.huawei.his.xgate:gpu/:render），SP_daemon 同样把包名
/// 解析出的主进程 + 子进程 pid 一起采集。精确匹配会漏掉全部子进程。
fn name_matches(expected: &str, name: &str) -> bool {
    name == expected
        || (name.len() > expected.len()
            && name.starts_with(expected)
            && name.as_bytes()[expected.len()] == b':')
}

/// 汇总归组键：`包名:子进程名` 取冒号前的包名，与任务管理器按应用汇总一致；
/// 非包名形态（如内核线程 kworker/0:1H）不含 `.`，保持原名不归组。
fn bundle_base_name(name: &str) -> &str {
    match name.split_once(':') {
        Some((base, _)) if base.contains('.') && !base.contains('/') => base,
        _ => name,
    }
}

fn aggregate_processes(processes: &[ProcessInfo]) -> Vec<AggregatedProcessInfo> {
    let mut groups: HashMap<String, Vec<&ProcessInfo>> = HashMap::new();
    for process in processes {
        groups
            .entry(bundle_base_name(&process.name).to_string())
            .or_default()
            .push(process);
    }
    let mut values = groups
        .into_iter()
        .map(|(name, items)| AggregatedProcessInfo {
            name,
            pids: items.iter().map(|item| item.pid).collect(),
            cpu_percent_total: clamp_percent(items.iter().map(|item| item.cpu_percent).sum()),
            working_set_mb_total: items.iter().map(|item| item.working_set_mb).sum(),
            committed_memory_mb_total: items.iter().map(|item| item.committed_memory_mb).sum(),
            gpu_percent_total: clamp_percent(items.iter().map(|item| item.gpu_percent).sum()),
            handle_count_total: items.iter().map(|item| item.handle_count).sum(),
            process_count: items.len(),
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| left.name.cmp(&right.name));
    values
}

fn add_sensor(raw: &mut HashMap<String, SensorValue>, key: &str, value: f64, unit: &str) {
    raw.insert(
        key.to_string(),
        SensorValue {
            value: clamp_non_negative(value),
            unit: unit.to_string(),
        },
    );
}

fn clamp_percent(value: f64) -> f64 {
    value.clamp(0.0, 100.0)
}

fn clamp_non_negative(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        aggregate_processes, bundle_base_name, collect_sample, filtered_processes, memory_to_mb,
        parse_top_processes, process_details, MonitorConfigOwned, MonitorInner, ShellClient,
    };
    use crate::data::{ProcessFilter, ProcessInfo};
    use crate::error::Error;
    use crate::parse::{parse_ps_ef, ProcessMemSnapshot};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    #[test]
    fn parses_common_top_line() {
        let output = "  123 shell  20  0  120m  10m  2m S  12.5  0.3  00:00 app\n";
        let values = parse_top_processes(output);
        assert_eq!(values.get(&123).unwrap().cpu_percent, 12.5);
        assert_eq!(values.get(&123).unwrap().memory_mb, 10.0);
    }

    #[test]
    fn parses_simple_process_line() {
        let values = parse_top_processes("123 12.5 10m app\n");
        assert_eq!(values.get(&123).unwrap().memory_mb, 10.0);
    }

    #[test]
    fn parses_real_device_top_output() {
        let text = include_str!("../tests/fixtures/real/harmony_pc/top_n1_b.txt");
        let values = parse_top_processes(text);

        assert!(!values.is_empty());
        assert!(values.values().all(|value| value.cpu_percent >= 0.0));
    }

    #[test]
    fn process_details_require_cpu_and_memory_and_use_rss_vss() {
        let processes = parse_ps_ef(include_str!("../tests/fixtures/ps_ef.txt"));
        let cpu = parse_top_processes("200 12.5 10m com.example.app\n");
        let memory = HashMap::from([(
            200,
            ProcessMemSnapshot {
                pss_kb: 8192,
                vss_kb: 32768,
                rss_kb: 16384,
                uss_kb: 4096,
            },
        )]);

        let details = process_details(&processes, &cpu, &memory);
        assert_eq!(details.len(), 1);
        assert_eq!(details[0].working_set_mb, 16.0);
        assert_eq!(details[0].committed_memory_mb, 32.0);
        assert!(process_details(&processes, &cpu, &HashMap::new()).is_empty());
    }

    #[test]
    fn real_device_process_snapshot_maps_cpu_rss_and_vss() {
        let processes = parse_ps_ef(include_str!("../tests/fixtures/real/harmony_pc/ps_ef.txt"));
        let cpu = parse_top_processes(include_str!(
            "../tests/fixtures/real/harmony_pc/top_n1_b.txt"
        ));
        let memory = crate::parse::parse_hidumper_mem_processes(include_str!(
            "../tests/fixtures/real/harmony_pc/hidumper_mem.txt"
        ));

        let details = process_details(&processes, &cpu, &memory);
        let process = details.iter().find(|item| item.pid == 14751).unwrap();
        assert_eq!(process.name, "com.hisec.endpoint:EDRManager1");
        assert_eq!(process.cpu_percent, 57.1);
        assert_eq!(process.working_set_mb, 130924.0 / 1024.0);
        assert_eq!(process.committed_memory_mb, 6480768.0 / 1024.0);
    }

    #[test]
    fn aggregation_is_bounded() {
        let process = ProcessInfo {
            pid: 1,
            name: "app".to_string(),
            cpu_percent: 80.0,
            working_set_mb: 1.0,
            committed_memory_mb: 1.0,
            gpu_percent: 90.0,
            gpu_memory_mb: 0.0,
            handle_count: 0,
        };
        let values = aggregate_processes(&[process.clone(), process]);
        assert_eq!(values[0].cpu_percent_total, 100.0);
        assert_eq!(values[0].gpu_percent_total, 100.0);
    }

    #[test]
    fn name_filter_matches_bundle_child_processes() {
        // 真机形态：主进程 + `包名:子进程`（xgate 有 :gpu 和两个 :render）。
        let values = vec![
            (100, "com.huawei.his.xgate".to_string()),
            (101, "com.huawei.his.xgate:gpu".to_string()),
            (102, "com.huawei.his.xgate:render".to_string()),
            (103, "com.huawei.his.xgate.esm".to_string()),
            (104, "com.other.app".to_string()),
        ];
        let filter = ProcessFilter::Names(vec!["com.huawei.his.xgate".to_string()]);
        let matched: Vec<u32> = filtered_processes(&filter, &values)
            .into_iter()
            .map(|(pid, _)| pid)
            .collect();
        // 命中主进程与冒号子进程，但不能误伤同前缀的另一个包 xgate.esm。
        assert_eq!(matched, vec![100, 101, 102]);
    }

    #[test]
    fn aggregation_groups_children_under_bundle_name() {
        let make = |pid: u32, name: &str, cpu: f64, mem: f64| ProcessInfo {
            pid,
            name: name.to_string(),
            cpu_percent: cpu,
            working_set_mb: mem,
            committed_memory_mb: mem * 2.0,
            gpu_percent: 0.0,
            gpu_memory_mb: 0.0,
            handle_count: 0,
        };
        let values = aggregate_processes(&[
            make(100, "com.huawei.his.xgate", 10.0, 100.0),
            make(101, "com.huawei.his.xgate:gpu", 5.0, 50.0),
            make(102, "com.huawei.his.xgate:render", 3.0, 30.0),
        ]);
        // 任务管理器语义：一行应用，三个 pid 求和。
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].name, "com.huawei.his.xgate");
        assert_eq!(values[0].pids, vec![100, 101, 102]);
        assert_eq!(values[0].process_count, 3);
        assert_eq!(values[0].cpu_percent_total, 18.0);
        assert_eq!(values[0].working_set_mb_total, 180.0);
    }

    #[test]
    fn bundle_base_name_keeps_non_bundle_names() {
        assert_eq!(bundle_base_name("com.app:render"), "com.app");
        assert_eq!(bundle_base_name("com.app"), "com.app");
        // 内核线程等非包名形态不归组。
        assert_eq!(bundle_base_name("kworker/0:1H"), "kworker/0:1H");
        assert_eq!(bundle_base_name("0:1H"), "0:1H");
    }

    #[test]
    fn parses_memory_units() {
        assert_eq!(memory_to_mb("1g").unwrap(), 1024.0);
        assert_eq!(memory_to_mb("1024k").unwrap(), 1.0);
    }

    struct FakeHdc {
        calls: Mutex<Vec<String>>,
    }

    impl FakeHdc {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl ShellClient for FakeHdc {
        fn shell_with_cancel(
            &self,
            _udid: &str,
            command: &str,
            _timeout_secs: u64,
            cancel: &AtomicBool,
        ) -> Result<String, Error> {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled("fake hdc cancelled".to_string()));
            }
            self.calls
                .lock()
                .expect("fake HDC calls mutex poisoned")
                .push(command.to_string());
            match command {
                "hidumper --cpuusage" => {
                    Ok(include_str!("../tests/fixtures/hidumper_cpuusage.txt").to_string())
                }
                "hidumper --mem" => {
                    Ok(format!(
                        "{}\n200 8192(0 in SwapPss) kB 32768 kB 16384 kB 4096 kB 0 kB 0 kB 0 kB 0 kB 0 kB com.example.app\n",
                        include_str!("../tests/fixtures/hidumper_mem.txt")
                    ))
                }
                "ps -ef" => Ok(include_str!("../tests/fixtures/ps_ef.txt").to_string()),
                "top -n 1 -b" => Ok("200 12.5 10m com.example.app\n".to_string()),
                _ => Err(Error::CommandFailed(format!(
                    "unexpected fake command: {command}"
                ))),
            }
        }
    }

    #[test]
    fn fake_hdc_collects_a_p0_sample_without_real_device() {
        let config = MonitorConfigOwned {
            udid: "FAKE-UDID".to_string(),
            hdc_path: None,
            interval: 1.0,
            duration: Some(2.0),
            process_filter: Some(ProcessFilter::Names(vec!["com.example.app".to_string()])),
            top_n_cpu: None,
            top_n_gpu: None,
            enable_aggregation: true,
            enable_system_cpu: true,
            enable_system_mem: true,
            enable_process: true,
            enable_gpu: false,
            enable_network: false,
            enable_thermal_power: false,
            enable_cpu_freq: false,
            enable_disk_io: false,
        };
        let fake = FakeHdc::new();
        let inner = std::sync::Arc::new(MonitorInner::new());
        let mut previous_cpu = None;
        let sample = collect_sample(
            &config,
            &fake,
            &inner,
            &mut previous_cpu,
            std::time::Duration::from_millis(25),
        )
        .expect("fake HDC collection should not fail")
        .expect("P0 fixture should produce a sample");

        assert_eq!(sample.sequence, 1);
        assert_eq!(sample.system.cpu_percent, Some(35.0));
        assert!(sample.hwinfo_raw.contains_key("Harmony CPU Usage"));
        assert_eq!(sample.processes.as_ref().unwrap()[0].pid, 200);
        assert_eq!(sample.processes.as_ref().unwrap()[0].working_set_mb, 16.0);
        assert_eq!(
            sample.processes.as_ref().unwrap()[0].committed_memory_mb,
            32.0
        );
        assert_eq!(sample.aggregated.as_ref().unwrap()[0].process_count, 1);
        assert_eq!(
            fake.calls.lock().unwrap().as_slice(),
            [
                "hidumper --cpuusage",
                "hidumper --mem",
                "ps -ef",
                "top -n 1 -b"
            ]
        );
    }

    #[test]
    fn top_n_cpu_reuses_the_four_p0_commands() {
        let config = MonitorConfigOwned {
            udid: "FAKE-UDID".to_string(),
            hdc_path: None,
            interval: 1.0,
            duration: Some(2.0),
            process_filter: None,
            top_n_cpu: Some(1),
            top_n_gpu: None,
            enable_aggregation: true,
            enable_system_cpu: true,
            enable_system_mem: true,
            enable_process: true,
            enable_gpu: false,
            enable_network: false,
            enable_thermal_power: false,
            enable_cpu_freq: false,
            enable_disk_io: false,
        };
        let fake = FakeHdc::new();
        let inner = std::sync::Arc::new(MonitorInner::new());
        let mut previous_cpu = None;
        let sample = collect_sample(
            &config,
            &fake,
            &inner,
            &mut previous_cpu,
            std::time::Duration::from_millis(25),
        )
        .unwrap()
        .unwrap();

        let top_n = sample.top_n_cpu.unwrap();
        assert_eq!(top_n.len(), 1);
        assert_eq!(top_n[0].name, "com.example.app");
        assert_eq!(top_n[0].cpu_percent_total, 12.5);
        assert!(sample.processes.is_none());
        assert_eq!(
            fake.calls.lock().unwrap().as_slice(),
            [
                "hidumper --cpuusage",
                "hidumper --mem",
                "ps -ef",
                "top -n 1 -b"
            ]
        );
    }

    struct FailingProcessHdc {
        calls: Mutex<Vec<String>>,
    }

    impl ShellClient for FailingProcessHdc {
        fn shell_with_cancel(
            &self,
            _udid: &str,
            command: &str,
            _timeout_secs: u64,
            cancel: &AtomicBool,
        ) -> Result<String, Error> {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled("fake hdc cancelled".to_string()));
            }
            self.calls
                .lock()
                .expect("fake HDC calls mutex poisoned")
                .push(command.to_string());
            match command {
                "hidumper --cpuusage" => {
                    Ok(include_str!("../tests/fixtures/hidumper_cpuusage.txt").to_string())
                }
                "hidumper --mem" => {
                    Ok(include_str!("../tests/fixtures/hidumper_mem.txt").to_string())
                }
                "ps -ef" => Err(Error::Timeout("ps timed out".to_string())),
                "top -n 1 -b" => Err(Error::Timeout("top timed out".to_string())),
                _ => Err(Error::CommandFailed(format!(
                    "unexpected fake command: {command}"
                ))),
            }
        }
    }

    #[test]
    fn process_shell_failure_keeps_system_sample() {
        let config = MonitorConfigOwned {
            udid: "FAKE-UDID".to_string(),
            hdc_path: None,
            interval: 1.0,
            duration: Some(2.0),
            process_filter: Some(ProcessFilter::Names(vec!["com.example.app".to_string()])),
            top_n_cpu: None,
            top_n_gpu: None,
            enable_aggregation: true,
            enable_system_cpu: true,
            enable_system_mem: true,
            enable_process: true,
            enable_gpu: false,
            enable_network: false,
            enable_thermal_power: false,
            enable_cpu_freq: false,
            enable_disk_io: false,
        };
        let fake = FailingProcessHdc {
            calls: Mutex::new(Vec::new()),
        };
        let inner = std::sync::Arc::new(MonitorInner::new());
        let mut previous_cpu = None;
        let sample = collect_sample(
            &config,
            &fake,
            &inner,
            &mut previous_cpu,
            std::time::Duration::from_millis(25),
        )
        .expect("system P0 should survive process shell failure")
        .expect("system-only sample should be produced");

        assert_eq!(sample.system.cpu_percent, Some(35.0));
        assert!(sample.processes.is_none());
        assert_eq!(
            fake.calls.lock().unwrap().as_slice(),
            ["hidumper --cpuusage", "hidumper --mem", "ps -ef"]
        );
    }

    #[test]
    fn fake_hdc_stop_is_observed_before_shell_work() {
        let config = MonitorConfigOwned {
            udid: "FAKE-UDID".to_string(),
            hdc_path: None,
            interval: 1.0,
            duration: None,
            process_filter: None,
            top_n_cpu: None,
            top_n_gpu: None,
            enable_aggregation: true,
            enable_system_cpu: true,
            enable_system_mem: true,
            enable_process: false,
            enable_gpu: false,
            enable_network: false,
            enable_thermal_power: false,
            enable_cpu_freq: false,
            enable_disk_io: false,
        };
        let fake = FakeHdc::new();
        let inner = MonitorInner::new();
        inner.request_stop();
        let mut previous_cpu = None;
        let result = collect_sample(
            &config,
            &fake,
            &inner,
            &mut previous_cpu,
            std::time::Duration::ZERO,
        );
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
        assert!(fake.calls.lock().unwrap().is_empty());
    }
}
