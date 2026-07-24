use crate::error::Error;
use regex::Regex;

/// Hidumper CPU 使用率结构体
#[derive(Debug, Clone, PartialEq)]
pub struct HidumperCpuUsage {
    pub total_percent: f64,
    pub user_percent: Option<f64>,
    pub system_percent: Option<f64>,
    pub idle_percent: Option<f64>,
}

/// 解析 hidumper --cpuusage 输出
///
/// 合成 fixture 格式（待真机冻结后替换正则）：
/// ```
/// CPU usage for the last time:
/// Total: 35%
/// User space: 20%
/// Kernel space: 15%
/// iowait: 0%
/// ```
pub fn parse_hidumper_cpuusage(text: &str) -> Result<HidumperCpuUsage, Error> {
    // TODO: 待真机 hidumper --cpuusage 输出冻结后替换此正则

    let total_regex = Regex::new(r"Total:\s*(\d+(?:\.\d+)?)%")
        .map_err(|_| Error::ParseFailed("正则编译失败".to_string()))?;
    let user_regex = Regex::new(r"User\s+space:\s*(\d+(?:\.\d+)?)%")
        .map_err(|_| Error::ParseFailed("正则编译失败".to_string()))?;
    let system_regex = Regex::new(r"Kernel\s+space:\s*(\d+(?:\.\d+)?)%")
        .map_err(|_| Error::ParseFailed("正则编译失败".to_string()))?;
    let idle_regex = Regex::new(r"(?i)(?:Idle|iowait):\s*(\d+(?:\.\d+)?)%")
        .map_err(|_| Error::ParseFailed("正则编译失败".to_string()))?;

    let total_str = total_regex
        .captures(text)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str())
        .ok_or(Error::ParseFailed("未找到 Total 字段".to_string()))?;

    let total_percent = total_str
        .parse::<f64>()
        .map_err(|_| Error::ParseFailed("Total 数值解析失败".to_string()))?;

    let user_percent = user_regex
        .captures(text)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().parse::<f64>().ok())
        .flatten();

    let system_percent = system_regex
        .captures(text)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().parse::<f64>().ok())
        .flatten();

    let idle_percent = idle_regex
        .captures(text)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().parse::<f64>().ok())
        .flatten();

    Ok(HidumperCpuUsage {
        total_percent,
        user_percent,
        system_percent,
        idle_percent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_total_user_system_idle() {
        let text = r#"CPU usage for the last time:
Total: 35%
User space: 20%
Kernel space: 15%
iowait: 0%
"#;

        let result = parse_hidumper_cpuusage(text).unwrap();
        assert_eq!(result.total_percent, 35.0);
        assert_eq!(result.user_percent, Some(20.0));
        assert_eq!(result.system_percent, Some(15.0));
        assert_eq!(result.idle_percent, Some(0.0));
    }

    #[test]
    fn parses_with_missing_optional_fields() {
        let text = r#"CPU usage for the last time:
Total: 42.5%
"#;

        let result = parse_hidumper_cpuusage(text).unwrap();
        assert_eq!(result.total_percent, 42.5);
        assert!(result.user_percent.is_none());
        assert!(result.system_percent.is_none());
        assert!(result.idle_percent.is_none());
    }
}
