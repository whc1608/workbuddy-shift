//! 取字段加密密钥：Node 模式拉起客户端 → `loggerGet()` → `SHA-256` 派生。
//!
//! 命令（本机实测 0.127s，不需要启动客户端 UI）：
//!
//! ```text
//! ELECTRON_RUN_AS_NODE=1 <客户端可执行文件> -e \
//!   "process.stdout.write(process._linkedBinding('electron_browser_workbuddy_storage').loggerGet())"
//! ```
//!
//! 派生（与客户端 `normalizeAtRestKeyPayload` 一致）：
//!
//! ```text
//! key   = SHA-256(atRestSecretKey 的 UTF-8 字节)   // 32 字节
//! keyId = SHA-256(key) 的十六进制前 16 个字符
//! ```
//!
//! 纪律：
//! - key 只在进程内存里，用 [`zeroize::Zeroizing`] 持有，**不进** `Debug` / 日志 / 错误串；
//! - 进程内缓存（按档位 + TTL 10 分钟），**并发只拉起一个子进程**（串行化整段取密钥）；
//! - 失败不缓存：调用方下一次请求可以重试（客户端刚升级完的场景）。

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::Value;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::AtRestError;
use crate::modules::locks;
use crate::modules::process::cmd_builder;

/// 取到的密钥在进程内复用的时长。
pub(super) const KEY_TTL_MS: i64 = 10 * 60 * 1000;

/// 子进程超时：暖启动实测 0.097s，冷启动（客户端刚升级 / 杀软首扫）可达数秒。
/// 取 8s：超时只会让本次降级为「读不到凭据」，宁慢不误判。
const SPAWN_TIMEOUT: Duration = Duration::from_secs(8);

/// 首次取不到时的重试间隔：客户端单实例交接 / 刚启动时偶尔会「进程起来了但没输出」。
const SPAWN_RETRY_DELAY: Duration = Duration::from_millis(150);

/// 取密钥的 Node 片段。字符串里不含任何用户数据，也不写 stdout 之外的地方。
const LOGGER_GET_SCRIPT: &str = "process.stdout.write(process._linkedBinding('electron_browser_workbuddy_storage').loggerGet())";

/// 一把字段加密密钥（含派生出的 keyId）。
///
/// **不实现 `Debug`**：任何 `{:?}` 都不该有机会带出 key 字节。
pub(super) struct AtRestKey {
    key: Zeroizing<[u8; 32]>,
    key_id: String,
}

impl AtRestKey {
    pub(super) fn key_bytes(&self) -> &[u8; 32] {
        &self.key
    }

    pub(super) fn key_id(&self) -> &str {
        &self.key_id
    }

    /// 复制一份（缓存命中时用）。key 字节同样在 Drop 时清零。
    fn duplicate(&self) -> Self {
        Self {
            key: Zeroizing::new(*self.key),
            key_id: self.key_id.clone(),
        }
    }
}

struct CachedKey {
    key: AtRestKey,
    fetched_at: i64,
}

/// 进程内 key 缓存。键是命名空间（生产用档位名，单测注入独立命名空间）。
///
/// 用 `locks::recover`：这是**可重建缓存**（下一次请求会整体覆盖），中毒时不值得
/// 把「取密钥」整条链路报错。
static KEY_CACHE: std::sync::Mutex<Option<HashMap<String, CachedKey>>> =
    std::sync::Mutex::new(None);

