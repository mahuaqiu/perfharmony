/// 从 ps -ef 输出解析进程列表，返回 (pid, name) 列表
///
/// 格式（ps -ef）：
/// ```
/// UID        PID  PPID  C STIME TTY          TIME CMD
/// root         1     0  0 00:00 ?        00:00:01 /init
/// shell      200   100  0 00:00 ?        00:00:00 com.example.app --flag
/// ```
///
/// 提取规则：
/// - 跳过表头行（含 UID/PID 等字段名）
/// - CMD 从第 8 列起整段保留（含参数），不要只取最后一个 token
/// - 进程名优先保留包名/可执行名，供 ProcessFilter 精确匹配
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
                // 标准 ps -ef 至少 8 列：UID PID PPID C STIME TTY TIME CMD...
                // CMD 从索引 7 开始拼接，保留参数，便于包名匹配。
                let cmd = if parts.len() >= 8 {
                    parts[7..].join(" ")
                } else {
                    parts.last().copied().unwrap_or("").to_string()
                };
                let name = process_display_name(&cmd);
                if name.is_empty() {
                    continue;
                }
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

/// 从 CMD 提取用于筛选和展示的进程名。
///
/// - 优先识别类似 `com.example.app` 的包名
/// - 否则取可执行路径 basename，并去掉首个参数后的其余参数
fn process_display_name(cmd: &str) -> String {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // 在整段 CMD 中优先找包名，避免只拿到 `--flag` 或 `appspawn` 宿主进程名。
    for token in trimmed.split_whitespace() {
        if looks_like_bundle_name(token) {
            return token.to_string();
        }
    }

    let executable = trimmed.split_whitespace().next().unwrap_or(trimmed);
    extract_basename(executable).to_string()
}

fn looks_like_bundle_name(token: &str) -> bool {
    if !token.contains('.') || token.starts_with('.') || token.ends_with('.') {
        return false;
    }
    if token.contains('/') || token.contains('\\') {
        return false;
    }
    if token.starts_with('-') {
        return false;
    }
    token
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        && token.split('.').count() >= 2
        && token.split('.').all(|part| !part.is_empty())
}

/// 辅助函数：从路径/命令字符串中提取 basename
/// 例如：/system/bin/hilogd -> hilogd，com.example.app -> com.example.app
fn extract_basename(cmd: &str) -> &str {
    // 通过 / 或 \ 分割，取最后一部分
    if let Some(pos) = cmd.rfind(['/', '\\']) {
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
    fn keeps_bundle_name_when_cmd_has_arguments() {
        let text = r#"UID        PID  PPID  C STIME TTY          TIME CMD
shell      200   100  0 00:00 ?        00:00:00 com.example.app --ability MainAbility
root       300     1  0 00:00 ?        00:00:00 /system/bin/appspawn com.example.demo
"#;

        let result = parse_ps_ef(text);
        assert_eq!(result[0], (200, "com.example.app".to_string()));
        assert_eq!(result[1], (300, "com.example.demo".to_string()));
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