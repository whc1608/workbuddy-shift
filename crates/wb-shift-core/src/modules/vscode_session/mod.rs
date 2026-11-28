//! 扩展数据仓（VS Code 内 CodeBuddy 插件 / CodeBuddy IDE 桌面客户端）会话复制。
//!
//! 两棵树同根同构，只差客户端段目录名（见 [`SessionStoreSpec`]）：
//! - VS Code 插件：`<root>/<uid>/VSCode/<uid>/history`（[`VSCODE_STORE`]）；
//! - CodeBuddy IDE：`<root>/<uid>/CodeBuddyIDE/<uid>/history`（[`CODEBUDDY_IDE_STORE`]）。
//!
//! 切换账号时，可把「当前账号」的会话（正文 + 索引）复制到「目标账号」目录，
//! 使目标账号重新打开客户端后能在对应工作区看到并续聊。与桌面版 `session` 模块（`modules/session/`）同构：
//! **本地目录复制 + 合并工作区索引**；插件侧副本一律取新 id（[`CopyIdPolicy::AlwaysNew`]），
//! IDE 侧默认沿用源 id、仅冲突时重随机（[`CopyIdPolicy::KeepUnlessConflict`]）。
//!
//! 存储布局（Windows 实测）：
//! ```text
//! %LOCALAPPDATA%\CodeBuddyExtension\Data\<uid>\VSCode\<uid>\
//!   history\<md5(工作区)>\
//!     index.json              # 工作区级会话索引 {conversations:[...], current}
//!     .index_bak.json         # 同结构备份
//!     <conversationId>\       # 32 位小写 hex
//!       index.json            # {messages:[...], requests:[...]}
//!       messages\<messageId>.json
//!       assets\*
//! ```
//!
//! **JetBrains 插件不是这个形状**（同一份会话内容，只多一层项目分段）：
//! ```text
//! %LOCALAPPDATA%\CodeBuddyExtension\Data\<uid>\JetBrains\<projectKey>\
//!   history\<bucket-32hex>\    # bucket 是**安装级常量**，本机所有项目/账号共用一个值
//!     ...                      # 内部与 VS Code 逐字同构
//!   check-point\ file-tree\ plan-task\   # 与 history 并列的同层目录（用途未验证，见下）
//! ```
//!
//! `projectKey` = 项目路径的 base64url（无 padding，如 `ZDovUHl0aG9u5Z+656GA57uD5Lmg`
//! ↔ `D:/Python基础练习`）；**没有打开项目时退化成 `<uid>`**（无项目桶）。所以
//! 「客户端段之后那一级」的语义按数据仓区分（[`ScopeSegment`]）：VS Code 系是账号 uid，
//! JetBrains 是项目 key。读写都按 [`history_scopes_in`] 枚举出来的多项目历史根进行，
//! 绝不把该级硬编码成 uid。
//!
//! 关于 `check-point/`、`file-tree/`、`plan-task/` 三个与 `history/` 并列的目录
//! （同样按 `<bucket>/<convId>` 分层）——**本轮仍只复制 `history/`**：
//!
//! - **2026-11-27 真机检查结论**（JetBrains 2026.2 / IntelliJ + PyCharm 已装插件）：
//!   路径 `%LOCALAPPDATA%\CodeBuddyExtension\Data\<uid>\JetBrains\<projectKey>\` 下，
//!   `check-point/`、`file-tree/`、`plan-task/` 三个目录**全部没有有效内容**——只有
//!   0 字节 `meta.json`、空的 `files/` 或空目录；
//! - **对照**：同一台机器的 VS Code 数据仓（`<uid>\VSCode\<uid>\history\` 同一级）这三个
//!   目录里有真实内容（单个工作区最多 1138 个文件 / 17MB）。说明「客户端有内容时会写进
//!   来」这件事成立，只是 JetBrains 侧本机没有产生计划/文件树数据；
//! - 因此**没有可复制的数据**：把三个空目录跟着会话搬过去既不会让复制体多出功能，
//!   还会在目标账号目录里留下无意义（且可能与源账号互相覆盖）的空结构；
//! - 若将来在 JetBrains 侧**观察到非空**的 `check-point/`（计划）/ `file-tree/`（工作区
//!   文件树）/ `plan-task/`（任务列表），需要**重新评估**是否随会话复制，并按 `AGENTS.md`
//!   第 4 节处理：这些目录是插件运行时写入的活数据，必须走「先关进程 + 备份 + 锁内合并」
//!   那套流程，不能直接 `copy_dir_recursive` 了事（索引可能被并发写坏）。
//!
//! 约定与约束：
//! - 会话 / 消息 / 请求 id 一律 **32 位小写 hex**（扩展 `generateId()` 产物），
//!   不得使用带连字符的 UUID。
//! - **只写目标 uid 目录**；源 uid 目录只读，绝不修改或删除。
//! - 显式排除 `default\` / `Public\` 目录（结构不同，见 [`is_safe_uid`]），
//!   uid 只能来自账号库 / 扩展登录态，不接受用户任意输入。
//! - 会话复制本体要求 VS Code 未运行（复用 [`vscode_ext::is_vscode_running`]）；
//!   由 [`switch_vscode_ext_with_copy`] 编排时，`restart = true` 会先自动关闭编辑器再复制。
//! - 单条原子性：先写 `<ws>\.tmp-<newId>\` 再 `rename` 成 `<ws>\<newId>\`；
//!   索引合并用「读-改-写 + `atomic_write`」，写前把将被改的索引备份到
//!   `backup_dir()/vscode-sessions/<utc_iso>/`。
//! - 失败逐条隔离：删临时/成品目录 + 回退该工作区索引，记入 `errors[]`，继续其余条目。
//!
//! macOS / Linux 的数据根**未实测**：按同一相对布局 best-effort 推导
//! （见 [`ext_data_root_candidates`]），仅在对应候选路径真实存在时才使用。

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::modules::account::{self, get_str};
use crate::modules::config::{atomic_write, backup_dir, utc_iso};
use crate::modules::session::{SessionPaths, SyncSelection};
use crate::modules::vscode_ext;
use crate::modules::vscode_session_sync;

