//! 客户端版本读取——**全仓唯一实现**。
//!
//! R6 的两处消费方必须共用本函数，禁止各写一份：
//! - 诊断包 `[WorkBuddy]` 段的「版本」字段；
//! - R6-06 解密通道的平台闸门（`platformProbeVersion`，客户端升级后要重新探测）。
//!
//! # 为什么不用 Win32 版本资源
//!
//! WorkBuddy 客户端（Electron 打包）在安装目录的 `resources/install-manifest.json`
//! 里带 `appVersion`，本机实测 `5.11.3` 与资源管理器「文件版本」一致。读 JSON 不需要
//! 新增 `windows` crate feature，也能在 macOS 的 `.app` 包内同构命中，因此作为首选。
//! 取不到时退回 macOS `Info.plist` 的 `CFBundleShortVersionString`；再取不到返回
//! `None`（调用方写「版本未知」），**不报错、不阻塞**。
//!
//! 入参是 `auth_file::workbuddy_app_path(variant)` 的结果：Windows/Linux 是客户端
//! 可执行文件，macOS 是 `.app` 目录。两种形态都在本函数里归一。

use std::path::{Path, PathBuf};

/// 读取客户端版本；取不到返回 `None`。
///
/// 纯只读：只 `read` 候选文件，不创建、不修改、不删除任何东西。
pub fn client_version(app_path: &Path) -> Option<String> {
    if app_path.as_os_str().is_empty() {
        return None;
    }
    for candidate in manifest_candidates(app_path) {
        if let Some(version) = read_manifest_version(&candidate) {
            return Some(version);
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(version) = read_plist_version(app_path) {
        return Some(version);
    }
    None
}

/// `install-manifest.json` 的候选位置（exe 目录 / `.app` 包内）。
fn manifest_candidates(app_path: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if app_path.is_dir() {
        roots.push(app_path.to_path_buf());
        // macOS `.app`：Contents/Resources/install-manifest.json
        roots.push(app_path.join("Contents").join("Resources"));
    } else if let Some(parent) = app_path.parent() {
        roots.push(parent.to_path_buf());
    }
    roots
        .into_iter()
        .map(|root| root.join("resources").join("install-manifest.json"))
        .chain(
            // `.app` 形态已经拼过 Contents/Resources，这里再补一次不带 resources 的
            // 直落文件（个别打包器会把清单放在 Contents/Resources 根下）。
            app_path.is_dir().then(|| {
                app_path
                    .join("Contents")
                    .join("Resources")
                    .join("install-manifest.json")
            }),
        )
        .filter(|path| path.is_file())
        .collect()
}

/// 从清单 JSON 取 `appVersion`；形态不符或读失败一律 `None`。
fn read_manifest_version(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let version = value.get("appVersion")?.as_str()?.trim();
    (!version.is_empty()).then(|| version.to_string())
}

/// macOS `Info.plist` 的最小解析：只认 `CFBundleShortVersionString` 的下一处 `<string>`。
///
/// 不引入 plist 解析依赖：这里只需要一个短字符串，`plutil` 又要起子进程。
#[cfg(target_os = "macos")]
fn read_plist_version(app_path: &Path) -> Option<String> {
    let plist = if app_path.is_dir() {
        app_path.join("Contents").join("Info.plist")
    } else {
        app_path.parent()?.join("Info.plist")
    };
    let text = std::fs::read_to_string(plist).ok()?;
    let key_at = text.find("<key>CFBundleShortVersionString</key>")?;
    let after_key = &text[key_at..];
    let start = after_key.find("<string>")? + "<string>".len();
    let rest = &after_key[start..];
    let end = rest.find("</string>")?;
    let version = rest[..end].trim();
    (!version.is_empty()).then(|| version.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wb-shift-version-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn reads_app_version_from_install_manifest() {
        let dir = temp_dir("manifest");
        let resources = dir.join("resources");
        std::fs::create_dir_all(&resources).expect("create resources");
        std::fs::write(
            resources.join("install-manifest.json"),
            r#"{"version":"1","appVersion":"5.11.3"}"#,
        )
        .expect("write manifest");
        let exe = dir.join("WorkBuddy.exe");
        std::fs::write(&exe, b"").expect("write exe placeholder");
        assert_eq!(client_version(&exe), Some("5.11.3".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_manifest_is_none_and_does_not_create_files() {
        let dir = temp_dir("missing");
        let exe = dir.join("WorkBuddy.exe");
        std::fs::write(&exe, b"").expect("write exe placeholder");
        assert_eq!(client_version(&exe), None);
        assert!(!dir.join("resources").exists());
        assert_eq!(client_version(Path::new("")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_manifest_is_none() {
        let dir = temp_dir("malformed");
        let resources = dir.join("resources");
        std::fs::create_dir_all(&resources).expect("create resources");
        std::fs::write(resources.join("install-manifest.json"), b"{not json")
            .expect("write manifest");
        let exe = dir.join("WorkBuddy.exe");
        std::fs::write(&exe, b"").expect("write exe placeholder");
        assert_eq!(client_version(&exe), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
