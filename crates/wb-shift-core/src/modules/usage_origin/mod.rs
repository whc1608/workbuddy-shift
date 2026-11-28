//! Token 归属索引：复制/同步后「历史消耗归来源账号」的分段继承记录（R6-07）。
//!
//! 客户端会在切号复制/同步时把来源会话的**整份正文**（含原始时间戳与 usage）写到目标
//! 会话里，于是「一条用量属于哪个账号」不能再用文件 mtime 或文件所在目录的 uid 判断：
//! 源会话复制后继续使用，mtime 立刻反超副本，历史就会翻到副本账号名下。
//!
//! 本模块存一份**独立的归属索引**（`<store_root>/usage-origins.json`），按
//! `segments: [{untilMs, uid}]` 记录「某个会话在某个时间点之前的历史属于谁」：
//!
//! - 写入方：复制/同步成功后继承来源段并补一条「本次 boundary」段（[`OriginIndex::inherit`]）；
//! - 读取方：`token_stats` 按**记录自身时间戳**落段（[`OriginIndex::attribute`]），索引缺失即
//!   回退「文件自身 uid」的老行为；
//! - 会话在索引里没有条目、或记录时间晚于所有段 → 返回 `None`，由调用方回退文件 uid，
//!   因此「复制之后目标自己新增的记录」天然归目标账号，不需要额外标记。
//!
//! 键必须带命名空间前缀（`wb:cn:` / `wb:ai:` / `ide:vscode:` / `ide:jetbrains:` /
//! `ide:codebuddy-ide:`）：不同客户端的 sessionId 生成规则不同，裸 id 会跨客户端碰撞。
//!
//! 落盘格式版本化、原子写；读取失败（缺失 / 损坏 / 版本不符）一律退化成空索引，
//! **不报错、不 panic**——归属索引只影响归属，绝不能让整次统计失败。

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::modules::config;
use crate::modules::locks;
use crate::modules::variant::WbVariant;

mod backfill;
mod index;

pub use backfill::{backfill_at, backfill_once, OperationFact};

#[cfg(test)]
mod tests;

/// 索引文件格式版本：结构变化（字段语义）才升，读侧遇到不认识的版本退化成空索引。
pub const VERSION: u32 = 1;
/// 索引文件相对工具存储根的名字。
pub const FILE_NAME: &str = "usage-origins.json";
/// 条目上限：超出按 `updatedAt` 淘汰最旧（避免长期使用后文件无限增长）。
pub const MAX_ENTRIES: usize = 5_000;
/// 单会话段数上限：超出降级为单 boundary 并标 `truncated`。
pub const MAX_SEGMENTS: usize = 64;

/// 工具存储根目录名（`~/.workbuddy-shift`）。
///
/// 与 [`config::store_dir`] 同源，这里显式写出来是为了让**注入家目录**的调用方（单测、
/// 「Token 统计」的 `home` 参数）能算出同一个根，而不是去读进程环境变量。
pub const STORE_DIR_NAME: &str = ".workbuddy-shift";

/// WorkBuddy 国内版的键命名空间。
pub const NS_WORKBUDDY_CN: &str = "wb:cn";
/// WorkBuddy 国际版的键命名空间。
pub const NS_WORKBUDDY_AI: &str = "wb:ai";
/// VS Code 插件的键命名空间。
pub const NS_IDE_VSCODE: &str = "ide:vscode";
/// JetBrains 插件的键命名空间。
pub const NS_IDE_JETBRAINS: &str = "ide:jetbrains";
/// CodeBuddy IDE 的键命名空间。
pub const NS_IDE_CODEBUDDY: &str = "ide:codebuddy-ide";

/// 工具存储根（生产路径：`config::store_dir()`，认 `WB_SHIFT_HOME`）。
pub fn store_root() -> PathBuf {
    config::store_dir()
}

/// 注入家目录版本的工具存储根：`<home>/.workbuddy-shift`。
pub fn store_root_at(home: &Path) -> PathBuf {
    home.join(STORE_DIR_NAME)
}

/// 索引文件路径：`<store_root>/usage-origins.json`。
pub fn file_path(store_root: &Path) -> PathBuf {
    store_root.join(FILE_NAME)
}

