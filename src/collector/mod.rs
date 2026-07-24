//! 可选性能指标采集器。
//!
//! P0 采集由 `monitor` 负责。这里的采集器全部是 best-effort：设备端命令、权限和单位
//! 尚未经过 PC/Mobile 真机冻结时，失败只能表示指标不可用，不能用 0 伪造数据。

use std::collections::HashMap;

use crate::data::{Sample, SensorValue};
use crate::error::Error;
use crate::parse::{diff_net_kbps, parse_proc_net_dev, NetDevSnapshot};

pub(crate) const NETWORK_COMMAND: &str = "cat /proc/net/dev";
pub(crate) const POWER_COMMAND: &str = "for p in /sys/class/power_supply/Battery/current_now /sys/class/power_supply/Battery/voltage_now /sys/class/power_supply/battery/current_now /sys/class/power_supply/battery/voltage_now; do [ -r \"$p\" ] && printf '%s=%s\\n' \"$p\" \"$(cat \"$p\")\"; done";
pub(crate) const THERMAL_COMMAND: &str = "for p in /sys/class/thermal/thermal_zone*/temp; do [ -r \"$p\" ] && printf '%s=%s\\n' \"$p\" \"$(cat \"$p\")\"; done";
pub(crate) const GPU_COMMAND: &str = "hidumper --gpu";

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

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub udid: String,
    pub target: CapabilityStatus,
    pub capabilities: HashMap<String, CapabilityStatus>,
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
}

pub(crate) struct NetworkCollector;
pub(crate) struct PowerCollector;
pub(crate) struct ThermalCollector;
pub(crate) struct GpuCollector;

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

pub(crate) fn optional_command(
    enable_network: bool,
    enable_power: bool,
    enable_thermal: bool,
    enable_gpu: bool,
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
    (!parts.is_empty()).then(|| parts.join("; "))
}

pub(crate) fn split_optional_output(text: &str) -> HashMap<String, String> {
    let markers = [
        ("__PERFHARMONY_NETWORK__", "network"),
        ("__PERFHARMONY_POWER__", "power"),
        ("__PERFHARMONY_THERMAL__", "thermal"),
        ("__PERFHARMONY_GPU__", "gpu"),
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
}
