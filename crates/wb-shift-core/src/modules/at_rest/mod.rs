//! R6-06：**按需解密本机 live 凭据**（只读查询与写操作各自的开关控制）。
//!
//! # 它解决什么
//!
//! WorkBuddy 5.6 起客户端把认证文件里的 token 存成 `{$wbEncrypted, envelope}` 加密信封。
//! 账号库里因此也可能是信封，导致积分 / 签到 / 旅行不可用。这个模块用客户端自带的
//! 原生绑定取到字段加密密钥，**在内存里**解出 access token，只用于本次请求。
//!
//! # 安全红线（实现即约束，破坏其中任何一条都是评审拒绝项）
//!
//! 1. **不落盘、不回填账号库**：不写 `accounts.json`、不写客户端认证文件、不 adopt、
//!    不改 `credentialSource` / `needs_relogin`；导出备份里的信封仍是信封。
//! 2. **只解 access token**：refresh token 从不解密（见 [`live`]），从设计上排除
//!    「用解密出的 RT 刷新、把客户端的令牌轮换掉」。
//! 3. **零化**：key 与明文 token 都用 [`zeroize::Zeroizing`] 持有；不进 `Debug` / 日志 /
//!    诊断 / 错误字符串。
//! 4. **解密前必须 `same_account`**（uid + 档位），绝不把 B 的 live 凭据用到 A 的记录上。
//! 5. **临时副本带 [`AT_REST_FIELD`] 标记**，任何序列化路径都要过滤它；副本不携带
//!    可刷新的 refresh token（字段被移除），即使守卫被绕过也刷不动。
//! 6. 解密能力**不经 HTTP / Tauri 接口暴露**：宿主只有「读写开关」两个命令。
//!
//! # 凭据使用优先级（每个调用点按同一顺序）
//!
//! 1. 账号有**未过期明文** access → 用存储明文，行为与改造前完全一致，不碰解密；
//! 2. 明文已过期但自带可刷新 refresh token → 走既有刷新；**刷新没救回来**才尝试解密；
//! 3. 账号是**信封** → 读 live、`same_account` 命中且 access 未过期 → 解密出临时副本；
//! 4. live access 已过期 → 不刷新，回退「需重新登录 / 客户端凭据已更新」；
//! 5. live 不是同一 uid / 读不出 / 解密失败 → 回退现有行为，**不改账号**。
//!
//! # 平台与开关闸门
//!
//! - 只读解密（积分 / 官方用量）默认开启，写操作解密（签到 / 旅行 / 每日活跃 /
//!   自动轮换）默认关闭，必须用户单独打开；只读关闭时写开关一并失效（读取时归一，
//!   见 `config::configs::credential`）；
//! - Windows 本机已实测通过，直接启用；macOS / Linux **探测通过才启用**
//!   （[`probe_platform`]），失败记 `platformProbe=failed` + 版本，不反复重试；
//!   客户端版本变化后自动重新探测。

mod envelope;
mod key;
mod live;
#[cfg(test)]
mod tests;

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::modules::account;
use crate::modules::auth_file;
use crate::modules::config::{self, now_ms};
use crate::modules::locks;
use crate::modules::process;
use crate::modules::variant::WbVariant;

/// 解密通道的用途：只读（积分 / 官方用量）与写（签到 / 旅行 / 每日活跃 / 自动轮换）。
///
/// 两个开关是**单向包含**关系：只读关闭时写一并失效（见
/// `config::decrypt_write_enabled`），因此这里只表达用途，不做组合判断。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AccessMode {
    /// 只读查询：`credits::get_credit_expiry`、`official_usage`。
    ReadOnly,
    /// 写操作：签到 / 旅行 / 每日活跃。
    Write,
}

/// 临时账号副本的内存标记字段名（**禁止外泄**：不进 DTO / 诊断 / 日志 / 备份）。
pub const AT_REST_FIELD: &str = "__at_rest";

/// 平台探测结果。
///
/// `status` 只有 `"ok"` / `"failed"` 两个取值，直接写进 `credential.json` 的
/// `platformProbe`（与跨轨道契约一致）。
#[derive(Clone, PartialEq, Eq)]
pub struct PlatformProbe {
    status: &'static str,
    version: Option<String>,
    reason: Option<&'static str>,
}

impl PlatformProbe {
    /// 探测是否通过（= 该平台可以启用解密通道）。
    pub fn passed(&self) -> bool {
        self.status == "ok"
    }

