//! SP_daemon（SmartPerf）流式输出解析。
//!
//! 0.2.0 起设备侧唯一数据源：`SP_daemon -N <拍数> -c -g -f -t -p -r -net -d
//! [-PKG 包名]` 以固定 1s 节奏逐拍输出 `order:N key=value` 块，块间以空行
//! 分隔，正常结束时输出 `command exec finished!`。
//!
//! 注意：SP_daemon 检测到 stdout 非终端（设备端接管道）会静默不输出且退出码
//! 为 0，因此设备端命令绝不能带管道，过滤与解析全部在宿主机侧完成。
//!
//! 指标采用白名单制：真机实测恒 0 的字段（cluster0 / gpu 温度点 /
//! cpuNFrequency）不在白名单内，直接丢弃；npu_thermal / voltageNow
//! 在本真机恒 0（不支持），但官方透传 sysfs 留了口子：NPU 温度 /
//! 电压仅在有值（>0）设备上报；功耗只要有电流读数就产出，电压
//! 不支持的设备按 0W 上报。
//! 单位约定：系统内存 GB、进程内存 MB、频率 MHz、温度 ℃、电流 mA、
//! 电压 V、功耗 W、网络 KB/s。

use std::collections::HashMap;

use crate::data::{ProcessInfo, SensorValue};

/// SP_daemon 正常结束时的标记行。
pub const FINISH_MARKER: &str = "command exec finished!";

const KB_PER_GB: f64 = 1024.0 * 1024.0;
const KB_PER_MB: f64 = 1024.0;
const KHZ_PER_MHZ: f64 = 1000.0;
const HZ_PER_MHZ: f64 = 1_000_000.0;
const BYTES_PER_KB: f64 = 1024.0;
// sysfs voltage_now 标准单位为 µV（Linux power_supply ABI）。
const UV_PER_V: f64 = 1_000_000.0;
const MA_PER_A: f64 = 1000.0;

/// 系统 CPU 使用率细分（SP_daemon 键 → 对外传感器键），单位 %。
const CPU_USAGE_KEYS: [(&str, &str); 4] = [
    ("TotalcpuUsage", "Harmony CPU Usage"),
    ("TotalcpuuserUsage", "Harmony CPU User"),
    ("TotalcpusystemUsage", "Harmony CPU System"),
    ("TotalcpuidleUsage", "Harmony CPU Idle"),
];

/// 温度点白名单（真机实测有效），单位 ℃；cluster0/gpu 恒 0 不采，
/// npu_thermal 本真机恒 0 但口子保留，在白名单外门控上报。
const TEMP_KEYS: [(&str, &str); 6] = [
    ("Battery", "Harmony Battery Temp"),
    ("soc_thermal", "Harmony SoC Temp"),
    ("shell_front", "Harmony Shell Front Temp"),
    ("shell_back", "Harmony Shell Back Temp"),
    ("shell_frame", "Harmony Shell Frame Temp"),
    ("system_h", "Harmony System Temp"),
];

/// 构造 SP_daemon 长跑采集命令；`total_samples` 为总拍数（1 拍 = 1 秒）。
/// package（-PKG）与 pid（-PID）互斥，由上层校验；真机实证一次只能带其一。
pub fn sp_daemon_command(total_samples: u32, package: Option<&str>, pid: Option<u32>) -> String {
    let mut command = format!("SP_daemon -N {total_samples} -c -g -f -t -p -r -net -d");
    if let Some(package) = package {
        command.push_str(" -PKG ");
        command.push_str(package);
    }
    if let Some(pid) = pid {
        command.push_str(" -PID ");
        command.push_str(&pid.to_string());
    }
    command
}

/// 增量流式块解析器：逐行喂入 hdc stdout，攒满一拍返回该块的原始键值。
#[derive(Debug, Default)]
pub struct SpStreamParser {
    current: HashMap<String, String>,
}

