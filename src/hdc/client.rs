use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use regex::Regex;

use crate::error::Error;

/// Windows 下隐藏子进程控制台窗口（CREATE_NO_WINDOW）。
/// Worker 是无控制台的 GUI 进程，不加该标志每次拉起 hdc.exe
/// 都会新建控制台，采集时屏幕上黑框一闪而过。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn new_hdc_command(path: &PathBuf) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetInfo {
    pub udid: String,
    pub status: String,
    pub connection_type: String,
}

impl TargetInfo {
    /// 真机 HDC 的状态列是 Connected（如华为 MateBook 2in1），
    /// 部分版本/模拟器是 Ready，两者都视为可用。
    pub fn is_usable(&self) -> bool {
        self.status.eq_ignore_ascii_case("ready") || self.status.eq_ignore_ascii_case("connected")
    }
}

#[derive(Debug, Clone)]
pub struct HdcClient {
    path: PathBuf,
    default_timeout: Duration,
}

type TargetLocks = Mutex<HashMap<String, Arc<Mutex<()>>>>;

fn target_locks() -> &'static TargetLocks {
    static LOCKS: OnceLock<TargetLocks> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn target_lock(udid: &str) -> Arc<Mutex<()>> {
    let mut locks = target_locks().lock().expect("HDC target lock map poisoned");
    locks
        .entry(udid.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
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
            None,
        )?;
        let text = output_text(&output);
        if !output.status.success() {
            return Err(classify_command_error(udid, text));
        }
        Ok(text)
    }

    /// 可被 Monitor stop() 中断的 shell 调用。
    pub fn shell_with_cancel(
        &self,
        udid: &str,
        command: &str,
        timeout_secs: u64,
        cancel: &AtomicBool,
    ) -> Result<String, Error> {
        if udid.trim().is_empty() {
            return Err(Error::TargetOffline("udid 为空".to_string()));
        }
        let output = self.run(
            &["-t", udid, "shell", command],
            Duration::from_secs(timeout_secs.max(1)),
            Some(cancel),
        )?;
        let text = output_text(&output);
        if !output.status.success() {
            return Err(classify_command_error(udid, text));
        }
        Ok(text)
    }

    pub fn list_targets_raw(&self) -> Result<String, Error> {
        let output = self.run(&["list", "targets", "-v"], self.default_timeout, None)?;
        let text = output_text(&output);
        if !output.status.success() {
            return Err(Error::CommandFailed(text));
        }
        Ok(text)
    }

    #[cfg(test)]
    pub fn shell_args<'a>(&self, udid: &'a str, command: &'a str) -> [&'a str; 4] {
        ["-t", udid, "shell", command]
    }

    /// 打开长跑 shell 流（SP_daemon 连续采集等）：stdout 逐行经 channel 推送。
    ///
    /// 与 `run` 不同，流不持有 target 锁——SP_daemon 一跑可达数小时，期间
    /// `ps -ef` 等短命令仍需通过；hdc server 本身支持同设备多会话并发。
    pub fn shell_stream(&self, udid: &str, command: &str) -> Result<ShellStream, Error> {
        if udid.trim().is_empty() {
            return Err(Error::TargetOffline("udid 为空".to_string()));
        }
        let mut child = new_hdc_command(&self.path)
            .args(["-t", udid, "shell", command])
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
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::CommandFailed("无法获取 hdc stdout 管道".to_string()))?;
        let (sender, receiver) = std::sync::mpsc::channel::<String>();
        thread::Builder::new()
            .name("perfharmony-stream-reader".to_string())
            .spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    if sender.send(line).is_err() {
                        break;
                    }
                }
            })
            .map_err(Error::Io)?;
        // stderr 仍需排空，防止 hdc 因写满管道而阻塞。
        let stderr_reader = child.stderr.take().map(spawn_pipe_reader);
        Ok(ShellStream {
            child,
            receiver,
            stderr_reader,
        })
    }

    fn run(
        &self,
        args: &[&str],
        timeout: Duration,
        cancel: Option<&AtomicBool>,
    ) -> Result<Output, Error> {
        // 同一 UDID 的 Monitor、进程查询和其他 HDC 操作共用一把锁，避免设备端
        // 命令交错；等待锁时仍检查取消标志，保证 stop() 有界退出。
        let target_guard = args
            .windows(2)
            .find(|pair| pair[0] == "-t")
            .map(|pair| target_lock(pair[1]));
        let _target_lock_guard: Option<MutexGuard<'_, ()>> =
            if let Some(lock) = target_guard.as_ref() {
                Some(acquire_target_lock(lock, timeout, cancel)?)
            } else {
                None
            };

        let mut child = new_hdc_command(&self.path)
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

        // Windows 匿名管道缓冲仅约 64KB，真机 hidumper --mem 输出可超过 75KB；
        // 若等子进程退出后再读，hdc 会因写满管道而阻塞、直到超时被杀。
        // 必须在子进程运行期间用后台线程持续排空 stdout/stderr。
        let stdout_reader = child.stdout.take().map(spawn_pipe_reader);
        let stderr_reader = child.stderr.take().map(spawn_pipe_reader);

        let deadline = Instant::now() + timeout;
        loop {
            if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                terminate_child(&mut child);
                return Err(Error::Cancelled(format!(
                    "{} {:?}",
                    self.path.display(),
                    args
                )));
            }
            match child.try_wait()? {
                Some(status) => {
                    return Ok(Output {
                        status,
                        stdout: join_pipe_reader(stdout_reader),
                        stderr: join_pipe_reader(stderr_reader),
                    });
                }
                None if Instant::now() >= deadline => {
                    terminate_child(&mut child);
                    return Err(Error::Timeout(format!(
                        "{} {:?}",
                        self.path.display(),
                        args
                    )));
                }
                None => thread::sleep(Duration::from_millis(20)),
            }
        }
    }
}

