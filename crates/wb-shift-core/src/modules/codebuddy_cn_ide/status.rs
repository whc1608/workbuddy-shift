//! 运行状态与检测：Linux 进程/桌面项探测、应用状态查询与当前账号识别。

#[allow(unused_imports)]
use super::session_link::*;
#[allow(unused_imports)]
use super::switch::*;
use super::*;

#[cfg_attr(
    not(any(test, not(any(target_os = "macos", target_os = "windows")))),
    allow(dead_code)
)]
pub(super) fn linux_cmdline_is_codebuddy_cn(cmdline: &str) -> bool {
    let lower = cmdline.to_ascii_lowercase();
    // 自排除：命令行含本工具产物名（`workbuddy-shift` 子串已涵盖
    // `workbuddy-shift-desktop`）即不是 CodeBuddy CN 进程。
    if lower.contains("workbuddy-shift") {
        return false;
    }
    if lower.contains("crashpad") || lower.contains("--type=") {
        return false;
    }
    cmdline.contains("CodeBuddy CN")
        || lower.contains("codebuddy-cn")
        || lower.contains("codebuddycn")
        // deb 包把二进制装成 buddycn（/usr/share/buddycn/buddycn），
        // 只认 codebuddy-* 会漏掉这种打包方式。
        || lower.contains("buddycn")
}

/// Linux 可执行文件名是否属于 CN 客户端。
///
/// 各发行包命名不统一：deb 是 `buddycn`，也有 `codebuddy-cn` / `codebuddycn`，
/// AppImage 常见 `CodeBuddy-CN-1.2.3.AppImage`。统一去掉分隔符再比前缀，
/// 这样 `CodeBuddy_CN.AppImage` 一类写法也能命中；国际版 `codebuddy` /
/// `workbuddy` 不匹配。
pub(super) fn linux_exe_name_matches_cn(name: &str) -> bool {
    let normalized: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    normalized.starts_with("codebuddycn") || normalized.starts_with("buddycn")
}