/// 由「备份根」反推工具存储根：`<store>/backups/<kind>/<utc_iso>` → `<store>`。
///
/// 插件复制内核（[`crate::modules::vscode_session::copy`]）拿到的是备份根而不是
/// [`crate::modules::session::SessionPaths`]，三层目录名里只有 `backups` 是稳定标记
/// （由 `SessionPaths::backup_root()` 拼出），因此按该标记定位存储根，找不到就返回
/// `None`——**不猜**位置，宁可不写归属也不把索引写到不相干目录。
pub fn store_root_from_backup_root(backup_root: &Path) -> Option<PathBuf> {
    backup_root
        .ancestors()
        .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "backups"))
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

/// 档位 → 键命名空间（**唯一**映射来源，禁止在别处手拼 `"wb:cn"` 这类字符串）。
pub fn namespace_for_variant(variant: WbVariant) -> &'static str {
    match variant {
        WbVariant::Cn => NS_WORKBUDDY_CN,
        WbVariant::Ai => NS_WORKBUDDY_AI,
    }
}

/// IDE 客户端来源（与 `vscode_session::{VSCODE_STORE, JETBRAINS_STORE, CODEBUDDY_IDE_STORE}`
/// 的 `client_dir` / `backup_kind` 一一对应）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdeClientSource {
    /// VS Code 内的 CodeBuddy 插件（`client_dir = "VSCode"`）。
    VSCode,
    /// JetBrains 内的 CodeBuddy 插件（`client_dir = "JetBrains"`）。
    JetBrains,
    /// CodeBuddy IDE 桌面客户端（`client_dir = "CodeBuddyIDE"`）。
    CodeBuddyIde,
}

impl IdeClientSource {
    /// 由数据仓的 `client_dir`（`VSCode` / `JetBrains` / `CodeBuddyIDE`）判定。
    ///
    /// 未知客户端返回 `None`：调用方（统计侧）此时不做归属覆盖，回退文件 uid。
    pub fn from_client_dir(dir: &str) -> Option<Self> {
        match dir {
            "VSCode" => Some(Self::VSCode),
            "JetBrains" => Some(Self::JetBrains),
            "CodeBuddyIDE" => Some(Self::CodeBuddyIde),
            _ => None,
        }
    }

    /// 由备份根目录名（`vscode-sessions` / `jetbrains-sessions` / `codebuddy-ide-sessions`）判定。
    pub fn from_backup_kind(kind: &str) -> Option<Self> {
        match kind {
            "vscode-sessions" => Some(Self::VSCode),
            "jetbrains-sessions" => Some(Self::JetBrains),
            "codebuddy-ide-sessions" => Some(Self::CodeBuddyIde),
            _ => None,
        }
    }

    /// 索引键命名空间。
    pub fn namespace(self) -> &'static str {
        match self {
            Self::VSCode => NS_IDE_VSCODE,
            Self::JetBrains => NS_IDE_JETBRAINS,
            Self::CodeBuddyIde => NS_IDE_CODEBUDDY,
        }
    }
}

/// IDE 客户端 → 键命名空间（与 [`IdeClientSource::namespace`] 同一个实现）。
pub fn namespace_for_ide(client: IdeClientSource) -> &'static str {
    client.namespace()
}

/// 索引键：`"{namespace}:{session_id}"`。
pub fn key_of(namespace: &str, session_id: &str) -> String {
    format!("{namespace}:{session_id}")
}

/// 一段归属：`until_ms` 之前（含）的用量记录属于 `uid`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    /// 该段的上界（含）：记录时间戳 `ts <= untilMs` 命中本段。
    pub until_ms: i64,
    /// 归属账号 uid。
    pub uid: String,
}

/// 单会话的归属条目。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// 按 `until_ms` 升序、相邻同 uid 已合并的分段序列。
    #[serde(default)]
    pub segments: Vec<Segment>,
    /// 最后一次写入时间（GC / 淘汰按它排序）。
    #[serde(default)]
    pub updated_at: i64,
    /// 由「操作日志缺失时的启发式回填」建立（数据不如真实继承可靠，仅供兜底）。
    #[serde(default)]
    pub heuristic: bool,
    /// 段数超过 [`MAX_SEGMENTS`] 后被降级成单 boundary。
    #[serde(default)]
    pub truncated: bool,
}