/// 会话历史目录名。
const HISTORY_DIR: &str = "history";

/// 「客户端段之后那一级」的语义：决定 `history/` 的父目录是什么。
///
/// 两个插件型数据仓的目录前缀完全相同（`<app>/Data/<uid>/<client_dir>/`），只有这一级
/// 的含义不同——这正是历史上把所有会话都读进「无项目」桶的根因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeSegment {
    /// `<root>/<uid>/<client_dir>/<uid>/history/<bucket>`：该级是**账号 uid**。
    Uid,
    /// `<root>/<uid>/<client_dir>/<projectKey>/history/<bucket>`：该级是**项目 key**。
    ProjectKey,
}

/// 扩展数据仓的分段与备份命名：区分「VS Code 插件」与「CodeBuddy IDE」两棵同构的会话树。
///
/// 布局：`<平台本地数据根>/<app_dir>/Data/<uid>/<client_dir>/<scope_segment>/history/<工作区目录名>/`。
/// 工作区目录名是**不透明字符串**（实现只做目录枚举与按名复制，不重算 md5）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionStoreSpec {
    /// 应用数据目录名（`CodeBuddyExtension`）。
    pub app_dir: &'static str,
    /// 客户端段目录名（`VSCode` / `CodeBuddyIDE` / `JetBrains`）。
    pub client_dir: &'static str,
    /// 会话索引备份根目录名：`<工具存储根>/backups/<backup_kind>/<utc_iso>/`。
    pub backup_kind: &'static str,
    /// `history/` 的父目录语义（见 [`ScopeSegment`]）。
    pub scope: ScopeSegment,
}

/// VS Code 内 CodeBuddy 插件（`tencent-cloud.coding-copilot`）的数据仓。
pub const VSCODE_STORE: SessionStoreSpec = SessionStoreSpec {
    app_dir: "CodeBuddyExtension",
    client_dir: "VSCode",
    backup_kind: "vscode-sessions",
    scope: ScopeSegment::Uid,
};

/// CodeBuddy IDE（国内版桌面客户端）的数据仓：与插件同根，只有客户端段不同。
pub const CODEBUDDY_IDE_STORE: SessionStoreSpec = SessionStoreSpec {
    app_dir: "CodeBuddyExtension",
    client_dir: "CodeBuddyIDE",
    backup_kind: "codebuddy-ide-sessions",
    scope: ScopeSegment::Uid,
};

/// JetBrains IDE 内 CodeBuddy 插件的数据仓（IDEA / PyCharm 共用同一份）。
///
/// 索引读取、内容摘要、复制、登记、预览、增量同步全部走同一套 spec 参数化内核；
/// 与 VS Code 插件的差别是**多一层项目分段**（见 [`ScopeSegment::ProjectKey`]）。
pub const JETBRAINS_STORE: SessionStoreSpec = SessionStoreSpec {
    app_dir: "CodeBuddyExtension",
    client_dir: "JetBrains",
    backup_kind: "jetbrains-sessions",
    scope: ScopeSegment::ProjectKey,
};

