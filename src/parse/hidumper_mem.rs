use crate::error::Error;
use regex::Regex;
use std::collections::HashMap;

/// 系统内存信息结构体（来自 hidumper --mem）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SystemMem {
    pub total_kb: u64,
    pub used_kb: u64,
    pub free_kb: u64,
    pub available_kb: Option<u64>,
}

/// 进程内存信息结构体（来自 hidumper --mem <pid>）
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessMem {
    pub pss_kb: u64,
    pub native_heap_pss_kb: Option<u64>,
    pub gpu_pss_kb: Option<u64>,
    pub arkts_heap_pss_kb: Option<u64>,
    pub total_pss_kb: Option<u64>,
}

/// 全量进程内存快照（来自 hidumper --mem 的 PID 表）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessMemSnapshot {
    pub pss_kb: u64,
    pub vss_kb: u64,
    pub rss_kb: u64,
    pub uss_kb: u64,
}

/// 解析 hidumper --mem 输出（系统内存）
///
/// 格式：
/// ```
/// Total Memory: 3817212 kB
/// Used Memory: 2100000 kB
/// Free Memory: 1717212 kB
/// ```
pub fn parse_hidumper_mem_system(text: &str) -> Result<SystemMem, Error> {
    let total_regex = Regex::new(r"(?i)Total\s+(?:Memory|RAM):\s*(\d+)\s+kB")
        .map_err(|_| Error::ParseFailed("正则编译失败".to_string()))?;
    let used_regex = Regex::new(r"(?i)Used\s+(?:Memory|RAM):\s*(\d+)\s+kB")
        .map_err(|_| Error::ParseFailed("正则编译失败".to_string()))?;
    let free_regex = Regex::new(r"(?i)Free\s+(?:Memory|RAM):\s*(\d+)\s+kB")
        .map_err(|_| Error::ParseFailed("正则编译失败".to_string()))?;
    let available_regex = Regex::new(r"(?i)Available\s+(?:Memory|RAM):\s*(\d+)\s+kB")
        .map_err(|_| Error::ParseFailed("正则编译失败".to_string()))?;

    let total_kb = total_regex
        .captures(text)
        .and_then(|cap| cap.get(1))
        .and_then(|m| m.as_str().parse::<u64>().ok())
        .ok_or(Error::ParseFailed("未找到 Total Memory".to_string()))?;

    let used_kb = used_regex
        .captures(text)
        .and_then(|cap| cap.get(1))
        .and_then(|m| m.as_str().parse::<u64>().ok())
        .ok_or(Error::ParseFailed("未找到 Used Memory".to_string()))?;

    let free_kb = free_regex
        .captures(text)
        .and_then(|cap| cap.get(1))
        .and_then(|m| m.as_str().parse::<u64>().ok())
        .ok_or(Error::ParseFailed("未找到 Free Memory".to_string()))?;

    let available_kb = available_regex
        .captures(text)
        .and_then(|cap| cap.get(1))
        .and_then(|m| m.as_str().parse::<u64>().ok());

    Ok(SystemMem {
        total_kb,
        used_kb,
        free_kb,
        available_kb,
    })
}

/// 解析 hidumper --mem 输出中的全量进程内存表。
///
/// 真机 PSS 列形如 `37565(0 in SwapPss) kB`。输出中可能重复出现表格，
/// 相同 PID 以最后一次出现的数据为准。
pub fn parse_hidumper_mem_processes(text: &str) -> HashMap<u32, ProcessMemSnapshot> {
    let Ok(row_regex) = Regex::new(
        r"^\s*(\d+)\s+(\d+)\([^)]*\)\s+kB\s+(\d+)\s+kB\s+(\d+)\s+kB\s+(\d+)\s+kB(?:\s|$)",
    ) else {
        return HashMap::new();
    };
    let mut result = HashMap::new();

    for line in text.lines() {
        let Some(captures) = row_regex.captures(line) else {
            continue;
        };
        let values = (1..=5)
            .map(|index| {
                captures
                    .get(index)
                    .and_then(|value| value.as_str().parse::<u64>().ok())
            })
            .collect::<Option<Vec<_>>>();
        let Some(values) = values else {
            continue;
        };
        let Ok(pid) = u32::try_from(values[0]) else {
            continue;
        };
        result.insert(
            pid,
            ProcessMemSnapshot {
                pss_kb: values[1],
                vss_kb: values[2],
                rss_kb: values[3],
                uss_kb: values[4],
            },
        );
    }

    result
}

