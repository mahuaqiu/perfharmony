//! SP_daemon 长跑流采集内核（0.2.0）。
//!
//! 设备侧唯一数据源为官方性能守护 `SP_daemon`：单进程以固定 1s 节奏连续输出
//! 采样块，本模块逐行喂入 [`SpStreamParser`]，按块产出 Sample。
//! 采集间隔通过降采样实现：间隔 N 秒 = 每 N 拍取 1 拍入库。
//! 首拍是差分基线（CPU 全 0），必须丢弃。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;

use crate::data::{AggregatedProcessInfo, ProcessInfo, Sample, SensorValue, SystemMetrics};
use crate::hdc::{HdcClient, ShellStream, StreamEvent};
use crate::parse::sp_daemon::{
    build_processes, build_system_sensors, sp_daemon_command, SpStreamParser, FINISH_MARKER,
};
use crate::ring_buffer::RingBuffer;

const SAMPLE_BUFFER_CAPACITY: usize = 1024;
/// pkill 清理命令的 shell 超时。
const CLEANUP_TIMEOUT_SECS: u64 = 10;
/// 流读取轮询步长；每步醒来检查停止标志。
const POLL_STEP: Duration = Duration::from_millis(200);
/// 启动后一直没有任何 `order:` 输出则判定失败（含 hdc 建连耗时）。
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
/// 已有输出后流停滞超过该时长判定设备侧异常。
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
/// 不限时长时的默认总拍数（24 小时）。
const DEFAULT_TOTAL_SECONDS: u32 = 86_400;

/// 长跑行流抽象，测试桩靠它注入 fixture。
pub(crate) trait LineStream: Send {
    fn next_line(&mut self, timeout: Duration) -> StreamEvent;
    fn stop(&mut self);
}

impl LineStream for ShellStream {
    fn next_line(&mut self, timeout: Duration) -> StreamEvent {
        ShellStream::next_line(self, timeout)
    }

    fn stop(&mut self) {
        ShellStream::stop(self);
    }
}

/// Monitor 依赖的设备客户端：短命令 shell（pkill 清理）+ 长跑流。
pub(crate) trait MonitorClient: Send + Sync {
    type Stream: LineStream;

    fn shell(
        &self,
        udid: &str,
        command: &str,
        timeout_secs: u64,
        cancel: &AtomicBool,
    ) -> Result<String, crate::error::Error>;

    fn open_stream(&self, udid: &str, command: &str)
        -> Result<Self::Stream, crate::error::Error>;
}

impl MonitorClient for HdcClient {
    type Stream = ShellStream;

    fn shell(
        &self,
        udid: &str,
        command: &str,
        timeout_secs: u64,
        cancel: &AtomicBool,
    ) -> Result<String, crate::error::Error> {
        HdcClient::shell_with_cancel(self, udid, command, timeout_secs, cancel)
    }