impl Entry {
    /// 按记录时间戳落段：命中的第一个 `segment.until_ms >= ts`。
    pub fn attribute(&self, ts: i64) -> Option<String> {
        self.segments
            .iter()
            .find(|segment| ts <= segment.until_ms)
            .map(|segment| segment.uid.clone())
    }
}

/// 归属索引（内存形态；落盘由 [`OriginIndex::save`] 负责）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OriginIndex {
    entries: BTreeMap<String, Entry>,
    backfilled_at: Option<i64>,
}

impl OriginIndex {
    /// 空索引（缺失 / 损坏 / 版本不符时的返回值）。
    pub fn empty() -> Self {
        Self::default()
    }

    /// 读取生产路径上的索引；缺失 / 损坏 / 版本不符 → 空索引，不报错、不 panic。
    pub fn load() -> Self {
        Self::load_at(&store_root())
    }

    /// 读取指定工具存储根下的索引（单测与注入家目录的调用方用）。
    pub fn load_at(store_root: &Path) -> Self {
        index::load_file(&file_path(store_root))
    }

    /// 保存到生产路径。失败只由调用方记日志（归属不能拖垮业务）。
    pub fn save(&self) -> Result<(), String> {
        self.save_at(&store_root())
    }

    /// 保存到指定工具存储根。
    pub fn save_at(&self, store_root: &Path) -> Result<(), String> {
        index::save_file(&file_path(store_root), self)
    }

    /// 条目数量（单测与诊断用）。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否没有任何条目。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 读一个条目（只读视图）。
    pub fn entry(&self, namespace: &str, session_id: &str) -> Option<&Entry> {
        self.entries.get(&key_of(namespace, session_id))
    }

