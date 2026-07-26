use crate::error::Error;

/// CPU 时间快照结构体
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuStatSnapshot {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl CpuStatSnapshot {
    /// 计算总时间（所有字段之和）
    pub fn total(&self) -> u64 {
        self.user
            + self.nice
            + self.system
            + self.idle
            + self.iowait
            + self.irq
            + self.softirq
            + self.steal
    }

    /// 计算忙碌时间（总时间 - idle - iowait）
    pub fn busy(&self) -> u64 {
        self.total() - self.idle - self.iowait
    }
}

/// 解析 /proc/stat 文件，返回第一行的 CPU 统计数据
pub fn parse_proc_stat(text: &str) -> Result<CpuStatSnapshot, Error> {
    // 找到第一行，以 "cpu " 开头
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("cpu "))
        .ok_or(Error::ParseFailed("未找到 cpu 行".to_string()))?;

    // 跳过 "cpu " 前缀
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 9 {
        return Err(Error::ParseFailed("cpu 行字段不足".to_string()));
    }

    // 解析各字段
    let user = parts[1]
        .parse::<u64>()
        .map_err(|_| Error::ParseFailed("user 字段解析失败".to_string()))?;
    let nice = parts[2]
        .parse::<u64>()
        .map_err(|_| Error::ParseFailed("nice 字段解析失败".to_string()))?;
    let system = parts[3]
        .parse::<u64>()
        .map_err(|_| Error::ParseFailed("system 字段解析失败".to_string()))?;
    let idle = parts[4]
        .parse::<u64>()
        .map_err(|_| Error::ParseFailed("idle 字段解析失败".to_string()))?;
    let iowait = parts[5]
        .parse::<u64>()
        .map_err(|_| Error::ParseFailed("iowait 字段解析失败".to_string()))?;
    let irq = parts[6]
        .parse::<u64>()
        .map_err(|_| Error::ParseFailed("irq 字段解析失败".to_string()))?;
    let softirq = parts[7]
        .parse::<u64>()
        .map_err(|_| Error::ParseFailed("softirq 字段解析失败".to_string()))?;
    let steal = parts[8]
        .parse::<u64>()
        .map_err(|_| Error::ParseFailed("steal 字段解析失败".to_string()))?;

    Ok(CpuStatSnapshot {
        user,
        nice,
        system,
        idle,
        iowait,
        irq,
        softirq,
        steal,
    })
}

/// 计算 CPU 使用率百分比（0-100）
/// 当前 busy / 总差值 * 100，分母为 0 返回 None
pub fn diff_cpu_percent(prev: &CpuStatSnapshot, curr: &CpuStatSnapshot) -> Option<f64> {
    let total_diff = curr.total().saturating_sub(prev.total());
    if total_diff == 0 {
        return None;
    }

    let busy_diff = curr.busy().saturating_sub(prev.busy());
    let percent = (busy_diff as f64 / total_diff as f64) * 100.0;
    Some(percent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_diffs_cpu_percent() {
        let t0_text = "cpu  100 20 50 800 10 5 3 0 0 0\ncpu0 50 10 25 400 5 2 1 0 0 0";
        let t1_text = "cpu  200 20 100 900 10 5 3 0 0 0\ncpu0 100 10 50  450 5 2 1 0 0 0";

        let t0 = parse_proc_stat(t0_text).unwrap();
        let t1 = parse_proc_stat(t1_text).unwrap();

        // 验证解析
        assert_eq!(t0.user, 100);
        assert_eq!(t0.idle, 800);
        assert_eq!(t1.user, 200);
        assert_eq!(t1.idle, 900);

        // 验证 busy 计算
        // t0.busy = 100+20+50+5+3 = 178
        // t1.busy = 200+20+100+5+3 = 328
        assert_eq!(t0.busy(), 178);
        assert_eq!(t1.busy(), 328);

        // 验证 total 计算
        // t0.total = 100+20+50+800+10+5+3 = 988
        // t1.total = 200+20+100+900+10+5+3 = 1238
        assert_eq!(t0.total(), 988);
        assert_eq!(t1.total(), 1238);

        // 验证差分 CPU 使用率
        // busy_diff = 328 - 178 = 150
        // total_diff = 1238 - 988 = 250
        // percent = 150 / 250 * 100 = 60%
        let percent = diff_cpu_percent(&t0, &t1).unwrap();
        assert!((percent - 60.0).abs() < 0.01);
    }

    #[test]
    fn diff_zero_total_returns_none() {
        let snap = CpuStatSnapshot {
            user: 100,
            nice: 20,
            system: 50,
            idle: 800,
            iowait: 10,
            irq: 5,
            softirq: 3,
            steal: 0,
        };

        // 相同快照的差值应为 None（total_diff = 0）
        let result = diff_cpu_percent(&snap, &snap);
        assert!(result.is_none());
    }

    #[test]
    fn parses_real_device_snapshots() {
        let t0 = parse_proc_stat(include_str!(
            "../../tests/fixtures/real/harmony_pc/proc_stat_t0.txt"
        ))
        .unwrap();
        let t1 = parse_proc_stat(include_str!(
            "../../tests/fixtures/real/harmony_pc/proc_stat_t1.txt"
        ))
        .unwrap();

        let percent = diff_cpu_percent(&t0, &t1).unwrap();
        assert!((0.0..=100.0).contains(&percent));
    }
}