    fn open_stream(
        &self,
        udid: &str,
        command: &str,
    ) -> Result<Self::Stream, crate::error::Error> {
        HdcClient::shell_stream(self, udid, command)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct MonitorConfigOwned {
    pub udid: String,
    pub hdc_path: Option<String>,
    /// 采集间隔（秒），>=1；对 1s 流按四舍五入的整数倍降采样。
    pub interval: f64,
    /// 采集总时长（秒）；None 时长跑至 stop() 或 24h 上限。
    pub duration: Option<f64>,
    /// 单应用采集：SP_daemon -PKG 自动覆盖主进程 + 全部子进程。
    pub package: Option<String>,
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

pub(crate) fn run_monitor<C: MonitorClient>(
    config: MonitorConfigOwned,
    client: C,
    inner: Arc<MonitorInner>,
) {
    inner.mark_running();
    let started = Instant::now();

    // 启动前 best-effort 清理上次残留的 SP_daemon（非 root shell 可杀同 uid 进程）。
    let _ = client.shell(
        &config.udid,
        "pkill SP_daemon",
        CLEANUP_TIMEOUT_SECS,
        &inner.stop,
    );

    // 首拍差分基线要丢弃，总拍数 = 时长 + 1。
    let total = config
        .duration
        .map(|value| (value.ceil() as u32).saturating_add(1))
        .unwrap_or(DEFAULT_TOTAL_SECONDS)
        .max(2);
    let command = sp_daemon_command(total, config.package.as_deref());

    if !inner.is_stopped() {
        match client.open_stream(&config.udid, &command) {
            Ok(mut stream) => {
                run_stream_loop(&config, &inner, &mut stream, started);
                stream.stop();
            }
            Err(error) => inner.set_error(format!("启动 SP_daemon 采集流失败: {error}")),
        }
    }

    // 结束后清理设备端 SP_daemon；stop() 已置位也要执行，故用独立 cancel。
    let cleanup_cancel = AtomicBool::new(false);
    let _ = client.shell(
        &config.udid,
        "pkill SP_daemon",
        CLEANUP_TIMEOUT_SECS,
        &cleanup_cancel,
    );
    inner.finish();
}

fn run_stream_loop<S: LineStream>(
    config: &MonitorConfigOwned,
    inner: &MonitorInner,
    stream: &mut S,
    started: Instant,
) {
    let step = (config.interval.round() as u64).max(1);
    let mut parser = SpStreamParser::new();
    let mut block_index = 0u64;
    let mut saw_order = false;
    let mut last_line_at = Instant::now();

    while !inner.is_stopped() {
        match stream.next_line(POLL_STEP) {
            StreamEvent::Line(line) => {
                last_line_at = Instant::now();
                let trimmed = line.trim();
                if !saw_order && trimmed.starts_with("order:") {
                    saw_order = true;
                }
                if let Some(block) = parser.push_line(&line) {
                    handle_block(inner, &block, started, step, &mut block_index);
                }
                if trimmed == FINISH_MARKER {
                    return;
                }
            }
            StreamEvent::Idle => {
                // 管道静默陷阱等启动失败在设备端表现为退出码 0 且零输出，
                // 必须靠超时兜底判定，而不能等进程退出。
                if !saw_order && started.elapsed() >= STARTUP_TIMEOUT {
                    inner.set_error(
                        "SP_daemon 启动后无数据输出（设备可能不支持该命令）".to_string(),
                    );
                    return;
                }
                if saw_order && last_line_at.elapsed() >= STALL_TIMEOUT {
                    inner.set_error("SP_daemon 输出停滞，采集中断".to_string());
                    return;
                }
            }
            StreamEvent::Closed => {
                if let Some(block) = parser.finish() {
                    handle_block(inner, &block, started, step, &mut block_index);
                }
                if !saw_order {
                    inner.set_error(
                        "SP_daemon 未产出任何数据（流提前结束）".to_string(),
                    );
                }
                return;
            }
        }
    }
}

fn handle_block(
    inner: &MonitorInner,
    block: &std::collections::HashMap<String, String>,
    started: Instant,
    step: u64,
    block_index: &mut u64,
) {
    let index = *block_index;
    *block_index += 1;
    // 首拍是差分基线（CPU 全 0），丢弃。
    if index == 0 {
        return;
    }
    // 降采样：间隔 N 秒 = 每 N 拍取 1 拍。
    if (index - 1) % step != 0 {
        return;
    }
    if let Some(sample) = sample_from_block(inner, block, started.elapsed()) {
        inner.push(sample);
    }
}

fn sample_from_block(
    inner: &MonitorInner,
    block: &std::collections::HashMap<String, String>,
    elapsed: Duration,
) -> Option<Sample> {
    let raw = build_system_sensors(block);
    let processes = build_processes(block);
    if raw.is_empty() && processes.is_none() {
        return None;
    }

    let mut system = SystemMetrics::default();
    system.cpu_percent = sensor_value(&raw, "Harmony CPU Usage");
    system.gpu_percent = sensor_value(&raw, "Harmony GPU Load");
    if system.gpu_percent.is_some() {
        system.gpu_source = "sp_daemon".to_string();
    }

    let aggregated = processes.as_ref().map(|values| aggregate_processes(values));

    Some(Sample {
        sequence: inner.next_sequence(),
        elapsed_ms: elapsed.as_millis() as u64,
        timestamp: Utc::now(),
        system,
        hwinfo_raw: raw,
        processes,
        aggregated,
        top_n_cpu: None,
        top_n_gpu: None,
    })
}

fn sensor_value(
    raw: &std::collections::HashMap<String, SensorValue>,
    key: &str,
) -> Option<f64> {
    raw.get(key).map(|sensor| sensor.value)
}

/// 汇总归组键：`包名:子进程标识` 取冒号前的包名，与任务管理器按应用汇总一致；
/// 非包名形态（如内核线程 kworker/0:1H）不含 `.`，保持原名不归组。
fn bundle_base_name(name: &str) -> &str {
    match name.split_once(':') {
        Some((base, _)) if base.contains('.') && !base.contains('/') => base,
        _ => name,
    }
}

fn aggregate_processes(processes: &[ProcessInfo]) -> Vec<AggregatedProcessInfo> {
    let mut groups: std::collections::HashMap<String, Vec<&ProcessInfo>> =
        std::collections::HashMap::new();
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

fn clamp_percent(value: f64) -> f64 {
    value.clamp(0.0, 100.0)
}

#[cfg(test)]
mod tests {
    use super::{
        aggregate_processes, bundle_base_name, run_monitor, LineStream, MonitorClient,
        MonitorConfigOwned, MonitorInner,
    };
    use crate::data::ProcessInfo;
    use crate::error::Error;
    use crate::hdc::StreamEvent;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    const FIXTURE: &str = include_str!("../tests/fixtures/real_device2/sp_daemon_n2_full.txt");

    struct FakeStream {
        lines: Vec<String>,
        position: usize,
    }

    impl LineStream for FakeStream {
        fn next_line(&mut self, _timeout: Duration) -> StreamEvent {
            if self.position < self.lines.len() {
                let line = self.lines[self.position].clone();
                self.position += 1;
                StreamEvent::Line(line)
            } else {
                StreamEvent::Closed
            }
        }

        fn stop(&mut self) {}
    }

    struct FakeClient {
        lines: Vec<String>,
        shell_calls: Mutex<Vec<String>>,
        stream_commands: Mutex<Vec<String>>,
    }

    impl FakeClient {
        fn new(text: &str) -> Self {
            Self {
                lines: text.lines().map(str::to_string).collect(),
                shell_calls: Mutex::new(Vec::new()),
                stream_commands: Mutex::new(Vec::new()),
            }
        }
    }

    impl MonitorClient for FakeClient {
        type Stream = FakeStream;

        fn shell(
            &self,
            _udid: &str,
            command: &str,
            _timeout_secs: u64,
            cancel: &AtomicBool,
        ) -> Result<String, Error> {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled("fake shell cancelled".to_string()));
            }
            self.shell_calls
                .lock()
                .expect("fake shell mutex poisoned")
                .push(command.to_string());
            Ok(String::new())
        }

        fn open_stream(&self, _udid: &str, command: &str) -> Result<Self::Stream, Error> {
            self.stream_commands
                .lock()
                .expect("fake stream mutex poisoned")
                .push(command.to_string());
            Ok(FakeStream {
                lines: self.lines.clone(),
                position: 0,
            })
        }
    }

    fn config(interval: f64, duration: Option<f64>, package: Option<&str>) -> MonitorConfigOwned {
        MonitorConfigOwned {
            udid: "FAKE-UDID".to_string(),
            hdc_path: None,
            interval,
            duration,
            package: package.map(str::to_string),
        }
    }

    #[test]
    fn real_fixture_yields_one_sample_after_baseline_discard() {
        let client = FakeClient::new(FIXTURE);
        let inner = Arc::new(MonitorInner::new());
        run_monitor(
            config(1.0, Some(60.0), Some("com.huawei.it.works")),
            client,
            inner.clone(),
        );

        assert!(inner.last_error().is_none());
        let result = inner.drain();
        // 两拍：首拍基线丢弃，仅第二拍成样。
        assert_eq!(result.samples.len(), 1);
        let sample = &result.samples[0];
        assert_eq!(sample.sequence, 1);
        assert!((sample.system.cpu_percent.unwrap() - 7.714988).abs() < 1e-6);

        // 系统内存单位必须是 GB。
        assert_eq!(sample.hwinfo_raw["Harmony Mem Total"].unit, "GB");
        assert!(
            (sample.hwinfo_raw["Harmony Mem Total"].value - 32680648.0 / 1024.0 / 1024.0).abs()
                < 1e-9
        );

        // -PKG 主进程 + 4 个子进程展开，并归组为一个应用。
        let processes = sample.processes.as_ref().unwrap();
        assert_eq!(processes.len(), 5);
        let aggregated = sample.aggregated.as_ref().unwrap();
        assert_eq!(aggregated.len(), 1);
        assert_eq!(aggregated[0].name, "com.huawei.it.works");
        assert_eq!(aggregated[0].process_count, 5);
        // 旧 TopN 采集已删除。
        assert!(sample.top_n_cpu.is_none());
        assert!(sample.top_n_gpu.is_none());
    }

    #[test]
    fn issues_pkill_before_and_after_and_builds_full_command() {
        struct SharedClient {
            inner: Arc<FakeClient>,
        }
        impl MonitorClient for SharedClient {
            type Stream = FakeStream;
            fn shell(
                &self,
                udid: &str,
                command: &str,
                timeout_secs: u64,
                cancel: &AtomicBool,
            ) -> Result<String, Error> {
                self.inner.shell(udid, command, timeout_secs, cancel)
            }
            fn open_stream(&self, udid: &str, command: &str) -> Result<Self::Stream, Error> {
                self.inner.open_stream(udid, command)
            }
        }

        let fake = Arc::new(FakeClient::new(FIXTURE));
        let inner = Arc::new(MonitorInner::new());
        run_monitor(
            config(1.0, Some(60.0), Some("com.huawei.it.works")),
            SharedClient {
                inner: fake.clone(),
            },
            inner,
        );

        assert_eq!(
            fake.shell_calls.lock().unwrap().as_slice(),
            ["pkill SP_daemon", "pkill SP_daemon"]
        );
        assert_eq!(
            fake.stream_commands.lock().unwrap().as_slice(),
            // 时长 60s + 1 拍基线。
            ["SP_daemon -N 61 -c -g -f -t -p -r -net -d -PKG com.huawei.it.works"]
        );
    }

    #[test]
    fn downsampling_keeps_every_nth_block() {
        // 6 拍（1 基线 + 5 数据），间隔 2s → 保留第 1、3、5 拍数据。
        let mut lines = Vec::new();
        for shot in 0..6 {
            lines.push(format!("order:0 TotalcpuUsage={shot}.000000"));
            lines.push("order:1 memTotal=1048576".to_string());
            lines.push("order:2 memAvailable=524288".to_string());
            lines.push(format!("order:3 timestamp={}", 1000 + shot));
            lines.push(String::new());
        }
        lines.push(super::FINISH_MARKER.to_string());
        let client = FakeClient::new(&lines.join("\n"));
        let inner = Arc::new(MonitorInner::new());
        run_monitor(config(2.0, Some(10.0), None), client, inner.clone());

        let result = inner.drain();
        assert_eq!(result.samples.len(), 3);
        let cpu_values: Vec<f64> = result
            .samples
            .iter()
            .map(|sample| sample.system.cpu_percent.unwrap())
            .collect();
        assert_eq!(cpu_values, vec![1.0, 3.0, 5.0]);
        // 无 -PKG 时不产出进程明细。
        assert!(result.samples[0].processes.is_none());
        assert!(result.samples[0].aggregated.is_none());
    }

    #[test]
    fn premature_close_without_output_sets_error() {
        let client = FakeClient::new("");
        let inner = Arc::new(MonitorInner::new());
        run_monitor(config(1.0, Some(10.0), None), client, inner.clone());

        assert!(inner
            .last_error()
            .is_some_and(|message| message.contains("未产出任何数据")));
        assert!(inner.drain().samples.is_empty());
        assert!(!inner.is_running());
    }

    #[test]
    fn stop_before_start_skips_stream_but_still_cleans_up() {
        let fake = Arc::new(FakeClient::new(FIXTURE));
        struct SharedClient {
            inner: Arc<FakeClient>,
        }
        impl MonitorClient for SharedClient {
            type Stream = FakeStream;
            fn shell(
                &self,
                udid: &str,
                command: &str,
                timeout_secs: u64,
                cancel: &AtomicBool,
            ) -> Result<String, Error> {
                self.inner.shell(udid, command, timeout_secs, cancel)
            }
            fn open_stream(&self, udid: &str, command: &str) -> Result<Self::Stream, Error> {
                self.inner.open_stream(udid, command)
            }
        }
        let inner = Arc::new(MonitorInner::new());
        inner.request_stop();
        run_monitor(
            config(1.0, None, None),
            SharedClient {
                inner: fake.clone(),
            },
            inner.clone(),
        );

        // stop 已置位：不开流；启动前 pkill 被取消，结束后 pkill 用独立 cancel 仍执行。
        assert!(fake.stream_commands.lock().unwrap().is_empty());
        assert_eq!(
            fake.shell_calls.lock().unwrap().as_slice(),
            ["pkill SP_daemon"]
        );
        assert!(inner.drain().samples.is_empty());
    }

    #[test]
    fn default_duration_uses_24h_shot_budget() {
        let fake = Arc::new(FakeClient::new(FIXTURE));
        struct SharedClient {
            inner: Arc<FakeClient>,
        }
        impl MonitorClient for SharedClient {
            type Stream = FakeStream;
            fn shell(
                &self,
                udid: &str,
                command: &str,
                timeout_secs: u64,
                cancel: &AtomicBool,
            ) -> Result<String, Error> {
                self.inner.shell(udid, command, timeout_secs, cancel)
            }
            fn open_stream(&self, udid: &str, command: &str) -> Result<Self::Stream, Error> {
                self.inner.open_stream(udid, command)
            }
        }
        let inner = Arc::new(MonitorInner::new());
        run_monitor(
            config(1.0, None, None),
            SharedClient {
                inner: fake.clone(),
            },
            inner,
        );
        assert_eq!(
            fake.stream_commands.lock().unwrap().as_slice(),
            ["SP_daemon -N 86400 -c -g -f -t -p -r -net -d"]
        );
    }

    #[test]
    fn aggregation_groups_children_under_bundle_name() {
        let make = |pid: u32, name: &str, cpu: f64, mem: f64| ProcessInfo {
            pid,
            name: name.to_string(),
            cpu_percent: cpu,
            working_set_mb: mem,
            committed_memory_mb: 0.0,
            gpu_percent: 0.0,
            gpu_memory_mb: 0.0,
            handle_count: 0,
        };
        let values = aggregate_processes(&[
            make(100, "com.huawei.it.works", 10.0, 100.0),
            make(101, "com.huawei.it.works:44831", 5.0, 50.0),
            make(102, "com.huawei.it.works:44977", 3.0, 30.0),
        ]);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].name, "com.huawei.it.works");
        assert_eq!(values[0].pids, vec![100, 101, 102]);
        assert_eq!(values[0].process_count, 3);
        assert_eq!(values[0].cpu_percent_total, 18.0);
        assert_eq!(values[0].working_set_mb_total, 180.0);
    }

    #[test]
    fn bundle_base_name_keeps_non_bundle_names() {
        assert_eq!(bundle_base_name("com.app:render"), "com.app");
        assert_eq!(bundle_base_name("com.app:44831"), "com.app");
        assert_eq!(bundle_base_name("com.app"), "com.app");
        assert_eq!(bundle_base_name("kworker/0:1H"), "kworker/0:1H");
        assert_eq!(bundle_base_name("0:1H"), "0:1H");
    }
}
