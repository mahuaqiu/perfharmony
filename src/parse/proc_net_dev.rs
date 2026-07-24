use crate::error::Error;

/// 网络设备累计快照结构体
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NetDevSnapshot {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// 解析 /proc/net/dev 文件，累加 wlan0、eth0、rmnet0 的收发字节
pub fn parse_proc_net_dev(text: &str) -> Result<NetDevSnapshot, Error> {
    let mut rx_bytes = 0u64;
    let mut tx_bytes = 0u64;

    // 需要累加的接口名单
    let target_interfaces = ["wlan0", "eth0", "rmnet0"];

    for line in text.lines() {
        let line = line.trim();

        // 跳过空行和表头行
        if line.is_empty() || line.starts_with("Inter-") || line.starts_with("face") {
            continue;
        }

        // 格式：接口名: RX_bytes packets ... TX_bytes ...
        if let Some(colon_pos) = line.find(':') {
            let iface_name = line[..colon_pos].trim();

            // 检查是否为目标接口
            if !target_interfaces.contains(&iface_name) {
                continue;
            }

            let data_part = &line[colon_pos + 1..];
            let parts: Vec<&str> = data_part.split_whitespace().collect();

            // 需要至少 9 列（RX 数据为前 8 列，TX 数据从第 9 列开始）
            if parts.len() < 9 {
                return Err(Error::ParseFailed(format!("接口 {} 列数不足", iface_name)));
            }

            // RX_bytes 是第 1 列（冒号后第一个数据）
            let iface_rx = parts[0].parse::<u64>().map_err(|_| {
                Error::ParseFailed(format!("接口 {} RX_bytes 解析失败", iface_name))
            })?;

            // TX_bytes 是第 9 列
            let iface_tx = parts[8].parse::<u64>().map_err(|_| {
                Error::ParseFailed(format!("接口 {} TX_bytes 解析失败", iface_name))
            })?;

            rx_bytes += iface_rx;
            tx_bytes += iface_tx;
        }
    }

    Ok(NetDevSnapshot { rx_bytes, tx_bytes })
}

/// 计算网络速率（KB/s）
/// 差值为负时直接使用当前值（计数器重置）
pub fn diff_net_kbps(
    prev: &NetDevSnapshot,
    curr: &NetDevSnapshot,
    interval_secs: f64,
) -> (f64, f64) {
    // 计算 RX 差值
    let rx_diff = if curr.rx_bytes >= prev.rx_bytes {
        (curr.rx_bytes - prev.rx_bytes) as f64
    } else {
        // 计数器重置，使用当前值
        curr.rx_bytes as f64
    };

    // 计算 TX 差值
    let tx_diff = if curr.tx_bytes >= prev.tx_bytes {
        (curr.tx_bytes - prev.tx_bytes) as f64
    } else {
        // 计数器重置，使用当前值
        curr.tx_bytes as f64
    };

    // 转换为 KB/s
    let rx_kbps = rx_diff / 1024.0 / interval_secs;
    let tx_kbps = tx_diff / 1024.0 / interval_secs;

    (rx_kbps, tx_kbps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_wlan_eth_rmnet_skips_lo() {
        let text = r#"Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:  123456    1000    0    0    0     0          0         0   123456    1000    0    0    0     0       0          0
  wlan0: 10000000  50000   0    0    0     0          0         0  5000000   30000    0    0    0     0       0          0
   eth0:  2000000  10000   0    0    0     0          0         0  1000000    8000    0    0    0     0       0          0
 rmnet0:   500000   5000   0    0    0     0          0         0   300000    2000    0    0    0     0       0          0
"#;

        let snap = parse_proc_net_dev(text).unwrap();

        // 验证累加（跳过 lo）
        // RX: 10000000 + 2000000 + 500000 = 12500000
        // TX: 5000000 + 1000000 + 300000 = 6300000
        assert_eq!(snap.rx_bytes, 12500000);
        assert_eq!(snap.tx_bytes, 6300000);
    }

    #[test]
    fn negative_diff_uses_curr() {
        let prev = NetDevSnapshot {
            rx_bytes: 10000000,
            tx_bytes: 5000000,
        };
        let curr = NetDevSnapshot {
            rx_bytes: 5000000, // 计数器重置，减少了
            tx_bytes: 8000000, // 正常增加
        };

        let (rx_kbps, tx_kbps) = diff_net_kbps(&prev, &curr, 10.0);

        // RX：计数器重置，使用 curr = 5000000 字节 = 4882.8125 KB / 10s = 488.28125 KB/s
        // TX：正常差值 = 3000000 字节 = 2929.6875 KB / 10s = 292.96875 KB/s
        assert!(rx_kbps > 400.0); // 大约 488 KB/s
        assert!((tx_kbps - 292.96875).abs() < 0.1);
    }
}
