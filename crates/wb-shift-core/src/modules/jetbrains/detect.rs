//! JetBrains 安装/运行检测：配置根与插件目录、运行中 IDE 枚举、切换目标与当前账号识别。
//!
//! 本模块只做「识别」，不做任何写入；识别不到时必须留下**可诊断的痕迹**
//! （[`MappingSource`] / [`ProductInfo::note`] / [`DirScan`]），而不是静默过滤掉。

#[allow(unused_imports)]
use super::status::*;
#[allow(unused_imports)]
use super::switch::*;
use super::*;

/// 配置根目录名（平台基础目录下的并列子目录）。
///
/// - `JetBrains`：IDEA / PyCharm / GoLand 等 JetBrains 自家 IDE；
/// - `Google`：Android Studio（Google 发行，配置写在 `.../Google/AndroidStudio<版本>`）。
const CONFIG_ROOT_DIR_NAMES: [&str; 2] = ["JetBrains", "Google"];

/// JetBrains 系配置根（应用级，各产品 `<产品><版本>` 子目录并列）。
///
/// 平台基础目录：
/// - Windows: `%APPDATA%`
/// - macOS: `~/Library/Application Support`
/// - Linux: `$XDG_CONFIG_HOME`（缺省 `~/.config`）
///
/// 返回**全部候选根**（通常 2 个，可能不存在）；调用方必须逐个扫描，不能只取第一个
/// ——只扫 JetBrains 根会把 Android Studio 整体判成「未安装」。
pub fn jetbrains_config_roots() -> Vec<PathBuf> {
    let Some(base) = config_root_base() else {
        return Vec::new();
    };
    CONFIG_ROOT_DIR_NAMES
        .iter()
        .map(|name| base.join(name))
        .collect()
}

/// 配置根的平台基础目录；拿不到（无法定位用户目录）时返回 `None`。
fn config_root_base() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Some(crate::modules::config::home_dir().join("Library/Application Support"))
    }
    #[cfg(target_os = "windows")]
    {
        dirs::data_dir()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        dirs::config_dir()
    }
}

// ---------------------------------------------------------------------------
// 配置目录名判定
// ---------------------------------------------------------------------------

/// 版本号后可跟的发行通道限定符（大小写不敏感）。
///
/// 2026.3 起 EAP 用户会得到 `IntelliJIdea2026.3-eap` 这类目录名；旧实现只认
/// 数字与点，把 EAP 用户整体判成「没有可切换的 IDE」。仍然拒绝 `-backup`
/// 这类迁移残留（不在白名单内）。
const RELEASE_CHANNELS: [&str; 5] = ["-eap", "-nightly", "-beta", "-rc", "-snapshot"];

/// `2026.3` / `2026.3.1` / `2026.3-eap` 形态的版本号后缀。
///
/// 版本主体必须是「数字与点、且以数字结尾」（排除 `PyCharm`、`PyCharmX`），
/// 可再接一个 [`RELEASE_CHANNELS`] 里的限定符；其余一律拒绝。
fn is_version_suffix(suffix: &str) -> bool {
    let (version, channel) = match suffix.find('-') {
        Some(index) => (&suffix[..index], Some(&suffix[index..])),
        None => (suffix, None),
    };
    if version.is_empty()
        || !version.chars().all(|c| c.is_ascii_digit() || c == '.')
        || !version.ends_with(|c: char| c.is_ascii_digit())
    {
        return false;
    }
    match channel {
        None => true,
        Some(rest) => RELEASE_CHANNELS
            .iter()
            .any(|allowed| rest.eq_ignore_ascii_case(allowed)),
    }
}

/// 配置目录名是否属于受支持产品：产品前缀 + 版本号（可带发行通道限定符）。
pub(super) fn is_supported_config_dir_name(name: &str) -> bool {
    PRODUCT_PREFIXES.iter().any(|prefix| {
        name.len() > prefix.len()
            && name.starts_with(prefix)
            && is_version_suffix(&name[prefix.len()..])
    })
}