/// 待复制的会话引用（项目段 + 工作区桶 + 会话 id）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyItem {
    /// 工作区目录名：VS Code 系 = `md5(工作区)`，JetBrains = 安装级 bucket（均 32 位小写 hex）。
    pub workspace_hash: String,
    /// 会话 id：旧目录树为 32 位小写 hex；新版 CodeBuddy IDE 会话库为 UUID（含 `-`）。
    pub conversation_id: String,
    /// 项目 key（`<client_dir>` 之后那一级目录名）。
    ///
    /// 只有 [`ScopeSegment::ProjectKey`] 的数据仓（JetBrains）用它定位项目；缺省 / 非法时
    /// **回退源账号 uid**（= 插件「未打开项目」时写入的退化项目段，与历史行为一致）。
    /// VS Code 系忽略该字段：那一级恒为账号 uid。
    #[serde(default)]
    pub project_key: Option<String>,
    /// 会话所在存储：`None`（旧目录树，默认）或 `"db"`（新版 CodeBuddy IDE 的 CLI 会话库）。
    ///
    /// 只有 CodeBuddy IDE 会写该字段；VS Code / JetBrains 忽略它，旧客户端不传时行为不变。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
}

/// 副本 id 策略：决定复制体沿用源 id 还是取新 id。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CopyIdPolicy {
    /// 副本一律取新 id（VS Code 插件：扩展按 id 归类，复制体必须与源区分）。
    AlwaysNew,
    /// 默认沿用源 id，仅当目标工作区已存在同 id 时取新 id 并重写引用
    /// （CodeBuddy IDE：真机实测沿用 id 的目标账号可直接加载；冲突重随机避免覆盖既有会话）。
    KeepUnlessConflict,
}

/// 复制策略：id 生成方式与目标工作区索引的写入范围。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CopyOptions {
    /// 副本 id 策略（见 [`CopyIdPolicy`]）。
    pub id_policy: CopyIdPolicy,
    /// 是否把合并后的索引同步写入目标工作区的 `.index_bak.json`。
    ///
    /// 插件侧写（扩展自用的索引备份需要与主索引一致）；IDE 侧**不写**——目标侧原有的
    /// `.index_bak.json` 保持原样，不存在也不新建（见 codebuddy-ide-target 契约）。
    pub write_workspace_index_backup: bool,
}

/// VS Code 插件侧的复制策略（历史行为：恒取新 id + 同步工作区索引备份）。
pub(crate) const VSCODE_COPY: CopyOptions = CopyOptions {
    id_policy: CopyIdPolicy::AlwaysNew,
    write_workspace_index_backup: true,
};

/// CodeBuddy IDE 侧的复制策略（沿用 id，冲突才重随机；不动目标侧索引备份）。
pub(crate) const IDE_COPY: CopyOptions = CopyOptions {
    id_policy: CopyIdPolicy::KeepUnlessConflict,
    write_workspace_index_backup: false,
};

mod copy;
mod index_merge;
mod list;
mod paths;
mod remap;

#[allow(unused_imports)]
pub(crate) use copy::copy_sessions_in_with;
pub use copy::{copy_sessions_in, copy_vscode_sessions, switch_vscode_ext_with_copy};
pub use list::{list_sessions_in, list_sessions_in_store, list_vscode_sessions};
pub use paths::{
    account_client_dir_in, ext_data_root, ext_data_root_candidates, history_root, history_root_in,
    history_scopes_in, scope_history_root_in, store_data_root, store_data_root_candidates,
    uid_data_dir_in, HistoryScope,
};
#[allow(unused_imports)]
pub(crate) use paths::{
    copy_dir_recursive, decode_project_key, is_hex32, is_safe_segment, is_safe_uid, read_json,
    remove_dir_all_if_exists,
};
#[allow(unused_imports)]
pub(crate) use remap::{
    remap_plan_from_maps, replace_exact_ids, replace_ids_in_extra, write_conversation, RemapPlan,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(unused_imports)]
    use super::copy::*;
    #[allow(unused_imports)]
    use super::index_merge::*;
    #[allow(unused_imports)]
    use super::list::*;
    #[allow(unused_imports)]
    use super::paths::*;
    #[allow(unused_imports)]
    use super::remap::*;
    include!("tests/copy.rs");
    include!("tests/remap.rs");
}