    /// `"ok"` / `"failed"`，与 `credential.json` 的 `platformProbe` 同源。
    pub fn status(&self) -> &'static str {
        self.status
    }

    /// 探测到的客户端版本（取不到为 `None`）。
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// 失败类别（只用于事件日志与设置页，**不含任何明文**）。
    pub fn reason(&self) -> Option<&'static str> {
        self.reason.as_ref().copied()
    }
}

impl std::fmt::Debug for PlatformProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformProbe")
            .field("status", &self.status)
            .field("version", &self.version)
            .field("reason", &self.reason)
            .finish()
    }
}

/// 最近一次解密失败的类别（供事件日志 / 设置页用；**不含明文**）。
///
/// 单值「最后一次结果」缓存：中毒（持锁线程 panic）不需要报错——它只用于展示与事件
/// 归因，下一次解密会整体覆盖，因此用 `locks::recover` 取回继续。
static LAST_ERROR: std::sync::Mutex<Option<&'static str>> = std::sync::Mutex::new(None);

/// 解密通道失败 / 被跳过的类别。
///
/// 与跨轨道契约的 reason 枚举对齐；`Expired` 是「正常等待客户端续期」，不是错误，
/// 只用于 [`last_error_kind`] 的展示，**不写事件日志**。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AtRestError {
    /// 取不到密钥 / 没有 live 文件 / 不是同一账号（`decrypt_binding_missing`）。
    BindingMissing,
    /// 信封 keyId 与派生 key 不一致（换机 / 客户端重建密钥，`decrypt_key_mismatch`）。
    KeyMismatch,
    /// GCM 校验失败（`decrypt_gcm_failed`）。
    GcmFailed,
    /// 子进程超时（`decrypt_timeout`）。
    Timeout,
    /// 信封形态不认识（base64 / 缺字段 / suite≠1）：归入 `decrypt_gcm_failed` 一档，
    /// 因为对用户而言都是「这份信封解不开」，且契约的 reason 枚举没有单独的「格式错」。
    Malformed,
    /// live access 已过期：不刷新，等客户端续期。
    Expired,
}

impl AtRestError {
    /// 事件日志的 `reason` 取值。
    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::BindingMissing => "decrypt_binding_missing",
            Self::KeyMismatch => "decrypt_key_mismatch",
            Self::GcmFailed | Self::Malformed => "decrypt_gcm_failed",
            Self::Timeout => "decrypt_timeout",
            Self::Expired => "decrypt_expired",
        }
    }

    /// 事件日志的 `action`：过期不是错误，返回 `None`（不写事件）。
    fn action(self) -> Option<&'static str> {
        match self {
            Self::Expired => None,
            _ => Some("decrypt_failed"),
        }
    }

    fn kind(self) -> &'static str {
        self.reason()
    }
}

/// 闸门求值结果。
///
/// 单独成枚举而不是 `bool`：调用方需要区分「用户关了开关」（记 `skipped_encrypted`）与
/// 「这个平台探测没过」（不反复探测），两者对事件日志与设置页文案的语义不同。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Gate {
    /// 允许解密。
    Open,
    /// 用户关掉了对应开关。
    SwitchOff,
    /// 该平台探测未通过（`platformProbe=failed` 且客户端版本未变）。
    PlatformDisabled,
    /// 需要跑一次真实探测。
    NeedsProbe,
    /// 本机没装客户端 / 找不到可执行文件。
    NoClient,
}

/// 可注入的上下文：把「可注入」的东西收在一处，避免给 5 个上游各加一堆参数。
///
/// 生产用 [`AtRestContext::for_variant`]；单测用 builder 注入临时 store 根、临时 live
/// 文件、固定时钟与固定密钥（[`AtRestContext::with_key`]），因此每条分支都能在
/// 「不拉起真实客户端」的前提下被覆盖。
pub struct AtRestContext {
    /// 档位（决定 live 文件、密钥命名空间与请求头）。
    pub variant: WbVariant,
    /// 客户端可执行文件 / `.app` 目录（`auth_file::workbuddy_app_path`）。
    pub exe: PathBuf,
    /// 固定时钟（生产 `config::now_ms()`；单测注入）。
    pub now_ms: i64,
    /// 工具存储根（`credential.json` 所在目录）。
    pub store_root: PathBuf,
    /// live 认证文件路径（单测注入临时文件，生产 = `variant.auth_file_path()`）。
    pub live_path: PathBuf,
    /// 注入的密钥（单测用；生产为 `None` → 走进程内 key 缓存 + 子进程）。
    ///
    /// 字段私有：`AtRestKey` 不实现 `Debug`，也不该出现在任何对外结构里。
    key: Option<Box<key::AtRestKey>>,
}

