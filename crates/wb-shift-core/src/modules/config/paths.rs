use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// 路径
// ---------------------------------------------------------------------------

/// 覆盖家目录的环境变量名。
///
/// 设置后 `home_dir()` 返回它的值，于是 `~/.workbuddy-shift`、`~/.codebuddy`、
/// `~/.codebuddy-rotate` 全部落在指定目录下。用途：
/// - 集成测试把家目录沙箱化，避免读写真实账号与 CLI 配置；
/// - 自定义部署位置。
///
/// 未设置时行为与之前完全一致（`dirs::home_dir()`）。
pub const HOME_ENV_VAR: &str = "WB_SHIFT_HOME";

pub fn home_dir() -> PathBuf {
    if let Some(overridden) = std::env::var_os(HOME_ENV_VAR) {
        if !overridden.is_empty() {
            return PathBuf::from(overridden);
        }
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// 真实家目录（**忽略** `WB_SHIFT_HOME` 覆盖）。
///
/// 与 `home_dir()` 的唯一差别是「有没有被环境变量重定向」：沙箱角标要把两者摆在一起
/// 展示，让开发者确认当前窗口读到的是沙箱副本还是真实数据。
pub fn real_home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// 开发沙箱根目录的环境变量名。
///
/// 启动器用它把开发沙箱放大分区（如 `D:\wb-shift-dev`）。它只参与「开发构建的默认值
/// 推导」，不改 [`HOME_ENV_VAR`] 的语义：最终生效的仍然是 `WB_SHIFT_HOME`。
pub const DEV_SANDBOX_ENV_VAR: &str = "WB_DEV_SANDBOX";

/// 未指定 [`DEV_SANDBOX_ENV_VAR`] 时，开发构建在家目录下使用的沙箱目录名。
pub const DEV_SANDBOX_DIR_NAME: &str = ".wb-shift-dev";

/// 开发构建（`tauri dev`）默认使用的沙箱根目录。
///
/// 优先用 `WB_DEV_SANDBOX`（启动器用它指定位置，可能是 D 盘这种大分区），否则退回真实
/// 家目录下的 `.wb-shift-dev`。宿主在 `run()` 最早期把它写进 [`HOME_ENV_VAR`]，于是
/// 开发者绕过启动器直接 `npm run tauri dev` 也不会碰到真实账号与配置。
pub fn dev_home_dir() -> PathBuf {
    let candidate = std::env::var(DEV_SANDBOX_ENV_VAR).ok();
    dev_home_dir_at(candidate.as_deref(), &real_home_dir())
}

/// 纯函数版本：不读环境变量、不碰真实家目录，便于单测注入。
///
/// 规则：
/// - `candidate` 为 `None` / 空串 / 全空白 → `real_home/.wb-shift-dev`；
/// - `candidate` 是绝对路径 → 原样使用（启动器设的 `D:\wb-shift-dev` 必须原样生效）；
/// - `candidate` 是相对路径 → 相对 `real_home` 解析，结果恒为绝对路径。
///
/// 相对路径一律基于 `real_home` 是刻意的：当前工作目录随启动方式变化（`npm run tauri
/// dev` 会落在 `src-tauri/`），直接把它当根目录会让沙箱数据散落在仓库里。
pub fn dev_home_dir_at(candidate: Option<&str>, real_home: &Path) -> PathBuf {
    let Some(raw) = candidate.map(str::trim).filter(|value| !value.is_empty()) else {
        return real_home.join(DEV_SANDBOX_DIR_NAME);
    };
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        real_home.join(path)
    }
}

/// 环境变量是否把家目录重定向到了沙箱。
///
/// 判定：值非空、且规范化后不等于真实家目录。
/// 后者防误标——有人把 WB_SHIFT_HOME 指回真实家目录时，界面不该显示沙箱角标。
///
/// 纯计算（不读文件、不加锁）：宿主首屏就会调用，`canonicalize` 失败也照旧给出结论。
pub fn is_sandboxed_home(candidate: Option<&str>, real_home: &Path) -> bool {
    let Some(raw) = candidate else {
        return false;
    };
    let candidate = raw.trim();
    if candidate.is_empty() {
        return false;
    }
    sandbox_path_key(Path::new(candidate)) != sandbox_path_key(real_home)
}

/// 路径比较口径：canonicalize 成功后取小写并剥尾分隔符；失败时退回原样规范化。
///
/// - 小写：Windows 路径大小写不敏感；
/// - 剥尾分隔符：`D:\wb-shift-dev\` 与 `D:\wb-shift-dev` 是同一个目录；
/// - canonicalize 需要目录存在，失败时用原字符串，判定不能因为沙箱目录尚未创建而翻转。
fn sandbox_path_key(path: &Path) -> String {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = resolved.to_string_lossy().to_lowercase();
    // 根目录（`/`）剥完成空串：只在仍有内容时剥尾分隔符。
    match text.trim_end_matches(&['/', '\\'][..]) {
        "" => text,
        trimmed => trimmed.to_string(),
    }
}

pub fn store_dir() -> PathBuf {
    home_dir().join(".workbuddy-shift")
}

pub fn accounts_file() -> PathBuf {
    store_dir().join("accounts.json")
}

pub fn backup_dir() -> PathBuf {
    store_dir().join("backups")
}

pub fn checkin_config_file() -> PathBuf {
    store_dir().join("auto_checkin_config.json")
}

pub fn checkin_logs_file() -> PathBuf {
    store_dir().join("auto_checkin_logs.json")
}

pub fn travel_config_file() -> PathBuf {
    store_dir().join("auto_travel_config.json")
}

/// 凭据（R6-06 按需解密）设置：只读 / 写开关、首次说明、平台探测结论。
///
/// 独立文件而不塞进签到配置：这份配置描述「本机客户端能不能解密」，
/// 与自动化调度无关，也不进设置包导出（换机后必须重新探测）。
pub fn credential_config_file() -> PathBuf {
    store_dir().join("credential.json")
}

pub fn travel_cache_file() -> PathBuf {
    store_dir().join("travel_cache.json")
}

pub fn credit_usage_snapshots_file() -> PathBuf {
    store_dir().join("credit_usage_snapshots.json")
}

pub fn rate_limit_config_file() -> PathBuf {
    store_dir().join("rate_limit_config.json")
}

pub fn official_usage_cache_file() -> PathBuf {
    store_dir().join("official_usage_cache.json")
}

pub fn auto_rotate_config_file() -> PathBuf {
    store_dir().join("auto_rotate_config.json")
}

pub fn auto_rotate_logs_file() -> PathBuf {
    store_dir().join("auto_rotate_logs.json")
}

pub fn workbuddy_exe_cache_file() -> PathBuf {
    store_dir().join("workbuddy_exe.json")
}