/// `<配置目录>/plugins/` 下所有 CodeBuddy 插件安装目录（按名排序，便于诊断稳定输出）。
///
/// 匹配规则是**包含** `coding-copilot`（大小写不敏感），而不是「以它开头」：
/// 插件目录名可能是 marketplace id（`Tencent-Cloud.coding-copilot`）或带前后缀的
/// 版本目录，用前缀判定会把已安装插件判成未安装。
pub(super) fn plugin_dirs(config_dir: &Path) -> Vec<PathBuf> {
    let plugins = config_dir.join("plugins");
    let Ok(entries) = std::fs::read_dir(&plugins) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains(PLUGIN_DIR_MARKER)
        })
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}

/// `<配置目录>/plugins/` 下是否安装了 CodeBuddy 插件（返回首个命中目录）。
pub(super) fn plugin_dir(config_dir: &Path) -> Option<PathBuf> {
    plugin_dirs(config_dir).into_iter().next()
}

// ---------------------------------------------------------------------------
// product-info.json 探测与配置目录映射
// ---------------------------------------------------------------------------

/// 运行中实例的配置目录映射来源（诊断口径，不含任何 secret）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MappingSource {
    /// 由 `<安装根>/product-info.json#dataDirectoryName` 精确解析。
    ProductInfo,
    /// `product-info.json` 缺失 / 损坏，按 exe 名与安装目录名推断（P1 兜底）。
    Fallback,
    /// 映射失败：切换前必须阻断（否则写入会被 IDE 内存态覆盖）。
    Unmapped,
}

impl MappingSource {
    /// JSON / 日志里的稳定字符串（前端按它区分「精确」与「推断」）。
    pub(super) fn as_str(self) -> &'static str {
        match self {
            MappingSource::ProductInfo => "product-info",
            MappingSource::Fallback => "fallback",
            MappingSource::Unmapped => "unmapped",
        }
    }
}

/// `product-info.json` 的解析结论；失败原因留在 [`ProductInfo::note`] 里供诊断展示。
#[derive(Debug, Clone, Default)]
pub(super) struct ProductInfo {
    pub(super) data_dir_name: Option<String>,
    pub(super) name: Option<String>,
    pub(super) version: Option<String>,
    /// 读取/解析/守卫失败原因（成功时为空）。
    pub(super) note: Option<String>,
}

/// 读取 `<安装根>/product-info.json` 并按配置目录名规则做守卫。
///
/// 守卫的意义：`dataDirectoryName` 是写入目标名，若它不符合 [`is_supported_config_dir_name`]，
/// 说明该目录不会被 [`list_targets`] 列出——据此映射去关闭 IDE 只会造成「关了却不写」。
pub(super) fn read_product_info(root: &Path) -> ProductInfo {
    let path = root.join("product-info.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            return ProductInfo {
                note: Some(if path.exists() {
                    format!("读取 product-info.json 失败: {error}")
                } else {
                    "product-info.json 不存在".to_string()
                }),
                ..ProductInfo::default()
            }
        }
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            return ProductInfo {
                note: Some(format!("product-info.json 解析失败: {error}")),
                ..ProductInfo::default()
            }
        }
    };
    let as_trimmed = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let name = as_trimmed("name");
    let version = as_trimmed("version");
    let raw_dir = as_trimmed("dataDirectoryName");
    let (data_dir_name, note) = match raw_dir {
        None => (
            None,
            Some("product-info.json 缺少 dataDirectoryName".to_string()),
        ),
        Some(dir) if !is_supported_config_dir_name(&dir) => (
            None,
            Some(format!(
                "dataDirectoryName 不在受支持产品范围内（{dir}），已忽略 product-info 结果"
            )),
        ),
        Some(dir) => (Some(dir), None),
    };
    ProductInfo {
        data_dir_name,
        name,
        version,
        note,
    }
}

/// 从主进程可执行文件路径反推安装根：`<root>/bin/<x>.exe` → `<root>`。
#[cfg(target_os = "windows")]
pub(super) fn install_root_from_exe(exe: &Path) -> Option<PathBuf> {
    exe.parent().and_then(Path::parent).map(Path::to_path_buf)
}