impl std::fmt::Debug for AtRestContext {
    /// 手写 `Debug`：`key` 字段可能含真实密钥材料，永远不打印。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AtRestContext")
            .field("variant", &self.variant)
            .field("exe", &self.exe)
            .field("now_ms", &self.now_ms)
            .field("store_root", &self.store_root)
            .field("live_path", &self.live_path)
            .finish_non_exhaustive()
    }
}

impl AtRestContext {
    /// 生产构造：真实 exe / 真实 store 根 / 真实 live 文件 / `now_ms()`。
    pub fn for_variant(variant: WbVariant) -> Self {
        Self {
            variant,
            exe: auth_file::workbuddy_app_path(variant),
            now_ms: now_ms(),
            store_root: config::store_dir(),
            live_path: variant.auth_file_path(),
            key: None,
        }
    }

    /// 覆盖时钟（单测）。
    pub fn with_now(mut self, now_ms: i64) -> Self {
        self.now_ms = now_ms;
        self
    }

    /// 覆盖 live 认证文件路径（单测）。
    pub fn with_live_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.live_path = path.into();
        self
    }

    /// 覆盖工具存储根（单测：隔离 `credential.json`）。
    pub fn with_store_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.store_root = root.into();
        self
    }

    /// 覆盖客户端路径（单测：指向不存在的路径以断言「不 spawn」）。
    pub fn with_exe(mut self, exe: impl Into<PathBuf>) -> Self {
        self.exe = exe.into();
        self
    }

    /// 注入一把固定密钥（单测）。
    #[cfg(test)]
    pub(in crate::modules::at_rest) fn with_key(mut self, key: key::AtRestKey) -> Self {
        self.key = Some(Box::new(key));
        self
    }

    /// 注入的密钥（生产为 `None`）。
    fn injected_key(&self) -> Option<&key::AtRestKey> {
        self.key.as_deref()
    }

    /// 本次上下文使用的 `credential.json` 路径（跟着 `store_root`）。
    fn credential_file(&self) -> PathBuf {
        self.store_root.join("credential.json")
    }

    /// 客户端版本（`process::client_version` 是**全仓唯一实现**，诊断包共用）。
    fn client_version(&self) -> Option<String> {
        process::client_version(&self.exe)
    }
}

/// 解密通道是否可用（只读 / 写各自的开关 + 平台闸门）。
///
/// 副作用仅限「闸门需要探测时写一次 `credential.json`」；开关状态每次实时读文件，
/// 因此用户关掉开关后**下一个请求**立刻生效。
pub fn available(variant: WbVariant, mode: AccessMode) -> bool {
    gate(variant, mode) == Gate::Open
}

/// 求值闸门（不做任何 spawn；`NeedsProbe` 才是「需要探测」）。
pub fn gate(variant: WbVariant, mode: AccessMode) -> Gate {
    let ctx = AtRestContext::for_variant(variant);
    gate_for(&ctx, mode)
}

fn gate_for(ctx: &AtRestContext, mode: AccessMode) -> Gate {
    let credential = config::load_credential_config_at(&ctx.credential_file());
    let version = ctx.client_version();
    match gate_with(&credential, mode, version.as_deref()) {
        // 「需要探测」但本机根本没有客户端：直接判不可用，别浪费一次文件系统探测。
        Gate::NeedsProbe if key::resolve_executable(&ctx.exe).is_none() => Gate::NoClient,
        other => other,
    }
}

