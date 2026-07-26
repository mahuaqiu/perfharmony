use std::env;
use std::path::{Path, PathBuf};

use crate::error::Error;

/// 按显式配置、环境变量、Worker 包内 tools/hdc、SDK 和 PATH 的顺序解析 HDC。
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

    // Worker 打包目录 / 仓库内置工具：tools/hdc/hdc.exe。
    for relative in [
        "tools/hdc/hdc.exe",
        "tools/hdc/hdc",
        "tools/hdc",
        "hdc/hdc.exe",
        "hdc/hdc",
    ] {
        if let Some(path) = find_command(relative) {
            return Ok(path);
        }
        if let Ok(cwd) = env::current_dir() {
            let candidate = cwd.join(relative);
            if let Some(path) = find_command(&candidate.to_string_lossy()) {
                return Ok(path);
            }
        }
        if let Ok(exe) = env::current_exe() {
            if let Some(dir) = exe.parent() {
                let candidate = dir.join(relative);
                if let Some(path) = find_command(&candidate.to_string_lossy()) {
                    return Ok(path);
                }
            }
        }
    }

    for variable in [
        "HOS_SDK_HOME",
        "DEVECO_SDK_HOME",
        "OHOS_SDK_HOME",
        "HARMONY_SDK_HOME",
    ] {
        if let Ok(root) = env::var(variable) {
            for relative in [
                "toolchains/hdc",
                "toolchains/hdc.exe",
                "sdk/default/openharmony/toolchains/hdc",
                "sdk/default/openharmony/toolchains/hdc.exe",
            ] {
                let candidate = Path::new(&root).join(relative);
                if let Some(path) = find_command(&candidate.to_string_lossy()) {
                    return Ok(path);
                }
            }
            if let Some(path) = find_command(&root) {
                return Ok(path);
            }
        }
    }

    if let Some(path) = find_command("hdc") {
        return Ok(path);
    }

    Err(Error::HdcNotFound(PathBuf::from(explicit.unwrap_or("hdc"))))
}

fn find_command(candidate: &str) -> Option<PathBuf> {
    let candidate_path = Path::new(candidate);
    if candidate_path.components().count() > 1 || candidate_path.is_absolute() {
        if candidate_path.is_file() {
            return Some(candidate_path.to_path_buf());
        }
        if candidate_path.is_dir() {
            for relative in [
                "hdc.exe",
                "hdc",
                "toolchains/hdc.exe",
                "toolchains/hdc",
                "sdk/default/openharmony/toolchains/hdc.exe",
                "sdk/default/openharmony/toolchains/hdc",
            ] {
                let nested = candidate_path.join(relative);
                if nested.is_file() {
                    return Some(nested);
                }
            }
        }
        return None;
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

#[cfg(test)]
mod tests {
    use super::find_command;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn resolves_sdk_root_directory_to_nested_hdc() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("perfharmony-hdc-{stamp}"));
        let nested = root
            .join("sdk")
            .join("default")
            .join("openharmony")
            .join("toolchains");
        fs::create_dir_all(&nested).expect("create nested hdc dir");
        let hdc = nested.join(if cfg!(windows) { "hdc.exe" } else { "hdc" });
        fs::write(&hdc, b"fake").expect("write fake hdc");

        let resolved = find_command(&root.to_string_lossy()).expect("sdk root should resolve");
        assert_eq!(resolved, hdc);

        let _ = fs::remove_dir_all(root);
    }
}
