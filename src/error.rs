use std::path::PathBuf;

use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyFileNotFoundError, PyRuntimeError, PyTimeoutError};
use thiserror::Error;

create_exception!(perfharmony, HdcError, PyException);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdcErrorKind {
    NotFound,
    TargetOffline,
    Timeout,
    CommandFailed,
    ParseFailed,
}

impl HdcErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::TargetOffline => "target_offline",
            Self::Timeout => "timeout",
            Self::CommandFailed => "command_failed",
            Self::ParseFailed => "parse_failed",
        }
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("未找到 HDC 可执行文件: {0}")]
    HdcNotFound(PathBuf),
    #[error("HDC 目标设备离线或不存在: {0}")]
    TargetOffline(String),
    #[error("HDC 命令超时: {0}")]
    Timeout(String),
    #[error("HDC 命令已取消: {0}")]
    Cancelled(String),
    #[error("HDC 命令失败: {0}")]
    CommandFailed(String),
    #[error("HDC 输出解析失败: {0}")]
    ParseFailed(String),
    #[error("启动 HDC 进程失败: {0}")]
    Io(#[from] std::io::Error),
}

impl Error {
    pub fn kind(&self) -> HdcErrorKind {
        match self {
            Self::HdcNotFound(_) => HdcErrorKind::NotFound,
            Self::TargetOffline(_) => HdcErrorKind::TargetOffline,
            Self::Timeout(_) => HdcErrorKind::Timeout,
            Self::Cancelled(_) => HdcErrorKind::Timeout,
            Self::CommandFailed(_) | Self::Io(_) => HdcErrorKind::CommandFailed,
            Self::ParseFailed(_) => HdcErrorKind::ParseFailed,
        }
    }
}

pub fn to_py_err(error: Error) -> pyo3::PyErr {
    let message = format!("{} [{}]", error, error.kind().as_str());
    match error {
        Error::HdcNotFound(_) => PyFileNotFoundError::new_err(message),
        Error::Timeout(_) | Error::Cancelled(_) => PyTimeoutError::new_err(message),
        Error::TargetOffline(_)
        | Error::CommandFailed(_)
        | Error::ParseFailed(_)
        | Error::Io(_) => PyRuntimeError::new_err(message),
    }
}