/// 安装目录名里的版本号（取最后一个形如 `2026.3[-eap]` 的空白分隔 token）。
fn version_from_dir_name(dir_name: &str) -> Option<String> {
    dir_name
        .rsplit(|c: char| c.is_whitespace() || c == '_')
        .find(|token| {
            token.chars().next().is_some_and(|c| c.is_ascii_digit()) && is_version_suffix(token)
        })
        .map(str::to_string)
}

/// P1 兜底：`product-info.json` 读不到时，按 exe 名 + 安装目录名推断配置目录名。
///
/// 只在「能同时拿到产品与版本」时返回；社区版按安装目录名里的 `community` 判到
/// `IdeaIC` / `PyCharmCE`（与 JetBrains 的 `dataDirectoryName` 规则一致）。
fn fallback_config_dir_name(root: &Path, exe: Option<&Path>) -> Option<String> {
    let prefix = product_prefix_from_exe(exe?)?;
    let dir_name = root.file_name()?.to_string_lossy().into_owned();
    let community = dir_name.to_ascii_lowercase().contains("community");
    let prefix = match (prefix, community) {
        ("IntelliJIdea", true) => "IdeaIC",
        ("PyCharm", true) => "PyCharmCE",
        (other, _) => other,
    };
    let version = version_from_dir_name(&dir_name)?;
    let candidate = format!("{prefix}{version}");
    // 兜底结论同样要过守卫：否则会得到 `list_targets` 永远不会列出的名字。
    is_supported_config_dir_name(&candidate).then_some(candidate)
}

/// 一个运行实例的配置目录映射结果。
#[derive(Debug, Clone, Default)]
pub(super) struct ConfigDirMapping {
    /// 映射到的配置目录名；`None` = 映射失败（[`MappingSource::Unmapped`]）。
    pub(super) name: Option<String>,
    pub(super) source: Option<MappingSource>,
    pub(super) product_name: Option<String>,
    pub(super) version: Option<String>,
    /// 降级 / 失败原因（诊断展示用）。
    pub(super) note: Option<String>,
}

/// 解析一个安装根的配置目录名：product-info 优先，失败则按 exe/目录名兜底。
pub(super) fn resolve_config_dir(root: &Path, exe: Option<&Path>) -> ConfigDirMapping {
    let info = read_product_info(root);
    if let Some(name) = info.data_dir_name.clone() {
        return ConfigDirMapping {
            name: Some(name),
            source: Some(MappingSource::ProductInfo),
            product_name: info.name,
            version: info.version,
            note: None,
        };
    }
    let fallback = fallback_config_dir_name(root, exe);
    let product_name = info.name.clone().or_else(|| {
        exe.and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
    });
    let dir_name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match fallback {
        Some(name) => ConfigDirMapping {
            name: Some(name),
            source: Some(MappingSource::Fallback),
            product_name,
            version: version_from_dir_name(&dir_name),
            note: info.note,
        },
        None => ConfigDirMapping {
            name: None,
            source: Some(MappingSource::Unmapped),
            product_name,
            version: None,
            note: info
                .note
                .or_else(|| Some("无法从安装目录推断产品与版本".to_string())),
        },
    }
}

// ---------------------------------------------------------------------------
// 进程发现：运行中的 IDE 与其配置目录的精确映射
// ---------------------------------------------------------------------------

/// 运行中的 JetBrains IDE 实例快照（关闭前采集）。
#[derive(Debug, Clone)]
pub(super) struct RunningIde {
    pub(super) pid: u32,
    /// Windows: 主进程 exe；macOS: `.app` bundle；Linux: 安装根（best-effort）。
    pub(super) launch_target: Option<PathBuf>,
    /// 由安装根解析出的配置目录名；`None` = 映射失败（切换必须阻断）。
    pub(super) config_dir_name: Option<String>,
    pub(super) mapping_source: MappingSource,
    /// 产品显示名（product-info 的 `name` 或 exe 名），仅用于提示与诊断。
    pub(super) product_name: Option<String>,
    pub(super) version: Option<String>,
    /// 映射降级 / 失败原因。
    pub(super) mapping_note: Option<String>,
}