#[cfg_attr(
    not(any(test, not(any(target_os = "macos", target_os = "windows")))),
    allow(dead_code)
)]
pub(super) fn linux_exe_is_codebuddy_cn(exe: &Path) -> bool {
    exe.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.trim())
        .map(linux_exe_name_matches_cn)
        .unwrap_or(false)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn linux_codebuddy_cn_pids() -> Vec<u32> {
    let self_pid = std::process::id();
    let mut pids = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return pids;
    };
    for entry in entries.flatten() {
        let pid: u32 = match entry.file_name().to_string_lossy().parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if pid == self_pid {
            continue;
        }
        if let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) {
            if linux_exe_is_codebuddy_cn(&exe) {
                pids.push(pid);
                continue;
            }
        }
        let cmdline = match std::fs::read(format!("/proc/{pid}/cmdline")) {
            Ok(bytes) if !bytes.is_empty() => String::from_utf8_lossy(&bytes).replace('\0', " "),
            _ => continue,
        };
        if linux_cmdline_is_codebuddy_cn(&cmdline) {
            pids.push(pid);
        }
    }
    pids
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn wait_linux_pids_gone(pids: &[u32], timeout: Duration) -> Vec<u32> {
    if pids.is_empty() {
        return Vec::new();
    }
    let deadline = Instant::now() + timeout;
    loop {
        let alive: Vec<u32> = pids
            .iter()
            .copied()
            .filter(|pid| Path::new(&format!("/proc/{pid}")).exists())
            .collect();
        if alive.is_empty() || Instant::now() >= deadline {
            return alive;
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn kill_linux_pids(pids: &[u32], signal: &str) {
    if pids.is_empty() {
        return;
    }
    let owned: Vec<String> = std::iter::once(signal.to_string())
        .chain(pids.iter().map(|pid| pid.to_string()))
        .collect();
    let args: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
    let _ = run_cmd("kill", &args, 10);
}

/// Linux 可执行文件名候选（按顺序在 PATH 里查找）。
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) const LINUX_CN_EXE_NAMES: &[&str] = &["buddycn", "codebuddy-cn", "codebuddycn"];

/// Linux: 按 XDG 规则列出 applications 目录（用户级在前，用户条目可覆盖系统条目）。
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn linux_application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(home_dir().join(".local/share")));
    if let Some(data_home) = data_home {
        dirs.push(data_home.join("applications"));
        // flatpak 把导出的条目放在这里，不在 XDG_DATA_DIRS 里。
        dirs.push(data_home.join("flatpak/exports/share/applications"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for base in data_dirs.split(':').filter(|s| !s.is_empty()) {
        dirs.push(Path::new(base).join("applications"));
    }
    // snap 导出的 .desktop 固定在这里，同样不在 XDG_DATA_DIRS 里。
    dirs.push(PathBuf::from("/var/lib/snapd/desktop/applications"));
    dirs
}

/// 按 shell 规则切分 .desktop 的 `Exec=`（双引号 / 反斜杠转义）。
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn linux_split_exec(exec: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else if c == '\\' && q == '"' {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                } else {
                    current.push(c);
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
                c if c.is_whitespace() => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                c => current.push(c),
            },
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// 取 `Exec=` 里的启动程序：跳过 `env VAR=x` 前缀与 `%F`/`%U` 一类字段码。
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn linux_exec_program(exec: &str) -> Option<String> {
    let mut tokens = linux_split_exec(exec);
    while let Some(first) = tokens.first() {
        if first == "env" || first.contains('=') {
            tokens.remove(0);
        } else {
            break;
        }
    }
    tokens.into_iter().next()
}

/// 从单个 .desktop 条目解析 CN 的启动命令；不是 CN、是 URL handler 或程序不存在时返回 None。
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn linux_cn_exe_from_desktop_entry(path: &Path) -> Option<PathBuf> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let text = std::fs::read_to_string(path).ok()?;
    let mut name: Option<String> = None;
    let mut exec: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Name=") {
            name.get_or_insert_with(|| value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Exec=") {
            exec.get_or_insert_with(|| value.trim().to_string());
        }
    }
    // 非本地化 `Name=` 才算数：本地化条目与主条目指向同一个程序。
    let name = name?;
    let is_cn_name = name.eq_ignore_ascii_case("CodeBuddy CN")
        || stem.starts_with("buddycn")
        || stem.starts_with("codebuddy-cn")
        || stem.starts_with("codebuddycn");
    if !is_cn_name {
        return None;
    }
    let exec = exec?;
    // URL handler 条目同样是 CN 的，但需要带 URL 参数才能启动主程序。
    if stem.contains("url-handler") || exec.contains("--open-url") {
        return None;
    }
    let program = linux_exec_program(&exec)?;
    let program_path = if program.contains('/') {
        PathBuf::from(program)
    } else {
        linux_which(&program)?
    };
    program_path.is_file().then_some(program_path)
}

/// 在 PATH 里查找可执行文件（不依赖 which 命令）。
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn linux_which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Linux 上 CN 可执行文件的候选路径，按可信度排序。
///
/// 第一优先级是 XDG 桌面条目：它与打包方式无关（deb / snap / flatpak / 做过桌面集成的
/// AppImage 都会导出 .desktop），且直接给出真正的启动命令——例如 deb 包
/// `buddycn.desktop` 的 `Exec=/usr/share/buddycn/bin/buddycn %F`，而旧实现只找
/// `/usr/bin/codebuddy-cn`，在真实发行包上必然落空。
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn linux_cn_exe_candidates() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    let mut desktop_entries: Vec<PathBuf> = Vec::new();
    for dir in linux_application_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            // 先按文件名粗筛，避免逐个读入上百个无关条目。
            let stem = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if stem.contains("buddy") {
                desktop_entries.push(path);
            }
        }
    }
    desktop_entries.sort();
    for entry in desktop_entries {
        if let Some(exe) = linux_cn_exe_from_desktop_entry(&entry) {
            candidates.push(exe);
        }
    }

    for name in LINUX_CN_EXE_NAMES {
        if let Some(path) = linux_which(name) {
            candidates.push(path);
        }
    }

    candidates.extend(
        [
            "/usr/bin/buddycn",
            "/usr/share/buddycn/bin/buddycn",
            "/usr/share/buddycn/buddycn",
            "/usr/local/bin/buddycn",
            "/usr/bin/codebuddy-cn",
            "/usr/local/bin/codebuddy-cn",
            "/opt/codebuddy-cn/codebuddy-cn",
            "/opt/buddycn/buddycn",
        ]
        .iter()
        .map(PathBuf::from),
    );

    let mut deduped: Vec<PathBuf> = Vec::new();
    for candidate in candidates {
        if !deduped.contains(&candidate) {
            deduped.push(candidate);
        }
    }
    deduped
}

/// 解析 CodeBuddy CN 应用路径（macOS: .app bundle；Windows: exe）。
pub fn codebuddy_cn_app_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        macos_cn_app_path_resolved()
    }
    #[cfg(target_os = "windows")]
    {
        windows_cn_exe_path_resolved()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Some(cached) = load_codebuddy_cn_app_cache() {
            if cached.is_file() && linux_exe_is_codebuddy_cn(&cached) {
                return Some(cached);
            }
            clear_codebuddy_cn_app_cache();
        }
        for path in linux_cn_exe_candidates() {
            if path.is_file() && linux_exe_is_codebuddy_cn(&path) {
                persist_cn_app_cache(&path);
                return Some(path);
            }
        }
        None
    }
}

