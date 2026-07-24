use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use regex::Regex;

use crate::error::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetInfo {
    pub udid: String,
    pub status: String,
    pub connection_type: String,
}

#[derive(Debug, Clone)]
pub struct HdcClient {
    path: PathBuf,
    default_timeout: Duration,
}

impl HdcClient {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            default_timeout: Duration::from_secs(15),
        }
    }

    pub fn from_optional_path(path: Option<String>) -> Result<Self, Error> {
        Ok(Self::new(crate::hdc::resolve_hdc_path(path.as_deref())?))
    }

    pub fn shell(&self, udid: &str, command: &str, timeout_secs: u64) -> Result<String, Error> {
        if udid.trim().is_empty() {
            return Err(Error::TargetOffline("udid 为空".to_string()));
        }
        let output = self.run(
            &["-t", udid, "shell", command],
            Duration::from_secs(timeout_secs.max(1)),
        )?;
        let text = output_text(&output);
        if !output.status.success() {
            return Err(classify_command_error(udid, text));
        }
        Ok(text)
    }

    pub fn list_targets_raw(&self) -> Result<String, Error> {
        let output = self.run(&["list", "targets", "-v"], self.default_timeout)?;
        let text = output_text(&output);
        if !output.status.success() {
            return Err(Error::CommandFailed(text));
        }
        Ok(text)
    }

    pub fn shell_args<'a>(&self, udid: &'a str, command: &'a str) -> [&'a str; 4] {
        ["-t", udid, "shell", command]
    }

    fn run(&self, args: &[&str], timeout: Duration) -> Result<Output, Error> {
        let mut child = Command::new(&self.path)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    Error::HdcNotFound(self.path.clone())
                } else {
                    Error::Io(error)
                }
            })?;

        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait()? {
                Some(status) => return collect_output(child, status),
                None if Instant::now() >= deadline => {
                    terminate_child(&mut child);
                    return Err(Error::Timeout(format!("{} {:?}", self.path.display(), args)));
                }
                None => thread::sleep(Duration::from_millis(20)),
            }
        }
    }
}

fn collect_output(mut child: Child, status: std::process::ExitStatus) -> Result<Output, Error> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(mut pipe) = child.stdout.take() {
        pipe.read_to_end(&mut stdout)?;
    }
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_end(&mut stderr)?;
    }
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn terminate_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn output_text(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        stdout
    } else if stdout.is_empty() {
        stderr
    } else {
        format!("{stdout}\n{stderr}")
    }
}

fn classify_command_error(udid: &str, text: String) -> Error {
    let lower = text.to_ascii_lowercase();
    if ["offline", "unauthorized", "not found", "no devices", "not connected"]
        .iter()
        .any(|part| lower.contains(part))
    {
        Error::TargetOffline(format!("{udid}: {text}"))
    } else {
        Error::CommandFailed(format!("{udid}: {text}"))
    }
}

/// 解析 `hdc list targets -v`，兼容表头、空行和字段数量变化。
pub fn parse_list_targets(output: &str) -> Vec<TargetInfo> {
    let status_re = Regex::new(r"(?i)^(ready|offline|unauthorized|connected|disconnected)$")
        .expect("静态正则必须有效");
    let connection_re = Regex::new(r"(?i)^(usb|tcp|uart|serial|wlan)$")
        .expect("静态正则必须有效");
    let mut targets = Vec::new();

    for line in output.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let columns: Vec<&str> = line.split_whitespace().collect();
        if columns.len() < 2 {
            continue;
        }
        let status_index = columns.iter().position(|column| status_re.is_match(column));
        let Some(status_index) = status_index else {
            continue;
        };
        let connection_index = columns
            .iter()
            .position(|column| connection_re.is_match(column));
        let Some(connection_index) = connection_index else {
            continue;
        };
        let udid = columns[..status_index]
            .iter()
            .copied()
            .find(|column| !connection_re.is_match(column))
            .unwrap_or("");
        if udid.is_empty() || udid.eq_ignore_ascii_case("device") || status_index == 0 {
            continue;
        }
        targets.push(TargetInfo {
            udid: udid.to_string(),
            status: columns[status_index].to_string(),
            connection_type: columns[connection_index].to_string(),
        });
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::{parse_list_targets, HdcClient};
    use std::path::PathBuf;

    #[test]
    fn parses_mixed_target_output() {
        let output = include_str!("../../tests/fixtures/list_targets_v.txt");
        let targets = parse_list_targets(output);
        assert_eq!(targets.len(), 4);
        assert_eq!(targets[0].udid, "127.0.0.1:5555");
        assert_eq!(targets[1].connection_type, "USB");
        assert_eq!(targets[3].status, "Offline");
    }

    #[test]
    fn shell_arguments_keep_remote_command_as_one_argument() {
        let client = HdcClient::new(PathBuf::from("hdc"));
        assert_eq!(
            client.shell_args("ABC", "cat /proc/stat"),
            ["-t", "ABC", "shell", "cat /proc/stat"]
        );
    }
}