impl RunningIde {
    /// 诊断与报错里的一行实例描述（不含 secret，只含产品名/PID/路径）。
    pub(super) fn describe(&self) -> String {
        let product = self
            .product_name
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| {
                self.launch_target
                    .as_deref()
                    .and_then(Path::file_name)
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "未知 JetBrains IDE".to_string());
        let version = self
            .version
            .as_deref()
            .map(|v| format!(" {v}"))
            .unwrap_or_default();
        match self.launch_target.as_deref() {
            Some(path) => format!(
                "{product}{version}（PID {}，{}）",
                self.pid,
                path.to_string_lossy()
            ),
            None => format!("{product}{version}（PID {}）", self.pid),
        }
    }

    /// 由解析结果构造实例快照。
    fn from_mapping(
        pid: u32,
        launch_target: Option<PathBuf>,
        mapping: Option<ConfigDirMapping>,
        default_note: &str,
    ) -> RunningIde {
        match mapping {
            Some(mapping) => RunningIde {
                pid,
                launch_target,
                config_dir_name: mapping.name,
                mapping_source: mapping.source.unwrap_or(MappingSource::Unmapped),
                product_name: mapping.product_name,
                version: mapping.version,
                mapping_note: mapping.note,
            },
            None => RunningIde {
                pid,
                launch_target,
                config_dir_name: None,
                mapping_source: MappingSource::Unmapped,
                product_name: None,
                version: None,
                mapping_note: Some(default_note.to_string()),
            },
        }
    }
}

/// 从运行进程行构造快照；映射不上的实例 `config_dir_name = None`。
#[cfg(target_os = "windows")]
pub(super) fn running_ide_from_row(row: &process::WindowsProcessRow) -> RunningIde {
    let root = row
        .exe_path
        .as_deref()
        .and_then(install_root_from_exe)
        .map(|root| (root, row.exe_path.as_deref()));
    let mapping = root.map(|(root, exe)| resolve_config_dir(&root, exe));
    RunningIde::from_mapping(
        row.pid,
        row.exe_path.clone(),
        mapping,
        "未能取得进程可执行文件路径（tasklist 兜底），无法映射配置目录",
    )
}

#[cfg(target_os = "windows")]
pub(super) fn windows_ide_process_rows() -> Vec<process::WindowsProcessRow> {
    let self_pid = std::process::id();
    // 映像名清单由 `PRODUCTS` 派生（16 个产品 × 64/32 位），PowerShell 的 `-in`、
    // tasklist 兜底与结果过滤三处共用同一份，杜绝「补了产品但漏了某一处」。
    let images = windows_image_names();
    let image_filter: String = images
        .iter()
        .map(|image| format!("'{image}'"))
        .collect::<Vec<_>>()
        .join(",");
    let script = format!(
        "Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | \
         Where-Object {{ $_.Name -in {image_filter} }} | \
         ForEach-Object {{ '{{0}}|{{1}}|{{2}}' -f $_.ProcessId, $_.Name, $_.ExecutablePath }}"
    );
    let mut rows: Vec<_> = process::ps_output(&script, 8)
        .map(|stdout| {
            process::parse_windows_process_rows(&stdout)
                .into_iter()
                .filter(|row| row.pid != self_pid)
                .collect()
        })
        .unwrap_or_default();
    if rows.is_empty() {
        // CIM 不可用时退回 tasklist（拿到 PID 关闭，但丢失 exe 路径 → 无法重开）。
        for image in &images {
            rows.extend(process::windows_tasklist_image_rows(image));
        }
    }
    rows.retain(|row| {
        let name = row.name.trim();
        images.iter().any(|n| name.eq_ignore_ascii_case(n))
    });
    rows
}