    /// 取出全部键（GC 与单测用）。
    pub fn keys(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    /// 按**记录时间戳**落段：`ts <= segment.untilMs` 的第一个段。
    ///
    /// `ts = None`（记录没有时间戳）或索引里没有这个会话 → `None`，调用方回退文件 uid。
    pub fn attribute(&self, namespace: &str, session_id: &str, ts: Option<i64>) -> Option<String> {
        let ts = ts?;
        self.entries
            .get(&key_of(namespace, session_id))?
            .attribute(ts)
    }

    /// 复制/同步成功后继承来源段并补 boundary。
    ///
    /// 语义：`targetSegments = sourceSegments + [(boundaryMs, sourceUid)]`，相邻同 uid 合并；
    /// 重复同步**替换**目标段（目标正文被整份替换，追加会造出永远命不中的段）。
    ///
    /// `source_uid` 是「来源会话当前的归属账号」：boundary 之前的来源历史按来源段走，
    /// boundary 之后（目标自己新增的记录）索引不再命中，由调用方回退**目标文件**的 uid。
    ///
    /// 返回是否真的改动了段（相同内容重复写入时不落盘）。
    pub fn inherit(
        &mut self,
        target_ns: &str,
        target_sid: &str,
        source_ns: &str,
        source_sid: &str,
        boundary_ms: i64,
        source_uid: &str,
    ) -> bool {
        let key = key_of(target_ns, target_sid);
        let Some(entry) =
            self.inherited_entry(source_ns, source_sid, boundary_ms, source_uid, false)
        else {
            return false;
        };
        let changed = self
            .entries
            .get(&key)
            .is_none_or(|current| current.segments != entry.segments);
        self.entries.insert(key, entry);
        self.trim();
        changed
    }

    /// 仅当目标**已有条目**时整体替换（段计算与 [`Self::inherit`] 相同）。
    ///
    /// 用于「恢复/重放」这类必须幂等、且不允许凭空造条目的场景：目标还没条目说明本次
    /// 写入没有对应的成功复制，宁可不写。
    pub fn replace(
        &mut self,
        target_ns: &str,
        target_sid: &str,
        source_ns: &str,
        source_sid: &str,
        boundary_ms: i64,
        source_uid: &str,
    ) -> bool {
        let key = key_of(target_ns, target_sid);
        if !self.entries.contains_key(&key) {
            return false;
        }
        let Some(entry) =
            self.inherited_entry(source_ns, source_sid, boundary_ms, source_uid, false)
        else {
            return false;
        };
        let changed = self
            .entries
            .get(&key)
            .is_none_or(|current| current.segments != entry.segments);
        self.entries.insert(key, entry);
        self.trim();
        changed
    }

    /// 启发式写入（回填用）：无来源条目时按「组内最早成员 + linkedAt」补一段。
    ///
    /// 与 [`Self::inherit`] 的差别只有两点：段来自调用方给定的事实、条目带 `heuristic` 标记。
    pub fn inherit_heuristic(
        &mut self,
        target_ns: &str,
        target_sid: &str,
        boundary_ms: i64,
        source_uid: &str,
    ) -> bool {
        let target_uid = source_uid.trim();
        if target_uid.is_empty() {
            return false;
        }
        let key = key_of(target_ns, target_sid);
        if self.entries.contains_key(&key) {
            // 已有真实继承（或更早的启发式）：不覆盖，避免把可靠数据降级成猜测。
            return false;
        }
        let entry = Entry {
            segments: vec![Segment {
                until_ms: boundary_ms,
                uid: target_uid.to_string(),
            }],
            updated_at: config::now_ms(),
            heuristic: true,
            truncated: false,
        };
        self.entries.insert(key, entry);
        self.trim();
        true
    }

    /// 回填用的辅助：把「操作日志推出来的链」应用到索引（不带 heuristic）。
    pub fn apply_operation(
        &mut self,
        target_ns: &str,
        target_sid: &str,
        source_ns: &str,
        source_sid: &str,
        boundary_ms: i64,
        source_uid: &str,
    ) -> bool {
        self.inherit(
            target_ns,
            target_sid,
            source_ns,
            source_sid,
            boundary_ms,
            source_uid,
        )
    }

    /// GC：删除不在 `alive_keys` 里的条目，返回删除条数。
    ///
    /// 调用方必须保证 `alive_keys` 覆盖**所有客户端**仍然存在的会话键；空集合视为
    /// 「这一轮什么都没读到」，由调用方决定跳过 GC（见 `token_stats::collect_statistics`）。
    pub fn gc(&mut self, alive_keys: &HashSet<String>) -> usize {
        let before = self.entries.len();
        self.entries.retain(|key, _| alive_keys.contains(key));
        before - self.entries.len()
    }

    /// 一次性回填标记（`backfilledAt`）。
    pub fn backfilled_at(&self) -> Option<i64> {
        self.backfilled_at
    }

    /// 标记回填已完成（写 `backfilledAt`）。
    pub fn mark_backfilled(&mut self, ts: i64) {
        self.backfilled_at = Some(ts);
    }

    /// 内部：跨命名空间克隆来源段并补 boundary。
    fn inherited_entry(
        &self,
        source_ns: &str,
        source_sid: &str,
        boundary_ms: i64,
        source_uid: &str,
        heuristic: bool,
    ) -> Option<Entry> {
        let source = self.entries.get(&key_of(source_ns, source_sid));
        let (mut segments, source_truncated, source_heuristic) = match source {
            Some(entry) => (entry.segments.clone(), entry.truncated, entry.heuristic),
            None => (Vec::new(), false, false),
        };
        let uid = source_uid.trim();
        if uid.is_empty() {
            // 来源账号未知：只搬来源段；来源段也没有就不造空条目。
            if segments.is_empty() {
                return None;
            }
        } else {
            segments.push(Segment {
                until_ms: boundary_ms,
                uid: uid.to_string(),
            });
        }
        let (segments, truncated) = index::normalize_segments(&segments);
        if segments.is_empty() {
            return None;
        }
        Some(Entry {
            segments,
            updated_at: config::now_ms(),
            heuristic: heuristic || source_heuristic,
            truncated: truncated || source_truncated,
        })
    }

    /// 超出 [`MAX_ENTRIES`] 时按 `updatedAt` 淘汰最旧。
    fn trim(&mut self) {
        index::trim_entries(&mut self.entries, MAX_ENTRIES);
    }

    /// 供 `index` 子模块序列化时读取内部字段。
    pub(super) fn entries(&self) -> &BTreeMap<String, Entry> {
        &self.entries
    }

    /// 供 `index` 子模块反序列化时写入内部字段。
    pub(super) fn from_parts(entries: BTreeMap<String, Entry>, backfilled_at: Option<i64>) -> Self {
        Self {
            entries,
            backfilled_at,
        }
    }
}

// ---------------------------------------------------------------------------
// 写入助手：四个写入内核共用，失败只记日志，绝不让业务失败
// ---------------------------------------------------------------------------

/// 写入串行化守卫。
///
/// 索引是**可重建的派生数据**（下一次统计会重算归属，最坏情况是短暂回退老行为），
/// 因此中毒时取回内部值继续即可（`locks::recover` 的 (a) 类用法）。
fn write_guard() -> MutexGuard<'static, ()> {
    static WRITE_GUARD: Mutex<()> = Mutex::new(());
    locks::recover(&WRITE_GUARD)
}

