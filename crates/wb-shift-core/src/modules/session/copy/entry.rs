//! 复制的对外入口与开关参数（`plan.rs` 只保留编排与报告）。
//!
//! 这里只做三件事：解析档位 / 账号身份、组装两侧 [`CopySide`]、把 [`CopyOptions`]
//! 透传给 [`run_copy`]。真正的锁顺序、恢复与逐会话写入在 `plan.rs`。
//!
//! 开关口径：存量调用方（切号流程、关联组成员同步）保持原签名不变，一律按默认值
//! （全 `false`）执行；带开关的调用走 `*_opts`，避免给公开入口加第二个布尔参数后
//! 每处调用都要重新数参数。

use super::plan::{ensure_session_copy_supported, run_copy};
use super::*;

/// 一次复制请求的附加开关（与 `sessionIds` 一样按操作传入，不做持久化配置）。
///
/// 几个开关都只影响**增强数据**：忽略账本仍会写正文与 DB，复制工作记忆失败只落报告字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyOptions {
    /// 显式忽略会话账本（用量 + 中断记录）：只由「仍然复制」按钮传 `true`。
    ///
    /// 语义是「用户显式放弃账本，且不可逆、不做后台补写」，被 UI 的「仍然复制」按钮与
    /// 操作恢复逻辑依赖，**不要**改它的含义（R6-08 新增了独立的 `copy_usage`）。
    pub ignore_aux: bool,
    /// R6-08：是否把源账号的 `session_usage` 搬到副本。
    ///
    /// **默认 `false`**：历史消耗留在来源账号，副本只累计自己的新增（「谁用的记在谁
    /// 头上」）。`ignore_aux = true` 时本字段无意义（账本整块跳过）。
    ///
    /// 不提供「连账本一起复制」的默认路径；将来若保留高级选项，必须二次确认并提示
    /// 「会把来源账号的消耗记到目标账号」。
    pub copy_usage: bool,
    /// 中断记录：维持现状（默认 `true`，与积分无关）。
    pub copy_interruptions: bool,
    /// 同时把源账号的工作记忆（`MEMORY.md`）追加到目标账号（issue #209）。
    pub copy_memory: bool,
}

impl Default for CopyOptions {
    fn default() -> Self {
        Self::new(false, false)
    }
}

impl CopyOptions {
    /// 存量签名（不变）：`ignore_aux` 为「仍然复制」开关，`copy_memory` 为工作记忆开关。
    ///
    /// 新增的 `copy_usage` 恒为 `false`（R6-08 的默认口径），`copy_interruptions` 随
    /// `ignore_aux` 反向取值（忽略账本时连中断记录一起跳过）。
    pub fn new(ignore_aux: bool, copy_memory: bool) -> Self {
        Self {
            ignore_aux,
            copy_usage: false,
            copy_interruptions: !ignore_aux,
            copy_memory,
        }
    }

    /// 显式打开「连账本一起复制」。
    ///
    /// 生产路径**不**调用它：默认口径是「历史消耗归来源账号」。只有单测用它验证
    /// 「开关打开时行为与 R6-08 之前逐字一致」，以及将来若做二次确认的高级选项。
    pub fn with_copy_usage(mut self, copy_usage: bool) -> Self {
        self.copy_usage = copy_usage;
        self
    }

    /// 显式打开/关闭中断记录复制（默认随 `ignore_aux` 反向取值）。
    pub fn with_copy_interruptions(mut self, copy_interruptions: bool) -> Self {
        self.copy_interruptions = copy_interruptions;
        self
    }
}

/// 复制的一方：档位数据根、档位名与账号身份（源侧只读、目标侧写入）。
///
/// 由本模块组装、交给 `plan::run_copy` 消费；单测直接构造它注入临时双档目录。
pub(in crate::modules::session) struct CopySide<'a> {
    pub(in crate::modules::session) paths: &'a SessionPaths,
    pub(in crate::modules::session) variant: WbVariant,
    pub(in crate::modules::session) uid: String,
    /// 账号库中的账号 id（仅作展示，身份判定以 uid 为准）。
    pub(in crate::modules::session) account_id: Option<String>,
}

/// 云端映射登记结果：失败必须上报，不能静默降级成成功。
/// 把勾选的会话复制到目标账号（路径 B）。返回复制报告。
///
/// 档位以**目标账号**自身为准：数据根、数据库、备份目录、认证文件都取该档位。
/// 国际版能力不满足时直接返回明确错误，绝不写半成品（design D6）。
/// App 正在运行时拒绝写入：独立复制 API 与桌面端共用同一条生命周期保护。
pub fn copy_sessions_for_switch(
    target_acc: &Value,
    session_ids: &[String],
    ignore_aux: bool,
) -> Result<Value, String> {
    copy_sessions_for_switch_opts(target_acc, session_ids, CopyOptions::new(ignore_aux, false))
}

/// [`copy_sessions_for_switch`] 的选项版：多一个「同时复制工作记忆」开关。
pub fn copy_sessions_for_switch_opts(
    target_acc: &Value,
    session_ids: &[String],
    options: CopyOptions,
) -> Result<Value, String> {
    let variant = account::variant_of(target_acc);
    let paths = SessionPaths::for_variant(variant);
    copy_sessions_for_switch_at_with_options(
        &paths,
        variant,
        target_acc,
        session_ids,
        options,
        process::is_workbuddy_running,
    )
}