/// 枚举当前运行中的受支持 IDE 实例（同配置目录多实例按 PID 全保留）。
pub(super) fn running_ides() -> Vec<RunningIde> {
    #[cfg(target_os = "windows")]
    {
        windows_ide_process_rows()
            .iter()
            .map(running_ide_from_row)
            .collect()
    }
    #[cfg(target_os = "macos")]
    {
        // 进程 cmdline 形如 `/Applications/IntelliJ IDEA.app/Contents/MacOS/idea`；
        // bundle 路径回溯后读 `Contents/Resources/product-info.json`。
        // `.app` 名清单由 `products.rs` 的产品表派生（含 `Android Studio.app`）。
        let patterns: Vec<String> = macos_process_patterns();
        let mut rows = Vec::new();
        for (pid, args) in process::macos_rows_by_patterns(&patterns) {
            let bundle = process::extract_app_bundle_from_args(&args);
            let mapping = bundle
                .as_deref()
                .map(|b| resolve_config_dir(&b.join("Contents/Resources"), None));
            rows.push(RunningIde::from_mapping(
                pid,
                bundle,
                mapping,
                "未能确定 .app bundle 路径，无法映射配置目录",
            ));
        }
        rows
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // Linux：脚本启动时 argv[0] 是 `<root>/bin/<x>`；java 直启时 cmdline 含
        // `-Didea.home.path=<root>`。两者都取不到就放弃映射（只报告、不关闭）。
        let self_pid = std::process::id();
        let mut rows = Vec::new();
        if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let pid: u32 = match entry.file_name().to_string_lossy().parse() {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                if pid == self_pid {
                    continue;
                }
                let cmdline = match std::fs::read(format!("/proc/{pid}/cmdline")) {
                    Ok(bytes) if !bytes.is_empty() => {
                        String::from_utf8_lossy(&bytes).replace('\0', " ")
                    }
                    _ => continue,
                };
                let lower = cmdline.to_ascii_lowercase();
                // 可执行名清单由 `PRODUCTS` 派生（含 `studio`、`appcode` 等 16 个产品）。
                if !linux_executable_names().iter().any(|name| {
                    lower.contains(&format!("/bin/{name} "))
                        || lower.contains(&format!("/bin/{name}\""))
                        || lower.ends_with(&format!("/bin/{name}"))
                }) && !lower.contains("-didea.home.path=")
                {
                    continue;
                }
                let root = cmdline.split_whitespace().find_map(|arg| {
                    let path = Path::new(arg);
                    if path.is_dir() && path.join("product-info.json").exists() {
                        return Some(path.to_path_buf());
                    }
                    // `<root>/bin/<x>` → `<root>`
                    let mut ancestors = path.ancestors();
                    ancestors.next()?; // 自身
                    let bin = ancestors.next()?;
                    let root = bin.parent()?;
                    if root.join("product-info.json").exists() && bin.ends_with("bin") {
                        return Some(root.to_path_buf());
                    }
                    None
                });
                let mapping = root.as_deref().map(|r| resolve_config_dir(r, None));
                rows.push(RunningIde::from_mapping(
                    pid,
                    root,
                    mapping,
                    "未能从 /proc cmdline 定位安装根，无法映射配置目录",
                ));
            }
        }
        rows
    }
}

// ---------------------------------------------------------------------------
// 状态与切换
// ---------------------------------------------------------------------------

/// 配置根下一个子目录的扫描结论（诊断用，不含任何 secret 值）。
#[derive(Debug, Clone)]
pub(super) struct DirScan {
    pub(super) name: String,
    /// 是否通过了配置目录名判定（否则不会出现在切换列表里）。
    pub(super) accepted: bool,
    /// `plugins/` 下命中的 CodeBuddy 插件目录名（排序后）。
    pub(super) plugin_dirs: Vec<String>,
    /// `options/secret-storage.xml` 是否存在（只报是否存在，不读内容）。
    pub(super) has_secret_file: bool,
}

impl DirScan {
    pub(super) fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "accepted": self.accepted,
            "pluginDirs": self.plugin_dirs,
            "pluginInstalled": !self.plugin_dirs.is_empty(),
            "hasSecretFile": self.has_secret_file,
        })
    }
}