impl SpStreamParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一行；当一拍数据块完整时返回该块。
    ///
    /// 块完整判定：块间空行且已见 timestamp 字段（timestamp 位于块内排序
    /// 位置而非末行，之后仍可能有 voltageNow 等字段，故以空行为准）；
    /// 缺空行时由下一块的 order:0 兜底冲刷；结束标记行冲刷最后一块。
    pub fn push_line(&mut self, line: &str) -> Option<HashMap<String, String>> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if self.current.contains_key("timestamp") {
                return Some(std::mem::take(&mut self.current));
            }
            return None;
        }
        if trimmed == FINISH_MARKER {
            return self.finish();
        }
        let rest = trimmed.strip_prefix("order:")?;
        let (order, pair) = rest.split_once(' ')?;
        let (key, value) = pair.split_once('=')?;
        let flushed = if order == "0" && !self.current.is_empty() {
            Some(std::mem::take(&mut self.current))
        } else {
            None
        };
        self.current.insert(key.to_string(), value.to_string());
        flushed
    }

    /// 流结束时冲刷残余块（如 hdc 进程被终止、无结束标记）。
    pub fn finish(&mut self) -> Option<HashMap<String, String>> {
        if self.current.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.current))
        }
    }
}

/// 一次性解析完整输出中的全部块（probe 等非流式场景）。
pub fn parse_blocks(text: &str) -> Vec<HashMap<String, String>> {
    let mut parser = SpStreamParser::new();
    let mut blocks = Vec::new();
    for line in text.lines() {
        if let Some(block) = parser.push_line(line) {
            blocks.push(block);
        }
    }
    if let Some(block) = parser.finish() {
        blocks.push(block);
    }
    blocks
}

/// 块内 timestamp 字段（设备侧毫秒时间戳）。
#[cfg(test)]
pub fn block_timestamp_ms(block: &HashMap<String, String>) -> Option<u64> {
    block.get("timestamp")?.trim().parse().ok()
}