/// 闸门的**纯函数**部分（开关 → 平台记录值 → 版本），便于表驱动单测。
///
/// 顺序很关键：开关先判、平台后判 —— 用户关掉只读后，不应该再去探测平台
/// （探测会拉起客户端二进制，正是「关闭开关后不再拉起客户端」的验收要求）。
pub(crate) fn gate_with(credential: &Value, mode: AccessMode, version: Option<&str>) -> Gate {
    if !config::decrypt_read_enabled(credential) {
        return Gate::SwitchOff;
    }
    if mode == AccessMode::Write && !config::decrypt_write_enabled(credential) {
        return Gate::SwitchOff;
    }
    let (probe, probed_version) = config::platform_probe(credential);
    match probe {
        config::ProbeState::Ok => Gate::Open,
        // Windows 已实测通过（见 wb-shift-envelope-decrypt-feasibility.md），不需要探测。
        config::ProbeState::Unknown if cfg!(target_os = "windows") => Gate::Open,
        config::ProbeState::Unknown => Gate::NeedsProbe,
        // 「失败即记住」：版本没变就不再探测（否则每次查积分都要白等一次 2s 超时）。
        config::ProbeState::Failed => {
            if probed_version.as_deref() == version {
                Gate::PlatformDisabled
            } else {
                Gate::NeedsProbe
            }
        }
    }
}

/// 平台探测：取密钥 + 校验 keyId + 试解一个字段，结果写回 `credential.json`。
///
/// Windows 已实测通过 → 直接 ok（不拉起子进程、不改配置）。macOS / Linux 才真探测。
pub fn probe_platform(variant: WbVariant) -> PlatformProbe {
    let ctx = AtRestContext::for_variant(variant);
    probe_platform_in(&ctx)
}

/// 可注入上下文的探测实现（单测用临时 live 文件 + 注入密钥）。
pub(crate) fn probe_platform_in(ctx: &AtRestContext) -> PlatformProbe {
    let version = ctx.client_version();
    if cfg!(target_os = "windows") {
        return PlatformProbe {
            status: "ok",
            version,
            reason: None,
        };
    }
    let probe = {
        // 探测 = 完整重新识别一次，所以先把进程内 key 缓存清掉（客户端可能刚升级 /
        // 换了可执行文件路径）。
        key::clear_cache();
        run_probe(ctx, version)
    };
    persist_probe(ctx, &probe);
    probe
}

fn run_probe(ctx: &AtRestContext, version: Option<String>) -> PlatformProbe {
    let failed = |reason: &'static str| PlatformProbe {
        status: "failed",
        version: version.clone(),
        reason: Some(reason),
    };
    let ok = || PlatformProbe {
        status: "ok",
        version: version.clone(),
        reason: None,
    };
    if key::resolve_executable(&ctx.exe).is_none() {
        return failed(AtRestError::BindingMissing.reason());
    }
    let key = match key::key_for(ctx) {
        Ok(key) => key,
        Err(error) => return failed(error.reason()),
    };
    // 没有 live 文件 / 没有信封时无法证伪：绑定能取到密钥就算通过，等真正解密时再判。
    let Some(root) = auth_file::read_auth_file_at(&ctx.live_path) else {
        return ok();
    };
    let Some(secret) = root
        .get("auth")
        .and_then(|auth| account::secret_value(auth, "accessToken"))
    else {
        return ok();
    };
    // 明文：不需要解密能力，按通过处理。
    if !matches!(secret, Value::Object(_)) {
        return ok();
    }
    let parsed = match envelope::parse(&secret) {
        Ok(parsed) => parsed,
        Err(error) => return failed(error.reason()),
    };
    if parsed.key_id() != key.key_id() {
        return failed(AtRestError::KeyMismatch.reason());
    }
    match envelope::open(&parsed, key.key_bytes(), key.key_id()) {
        Ok(plain) if !plain.trim().is_empty() => ok(),
        Ok(_) => failed(AtRestError::GcmFailed.reason()),
        Err(error) => failed(error.reason()),
    }
}

fn persist_probe(ctx: &AtRestContext, probe: &PlatformProbe) {
    let mut credential = config::load_credential_config_at(&ctx.credential_file());
    config::set_platform_probe(&mut credential, probe_state(probe), probe.version());
    let _ = config::save_credential_config_at(&ctx.credential_file(), &credential);
}

fn probe_state(probe: &PlatformProbe) -> config::ProbeState {
    if probe.passed() {
        config::ProbeState::Ok
    } else {
        config::ProbeState::Failed
    }
}

/// R6-06 的**唯一入口**：账号是信封且本机 live 凭据可用时，返回本次请求专用的临时副本。
///
/// 返回 `None` 表示「维持原有路径」：明文未过期、开关关闭、平台未启用、解密失败、
/// live 不是同一账号 —— 调用方**不得**为这些情况新增分支。
///
/// 返回的副本：`access_token` 是明文；带 `"__at_rest": true`；**不含 `refresh_token`**。
pub fn usable_account(account: &Value, mode: AccessMode) -> Option<Value> {
    usable_account_with(
        account,
        mode,
        &AtRestContext::for_variant(account::variant_of(account)),
    )
}