/// 扫描配置根下所有子目录，给出「为什么这个目录没进列表」的依据。
pub(super) fn scan_config_root(root: &Path) -> Vec<DirScan> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut scans: Vec<DirScan> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            DirScan {
                accepted: is_supported_config_dir_name(&name),
                plugin_dirs: plugin_dirs(&path)
                    .into_iter()
                    .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .collect(),
                has_secret_file: path.join(SECRET_RELATIVE).exists(),
                name,
            }
        })
        .collect();
    scans.sort_by(|a, b| a.name.cmp(&b.name));
    scans
}

/// 逐个配置根的存在性结论（诊断输出 `rootStates`）。
///
/// 保留「路径 + 是否存在」两段信息：只给一个布尔值的话，用户看不出我们到底扫了哪些位置
/// ——Android Studio 的根不存在时，用户会以为工具没支持 AS。
pub(super) fn config_root_states() -> Vec<(PathBuf, bool)> {
    jetbrains_config_roots()
        .into_iter()
        .map(|root| {
            let exists = root.exists();
            (root, exists)
        })
        .collect()
}

/// 遍历**全部存在的**配置根，汇总子目录扫描结论（诊断用）。
///
/// 两个根下同名目录只保留第一个（与 [`list_targets`] 同口径），否则诊断清单里会出现
/// 两行同名条目，用户无法判断哪一行才是真正被写入的目标。
pub(super) fn scan_config_roots() -> Vec<DirScan> {
    scan_config_roots_from(&jetbrains_config_roots())
}

/// [`scan_config_roots`] 的可注入版本（用例传临时根，不读真实 `%APPDATA%`）。
pub(super) fn scan_config_roots_from(roots: &[PathBuf]) -> Vec<DirScan> {
    let mut scans: Vec<DirScan> = Vec::new();
    for root in roots.iter().filter(|root| root.exists()) {
        for scan in scan_config_root(root) {
            if !scans.iter().any(|known| known.name == scan.name) {
                scans.push(scan);
            }
        }
    }
    scans.sort_by(|a, b| a.name.cmp(&b.name));
    scans
}

/// 切换目标：一个装有（或可装）CodeBuddy 插件的产品配置目录。
#[derive(Debug, Clone)]
pub(super) struct Target {
    /// 产品配置目录（如 `%APPDATA%\JetBrains\PyCharm2026.2`）。
    pub(super) config_dir: PathBuf,
    /// 配置目录名（如 `PyCharm2026.2`）。
    pub(super) name: String,
    /// 是否安装了 CodeBuddy 插件。
    pub(super) plugin_installed: bool,
}

impl Target {
    pub(super) fn secret_path(&self) -> PathBuf {
        secret_path_for(&self.config_dir)
    }

    pub(super) fn to_status_json(&self, running_dirs: &[String]) -> Value {
        let logged_in = read_secret_for(&self.config_dir).ok().flatten().is_some();
        json!({
            "configDir": self.name,
            "pluginInstalled": self.plugin_installed,
            "running": running_dirs.iter().any(|n| n == &self.name),
            "loggedIn": logged_in,
            "secretPath": self.secret_path().to_string_lossy(),
        })
    }
}

/// 扫描全部配置根，列出受支持产品的配置目录（存在即列出，无论是否装插件）。
///
/// 两个根都扫（`.../JetBrains` 与 `.../Google`）：Android Studio 只在后者下面，
/// 只看第一个根会把 AS 用户整体判成「未安装」。
/// 两个根下出现同名目录（历史迁移残留）时只认**先命中的那个**（JetBrains 根优先），
/// 否则切换列表里会出现两行同名目标、写入时后一行静默覆盖前一行。
pub(super) fn list_targets() -> Vec<Target> {
    list_targets_from(&jetbrains_config_roots())
}