/// 把一拍原始块转换为系统级传感器（白名单 + 单位换算）。
pub fn build_system_sensors(block: &HashMap<String, String>) -> HashMap<String, SensorValue> {
    let mut raw = HashMap::new();

    // 系统 CPU 使用率细分（%）。
    for (sp_key, sensor_key) in CPU_USAGE_KEYS {
        if let Some(value) = get_f64(block, sp_key) {
            add_sensor(&mut raw, sensor_key, clamp_percent(value), "%");
        }
    }

    // 各核当前主频 cpuN_curFrequency（kHz→MHz）仅用于计算平均主频；
    // 逐核键信息量低且核数随设备动态变化，不单独上报。
    // cpuNFrequency 真机恒 0，不采。
    let mut freq_values: Vec<f64> = Vec::new();
    for (key, _) in block.iter() {
        let Some(rest) = key.strip_prefix("cpu") else {
            continue;
        };
        let Some(core) = rest.strip_suffix("_curFrequency") else {
            continue;
        };
        if core.parse::<u32>().is_err() {
            continue;
        }
        let Some(khz) = get_f64(block, key) else {
            continue;
        };
        if khz <= 0.0 {
            continue;
        }
        freq_values.push(khz / KHZ_PER_MHZ);
    }
    if !freq_values.is_empty() {
        let avg = freq_values.iter().sum::<f64>() / freq_values.len() as f64;
        add_sensor(&mut raw, "Harmony CPU Freq Avg", avg, "MHz");
    }

    // 系统内存（KB→GB）；Used = Total − Available，与任务管理器口径一致。
    let total_kb = get_f64(block, "memTotal");
    let available_kb = get_f64(block, "memAvailable");
    if let Some(total) = total_kb {
        add_sensor(&mut raw, "Harmony Mem Total", total / KB_PER_GB, "GB");
    }
    if let Some(available) = available_kb {
        add_sensor(
            &mut raw,
            "Harmony Mem Available",
            available / KB_PER_GB,
            "GB",
        );
    }
    if let (Some(total), Some(available)) = (total_kb, available_kb) {
        add_sensor(
            &mut raw,
            "Harmony Mem Used",
            (total - available).max(0.0) / KB_PER_GB,
            "GB",
        );
    }

    // 主进程内存细分（KB→MB），诊断用：与任务管理器/手机管家对数时逐项比对。
    const PROCESS_MEM_KEYS: [(&str, &str); 4] = [
        ("nativeHeapPss", "Harmony Native Heap Pss"),
        ("graphicPss", "Harmony Graphic Pss"),
        ("arktsHeapPss", "Harmony ArkTS Heap Pss"),
        ("swapPss", "Harmony Swap Pss"),
    ];
    for (sp_key, sensor_key) in PROCESS_MEM_KEYS {
        if let Some(kb) = get_f64(block, sp_key) {
            add_sensor(&mut raw, sensor_key, kb / KB_PER_MB, "MB");
        }
    }

    // GPU 负载（%）与频率（Hz→MHz）。
    if let Some(value) = get_f64(block, "gpuLoad") {
        add_sensor(&mut raw, "Harmony GPU Load", clamp_percent(value), "%");
    }
    if let Some(value) = get_f64(block, "gpuFrequency") {
        add_sensor(&mut raw, "Harmony GPU Freq", value / HZ_PER_MHZ, "MHz");
    }

    // DDR 内存频率（Hz→MHz）。
    if let Some(value) = get_f64(block, "ddrFrequency") {
        add_sensor(&mut raw, "Harmony DDR Freq", value / HZ_PER_MHZ, "MHz");
    }

    // 温度点（℃）。
    for (sp_key, sensor_key) in TEMP_KEYS {
        if let Some(value) = get_f64(block, sp_key) {
            add_sensor(&mut raw, sensor_key, value, "℃");
        }
    }
    // NPU 温度（℃）：本真机恒 0（不支持），仅在有值设备上报。
    if let Some(value) = get_f64(block, "npu_thermal").filter(|v| *v > 0.0) {
        add_sensor(&mut raw, "Harmony NPU Temp", value, "℃");
    }

    // 电池电流（mA）；符号仅表示充放电方向，取绝对值。
    let current_ma = get_f64(block, "currentNow").map(f64::abs);
    if let Some(value) = current_ma {
        add_sensor(&mut raw, "Harmony Battery Current", value, "mA");
    }
    // 电池电压（sysfs voltage_now µV→V）与功耗（W = V × A）：
    // 电压仅在有值（>0）设备上报；功耗只要有电流读数就产出，
    // voltageNow 恒 0（不支持）的设备按 0W 上报，保持功耗序列恒存在。
    if let Some(voltage) = get_f64(block, "voltageNow").map(|uv| uv.abs() / UV_PER_V) {
        if voltage > 0.0 {
            add_sensor(&mut raw, "Harmony Battery Voltage", voltage, "V");
        }
        if let Some(ma) = current_ma {
            add_sensor(&mut raw, "Harmony Power", voltage * ma / MA_PER_A, "W");
        }
    }

    // 网络上下行速率（B/s→KB/s）。
    if let Some(value) = get_f64(block, "networkUp") {
        add_sensor(&mut raw, "Harmony Net Up", value / BYTES_PER_KB, "KB/s");
    }
    if let Some(value) = get_f64(block, "networkDown") {
        add_sensor(&mut raw, "Harmony Net Down", value / BYTES_PER_KB, "KB/s");
    }

    // 实时帧率；前台无渲染时 fps=0 属正常。refreshrate 常为空，空值不上报。
    if let Some(value) = get_f64(block, "fps") {
        add_sensor(&mut raw, "Harmony FPS", value, "fps");
    }
    if let Some(value) = get_f64(block, "refreshrate") {
        add_sensor(&mut raw, "Harmony Refresh Rate", value, "Hz");
    }

    raw
}

/// 汇总归组键：`包名:子进程标识` 取冒号前的包名，与任务管理器按应用汇总一致；
/// 非包名形态（如内核线程 kworker/0:1H）不含 `.`，保持原名不归组。
pub fn bundle_base_name(name: &str) -> &str {
    match name.split_once(':') {
        Some((base, _)) if base.contains('.') && !base.contains('/') => base,
        _ => name,
    }
}

