//! `hidumper --cpufreq` 解析。
//!
//! 真机（Harmony PC，API 24）输出为「cmd is: cat .../cpuN/cpufreq/cpuinfo_cur_freq」
//! 提示行后跟一个 kHz 数值行，cur/max 交替出现：
//!
//! ```text
//! cmd is: cat /sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_cur_freq
//!
//! 1431000
//!
//! cmd is: cat /sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq
//!
//! 2050000
//! ```

use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuCoreFreq {
    pub core: u32,
    /// 当前频率，MHz。
    pub cur_mhz: f64,
}

/// 只提取各核 cpuinfo_cur_freq；max_freq 是静态属性，不作为采样指标。
pub fn parse_hidumper_cpufreq(text: &str) -> Result<Vec<CpuCoreFreq>, Error> {
    let mut result: Vec<CpuCoreFreq> = Vec::new();
    let mut pending_core: Option<u32> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(path) = trimmed.strip_prefix("cmd is:") {
            pending_core = if path.contains("cpuinfo_cur_freq") {
                extract_core_index(path)
            } else {
                None
            };
            continue;
        }
        if let Some(core) = pending_core {
            if let Ok(khz) = trimmed.parse::<f64>() {
                result.push(CpuCoreFreq {
                    core,
                    cur_mhz: khz / 1000.0,
                });
            }
            pending_core = None;
        }
    }
    if result.is_empty() {
        return Err(Error::ParseFailed(
            "hidumper --cpufreq 输出没有可解析的 cpuinfo_cur_freq".to_string(),
        ));
    }
    result.sort_by_key(|item| item.core);
    Ok(result)
}

/// 从 `/sys/devices/system/cpu/cpu12/cpufreq/...` 提取核编号。
fn extract_core_index(path: &str) -> Option<u32> {
    let start = path.find("/cpu/cpu")? + "/cpu/cpu".len();
    let digits: String = path[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_SAMPLE: &str = "\n-------------------------------[cpufreq]-------------------------------\n\n\ncmd is: cat /sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_cur_freq\n\n1431000\n\ncmd is: cat /sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq\n\n2050000\n\ncmd is: cat /sys/devices/system/cpu/cpu1/cpufreq/cpuinfo_cur_freq\n\n1531000\n\ncmd is: cat /sys/devices/system/cpu/cpu1/cpufreq/cpuinfo_max_freq\n\n2050000\n";

    #[test]
    fn parses_real_device_cur_freq_and_skips_max_freq() {
        let values = parse_hidumper_cpufreq(REAL_SAMPLE).unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].core, 0);
        assert_eq!(values[0].cur_mhz, 1431.0);
        assert_eq!(values[1].core, 1);
        assert_eq!(values[1].cur_mhz, 1531.0);
    }

    #[test]
    fn rejects_output_without_cur_freq() {
        assert!(parse_hidumper_cpufreq("cmd is: cat /x/cpuinfo_max_freq\n2050000\n").is_err());
    }
}