/// 串行化「检查缓存 + 拉起子进程 + 写缓存」：并发请求只 spawn 一个。
///
/// 纯 `Mutex<()>` 串行化守卫，中毒取回后继续（`locks::recover`）。
static SPAWN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// 单测用的「真的拉起过几次子进程」计数。
//
// **thread-local**：测试并行跑在不同线程上，共享一个原子量会让「开关关闭时不 spawn」
// 这类断言被别的用例的合法 spawn 污染（曾因此出现过随机失败）。
#[cfg(test)]
thread_local! {
    static SPAWN_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 按上下文取密钥：注入密钥（单测）优先，否则走「缓存 → 子进程」。
///
/// 命名空间带上 exe 路径：客户端升级 / 换安装位置后不会误命中旧密钥。
pub(super) fn key_for(ctx: &super::AtRestContext) -> Result<AtRestKey, AtRestError> {
    if let Some(injected) = ctx.injected_key() {
        return Ok(injected.duplicate());
    }
    let namespace = format!("{}|{}", ctx.variant.as_str(), ctx.exe.display());
    load_key_at(&namespace, ctx.now_ms, &mut || fetch_key_from(&ctx.exe))
}

/// 可注入 `now` 与取数闭包的缓存实现（单测用来断言「只 spawn 一次」「失败不缓存」）。
pub(super) fn load_key_at(
    namespace: &str,
    now: i64,
    fetch: &mut dyn FnMut() -> Result<AtRestKey, AtRestError>,
) -> Result<AtRestKey, AtRestError> {
    let _serial = locks::recover(&SPAWN_LOCK);
    if let Some(cached) = cache_get(namespace, now) {
        return Ok(cached);
    }
    match fetch() {
        Ok(key) => {
            cache_put(namespace, key, now);
            cache_get(namespace, now).ok_or(AtRestError::BindingMissing)
        }
        Err(error) => {
            // 失败不缓存：客户端升级 / 绑定改名后，下一次请求要能重新探测。
            cache_drop(namespace);
            Err(error)
        }
    }
}

fn cache_get(namespace: &str, now: i64) -> Option<AtRestKey> {
    let mut guard = locks::recover(&KEY_CACHE);
    let map = guard.as_mut()?;
    let entry = map.get(namespace)?;
    if now.saturating_sub(entry.fetched_at) >= KEY_TTL_MS {
        map.remove(namespace);
        return None;
    }
    Some(entry.key.duplicate())
}

fn cache_put(namespace: &str, key: AtRestKey, now: i64) {
    let mut guard = locks::recover(&KEY_CACHE);
    let map = guard.get_or_insert_with(HashMap::new);
    map.insert(
        namespace.to_string(),
        CachedKey {
            key,
            fetched_at: now,
        },
    );
}

fn cache_drop(namespace: &str) {
    let mut guard = locks::recover(&KEY_CACHE);
    if let Some(map) = guard.as_mut() {
        map.remove(namespace);
    }
}

/// 清空进程内 key 缓存（客户端路径 / 版本变化时由平台闸门调用）。
pub(super) fn clear_cache() {
    let mut guard = locks::recover(&KEY_CACHE);
    if let Some(map) = guard.as_mut() {
        map.clear();
    }
}

/// 真正取密钥：定位可执行文件 → 子进程取 `loggerGet()` 载荷 → 派生。
///
/// 首次失败重试一次：客户端刚被拉起（或正在被单实例交接）时，子进程会正常退出却
/// 什么都不打印，表现为「空载荷」。这种失败是瞬时的，再 spawn 一次即可；真失败
/// （exe 丢失 / 绑定不存在）会在第二次同样失败，代价只是多 150ms。
fn fetch_key_from(app_path: &Path) -> Result<AtRestKey, AtRestError> {
    let exe = resolve_executable(app_path).ok_or(AtRestError::BindingMissing)?;
    let payload = match run_logger_get(&exe) {
        Ok(payload) => payload,
        Err(_first) => {
            std::thread::sleep(SPAWN_RETRY_DELAY);
            run_logger_get(&exe)?
        }
    };
    let secret = payload
        .get("atRestSecretKey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or(AtRestError::BindingMissing)?;
    Ok(derive_key(secret))
}

/// 从 `atRestSecretKey` 派生 key / keyId。
///
/// 单独拆出来是为了让单测能在**不拉起任何子进程**的前提下验证派生结果与信封 keyId
/// 的匹配关系。
pub(super) fn derive_key(at_rest_secret_key: &str) -> AtRestKey {
    let mut key_hasher = Sha256::new();
    key_hasher.update(at_rest_secret_key.as_bytes());
    let key_bytes: [u8; 32] = key_hasher.finalize().into();
    let key = Zeroizing::new(key_bytes);
    let key_id = hex_lower(&Sha256::digest(key.as_slice()));
    AtRestKey {
        key,
        // keyId 取前 16 个十六进制字符（8 字节），与信封里的 keyId 长度一致。
        key_id: key_id.chars().take(16).collect(),
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap_or('0'));
    }
    out
}

/// 把「客户端路径」（Windows/Linux 是可执行文件，macOS 是 `.app` 目录）归一成可执行文件。
///
/// macOS 取 `Contents/MacOS/` 下的第一个普通文件：`.app` 包里的可执行文件名不固定
/// （可能带空格或大写），按目录枚举比写死名字稳。
pub(super) fn resolve_executable(app_path: &Path) -> Option<PathBuf> {
    if app_path.as_os_str().is_empty() {
        return None;
    }
    if app_path.is_file() {
        return Some(app_path.to_path_buf());
    }
    let macos_dir = app_path.join("Contents").join("MacOS");
    let entries = std::fs::read_dir(&macos_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// 拉起 `ELECTRON_RUN_AS_NODE=1 <exe> -e <script>` 并解析 stdout。
fn run_logger_get(exe: &Path) -> Result<Value, AtRestError> {
    #[cfg(test)]
    SPAWN_COUNT.with(|count| count.set(count.get() + 1));
    let mut child = cmd_builder(exe)
        .env("ELECTRON_RUN_AS_NODE", "1")
        .args(["-e", LOGGER_GET_SCRIPT])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| AtRestError::BindingMissing)?;

    let deadline = Instant::now() + SPAWN_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(AtRestError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AtRestError::BindingMissing);
            }
        }
    }

    let mut text = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        stdout
            .read_to_string(&mut text)
            .map_err(|_| AtRestError::BindingMissing)?;
    }
    parse_logger_payload(&text)
}

/// 解析 `loggerGet()` 的输出。
///
/// 两种形态都认：
/// - 直接是 JSON 对象文本（`{"version":1,"atRestSecretKey":"..."}`）；
/// - 是「JSON 字符串」（`"{\"version\":1,...}"`，Electron 侧若把结果再序列化一次）。
pub(super) fn parse_logger_payload(text: &str) -> Result<Value, AtRestError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(AtRestError::BindingMissing);
    }
    let parsed: Value = match serde_json::from_str(trimmed) {
        Ok(value) => value,
        Err(_) => {
            let inner: String =
                serde_json::from_str(trimmed).map_err(|_| AtRestError::BindingMissing)?;
            serde_json::from_str(&inner).map_err(|_| AtRestError::BindingMissing)?
        }
    };
    if parsed.is_object() {
        Ok(parsed)
    } else {
        Err(AtRestError::BindingMissing)
    }
}

#[cfg(test)]
pub(super) fn spawn_count() -> usize {
    SPAWN_COUNT.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(super) fn reset_spawn_count() {
    SPAWN_COUNT.with(|count| count.set(0));
}