/// 常驻内存（KB）= pss − swapPss；swapPss 缺失（NA/旧设备）时回退 pss。
/// 口径对齐：任务管理器展示的是真驻留在物理内存里、归属该进程的部分。
fn resident_kb(block: &HashMap<String, String>, pss_key: &str, swap_key: &str) -> Option<f64> {
    let pss = get_f64(block, pss_key)?;
    let swap = get_f64(block, swap_key).unwrap_or(0.0);
    Some((pss - swap).max(0.0))
}

/// 把 -PKG 块转换为进程明细：主进程 + 竖线数组展开的子进程。
///
/// 子进程无名字，命名为 `包名:pid`；ProcAppName 本身可能是子进程名
/// （如 `com.xx:Native_xx`），取包名基名拼接，避免出现三段名；
/// 内存取常驻口径 pss−swapPss（KB→MB），GPU 显存取 gpuPss/childGpuPss（KB→MB）。
pub fn build_processes(block: &HashMap<String, String>) -> Option<Vec<ProcessInfo>> {
    let app_name = block
        .get("ProcAppName")
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())?;
    let main_pid: u32 = block.get("ProcId")?.trim().parse().ok()?;
    let base_name = bundle_base_name(app_name);

    let child_ids = parse_pipe_u32(block.get("ChildProcId").map(String::as_str).unwrap_or(""));
    let child_cpu = parse_pipe_f64(
        block
            .get("ChildProcCpuUsage")
            .map(String::as_str)
            .unwrap_or(""),
    );
    let child_pss = parse_child_map(block.get("childPss").map(String::as_str).unwrap_or(""));
    let child_swap_pss =
        parse_child_map(block.get("childSwapPss").map(String::as_str).unwrap_or(""));
    let child_gpu_pss = parse_child_map(block.get("childGpuPss").map(String::as_str).unwrap_or(""));

    let mut result = vec![ProcessInfo {
        pid: main_pid,
        name: app_name.to_string(),
        cpu_percent: get_f64(block, "ProcCpuUsage")
            .map(clamp_percent)
            .unwrap_or(0.0),
        working_set_mb: resident_kb(block, "pss", "swapPss")
            .map(|kb| kb / KB_PER_MB)
            .unwrap_or(0.0),
        committed_memory_mb: 0.0,
        gpu_percent: 0.0,
        gpu_memory_mb: get_f64(block, "gpuPss")
            .map(|kb| kb / KB_PER_MB)
            .unwrap_or(0.0),
        handle_count: 0,
    }];

    for (index, pid) in child_ids.iter().enumerate() {
        let Some(pid) = pid else {
            continue;
        };
        if *pid == main_pid {
            continue;
        }
        result.push(ProcessInfo {
            pid: *pid,
            name: format!("{base_name}:{pid}"),
            cpu_percent: child_cpu
                .get(index)
                .copied()
                .flatten()
                .map(clamp_percent)
                .unwrap_or(0.0),
            working_set_mb: child_pss
                .get(pid)
                .map(|kb| {
                    (kb - child_swap_pss.get(pid).copied().unwrap_or(0.0)).max(0.0) / KB_PER_MB
                })
                .unwrap_or(0.0),
            committed_memory_mb: 0.0,
            gpu_percent: 0.0,
            gpu_memory_mb: child_gpu_pss
                .get(pid)
                .map(|kb| kb / KB_PER_MB)
                .unwrap_or(0.0),
            handle_count: 0,
        });
    }

    Some(result)
}

/// 解析竖线分隔数组（如 `0.1|0|0.2|`）；尾部分隔符去掉，空元素为 None 保位。
pub fn parse_pipe_f64(value: &str) -> Vec<Option<f64>> {
    split_pipe(value)
        .map(|item| {
            if item.is_empty() {
                None
            } else {
                item.parse().ok()
            }
        })
        .collect()
}

/// 解析竖线分隔 pid 数组（如 `44831|44974|`）。
pub fn parse_pipe_u32(value: &str) -> Vec<Option<u32>> {
    split_pipe(value)
        .map(|item| {
            if item.is_empty() {
                None
            } else {
                item.parse().ok()
            }
        })
        .collect()
}