/// [`list_targets`] 的可注入版本：`roots` 由调用方给（用例传临时根）。
pub(super) fn list_targets_from(roots: &[PathBuf]) -> Vec<Target> {
    let mut targets: Vec<Target> = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !is_supported_config_dir_name(&name) {
                continue;
            }
            if targets.iter().any(|known| known.name == name) {
                continue;
            }
            targets.push(Target {
                plugin_installed: plugin_dir(&path).is_some(),
                config_dir: path,
                name,
            });
        }
    }
    targets.sort_by(|a, b| a.name.cmp(&b.name));
    targets
}

/// 从 `secret-storage.xml` 读到的一次「当前登录」识别结论。
///
/// 只描述**读到了什么**，不含任何 secret 值；写入（回写本地状态文件）由调用方决定——
/// [`status`](super::status::status) 只读不写，`detect_current_account` 才落盘。
///
/// **红线**：本模块对 `secret-storage.xml` 永远只读，绝不写回。
#[derive(Debug, Clone)]
pub(super) enum SecretScan {
    /// 没有任何装了插件的配置目录读到可用登录态。
    NotFound,
    /// 读到登录态，但账号库里没有能与它对上的账号。
    Unmatched {
        config_dir: String,
        uid: Option<String>,
    },
    /// 命中账号库里的账号。
    Matched {
        config_dir: String,
        account: Value,
        uid: Option<String>,
    },
}

/// 扫描装了插件的配置目录，识别当前登录账号；`matcher` 由调用方注入。
///
/// 生产路径传账号库匹配（[`scan_secret_account`]），用例传固定夹具——这样「从
/// `secret-storage.xml` 读登录态」这段逻辑可以被单测覆盖，而不必读真实账号库
/// （AGENTS.md 第 8 节）。
///
/// 与历史实现同序：遇到**第一个**能解析出登录态的配置目录就给出结论（无论是否匹配上
/// 账号库），不再继续扫后面的产品——多个产品写的是同一份账号，先命中的就是真实值。
pub(super) fn scan_secret_account_with(
    targets: &[Target],
    matcher: &dyn Fn(Option<&str>, &str) -> Option<Value>,
) -> SecretScan {
    for target in targets.iter().filter(|target| target.plugin_installed) {
        // 单个目标的 secret 读取失败按「未登录」处理，不阻断其它目标。
        let Some(secret) = read_secret_for(&target.config_dir).ok().flatten() else {
            continue;
        };
        let Some((uid, token)) = parse_token_from_secret(&secret) else {
            continue;
        };
        return match matcher(uid.as_deref(), &token) {
            Some(account) => SecretScan::Matched {
                config_dir: target.name.clone(),
                account,
                uid,
            },
            None => SecretScan::Unmatched {
                config_dir: target.name.clone(),
                uid,
            },
        };
    }
    SecretScan::NotFound
}

/// 生产路径的识别：装了插件的配置目录 × 账号库。
pub(super) fn scan_secret_account() -> SecretScan {
    scan_secret_account_with(&plugin_targets(), &|uid, token| {
        match_account_for_token(uid, token)
    })
}

/// 从本机 JetBrains IDE 读取当前 token；若能匹配账号库则返回匹配信息。
pub fn detect_current_account() -> Result<Value, String> {
    Ok(match scan_secret_account() {
        SecretScan::Matched {
            config_dir,
            account,
            uid,
        } => {
            let id = get_str(&account, "id").unwrap_or_default();
            let _ = set_active_account_id(&id);
            json!({
                "ok": true,
                "found": true,
                "matched": true,
                "configDir": config_dir,
                "accountId": id,
                "account": account::account_meta(&account),
                "uid": uid,
            })
        }
        SecretScan::Unmatched { config_dir, uid } => json!({
            "ok": true,
            "found": true,
            "matched": false,
            "configDir": config_dir,
            "uid": uid,
            "message": "本机 JetBrains IDE 已登录 CodeBuddy 插件，但账号库中无匹配账号；请用该账号重新 OAuth 扫码登录后再切换。",
        }),
        SecretScan::NotFound => json!({
            "ok": true,
            "found": false,
            "message": "本机 JetBrains IDE 未找到 CodeBuddy 插件登录 secret",
        }),
    })
}