fn spawn_pipe_reader<R: Read + Send + 'static>(mut pipe: R) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = pipe.read_to_end(&mut buffer);
        buffer
    })
}

/// 长跑流读取事件。
#[derive(Debug)]
pub enum StreamEvent {
    /// 读到一行（不含行尾符）。
    Line(String),
    /// 等待超时，本轮无新行（调用方借此检查停止标志）。
    Idle,
    /// 流已结束（设备端进程退出或 hdc 断开）。
    Closed,
}

/// SP_daemon 等长跑命令的 stdout 行流句柄；drop 时确保杀掉 hdc 子进程。
#[derive(Debug)]
pub struct ShellStream {
    child: Child,
    receiver: Receiver<String>,
    stderr_reader: Option<thread::JoinHandle<Vec<u8>>>,
}

impl ShellStream {
    /// 等待下一行；在 `timeout` 内无新行返回 `Idle`。
    pub fn next_line(&mut self, timeout: Duration) -> StreamEvent {
        match self.receiver.recv_timeout(timeout) {
            Ok(line) => StreamEvent::Line(line),
            Err(RecvTimeoutError::Timeout) => StreamEvent::Idle,
            Err(RecvTimeoutError::Disconnected) => StreamEvent::Closed,
        }
    }

    /// 终止本地 hdc 子进程（设备端 SP_daemon 由调用方下发 `SP_daemon -stop` 清理）。
    pub fn stop(&mut self) {
        terminate_child(&mut self.child);
        if let Some(handle) = self.stderr_reader.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for ShellStream {
    fn drop(&mut self) {
        self.stop();
    }
}

fn join_pipe_reader(handle: Option<thread::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    handle
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default()
}

fn acquire_target_lock<'a>(
    lock: &'a Arc<Mutex<()>>,
    timeout: Duration,
    cancel: Option<&AtomicBool>,
) -> Result<MutexGuard<'a, ()>, Error> {
    let deadline = Instant::now() + timeout;
    loop {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(Error::Cancelled("等待 HDC target 锁".to_string()));
        }
        match lock.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(std::sync::TryLockError::Poisoned(error)) => return Ok(error.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) if Instant::now() >= deadline => {
                return Err(Error::Timeout("等待 HDC target 锁".to_string()));
            }
            Err(std::sync::TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(10)),
        }
    }
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
    if [
        "offline",
        "unauthorized",
        "not found",
        "no devices",
        "not connected",
    ]
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
    let connection_re = Regex::new(r"(?i)^(usb|tcp|uart|serial|wlan)$").expect("静态正则必须有效");
    let mut targets = Vec::new();

    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
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
        assert_eq!(targets.len(), 5);
        assert_eq!(targets[0].udid, "127.0.0.1:5555");
        assert_eq!(targets[1].connection_type, "USB");
        assert_eq!(targets[3].status, "Offline");
        // 真机（华为 MateBook 2in1）输出：tab 分隔 + Connected 状态
        assert_eq!(targets[4].udid, "3QC0124A10000066");
        assert_eq!(targets[4].status, "Connected");
        assert!(targets[4].is_usable());
        assert!(!targets[3].is_usable());
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