/// 可注入上下文版本（单测用）。
pub fn usable_account_in(account: &Value, mode: AccessMode, ctx: &AtRestContext) -> Option<Value> {
    usable_account_with(account, mode, ctx)
}

fn usable_account_with(account: &Value, mode: AccessMode, ctx: &AtRestContext) -> Option<Value> {
    // 前置条件（任一不满足立即 `None`，不产生任何 IO）：不是信封 / 已经是临时副本。
    if is_temp(account) || !account::is_envelope(account, "access_token") {
        return None;
    }
    resolve_temp(account, mode, ctx, true, true, &probe_platform_passed)
}

/// 探测回调：真实探测一次并持久化结论。
fn probe_platform_passed(ctx: &AtRestContext) -> bool {
    probe_platform_in(ctx).passed()
}

/// 401 之后**重新读一次 live 文件**（最多 1 次），绝不调刷新接口。
///
/// `temp` 是上一次 [`usable_account`] 的返回值（带 [`AT_REST_FIELD`]）；返回 `None`
/// 表示本次请求到此为止 —— 调用方必须把上游 401 原样上抛，不得再重试。
///
/// 「最多 1 次」由结构保证：本函数没有循环、也没有调用自身；调用点只有一处。
pub fn reread_after_unauthorized(temp: &Value) -> Option<Value> {
    if !is_temp(temp) {
        return None;
    }
    let ctx = AtRestContext::for_variant(account::variant_of(temp));
    reread_after_unauthorized_in(temp, &ctx)
}

/// 可注入上下文版本（单测用：临时 live 文件 + 注入密钥；与 [`usable_account_in`] 同构）。
pub fn reread_after_unauthorized_in(temp: &Value, ctx: &AtRestContext) -> Option<Value> {
    if !is_temp(temp) {
        return None;
    }
    // 不要求「仍是信封」（temp 的 access_token 已是明文），但开关必须仍然开着。
    // 只读开关是写开关的前提（单向包含），因此用它一次判定即可覆盖两种用途。
    resolve_temp(
        temp,
        AccessMode::ReadOnly,
        ctx,
        false,
        false,
        &probe_platform_passed,
    )
}

/// 单次解析：闸门 → 取密钥 → 读 live → `same_account` → 只解 accessToken → 临时副本。
///
/// `require_envelope`：`true` = 首次解析（信封才有意义）；`false` = 401 后重读
/// （调用方手里已经是临时副本，明文 access 在 live 里也允许直接用）。
fn resolve_temp(
    account: &Value,
    mode: AccessMode,
    ctx: &AtRestContext,
    allow_cache: bool,
    require_envelope: bool,
    platform_ok: &dyn Fn(&AtRestContext) -> bool,
) -> Option<Value> {
    if account::variant_of(account) != ctx.variant {
        return None;
    }
    if require_envelope && !account::is_envelope(account, "access_token") {
        return None;
    }
    let credential = config::load_credential_config_at(&ctx.credential_file());
    let version = ctx.client_version();
    match gate_with(&credential, mode, version.as_deref()) {
        Gate::Open => {}
        Gate::SwitchOff => {
            record(account, "skipped_encrypted", "switch_disabled");
            return None;
        }
        Gate::PlatformDisabled => {
            record(account, "skipped_encrypted", "platform_disabled");
            return None;
        }
        Gate::NoClient => return None,
        Gate::NeedsProbe => {
            // 本机没有客户端：探测无从谈起，直接回退（不 spawn、不写配置）。
            let _ = key::resolve_executable(&ctx.exe)?;
            if !platform_ok(ctx) {
                record(account, "skipped_encrypted", "platform_disabled");
                return None;
            }
        }
    }
    let key = match key::key_for(ctx) {
        Ok(key) => key,
        Err(error) => {
            set_last_error(error);
            live::invalidate_cache();
            record_error(account, error);
            return None;
        }
    };
    match live::read_live_access_at(
        &ctx.live_path,
        account,
        ctx.variant,
        &key,
        ctx.now_ms,
        allow_cache,
    ) {
        Ok(access) => {
            if !require_envelope
                && account.get("access_token").and_then(Value::as_str)
                    == Some(access.token.as_str())
            {
                // 令牌没变（客户端还没续期）：重试同一个令牌只会再拿一次 401。
                return None;
            }
            set_last_error_ok();
            record(account, "decrypt_ok", "ok");
            Some(temp_copy(account, &access))
        }
        Err(error) => {
            // 任何解密失败都让缓存立即失效：下一次请求重新读文件、重新解密。
            set_last_error(error);
            live::invalidate_cache();
            if error == AtRestError::KeyMismatch {
                // keyId 不匹配 = 客户端换了密钥（升级 / 重建）：清缓存让下次重新取。
                key::clear_cache();
            }
            record_error(account, error);
            None
        }
    }
}