/// 解析 `pid:值|pid:值|` 形式的 child 映射；空值（如 `44974:`）不入表。
pub fn parse_child_map(value: &str) -> HashMap<u32, f64> {
    let mut result = HashMap::new();
    for segment in split_pipe(value) {
        let Some((pid, item)) = segment.split_once(':') else {
            continue;
        };
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        if let Ok(item) = item.parse::<f64>() {
            result.insert(pid, item);
        }
    }
    result
}

fn split_pipe(value: &str) -> impl Iterator<Item = &str> {
    let trimmed = value.trim();
    let trimmed = trimmed.strip_suffix('|').unwrap_or(trimmed);
    trimmed.split('|').filter(move |_| !trimmed.is_empty())
}

fn get_f64(block: &HashMap<String, String>, key: &str) -> Option<f64> {
    let value = block.get(key)?.trim();
    if value.is_empty() {
        return None;
    }
    value.parse().ok()
}

fn add_sensor(raw: &mut HashMap<String, SensorValue>, key: &str, value: f64, unit: &str) {
    let value = if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    };
    raw.insert(
        key.to_string(),
        SensorValue {
            value,
            unit: unit.to_string(),
        },
    );
}

fn clamp_percent(value: f64) -> f64 {
    value.clamp(0.0, 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/real_device2/sp_daemon_n2_full.txt");

    fn parse_fixture_blocks() -> Vec<HashMap<String, String>> {
        parse_blocks(FIXTURE)
    }

    #[test]
    fn stream_parser_splits_real_device_blocks() {
        let blocks = parse_fixture_blocks();
        assert_eq!(blocks.len(), 2);
        // 全量组合真机一拍 264 字段。
        assert_eq!(blocks[0].len(), 264);
        assert_eq!(block_timestamp_ms(&blocks[0]), Some(1785075641331));
        assert_eq!(block_timestamp_ms(&blocks[1]), Some(1785075642354));
        // 第一拍是差分基线，CPU 全 0；第二拍有真实值。
        assert_eq!(blocks[0]["TotalcpuUsage"], "4.000000");
        assert_eq!(blocks[1]["TotalcpuUsage"], "7.714988");
    }

    #[test]
    fn stream_parser_flushes_on_next_order0_without_blank_line() {
        let mut parser = SpStreamParser::new();
        assert!(parser.push_line("order:0 a=1").is_none());
        assert!(parser.push_line("order:1 timestamp=100").is_none());
        let flushed = parser.push_line("order:0 a=2").unwrap();
        assert_eq!(flushed["a"], "1");
        let last = parser.finish().unwrap();
        assert_eq!(last["a"], "2");
    }

    #[test]
    fn stream_parser_ignores_blank_lines_before_timestamp() {
        let mut parser = SpStreamParser::new();
        assert!(parser.push_line("").is_none());
        assert!(parser.push_line("order:0 a=1").is_none());
        // timestamp 未出现，空行不冲刷（防御输出中间的意外空行）。
        assert!(parser.push_line("").is_none());
        assert!(parser.push_line("order:1 timestamp=5").is_none());
        let block = parser.push_line("").unwrap();
        assert_eq!(block.len(), 2);
        assert!(parser.finish().is_none());
    }

    #[test]
    fn system_sensors_use_expected_units_and_whitelist() {
        let blocks = parse_fixture_blocks();
        let sensors = build_system_sensors(&blocks[1]);

        // CPU %。
        assert!((sensors["Harmony CPU Usage"].value - 7.714988).abs() < 1e-6);
        assert_eq!(sensors["Harmony CPU Usage"].unit, "%");
        assert!((sensors["Harmony CPU System"].value - 4.127764).abs() < 1e-6);

        // 内存 GB。
        assert_eq!(sensors["Harmony Mem Total"].unit, "GB");
        assert!((sensors["Harmony Mem Total"].value - 32680648.0 / 1024.0 / 1024.0).abs() < 1e-9);
        assert!(
            (sensors["Harmony Mem Used"].value - (32680648.0 - 20165632.0) / 1024.0 / 1024.0).abs()
                < 1e-9
        );

        // 主频 MHz：逐核键不再单独上报，只保留平均主频。
        assert!(!sensors
            .keys()
            .any(|key| key.starts_with("Harmony CPU") && key.ends_with(" Freq")));
        // 第二拍有值核：cpu0=418、cpu1=830、cpu2=1315；空值核（cpu3）不参与平均。
        assert!(
            (sensors["Harmony CPU Freq Avg"].value - (418.0 + 830.0 + 1315.0) / 3.0).abs() < 1e-9
        );

        // GPU / DDR MHz。
        assert_eq!(sensors["Harmony GPU Freq"].value, 329.0);
        assert_eq!(sensors["Harmony GPU Load"].value, 0.0);
        assert_eq!(sensors["Harmony DDR Freq"].value, 418.0);

        // 温度 ℃ 与电流 mA。
        assert_eq!(sensors["Harmony SoC Temp"].value, 30.0);
        assert!((sensors["Harmony System Temp"].value - 34.0).abs() < 1e-9);
        assert_eq!(sensors["Harmony Battery Current"].value, 30.0);
        assert_eq!(sensors["Harmony Battery Current"].unit, "mA");

        // FPS 有值（0 正常）；refreshrate 为空不上报。
        assert_eq!(sensors["Harmony FPS"].value, 0.0);
        assert!(!sensors.contains_key("Harmony Refresh Rate"));

        // 恒 0 字段：cluster0 不在白名单；npu_thermal 门控不产出。
        // voltageNow 恒 0：电压不产出，功耗按 0W 恒上报。
        assert!(!sensors.keys().any(|key| key.contains("cluster0")));
        assert!(!sensors.keys().any(|key| key.contains("NPU")));
        assert!(!sensors
            .keys()
            .any(|key| key.to_lowercase().contains("voltage")));
        assert_eq!(sensors["Harmony Power"].value, 0.0);
        assert_eq!(sensors["Harmony Power"].unit, "W");

        // 主进程内存细分（MB），用于与任务管理器对数。
        assert_eq!(sensors["Harmony Native Heap Pss"].unit, "MB");
        let native_kb = blocks[1]["nativeHeapPss"].trim().parse::<f64>().unwrap();
        assert!((sensors["Harmony Native Heap Pss"].value - native_kb / 1024.0).abs() < 1e-9);
        let swap_kb = blocks[1]["swapPss"].trim().parse::<f64>().unwrap();
        assert!((sensors["Harmony Swap Pss"].value - swap_kb / 1024.0).abs() < 1e-9);
        assert!(sensors.contains_key("Harmony Graphic Pss"));
        assert!(sensors.contains_key("Harmony ArkTS Heap Pss"));
    }

    #[test]
    fn npu_temp_voltage_power_reported_when_device_supports() {
        // 支持设备上 npu_thermal/voltageNow 有值，门控放行并换算单位：
        // voltage_now µV→V，功耗 W = V × A。
        let mut block = HashMap::new();
        block.insert("npu_thermal".to_string(), "41.000000".to_string());
        block.insert("voltageNow".to_string(), "3850000".to_string());
        block.insert("currentNow".to_string(), "-200".to_string());
        let sensors = build_system_sensors(&block);

        assert_eq!(sensors["Harmony NPU Temp"].value, 41.0);
        assert_eq!(sensors["Harmony NPU Temp"].unit, "℃");
        assert!((sensors["Harmony Battery Voltage"].value - 3.85).abs() < 1e-9);
        assert_eq!(sensors["Harmony Battery Voltage"].unit, "V");
        assert_eq!(sensors["Harmony Battery Current"].value, 200.0);
        assert!((sensors["Harmony Power"].value - 3.85 * 0.2).abs() < 1e-9);
        assert_eq!(sensors["Harmony Power"].unit, "W");
    }

    #[test]
    fn processes_expand_main_and_children() {
        let blocks = parse_fixture_blocks();
        let processes = build_processes(&blocks[1]).unwrap();

        assert_eq!(processes.len(), 5);
        assert_eq!(processes[0].pid, 43950);
        assert_eq!(processes[0].name, "com.huawei.it.works");
        // 常驻内存 = pss − swapPss：258695 − 92360。
        assert!((processes[0].working_set_mb - (258695.0 - 92360.0) / 1024.0).abs() < 1e-9);

        let child = processes.iter().find(|item| item.pid == 44977).unwrap();
        assert_eq!(child.name, "com.huawei.it.works:44977");
        assert!((child.cpu_percent - 0.029499).abs() < 1e-9);
        // 子进程同样扣除 childSwapPss：169559 − 23696。
        assert!((child.working_set_mb - (169559.0 - 23696.0) / 1024.0).abs() < 1e-9);

        let gpu_child = processes.iter().find(|item| item.pid == 44831).unwrap();
        assert!((gpu_child.gpu_memory_mb - 111212.0 / 1024.0).abs() < 1e-9);
    }

    #[test]
    fn resident_memory_falls_back_to_pss_without_swappss() {
        // 旧设备/NA 场景：swapPss 缺失时回退 pss 原值。
        let mut block = HashMap::new();
        block.insert("ProcAppName".to_string(), "com.app".to_string());
        block.insert("ProcId".to_string(), "100".to_string());
        block.insert("pss".to_string(), "20480".to_string());
        let processes = build_processes(&block).unwrap();
        assert!((processes[0].working_set_mb - 20.0).abs() < 1e-9);
    }

    #[test]
    fn processes_absent_without_pkg() {
        let mut block = HashMap::new();
        block.insert("TotalcpuUsage".to_string(), "1.0".to_string());
        assert!(build_processes(&block).is_none());
    }

    #[test]
    fn child_names_use_bundle_base_when_app_name_is_subprocess() {
        // ProcAppName 本身可能是子进程名（带冒号后缀），子进程命名必须取包名基名，
        // 避免出现 `包名:子进程名:pid` 三段名。
        let mut block = HashMap::new();
        block.insert(
            "ProcAppName".to_string(),
            "com.huawei.it.works:Native_libadapter4".to_string(),
        );
        block.insert("ProcId".to_string(), "2929".to_string());
        block.insert("ChildProcId".to_string(), "43950|44831".to_string());

        let processes = build_processes(&block).unwrap();
        // 主进程保留 SP_daemon 上报的真实进程名，归组时按包名基名汇总。
        assert_eq!(processes[0].name, "com.huawei.it.works:Native_libadapter4");
        assert_eq!(processes[1].name, "com.huawei.it.works:43950");
        assert_eq!(processes[2].name, "com.huawei.it.works:44831");
    }

    #[test]
    fn command_covers_full_metric_set() {
        assert_eq!(
            sp_daemon_command(600, Some("com.huawei.it.works"), None),
            "SP_daemon -N 600 -c -g -f -t -p -r -net -d -PKG com.huawei.it.works"
        );
        assert_eq!(
            sp_daemon_command(60, None, None),
            "SP_daemon -N 60 -c -g -f -t -p -r -net -d"
        );
        assert_eq!(
            sp_daemon_command(60, None, Some(28645)),
            "SP_daemon -N 60 -c -g -f -t -p -r -net -d -PID 28645"
        );
    }

    #[test]
    fn pipe_helpers_handle_trailing_and_empty_segments() {
        assert_eq!(
            parse_pipe_u32("44831|44974|"),
            vec![Some(44831), Some(44974)]
        );
        assert_eq!(
            parse_pipe_f64("0.5||1.0|"),
            vec![Some(0.5), None, Some(1.0)]
        );
        assert!(parse_pipe_f64("").is_empty());

        let map = parse_child_map("44831:276|44974:|45200:12|");
        assert_eq!(map.len(), 2);
        assert_eq!(map[&44831], 276.0);
        assert_eq!(map[&45200], 12.0);
    }
}
