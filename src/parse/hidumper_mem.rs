use crate::error::Error;
use regex::Regex;

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

/// 解析 hidumper --mem 输出（系统内存）
///
/// 格式：
/// ```
/// Total Memory: 3817212 kB
/// Used Memory: 2100000 kB
/// Free Memory: 1717212 kB
/// ```
pub fn parse_hidumper_mem_system(text: &str) -> Result<SystemMem, Error> {
    let total_regex = Regex::new(r"Total\s+Memory:\s+(\d+)\s+kB")
        .map_err(|_| Error::ParseFailed("正则编译失败".to_string()))?;
    let used_regex = Regex::new(r"Used\s+Memory:\s+(\d+)\s+kB")
        .map_err(|_| Error::ParseFailed("正则编译失败".to_string()))?;
    let free_regex = Regex::new(r"Free\s+Memory:\s+(\d+)\s+kB")
        .map_err(|_| Error::ParseFailed("正则编译失败".to_string()))?;
    let available_regex = Regex::new(r"Available\s+Memory:\s+(\d+)\s+kB")
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
    let pss_column_regex = Regex::new(r"Pss\s*\(\s*kB\s*\)")
        .map_err(|_| Error::ParseFailed("正则编译失败".to_string()))?;

    // 验证存在 Pss(kB) 列
    if !pss_column_regex.is_match(text) {
        return Err(Error::ParseFailed("未找到 Pss(kB) 列".to_string()));
    }

    // 解析各行数据
    let mut gpu_pss_kb = None;
    let mut native_heap_pss_kb = None;
    let mut arkts_heap_pss_kb = None;
    let mut pss_kb = 0u64;
    let mut total_pss_kb = None;

    let number_regex =
        Regex::new(r"(\d+)").map_err(|_| Error::ParseFailed("正则编译失败".to_string()))?;

    for line in text.lines() {
        let line = line.trim();

        if line.is_empty() {
            continue;
        }

        // 解析 Total 行：提取第二个数字（Pss 列）
        if line.contains("Total") && line.contains("Pss") {
            let numbers: Vec<u64> = number_regex
                .find_iter(line)
                .filter_map(|m| m.as_str().parse::<u64>().ok())
                .collect();

            if !numbers.is_empty() {
                // 取第一个数字作为 Total Pss
                total_pss_kb = Some(numbers[0]);
                if pss_kb == 0 && !numbers.is_empty() {
                    pss_kb = numbers[0];
                }
            }
        }
        // 包含 "GL" 的行
        else if line.to_lowercase().contains("gl") {
            if let Some(num_str) = number_regex.find(line).map(|m| m.as_str()) {
                gpu_pss_kb = num_str.parse::<u64>().ok();
            }
        }
        // 包含 "native" 和 "heap" 的行
        else if line.to_lowercase().contains("native") && line.to_lowercase().contains("heap") {
            if let Some(num_str) = number_regex.find(line).map(|m| m.as_str()) {
                native_heap_pss_kb = num_str.parse::<u64>().ok();
            }
        }
        // 包含 "ark" 的行
        else if line.to_lowercase().contains("ark") {
            if let Some(num_str) = number_regex.find(line).map(|m| m.as_str()) {
                arkts_heap_pss_kb = num_str.parse::<u64>().ok();
            }
        }
        // 如果没有 Total 行，从第一行数据取 pss
        else if pss_kb == 0 {
            if let Some(num_str) = number_regex.find(line).map(|m| m.as_str()) {
                if let Ok(val) = num_str.parse::<u64>() {
                    pss_kb = val;
                }
            }
        }
    }

    // 如果还没有设置 pss_kb，返回错误
    if pss_kb == 0 && total_pss_kb.is_none() {
        return Err(Error::ParseFailed("无法解析 PSS 值".to_string()));
    }

    // 如果有 Total PSS 但 pss_kb 仍为 0，使用 Total PSS
    if pss_kb == 0 {
        pss_kb = total_pss_kb.unwrap_or(0);
    }

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
}