/// 把勾选的会话从**显式源账号**复制到**显式目标账号**（跨档复制入口，design §2.1）。
///
/// 与 [`copy_sessions_for_switch`] 的差异只在来源判定：源 uid 取自 `source_acc`，
/// 不再从目标档登录态读取；源档与目标档可为不同档位。写入侧仍以目标账号为准
/// （数据根、数据库、映射库、备份目录），源档只读。
pub fn copy_sessions_cross(
    source_acc: &Value,
    target_acc: &Value,
    session_ids: &[String],
    ignore_aux: bool,
) -> Result<Value, String> {
    copy_sessions_cross_opts(
        source_acc,
        target_acc,
        session_ids,
        CopyOptions::new(ignore_aux, false),
    )
}

/// [`copy_sessions_cross`] 的选项版：多一个「同时复制工作记忆」开关。
pub fn copy_sessions_cross_opts(
    source_acc: &Value,
    target_acc: &Value,
    session_ids: &[String],
    options: CopyOptions,
) -> Result<Value, String> {
    let source_variant = account::variant_of(source_acc);
    let target_variant = account::variant_of(target_acc);
    let source_paths = SessionPaths::for_variant(source_variant);
    let target_paths = SessionPaths::for_variant(target_variant);
    let source_uid = account_uid(source_acc);
    if source_uid.is_empty() {
        return Err("源账号缺少 uid，无法复制会话".to_string());
    }
    let target_uid = account_uid(target_acc);
    if target_uid.is_empty() {
        return Err("目标账号缺少 uid，无法复制会话".to_string());
    }
    let source = CopySide {
        paths: &source_paths,
        variant: source_variant,
        uid: source_uid,
        account_id: nonempty_text(
            source_acc
                .get("id")
                .and_then(|v| v.as_str())
                .map(String::from),
        ),
    };
    let target = CopySide {
        paths: &target_paths,
        variant: target_variant,
        uid: target_uid,
        account_id: nonempty_text(
            target_acc
                .get("id")
                .and_then(|v| v.as_str())
                .map(String::from),
        ),
    };
    copy_sessions_cross_at_with_options(
        source,
        target,
        session_ids,
        options,
        process::is_workbuddy_running,
        SessionPaths::for_variant,
    )
}

/// 可注入路径与「App 是否运行」探针的跨档复制入口（单测注入临时双档目录与假探针，
/// 不触碰真实路径、不探测真实进程）。
pub(in crate::modules::session) fn copy_sessions_cross_at_with_options(
    source: CopySide<'_>,
    target: CopySide<'_>,
    session_ids: &[String],
    options: CopyOptions,
    is_app_running: impl Fn(WbVariant) -> bool,
    resolve_source_paths: impl Fn(WbVariant) -> SessionPaths,
) -> Result<Value, String> {
    // 快拒只针对**目标档**（写侧）：源档只读，运行中的源客户端不影响读取安全；
    // 目标账号不是当前登录账号时免关直接写（实测，见门禁函数注释）。
    if app_running_blocks_target_write(target.paths, target.variant, &target.uid, &is_app_running) {
        return Err(SESSION_COPY_APP_RUNNING.to_string());
    }
    // 源档也要读（正文与源行），国际版数据根不同构时同样不支持（design D6）。
    ensure_session_copy_supported(source.paths, source.variant)?;
    ensure_session_copy_supported(target.paths, target.variant)?;
    if source.variant == target.variant && source.uid == target.uid {
        return Err("源账号与目标账号相同，无需复制会话".to_string());
    }
    run_copy(
        &source,
        &target,
        session_ids,
        options,
        is_app_running,
        resolve_source_paths,
    )
}

/// 可注入路径与「App 是否运行」探针的同档复制入口（单测注入临时目录与假探针，不触碰
/// 真实路径、不探测真实进程）。也是 `copy_sessions_for_switch_opts` 的实现。
///
/// App 运行检查做两次：拿档位操作锁之前先快速拒绝；拿锁之后再复查——锁前到拿锁之间
/// App 可能被启动，只有锁后复查才能保证会话写入发生在 App 停止写入之后（design §4.1）。
pub(in crate::modules::session) fn copy_sessions_for_switch_at_with_options(
    paths: &SessionPaths,
    variant: WbVariant,
    target_acc: &Value,
    session_ids: &[String],
    options: CopyOptions,
    is_app_running: impl Fn(WbVariant) -> bool,
) -> Result<Value, String> {
    // 探测只对国际版生效（design D6 针对的是国际版数据根不同构）。国内版数据根与
    // 改造前同构，保留改造前的路径与返回结构，不让国内版看到「暂不支持」类新文案。
    ensure_session_copy_supported(paths, variant)?;
    let target_uid = account_uid(target_acc);
    if target_uid.is_empty() {
        return Err("目标账号缺少 uid，无法复制会话".to_string());
    }
    let source_uid = current_user_uid_at(&paths.auth_file)
        .ok_or_else(|| "未读取到本机登录态，无法确定来源账号".to_string())?;
    if source_uid == target_uid {
        return Err("当前账号与目标账号相同，无需复制会话".to_string());
    }
    // 同档路径的源恒为当前登录账号、且上面已拦同账号，因此这里对"目标=当前登录"
    // 恒为放行；保留统一门禁只为与其他写入入口共用同一条规则。
    if app_running_blocks_target_write(paths, variant, &target_uid, &is_app_running) {
        return Err(SESSION_COPY_APP_RUNNING.to_string());
    }
    let source = CopySide {
        paths,
        variant,
        account_id: account_id_for_uid(paths, &source_uid),
        uid: source_uid,
    };
    let target = CopySide {
        paths,
        variant,
        account_id: nonempty_text(
            target_acc
                .get("id")
                .and_then(|v| v.as_str())
                .map(String::from),
        ),
        uid: target_uid,
    };
    run_copy(
        &source,
        &target,
        session_ids,
        options,
        is_app_running,
        SessionPaths::for_variant,
    )
}
