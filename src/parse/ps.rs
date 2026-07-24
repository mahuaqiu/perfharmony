/// 从 ps -ef 输出解析进程列表，返回 (pid, name) 列表
///
/// 格式（ps -ef）：
/// ```
/// UID        PID  PPID  C STIME TTY          TIME CMD
/// root         1     0  0 00:00 ?        00:00:01 /init
/// shell      200   100  0 00:00 ?        00:00:00 com.example.app
/// ```
///
/// 提取规则：
/// - 跳过表头行（含 UID/PID 等字段名）
/// - CMD 取最后一个空格分隔字段
/// - 取 basename（去掉路径和参数）
/// - PID 解析失败的行跳过
/// - 不返回 Result，空输入返回 []
pub fn parse_ps_ef(text: &str) -> Vec<(u32, String)> {
    let mut result = Vec::new();

    for line in text.lines() {
        let line = line.trim();

        if line.is_empty() {
            continue;
        }

        // 跳过表头行（包含 UID/PID 等字段名的行）
        if line.contains("UID") && line.contains("PID") {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }

        // PID 通常在第 2 列（索引 1）
        let pid_str = parts[1];
        match pid_str.parse::<u32>() {
            Ok(pid) => {
                // CMD 是最后一个字段
                let cmd = parts.last().unwrap_or(&"");
                // 提取 basename（去掉路径）
                let name = extract_basename(cmd).to_string();
                result.push((pid, name));
            }
            Err(_) => {
                // PID 解析失败，跳过该行
                continue;
            }
        }
    }

    result
}

/// 辅助函数：从路径/命令字符串中提取 basename
/// 例如：/system/bin/hilogd -> hilogd，com.example.app -> com.example.app
fn extract_basename(cmd: &str) -> &str {
    // 通过 / 分割，取最后一部分
    if let Some(pos) = cmd.rfind('/') {
        &cmd[pos + 1..]
    } else {
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pid_and_name() {
        let text = r#"UID        PID  PPID  C STIME TTY          TIME CMD
root         1     0  0 00:00 ?        00:00:01 /init
root       100     1  0 00:00 ?        00:00:00 /system/bin/hilogd
shell      200   100  0 00:00 ?        00:00:00 com.example.app
"#;

        let result = parse_ps_ef(text);
        assert_eq!(result.len(), 3);

        assert_eq!(result[0], (1, "init".to_string()));
        assert_eq!(result[1], (100, "hilogd".to_string()));
        assert_eq!(result[2], (200, "com.example.app".to_string()));
    }

    #[test]
    fn skips_header_line() {
        let text = r#"UID        PID  PPID  C STIME TTY          TIME CMD
root         1     0  0 00:00 ?        00:00:01 /init
"#;

        let result = parse_ps_ef(text);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, 1);
    }

    #[test]
    fn skips_invalid_pid() {
        let text = r#"UID        PID  PPID  C STIME TTY          TIME CMD
root       abc     0  0 00:00 ?        00:00:01 /init
root         1     0  0 00:00 ?        00:00:01 /init
"#;

        let result = parse_ps_ef(text);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, 1);
    }

    #[test]
    fn handles_empty_input() {
        let result = parse_ps_ef("");
        assert!(result.is_empty());
    }
}