/// 记录一次「目标继承来源」：读-改-写整个索引。
///
/// 任何失败（读不到、写不进）都只打印一行日志：复制/同步已经成功，归属索引不能反过来
/// 让业务失败。返回是否真正写入了新内容（单测用）。
pub fn record_inherit(
    store_root: &Path,
    target_ns: &str,
    target_sid: &str,
    source_ns: &str,
    source_sid: &str,
    boundary_ms: i64,
    source_uid: &str,
) -> bool {
    update(store_root, |index| {
        index.inherit(
            target_ns,
            target_sid,
            source_ns,
            source_sid,
            boundary_ms,
            source_uid,
        )
    })
}

/// 记录一次**复制**交接（四个写入内核的统一入口，语义见 [`OriginIndex::inherit`]）。
///
/// **绝不返回错误、绝不 panic**：归属索引是派生元数据，写不进去最多让归属回退到
/// 「文件自身 uid」（即 R6-07 之前的行为），绝不能因为索引失败把一次已经成功的会话
/// 复制报告成失败——因此签名里没有 `Result`，从类型上就不可能把错误往上抛。
/// 返回值只表示「是否真的写入了新内容」，供单测断言，业务调用方一律忽略。
pub fn record_copy_quiet(
    store_root: &Path,
    ns: &str,
    target_session_id: &str,
    source_ns: &str,
    source_session_id: &str,
    boundary_ms: i64,
    source_uid: &str,
) -> bool {
    record_inherit(
        store_root,
        ns,
        target_session_id,
        source_ns,
        source_session_id,
        boundary_ms,
        source_uid,
    )
}

/// 记录一次**同步**交接。与 [`record_copy_quiet`] 逐字同义（都是「继承来源段 + 补来源
/// 到本次 boundary」），分开命名只为让写入点自解释、日志与事件能区分来源。
pub fn record_sync_quiet(
    store_root: &Path,
    ns: &str,
    target_session_id: &str,
    source_ns: &str,
    source_session_id: &str,
    boundary_ms: i64,
    source_uid: &str,
) -> bool {
    record_inherit(
        store_root,
        ns,
        target_session_id,
        source_ns,
        source_session_id,
        boundary_ms,
        source_uid,
    )
}

/// [`record_inherit`] 的「目标必须已有条目」版本（恢复/重放路径用）。
pub fn record_replace(
    store_root: &Path,
    target_ns: &str,
    target_sid: &str,
    source_ns: &str,
    source_sid: &str,
    boundary_ms: i64,
    source_uid: &str,
) -> bool {
    update(store_root, |index| {
        index.replace(
            target_ns,
            target_sid,
            source_ns,
            source_sid,
            boundary_ms,
            source_uid,
        )
    })
}

/// 读-改-写：整个「load → 改 → save」在进程内串行化，避免两个并发复制互相覆盖。
fn update(store_root: &Path, mutate: impl FnOnce(&mut OriginIndex) -> bool) -> bool {
    let _guard = write_guard();
    let mut index = OriginIndex::load_at(store_root);
    if !mutate(&mut index) {
        return false;
    }
    if let Err(error) = index.save_at(store_root) {
        eprintln!("[usage-origin] 归属索引保存失败（不影响本次操作）：{error}");
        return false;
    }
    true
}