/// 组装临时副本：只替换 `access_token` / `expiresAt`，并移除一切可刷新字段。
///
/// 移除 `refresh_token` 是**防御纵深**：即使将来某个调用点漏了 [`is_temp`] 守卫，
/// 也拿不到可用于刷新的字符串令牌（信封对象不是字符串，`get_str` 取不到）。
fn temp_copy(account: &Value, access: &live::LiveAccess) -> Value {
    let mut copy = account.clone();
    if let Some(object) = copy.as_object_mut() {
        object.insert(
            "access_token".to_string(),
            Value::String(access.token.to_string()),
        );
        object.insert("expiresAt".to_string(), json!(access.expires_at));
        object.remove("refresh_token");
        object.remove("refreshExpiresAt");
        object.insert(AT_REST_FIELD.to_string(), Value::Bool(true));
    }
    copy
}

/// 本次账号是否为 R6-06 的临时副本（**任何持久化 / 刷新 / 序列化路径都必须先问这一句**）。
pub fn is_temp(account: &Value) -> bool {
    account.get(AT_REST_FIELD).and_then(Value::as_bool) == Some(true)
}

/// 该账号当前是否带**未过期的明文** access token。
///
/// 缺 `expiresAt` 时视为可用：与「优先级 1 命中就不解密」同一口径（不因缺字段丢明文）。
pub fn has_unexpired_plain_access(account: &Value) -> bool {
    let has_plain = account
        .get("access_token")
        .and_then(Value::as_str)
        .is_some_and(|token| !token.trim().is_empty());
    if !has_plain {
        return false;
    }
    match account.get("expiresAt").and_then(Value::as_i64) {
        Some(expires_at) => expires_at > now_ms(),
        None => true,
    }
}

/// 解析可用账号；只有解密也救不回来时才返回信封错误文案。
///
/// **调用点只能用它，不许自己先 `envelope_token_error(..)` 再另行解析** —— 顺序写反会让
/// 整个 R6-06 静默失效（功能「看起来上线了」但永远走不到解密分支）。
///
/// 返回 `Ok(None)` = 用调用方手上的账号原样（明文 / 回退）；`Ok(Some(temp))` = 用临时副本；
/// `Err(message)` = 面向用户的信封提示。
pub fn resolve_or_envelope_error(
    account: &Value,
    mode: AccessMode,
) -> Result<Option<Value>, String> {
    let temp = usable_account(account, mode);
    let usable = temp.as_ref().unwrap_or(account);
    match account::envelope_token_error(usable) {
        Some(message) => Err(message),
        None => Ok(temp),
    }
}

/// 最近一次解密失败类别（`None` = 最近一次成功或未尝试）。
pub fn last_error_kind() -> Option<&'static str> {
    *locks::recover(&LAST_ERROR)
}

fn set_last_error(error: AtRestError) {
    *locks::recover(&LAST_ERROR) = Some(error.kind());
}

fn set_last_error_ok() {
    *locks::recover(&LAST_ERROR) = None;
}

fn record_error(account: &Value, error: AtRestError) {
    if let Some(action) = error.action() {
        record(account, action, error.reason());
    }
}

/// 事件日志的薄封装（跨轨道契约，`credential_events::record_for_account`）。
///
/// 事件只写账号序号 / 档位 / action / reason 四个字段，**不含 token、昵称、路径**；
/// 写入失败在 `credential_events` 内部只打日志，绝不影响解密主流程（因此这里没有
/// `Result`，也不需要调用方处理错误）。
fn record(account: &Value, action: &str, reason: &str) {
    crate::modules::credential_events::record_for_account(account, action, reason);
}
