use std::env;
use std::path::{Path, PathBuf};

use crate::error::Error;

/// 按显式配置、环境变量、SDK 和 PATH 的顺序解析 HDC。
pub fn resolve_hdc_path(explicit: Option<&str>) -> Result<PathBuf, Error> {
    let explicit_value = explicit.map(str::to_string);
    let perfharmony_value = env::var("PERFHARMONY_HDC").ok();
    let hdc_value = env::var("HDC_PATH").ok();
    for candidate in [
        explicit_value.as_deref(),
        perfharmony_value.as_deref(),
        hdc_value.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(path) = find_command(candidate) {
            return Ok(path);
        }
    }

    for variable in ["HOS_SDK_HOME", "DEVECO_SDK_HOME"] {
        if let Ok(root) = env::var(variable) {
            for relative in [
                "toolchains/hdc",
                "toolchains/hdc.exe",
                "sdk/default/openharmony/toolchains/hdc",
            ] {
                let candidate = Path::new(&root).join(relative);
                if let Some(path) = find_command(&candidate.to_string_lossy()) {
                    return Ok(path);
                }
            }
        }
    }

    if let Some(path) = find_command("hdc") {
        return Ok(path);
    }

    Err(Error::HdcNotFound(PathBuf::from(
        explicit.unwrap_or("hdc"),
    )))
}

fn find_command(candidate: &str) -> Option<PathBuf> {
    let candidate_path = Path::new(candidate);
    if candidate_path.components().count() > 1 || candidate_path.is_absolute() {
        return candidate_path
            .is_file()
            .then(|| candidate_path.to_path_buf());
    }

    let path_var = env::var_os("PATH")?;
    for directory in env::split_paths(&path_var) {
        let direct = directory.join(candidate);
        if direct.is_file() {
            return Some(direct);
        }
        #[cfg(windows)]
        for extension in [".exe", ".cmd", ".bat"] {
            let with_extension = directory.join(format!("{candidate}{extension}"));
            if with_extension.is_file() {
                return Some(with_extension);
            }
        }
    }
    None
}