/// CodeBuddy CN 是否在运行（footer 语义 = GUI 主进程）。
pub fn is_codebuddy_cn_running() -> bool {
    #[cfg(target_os = "macos")]
    {
        let resolved = macos_cn_app_path_resolved();
        let patterns = macos_cn_main_patterns(resolved.as_deref());
        !process::macos_pids_by_patterns(&patterns).is_empty()
    }
    #[cfg(target_os = "windows")]
    {
        !windows_cn_process_rows().is_empty()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        !linux_codebuddy_cn_pids().is_empty()
    }
}

/// 状态：是否安装、是否运行、当前账号（仅来自本地状态文件 + 账号库，不读取钥匙串）。
pub fn status() -> Value {
    let data_dir = codebuddy_cn_data_dir();
    let db_path = codebuddy_cn_state_db_path();
    // `installed` 只表示"存在可运行的客户端"。数据目录存在不能算已安装——
    // 只读探测不再建目录（issue #91），残留空目录也不应误报「已接入」；
    // 是否有残留数据由 dataDir / dbExists 表达。
    let installed = codebuddy_cn_app_path().is_some();
    let db_exists = db_path.as_ref().map(|p| p.exists()).unwrap_or(false);
    let running = is_codebuddy_cn_running();
    // 登录态 = `state.vscdb` 里是否存在会话 secret 行：只查 key、不解密（macOS 解密会弹
    // 钥匙串授权，绝不能进这条轮询路径）。查询失败/文件不存在一律 false，仅用于文案与入口判定。
    let logged_in = has_secret_row_for(&CODEBUDDY_CN_TARGET, data_dir.as_deref()).unwrap_or(false);

    let mut active_account_id = active_account_id_from_state();
    let mut active_account_name: Option<String> = None;

    if let Some(id) = active_account_id.clone() {
        if let Some(acc) = account::find_account(&id) {
            active_account_name = Some(account::account_display_name(&acc));
        } else {
            // 状态文件有记录但账号库已无此账号：视为未检测到，不回退读取钥匙串
            active_account_id = None;
        }
    }

    json!({
        "installed": installed,
        "running": running,
        "loggedIn": logged_in,
        "dataDir": data_dir.map(|p| p.to_string_lossy().to_string()),
        "dbPath": db_path.map(|p| p.to_string_lossy().to_string()),
        "dbExists": db_exists,
        "appPath": codebuddy_cn_app_path().map(|p| p.to_string_lossy().to_string()),
        "activeAccountId": active_account_id,
        "activeAccountName": active_account_name,
        "detectedFrom": "state",
        "statePath": state_path().to_string_lossy(),
    })
}

/// 当前 CodeBuddy CN IDE 登录账号的 uid（用于定位可复制的会话目录）。
///
/// 优先从本机 IDE 登录 secret 解析 uid（与切号注入的是同一处真相）；不可用时回退到
/// 本地状态文件记录的账号 id → 账号库 uid。任一来源都拿不到时返回 `None`（调用方据此给出空态）。
pub fn active_cn_ide_uid() -> Option<String> {
    if let Ok(Some(secret)) = read_codebuddy_cn_secret(None) {
        if let Some((Some(uid), _token)) = parse_token_from_secret(&secret) {
            let uid = uid.trim().to_string();
            if !uid.is_empty() {
                return Some(uid);
            }
        }
    }
    active_account_id_from_state()
        .and_then(|id| account::find_account(&id))
        .and_then(|acc| get_str(&acc, "uid"))
}

/// 从本机 CN IDE 读取当前 token；若能匹配账号库则返回匹配信息（不新建账号）。
pub fn detect_current_account() -> Result<Value, String> {
    let secret = read_codebuddy_cn_secret(None)?;
    let Some(secret) = secret else {
        return Ok(json!({
            "ok": true,
            "found": false,
            "message": "本机 CodeBuddy CN 未找到登录 secret",
        }));
    };
    let Some((uid, token)) = parse_token_from_secret(&secret) else {
        return Err("本地 CodeBuddy CN 登录信息解析失败".to_string());
    };
    if let Some(acc) = match_account_for_token(uid.as_deref(), &token) {
        let id = get_str(&acc, "id").unwrap_or_default();
        let _ = set_active_account_id(&id);
        return Ok(json!({
            "ok": true,
            "found": true,
            "matched": true,
            "accountId": id,
            "account": account::account_meta(&acc),
            "uid": uid,
        }));
    }
    Ok(json!({
        "ok": true,
        "found": true,
        "matched": false,
        "uid": uid,
        "message": "本机已登录 CodeBuddy CN，但账号库中无匹配账号；请用该账号重新 OAuth 扫码登录后再切换。",
    }))
}