/// 解析 hidumper --mem <pid> 输出（进程内存）
///
/// 格式：
/// ```
///                      Pss(kB)
/// GL                    12345
/// Graph                  5678
/// ArkTS Heap            23456
/// native heap           11111
/// stack                  1024
/// Total            Pss   53614    SharedClean      0 SharedDirty      0 PrivateClean   0 PrivateDirty 53614
/// ```
pub fn parse_hidumper_mem_pid(text: &str) -> Result<ProcessMem, Error> {
    let pss_column_regex = Regex::new(r"(?is)\bPss\b.{0,500}?\(\s*kB\s*\)")
        .map_err(|_| Error::ParseFailed("正则编译失败".to_string()))?;

    // 验证存在 Pss(kB) 列
    if !pss_column_regex.is_match(text) {
        return Err(Error::ParseFailed("未找到 Pss(kB) 列".to_string()));
    }

    // 解析各行数据
    let mut gpu_pss_kb = None;
    let mut native_heap_pss_kb = None;
    let mut arkts_heap_pss_kb = None;
    let mut total_pss_kb = None;

    let row_regex = Regex::new(r"^\s*(.+?)\s+(\d+)(?:\s+\d+)*\s*$")
        .map_err(|_| Error::ParseFailed("正则编译失败".to_string()))?;
    let legacy_total_regex = Regex::new(r"(?i)^\s*Total\s+Pss\s+(\d+)")
        .map_err(|_| Error::ParseFailed("正则编译失败".to_string()))?;

    for line in text.lines() {
        let line = line.trim();

        if line.is_empty() {
            continue;
        }

        if let Some(captures) = legacy_total_regex.captures(line) {
            total_pss_kb = captures
                .get(1)
                .and_then(|value| value.as_str().parse().ok());
            continue;
        }

        let Some(captures) = row_regex.captures(line) else {
            continue;
        };
        let label = captures.get(1).map(|value| value.as_str()).unwrap_or("");
        let Some(value) = captures
            .get(2)
            .and_then(|value| value.as_str().parse::<u64>().ok())
        else {
            continue;
        };
        let normalized = label.to_ascii_lowercase();

        if normalized == "total" {
            total_pss_kb = Some(value);
        } else if normalized == "gl" {
            gpu_pss_kb = Some(value);
        } else if normalized.contains("native") && normalized.contains("heap") {
            native_heap_pss_kb = Some(value);
        } else if normalized.contains("ark") {
            arkts_heap_pss_kb = Some(value);
        }
    }

    let pss_kb = total_pss_kb.ok_or(Error::ParseFailed("无法解析 Total PSS 值".to_string()))?;

    Ok(ProcessMem {
        pss_kb,
        native_heap_pss_kb,
        gpu_pss_kb,
        arkts_heap_pss_kb,
        total_pss_kb,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_system_mem_total_used_free() {
        let text = r#"Total Memory: 3817212 kB
Used Memory: 2100000 kB
Free Memory: 1717212 kB
"#;

        let mem = parse_hidumper_mem_system(text).unwrap();
        assert_eq!(mem.total_kb, 3817212);
        assert_eq!(mem.used_kb, 2100000);
        assert_eq!(mem.free_kb, 1717212);
        assert!(mem.available_kb.is_none());
    }

    #[test]
    fn parses_real_device_system_mem() {
        let text = include_str!("../../tests/fixtures/real/harmony_pc/hidumper_mem.txt");
        let mem = parse_hidumper_mem_system(text).unwrap();

        assert_eq!(mem.total_kb, 32680648);
        assert_eq!(mem.free_kb, 24154539);
        assert_eq!(mem.used_kb, 5483865);
    }

    #[test]
    fn parses_real_device_process_table() {
        let text = include_str!("../../tests/fixtures/real/harmony_pc/hidumper_mem.txt");
        let processes = parse_hidumper_mem_processes(text);
        let process = processes.get(&14751).unwrap();

        assert_eq!(process.pss_kb, 37565);
        assert_eq!(process.vss_kb, 6480768);
        assert_eq!(process.rss_kb, 130924);
        assert_eq!(process.uss_kb, 32888);
    }

    #[test]
    fn parses_pid_mem_pss_and_gl() {
        let text = r#"                     Pss(kB)
GL                    12345
Graph                  5678
ArkTS Heap            23456
native heap           11111
stack                  1024
Total            Pss   53614    SharedClean      0 SharedDirty      0 PrivateClean   0 PrivateDirty 53614
"#;

        let mem = parse_hidumper_mem_pid(text).unwrap();
        assert_eq!(mem.gpu_pss_kb, Some(12345));
        assert_eq!(mem.native_heap_pss_kb, Some(11111));
        assert_eq!(mem.arkts_heap_pss_kb, Some(23456));
        assert_eq!(mem.total_pss_kb, Some(53614));
    }

    #[test]
    fn parses_real_device_pid_mem() {
        let text = include_str!("../../tests/fixtures/real/harmony_pc/hidumper_mem_pid.txt");
        let mem = parse_hidumper_mem_pid(text).unwrap();

        assert_eq!(mem.pss_kb, 207422);
        assert_eq!(mem.total_pss_kb, Some(207422));
        assert_eq!(mem.gpu_pss_kb, Some(0));
    }
}
