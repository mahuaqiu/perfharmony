use crate::error::Error;

/// 系统内存信息结构体
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemInfo {
    pub total_kb: u64,
    pub free_kb: u64,
    pub available_kb: u64,
}

/// 解析 /proc/meminfo 文件，提取 MemTotal、MemFree、MemAvailable（单位 kB）
pub fn parse_proc_meminfo(text: &str) -> Result<MemInfo, Error> {
    let mut total_kb = None;
    let mut free_kb = None;
    let mut available_kb = None;

    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("MemTotal:") {
            total_kb = parse_meminfo_line(line)?;
        } else if line.starts_with("MemFree:") {
            free_kb = parse_meminfo_line(line)?;
        } else if line.starts_with("MemAvailable:") {
            available_kb = parse_meminfo_line(line)?;
        }
    }

    let total = total_kb.ok_or(Error::ParseFailed("缺少 MemTotal 字段".to_string()))?;
    let free = free_kb.ok_or(Error::ParseFailed("缺少 MemFree 字段".to_string()))?;
    let available = available_kb.ok_or(Error::ParseFailed("缺少 MemAvailable 字段".to_string()))?;

    Ok(MemInfo {
        total_kb: total,
        free_kb: free,
        available_kb: available,
    })
}

/// 辅助函数：从单行提取数值（格式：字段名: 数值 单位）
fn parse_meminfo_line(line: &str) -> Result<Option<u64>, Error> {
    // 格式为 "字段名: 数值 kB"，分割后取数值部分
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(Error::ParseFailed("meminfo 行格式错误".to_string()));
    }

    // parts[0] 是 "字段名:"（带冒号），parts[1] 是数值
    let value = parts[1].parse::<u64>()
        .map_err(|_| Error::ParseFailed("数值解析失败".to_string()))?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mem_total_free_available() {
        let text = r#"MemTotal:        3817212 kB
MemFree:         1200000 kB
MemAvailable:    1500000 kB
Buffers:           50000 kB
Cached:           800000 kB
"#;

        let mem = parse_proc_meminfo(text).unwrap();
        assert_eq!(mem.total_kb, 3817212);
        assert_eq!(mem.free_kb, 1200000);
        assert_eq!(mem.available_kb, 1500000);
    }

    #[test]
    fn returns_error_on_missing_field() {
        let text = r#"MemTotal:        3817212 kB
MemFree:         1200000 kB
"#;
        let result = parse_proc_meminfo(text);
        assert!(result.is_err());
    }
}
