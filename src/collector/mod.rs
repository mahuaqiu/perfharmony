//! 可选性能指标采集器。
//!
//! P0 采集由 `monitor` 负责。这里的采集器全部是 best-effort：设备端命令、权限和单位
//! 尚未经过 PC/Mobile 真机冻结时，失败只能表示指标不可用，不能用 0 伪造数据。

use std::collections::HashMap;

use crate::data::{Sample, SensorValue};
use crate::error::Error;
use crate::parse::{diff_net_kbps, parse_hidumper_cpufreq, parse_proc_net_dev, NetDevSnapshot};

pub(crate) const NETWORK_COMMAND: &str = "cat /proc/net/dev";
pub(crate) const POWER_COMMAND: &str = "for p in /sys/class/power_supply/Battery/current_now /sys/class/power_supply/Battery/voltage_now /sys/class/power_supply/battery/current_now /sys/class/power_supply/battery/voltage_now; do [ -r \"$p\" ] && printf '%s=%s\\n' \"$p\" \"$(cat \"$p\")\"; done";
pub(crate) const THERMAL_COMMAND: &str = "for p in /sys/class/thermal/thermal_zone*/temp; do [ -r \"$p\" ] && printf '%s=%s\\n' \"$p\" \"$(cat \"$p\")\"; done";
pub(crate) const GPU_COMMAND: &str = "hidumper --gpu";
// 真机（Harmony PC）验证：sysfs cpufreq 节点对非 root shell 不可读，hidumper 代读可用。
pub(crate) const CPUFREQ_COMMAND: &str = "hidumper --cpufreq";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiskIoCounters {
    pub read_bytes: u64,
    pub write_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct OptionalState {
    pub previous_net: Option<NetDevSnapshot>,
}

impl Default for OptionalState {
    fn default() -> Self {
        Self { previous_net: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityStatus {
    pub status: String,
    pub reason: Option<String>,
}

impl CapabilityStatus {
    fn available(reason: impl Into<String>) -> Self {
        Self {
            status: "available".to_string(),
            reason: Some(reason.into()),
        }
    }

    fn degraded(reason: impl Into<String>) -> Self {
        Self {
            status: "degraded".to_string(),
            reason: Some(reason.into()),
        }
    }

    fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            status: "unavailable".to_string(),
            reason: Some(reason.into()),
        }
    }
}

/// Collector 的最小生命周期契约。每个 collector 只负责一个能力，输入文本由 Monitor
/// 统一通过 HDC 获取，这样可以在不连接真机时注入 fixture 测试。
pub(crate) trait Collector: Send {
    fn name(&self) -> &'static str;
    fn collect(&mut self, context: &mut CollectContext<'_>) -> Result<(), Error>;
}

pub(crate) struct CollectContext<'a> {
    pub sample: &'a mut Sample,
    pub optional_state: &'a mut OptionalState,
    pub interval_secs: f64,
    pub network_text: Option<&'a str>,
    pub power_text: Option<&'a str>,
    pub thermal_text: Option<&'a str>,
    pub gpu_text: Option<&'a str>,
    pub cpufreq_text: Option<&'a str>,
}

pub(crate) struct NetworkCollector;
pub(crate) struct PowerCollector;
pub(crate) struct ThermalCollector;
pub(crate) struct GpuCollector;
pub(crate) struct CpuFreqCollector;

impl Collector for NetworkCollector {
    fn name(&self) -> &'static str {
        "network"
    }

    fn collect(&mut self, context: &mut CollectContext<'_>) -> Result<(), Error> {
        let text = context
            .network_text
            .ok_or_else(|| Error::ParseFailed("network collector 没有输入".to_string()))?;
        collect_network(
            text,
            context.optional_state,
            context.interval_secs,
            &mut context.sample.hwinfo_raw,
        )
    }
}

impl Collector for PowerCollector {
    fn name(&self) -> &'static str {
        "power"
    }

    fn collect(&mut self, context: &mut CollectContext<'_>) -> Result<(), Error> {
        let text = context
            .power_text
            .ok_or_else(|| Error::ParseFailed("power collector 没有输入".to_string()))?;
        collect_power(text, &mut context.sample.hwinfo_raw)
    }
}

impl Collector for ThermalCollector {
    fn name(&self) -> &'static str {
        "thermal"
    }

    fn collect(&mut self, context: &mut CollectContext<'_>) -> Result<(), Error> {
        let text = context
            .thermal_text
            .ok_or_else(|| Error::ParseFailed("thermal collector 没有输入".to_string()))?;
        collect_thermal(text, &mut context.sample.hwinfo_raw)
    }
}

impl Collector for GpuCollector {
    fn name(&self) -> &'static str {
        "gpu"
    }

    fn collect(&mut self, context: &mut CollectContext<'_>) -> Result<(), Error> {
        let text = context
            .gpu_text
            .ok_or_else(|| Error::ParseFailed("gpu collector 没有输入".to_string()))?;
        collect_gpu(text, context.sample)
    }
}

impl Collector for CpuFreqCollector {
    fn name(&self) -> &'static str {
        "cpufreq"
    }

    fn collect(&mut self, context: &mut CollectContext<'_>) -> Result<(), Error> {
        let text = context
            .cpufreq_text
            .ok_or_else(|| Error::ParseFailed("cpufreq collector 没有输入".to_string()))?;
        collect_cpufreq(text, &mut context.sample.hwinfo_raw)
    }
}

pub(crate) fn optional_command(
    enable_network: bool,
    enable_power: bool,
    enable_thermal: bool,
    enable_gpu: bool,
    enable_cpu_freq: bool,
) -> Option<String> {
    let mut parts = Vec::new();
    if enable_network {
        parts.push(format!(
            "printf '__PERFHARMONY_NETWORK__\\n'; {NETWORK_COMMAND}"
        ));
    }
    if enable_power {
        parts.push(format!(
            "printf '__PERFHARMONY_POWER__\\n'; {POWER_COMMAND}"
        ));
    }
    if enable_thermal {
        parts.push(format!(
            "printf '__PERFHARMONY_THERMAL__\\n'; {THERMAL_COMMAND}"
        ));
    }
    if enable_gpu {
        parts.push(format!("printf '__PERFHARMONY_GPU__\\n'; {GPU_COMMAND}"));
    }
    if enable_cpu_freq {
        parts.push(format!(
            "printf '__PERFHARMONY_CPUFREQ__\\n'; {CPUFREQ_COMMAND}"
        ));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

pub(crate) fn split_optional_output(text: &str) -> HashMap<String, String> {
    let markers = [
        ("__PERFHARMONY_NETWORK__", "network"),
        ("__PERFHARMONY_POWER__", "power"),
        ("__PERFHARMONY_THERMAL__", "thermal"),
        ("__PERFHARMONY_GPU__", "gpu"),
        ("__PERFHARMONY_CPUFREQ__", "cpufreq"),
    ];
    let mut sections = HashMap::new();
    let mut markers_found: Vec<(usize, usize, &str)> = Vec::new();
    for (marker, key) in markers {
        if let Some(index) = text.find(marker) {
            markers_found.push((index, index + marker.len(), key));
        }
    }
    markers_found.sort_by_key(|(start, _, _)| *start);
    for (index, (_, content_start, key)) in markers_found.iter().enumerate() {
        let content_end = markers_found
            .get(index + 1)
            .map(|(start, _, _)| *start)
            .unwrap_or(text.len());
        sections.insert(
            (*key).to_string(),
            text[*content_start..content_end]
                .trim_matches(['\r', '\n', ';', ' '])
                .to_string(),
        );
    }
    sections
}

/// 将网络计数器写入当前样本。
pub(crate) fn collect_network(
    text: &str,
    state: &mut OptionalState,
    interval_secs: f64,
    raw: &mut HashMap<String, SensorValue>,
) -> Result<(), Error> {
    let current = parse_proc_net_dev(text)?;
    let (download_kbps, upload_kbps) = state
        .previous_net
        .map(|previous| diff_net_kbps(&previous, &current, interval_secs.max(0.001)))
        .unwrap_or((0.0, 0.0));
    state.previous_net = Some(current);
    add_sensor(raw, "Harmony Net Upload", upload_kbps, "KB/s");
    add_sensor(raw, "Harmony Net Download", download_kbps, "KB/s");
    Ok(())
}

/// 解析标准 power_supply 节点。节点通常只有数值，没有可靠的单位元数据，因此只保存
/// 原始读数；`Harmony Power` 必须等真机校准 current_now/voltage_now 的单位后再写入。
pub(crate) fn collect_power(
    text: &str,
    raw: &mut HashMap<String, SensorValue>,
) -> Result<(), Error> {
    let mut found = false;
    for line in text.lines() {
        let Some((path, value)) = line.split_once('=') else {
            continue;
        };
        let value = value
            .trim()
            .parse::<f64>()
            .map_err(|_| Error::ParseFailed(format!("power 节点数值无法解析: {value}")))?;
        let path_lower = path.to_ascii_lowercase();
        if path_lower.ends_with("current_now") {
            add_sensor(raw, "Harmony Battery Current Raw", value, "raw");
            found = true;
        } else if path_lower.ends_with("voltage_now") {
            add_sensor(raw, "Harmony Battery Voltage Raw", value, "raw");
            found = true;
        }
    }
    if found {
        Ok(())
    } else {
        Err(Error::ParseFailed(
            "未找到 current_now/voltage_now".to_string(),
        ))
    }
}

/// 解析标准 thermal_zone 温度节点。Harmony 设备实际节点名称和单位仍需真机校准。
pub(crate) fn collect_thermal(
    text: &str,
    raw: &mut HashMap<String, SensorValue>,
) -> Result<(), Error> {
    let mut first_temp = None;
    for line in text.lines() {
        let Some((path, value)) = line.split_once('=') else {
            continue;
        };
        let value = value
            .trim()
            .parse::<f64>()
            .map_err(|_| Error::ParseFailed(format!("thermal 节点数值无法解析: {value}")))?;
        if path.to_ascii_lowercase().contains("temp") {
            first_temp.get_or_insert(value / 1000.0);
        }
    }
    let Some(temp_c) = first_temp else {
        return Err(Error::ParseFailed(
            "未找到 thermal_zone 温度节点".to_string(),
        ));
    };
    add_sensor(raw, "Harmony CPU Temp", temp_c, "°C");
    Ok(())
}

/// 仅接受带有百分号的 GPU 输出，未知格式返回解析失败而不是写入 0。
pub(crate) fn collect_gpu(text: &str, sample: &mut Sample) -> Result<(), Error> {
    let percent = text.lines().find_map(|line| {
        let percent_index = line.find('%')?;
        let before = &line[..percent_index];
        let token = before.split_whitespace().last()?;
        token
            .parse::<f64>()
            .ok()
            .filter(|value| (0.0..=100.0).contains(value))
    });
    let Some(percent) = percent else {
        return Err(Error::ParseFailed(
            "hidumper GPU 输出没有可确认的百分比字段".to_string(),
        ));
    };
    sample.system.gpu_percent = Some(percent);
    sample.system.gpu_source = "harmony_hidumper".to_string();
    add_sensor(&mut sample.hwinfo_raw, "Harmony GPU Usage", percent, "%");
    Ok(())
}

/// 各核实时主频写入传感器表：每核一个 key + 全核平均值。
pub(crate) fn collect_cpufreq(
    text: &str,
    raw: &mut HashMap<String, SensorValue>,
) -> Result<(), Error> {
    let cores = parse_hidumper_cpufreq(text)?;
    let mut sum = 0.0;
    for core in &cores {
        add_sensor(
            raw,
            &format!("Harmony CPU{} Freq", core.core),
            core.cur_mhz,
            "MHz",
        );
        sum += core.cur_mhz;
    }
    add_sensor(raw, "Harmony CPU Freq Avg", sum / cores.len() as f64, "MHz");
    Ok(())
}

/// 目标进程磁盘 IO 采集命令。/proc/pid/io 对非 root shell 不可读（真机验证），
/// 只能经 hidumper --storage <pid> 代读，多个 pid 合并为一次 shell 往返。
pub(crate) fn disk_io_command(pids: &[u32]) -> String {
    let list = pids
        .iter()
        .map(|pid| pid.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "for p in {list}; do printf '__PERFHARMONY_DISK_%s__\\n' \"$p\"; hidumper --storage \"$p\"; done"
    )
}

/// 解析磁盘 IO 累计计数并与上一轮差分为 KB/s（目标进程合计）。
/// 首轮或 pid 新出现（进程重启）时没有基线，该 pid 不计入当轮增量。
pub(crate) fn collect_disk_io(
    text: &str,
    previous: &mut HashMap<u32, DiskIoCounters>,
    interval_secs: f64,
    raw: &mut HashMap<String, SensorValue>,
) -> Result<(), Error> {
    let current = parse_disk_sections(text);
    if current.is_empty() {
        return Err(Error::ParseFailed(
            "hidumper --storage 输出没有可解析的 read_bytes/write_bytes".to_string(),
        ));
    }
    let seconds = interval_secs.max(0.001);
    let mut read_kbps = 0.0;
    let mut write_kbps = 0.0;
    for (pid, counters) in &current {
        if let Some(prev) = previous.get(pid) {
            read_kbps +=
                counters.read_bytes.saturating_sub(prev.read_bytes) as f64 / 1024.0 / seconds;
            write_kbps +=
                counters.write_bytes.saturating_sub(prev.write_bytes) as f64 / 1024.0 / seconds;
        }
    }
    *previous = current;
    add_sensor(raw, "Harmony Disk Read", read_kbps, "KB/s");
    add_sensor(raw, "Harmony Disk Write", write_kbps, "KB/s");
    Ok(())
}

fn parse_disk_sections(text: &str) -> HashMap<u32, DiskIoCounters> {
    let mut result = HashMap::new();
    let mut pid: Option<u32> = None;
    let mut read_bytes: Option<u64> = None;
    let mut write_bytes: Option<u64> = None;
    let flush = |pid: &mut Option<u32>,
                 read_bytes: &mut Option<u64>,
                 write_bytes: &mut Option<u64>,
                 result: &mut HashMap<u32, DiskIoCounters>| {
        if let (Some(id), Some(read), Some(write)) = (*pid, *read_bytes, *write_bytes) {
            result.insert(
                id,
                DiskIoCounters {
                    read_bytes: read,
                    write_bytes: write,
                },
            );
        }
        *read_bytes = None;
        *write_bytes = None;
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("__PERFHARMONY_DISK_") {
            flush(&mut pid, &mut read_bytes, &mut write_bytes, &mut result);
            pid = rest.trim_end_matches("__").parse::<u32>().ok();
        } else if let Some(value) = trimmed.strip_prefix("read_bytes:") {
            read_bytes = value.trim().parse::<u64>().ok();
        } else if let Some(value) = trimmed.strip_prefix("write_bytes:") {
            write_bytes = value.trim().parse::<u64>().ok();
        }
    }
    flush(&mut pid, &mut read_bytes, &mut write_bytes, &mut result);
    result
}

pub(crate) fn add_sensor(
    raw: &mut HashMap<String, SensorValue>,
    key: &str,
    value: f64,
    unit: &str,
) {
    raw.insert(
        key.to_string(),
        SensorValue {
            value,
            unit: unit.to_string(),
        },
    );
}

pub(crate) fn classify_probe(
    output: Result<String, Error>,
    parsed: Result<(), Error>,
    command: &str,
) -> CapabilityStatus {
    match (output, parsed) {
        (Ok(_), Ok(())) => CapabilityStatus::available(format!("{command} 可解析")),
        (Ok(_), Err(error)) => {
            CapabilityStatus::degraded(format!("{command} 返回但格式未冻结: {error}"))
        }
        (Err(error), _) => CapabilityStatus::unavailable(format!("{command} 不可用: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_first_sample_is_zero_and_second_sample_is_delta() {
        let first = include_str!("../../tests/fixtures/proc_net_dev.txt");
        let second = first
            .replace("10000000", "11048576")
            .replace("5000000", "6048576");
        let mut state = OptionalState::default();
        let mut raw = HashMap::new();

        collect_network(first, &mut state, 1.0, &mut raw).unwrap();
        assert_eq!(raw["Harmony Net Upload"].value, 0.0);
        collect_network(&second, &mut state, 1.0, &mut raw).unwrap();
        assert!(raw["Harmony Net Upload"].value > 1000.0);
        assert!(raw["Harmony Net Download"].value > 1000.0);
    }

    #[test]
    fn power_keeps_raw_values_without_claiming_watt_unit() {
        let mut raw = HashMap::new();
        collect_power("/sys/class/power_supply/Battery/current_now=120000\n/sys/class/power_supply/Battery/voltage_now=3800000\n", &mut raw).unwrap();
        assert_eq!(raw["Harmony Battery Current Raw"].unit, "raw");
        assert!(!raw.contains_key("Harmony Power"));
    }

    #[test]
    fn cpufreq_writes_per_core_and_average() {
        let text = "cmd is: cat /sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_cur_freq\n\n1000000\n\ncmd is: cat /sys/devices/system/cpu/cpu1/cpufreq/cpuinfo_cur_freq\n\n2000000\n";
        let mut raw = HashMap::new();
        collect_cpufreq(text, &mut raw).unwrap();
        assert_eq!(raw["Harmony CPU0 Freq"].value, 1000.0);
        assert_eq!(raw["Harmony CPU1 Freq"].value, 2000.0);
        assert_eq!(raw["Harmony CPU Freq Avg"].value, 1500.0);
        assert_eq!(raw["Harmony CPU Freq Avg"].unit, "MHz");
    }

    #[test]
    fn optional_command_includes_cpufreq_marker() {
        let command = optional_command(false, false, false, false, true).unwrap();
        assert!(command.contains("__PERFHARMONY_CPUFREQ__"));
        assert!(command.contains("hidumper --cpufreq"));
        let sections = split_optional_output("__PERFHARMONY_CPUFREQ__\ncmd is: x\n");
        assert!(sections.contains_key("cpufreq"));
    }

    #[test]
    fn disk_io_first_sample_is_zero_and_second_sample_is_delta() {
        let first = "__PERFHARMONY_DISK_100__\n/proc/100/io\nread_bytes: 1048576\nwrite_bytes: 2097152\n__PERFHARMONY_DISK_200__\nread_bytes: 0\nwrite_bytes: 0\n";
        let second = "__PERFHARMONY_DISK_100__\nread_bytes: 2097152\nwrite_bytes: 4194304\n__PERFHARMONY_DISK_200__\nread_bytes: 1024\nwrite_bytes: 2048\n";
        let mut previous = HashMap::new();
        let mut raw = HashMap::new();

        collect_disk_io(first, &mut previous, 2.0, &mut raw).unwrap();
        assert_eq!(raw["Harmony Disk Read"].value, 0.0);
        assert_eq!(raw["Harmony Disk Write"].value, 0.0);

        collect_disk_io(second, &mut previous, 2.0, &mut raw).unwrap();
        // pid 100: +1MB 读/+2MB 写，pid 200: +1KB/+2KB，间隔 2s。
        assert_eq!(raw["Harmony Disk Read"].value, (1024.0 + 1.0) / 2.0);
        assert_eq!(raw["Harmony Disk Write"].value, (2048.0 + 2.0) / 2.0);
    }

    #[test]
    fn disk_io_command_joins_pids_in_single_shell() {
        let command = disk_io_command(&[100, 200]);
        assert!(command.contains("for p in 100 200;"));
        assert!(command.contains("hidumper --storage"));
        assert!(command.contains("__PERFHARMONY_DISK_%s__"));
    }

    #[test]
    fn thermal_converts_standard_millidegree_value() {
        let mut raw = HashMap::new();
        collect_thermal("/sys/class/thermal/thermal_zone0/temp=42000\n", &mut raw).unwrap();
        assert_eq!(raw["Harmony CPU Temp"].value, 42.0);
    }

    #[test]
    fn gpu_requires_a_confirmed_percent_field() {
        let mut sample = Sample {
            sequence: 1,
            elapsed_ms: 0,
            system: Default::default(),
            timestamp: chrono::Utc::now(),
            hwinfo_raw: HashMap::new(),
            processes: None,
            aggregated: None,
            top_n_cpu: None,
            top_n_gpu: None,
        };
        assert!(collect_gpu("unknown gpu output", &mut sample).is_err());
        collect_gpu("GPU load: 12.5%\n", &mut sample).unwrap();
        assert_eq!(sample.system.gpu_percent, Some(12.5));
    }

    #[test]
    fn split_optional_output_keeps_marker_boundaries() {
        let output = concat!(
            "__PERFHARMONY_NETWORK__\r\n",
            "eth0: 1 2 3 4\r\n",
            "__PERFHARMONY_POWER__\n",
            "current_now=120000\n",
            "__PERFHARMONY_GPU__\n",
            "GPU load: 12.5%\n",
        );
        let sections = split_optional_output(output);
        assert_eq!(sections["network"], "eth0: 1 2 3 4");
        assert_eq!(sections["power"], "current_now=120000");
        assert_eq!(sections["gpu"], "GPU load: 12.5%");
        assert!(!sections.contains_key("thermal"));
    }
}
