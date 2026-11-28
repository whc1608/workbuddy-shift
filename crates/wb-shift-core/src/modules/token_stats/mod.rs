//! 本地 WorkBuddy / WorkBuddy 国际版 / CodeBuddy CLI / CodeBuddy IDE Token 统计。
//!
//! 这个模块是统计数据的唯一归属：日志只在这里解码、去重和按时间聚合，
//! Tauri 与 HTTP 层只负责转发结果。响应只包含聚合数字和脱敏标识，不返回
//! 消息正文、arguments 或认证信息。
//!
//! CodeBuddy IDE 不写 JSONL，用量在 `CodeBuddyExtension/Data/**/history/**/index.json`
//! 的 `requests[].usage` 中；消息正文文件（`messages/`）不会被扫描。
//!
//! 国内版与国际版**分开统计**（`workbuddy` / `workbuddy-ai` 两个 source），
//! 不合并：两档位是不同账号体系，混算会让用量无法归因。

use chrono::{Datelike, Local, Timelike};
use serde::de::{IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use crate::modules::locks;
use crate::modules::usage_origin::{
    self, key_of, namespace_for_variant, IdeClientSource, OriginIndex,
};
use crate::modules::variant::{HostOs, WbVariant};

/// 某档位的候选 jsonl 根（顺序固定；纯函数，便于单测）。
///
/// 国内版保持既有单一 `projects/` 根；国际版两个根都探测（实测没有
/// `projects/`，只有 `sessions/`）。
fn jsonl_root_candidates(variant: WbVariant, root: &Path) -> Vec<PathBuf> {
    match variant {
        WbVariant::Cn => vec![root.join("projects")],
        WbVariant::Ai => vec![root.join("projects"), root.join("sessions")],
    }
}

/// 从候选根里挑出磁盘上确实存在的那些；都不存在则返回空列表（空源，不报错）。
///
/// 家目录一律由 [`stats_roots_at`] 注入，绝不在这里自己取（历史包袱见该函数注释）。
fn existing_roots(candidates: &[PathBuf]) -> Vec<PathBuf> {
    candidates
        .iter()
        .filter(|path| path.is_dir())
        .cloned()
        .collect()
}

/// 统计各来源的候选根（**纯函数**：只做路径拼接，不读环境变量、不碰真实家目录，
/// 也不过滤存在性）。
///
/// 家目录必须从参数进来。`get_statistics` 在调用它之前取 `config::home_dir()`
/// （认 `WB_SHIFT_HOME`），于是沙箱里「Token 统计」只扫沙箱副本。早期实现直接在
/// `get_statistics` 里调 `dirs::home_dir()`，绕过了这层重定向——沙箱里点开这个页面
/// 看到的是真实用户数据，「沙箱」二字就不成立了。
///
/// 拆成注入式纯函数是为了让这条约束可执行：注入一个临时目录，断言所有候选根都落在
/// 它下面。谁哪天改回 `dirs::home_dir()`，`stats_roots_stay_under_injected_home` 会红。
fn stats_roots_at(home: &Path) -> StatsRoots {
    StatsRoots {
        workbuddy: jsonl_root_candidates(WbVariant::Cn, &WbVariant::Cn.data_root_at(home)),
        // 国际版独立 source：不与国内版混算（不同账号体系）。
        workbuddy_ai: jsonl_root_candidates(WbVariant::Ai, &WbVariant::Ai.data_root_at(home)),
        codebuddy_cli: home.join(".codebuddy").join("projects"),
        codebuddy_ide: codebuddy_extension_data_dir_at(home),
    }
}

/// 统计各来源的候选根，按 source 分组（分组顺序就是响应里 `sources` 的顺序）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct StatsRoots {
    /// 国内版（`<home>/.workbuddy/projects`）。
    workbuddy: Vec<PathBuf>,
    /// 国际版（实测只有 `sessions/`，`projects/` 也一并探测）。
    workbuddy_ai: Vec<PathBuf>,
    /// CodeBuddy CLI（`<home>/.codebuddy/projects`）。
    codebuddy_cli: PathBuf,
    /// CodeBuddy IDE（`<home>/…/CodeBuddyExtension/Data`，与登录态同源）。
    codebuddy_ide: PathBuf,
}

impl StatsRoots {
    /// 摊平后的全部候选根：断言「全部落在注入的家目录下」时用。
    #[cfg(test)]
    fn all(&self) -> Vec<PathBuf> {
        let mut roots = self.workbuddy.clone();
        roots.extend(self.workbuddy_ai.iter().cloned());
        roots.push(self.codebuddy_cli.clone());
        roots.push(self.codebuddy_ide.clone());
        roots
    }
}

/// Return independent WorkBuddy (CN), WorkBuddy AI, CodeBuddy CLI, and
/// CodeBuddy IDE aggregates.
/// `days` is interpreted in Rust using the same millisecond clock for every source.
pub fn get_statistics(days: Option<i64>) -> Value {
    // 家目录走 `config::home_dir()`（认 `WB_SHIFT_HOME`）：沙箱里统计只扫沙箱副本。
    // 不要换成 `dirs::home_dir()`——那会绕过重定向去读真实用户数据。
    let home = crate::modules::config::home_dir();
    get_statistics_at(&home, days)
}

// ---------------------------------------------------------------------------
// 并发合并（single-flight）：同一进程内相同 `(home, days)` 只做一次完整统计
// ---------------------------------------------------------------------------

/// 合并窗口：只用来把「同时到达」的调用并到一次扫描上，**不是**长期缓存。
///
/// 窗口内文件变化不会立刻反映（≤ 1.5 s），也不会跨进程复用；这是有意的权衡：
/// 多标签页 / 预热与页面请求几乎同时到达，窗口足够覆盖它们，又不至于让用户点
/// 「刷新统计」时看到明显过期的数字。
const MEMO_WINDOW: Duration = Duration::from_millis(1_500);

/// memo 的键：家目录 + 归一化后的窗口天数。
///
/// `days` 先归一化成 `None / 7 / 30 / 90` 再进键：`Some(15)` 与 `None` 的统计口径
/// 完全相同，不归一化会让它们各扫一遍。不同 `home` 绝不共用同一个键。
#[derive(Clone, Debug, PartialEq, Eq)]
struct MemoKey {
    home: PathBuf,
    days: Option<i64>,
}

/// 一次完整统计的结果，以及它**算完**的时间点（窗口从这里起算）。
struct MemoEntry {
    key: MemoKey,
    value: Value,
    at: Instant,
}

/// 进程内「最近一次完整统计」的 memo（`None` = 还没算过）。
///
/// 只有一格：读的是「刚刚算完的那一份」，不做多 key 的历史缓存——这是有意的，
/// 目的是合并并发而不是长期缓存（T2 非目标明确不引入磁盘聚合缓存）。
static STATISTICS_MEMO: OnceLock<Mutex<Option<MemoEntry>>> = OnceLock::new();

fn statistics_memo() -> &'static Mutex<Option<MemoEntry>> {
    STATISTICS_MEMO.get_or_init(|| Mutex::new(None))
}

/// 清空 memo：单测用，保证用例之间互不影响。
#[cfg(test)]
pub(crate) fn clear_statistics_memo() {
    // memo 是可重建缓存，中毒时取回内部值清空即可（见 `locks::recover` 的适用条件）。
    *locks::recover(statistics_memo()) = None;
}

/// 归一化窗口天数：只认 7 / 30 / 90，其它一律当「全部」。
fn normalize_days(days: Option<i64>) -> Option<i64> {
    match days {
        Some(7) => Some(7),
        Some(30) => Some(30),
        Some(90) => Some(90),
        _ => None,
    }
}

/// 注入式入口：家目录由调用方给定（`get_statistics` 传 `config::home_dir()`，
/// 单测传临时目录），并在这一步做**并发合并**。
///
/// 合并语义：同一进程内、相同 `(home, 归一化 days)` 的并发调用只做一次完整统计，
/// 后来者在锁上等待，拿到同一份结果（同一个 `generatedAt`）。窗口只有
/// [`MEMO_WINDOW`]，过期即重算——它是并发合并，不是缓存层。
///
/// 持锁跨整次扫描是**有意为之**：
/// - 两个宿主都在 `spawn_blocking` 里调用（`crates/wb-shift-server/src/api/handlers/stats.rs`
///   与 `src-tauri/src/commands/stats.rs`），这里阻塞的是 blocking 线程池里的一个线程，
///   不是 async 运行时线程，不会卡住其它请求；
/// - 只有这样才能让「后来者」真的等待并复用，而不是各扫一遍——先放锁再计算就等于没合并。
///
/// 锁是全局的：不同 `days` / 不同 `home` 的并发也会排队。这是有意的 CPU 保护
/// （同时扫盘更慢），而窗口只有 1.5 s、正常页面只用一个 `days`，影响可忽略。
fn get_statistics_at(home: &Path, days: Option<i64>) -> Value {
    let key = MemoKey {
        home: home.to_path_buf(),
        days: normalize_days(days),
    };
    // memo 是可重建的进程内缓存：中毒只说明上一个持锁线程 panic 过，里面最多留下
    // 一条陈旧/算了一半的结果，而下面未命中时会把整格覆盖掉——最坏情况是重算一次，
    // 不需要把 panic 传播给用户。故用 `locks::recover` 而非 `locks::guard`。
    let mut memo = locks::recover(statistics_memo());
    if let Some(entry) = memo.as_ref() {
        if entry.key == key && entry.at.elapsed() <= MEMO_WINDOW {
            // 命中：克隆一份返回，不在持锁状态下做任何额外工作。
            return entry.value.clone();
        }
    }
    // 未命中：持锁计算（理由见函数文档），算完整体覆盖 memo 再返回。
    let value = collect_statistics(home, key.days);
    *memo = Some(MemoEntry {
        key,
        value: value.clone(),
        at: Instant::now(),
    });
    value
}

/// 一次完整统计（原 `get_statistics` 的函数体，口径与响应形状均未改动）。
fn collect_statistics(home: &Path, days: Option<i64>) -> Value {
    #[cfg(test)]
    note_collect_for_test(home);
    let roots = stats_roots_at(home);
    let generated_at = crate::modules::config::now_ms();
    let range_days = normalize_days(days);
    let cutoff = range_days.map(|value| generated_at - value * 86_400_000);
    let ide_projects = ide_project_by_session();
    // 账号归因：一次扫描就把 per-account 数字算出来（第 7 条），切账号不重新扫盘。
    // DB 读不到时返回空表 → 全部「未归属」，绝不阻塞整次统计。
    let cn_uids = session_uid_index_at(&WbVariant::Cn.data_root_at(home));
    let ai_uids = session_uid_index_at(&WbVariant::Ai.data_root_at(home));
    // R6-07：归属索引每轮**只读一次**，两个 WorkBuddy 来源与三个 IDE 来源共用同一份。
    // 升级后第一次统计顺带做一次性回填（无 UI 入口、写 `backfilledAt` 后不再重复）。
    let store_root = usage_origin::store_root_at(home);
    backfill_once_at(&store_root);
    let mut attribution = Attribution::new(OriginIndex::load_at(&store_root));

    let workbuddy = source_from_roots(
        &existing_roots(&roots.workbuddy),
        "workbuddy",
        cutoff,
        false,
        &cn_uids,
        &mut attribution,
        Some(namespace_for_variant(WbVariant::Cn)),
    );
    // CN ↔ AI 共用同一份 workbuddy 指纹集合（扫描顺序固定 CN → AI）：跨档位复制/同步的
    // 同一段历史只计一次，归属取首次命中的段（否则跨档位合计会翻倍）。
    let workbuddy_ai = source_from_roots(
        &existing_roots(&roots.workbuddy_ai),
        "workbuddy-ai",
        cutoff,
        false,
        &ai_uids,
        &mut attribution,
        Some(namespace_for_variant(WbVariant::Ai)),
    );
    // 请求明细目前只对 CodeBuddy CLI 开放（见 spec 的 requests 契约）。
    // CLI 日志不按账号分目录、也没有账号字段 → 全部归「未归属」，指纹集合独立
    // （不记录账号，和 WorkBuddy 共用只会少计）。
    let cli = source(roots.codebuddy_cli, "codebuddy-cli", cutoff, true);
    // 第四轮 06：按客户端拆成三个 tab（同一套解析/去重/归因，只是扫描根收窄）。
    // `WorkBuddy/` 是 WorkBuddy 客户端内置扩展写的空壳，**不扫**：日常用量已经在
    // 国内版 JSONL 里，扫它会把同一份用量重复计一次。
    // R6-07：三个 IDE 来源共用一份 ide 指纹集合（同名 ids 不会跨客户端撞车）。
    let vscode_ext = ide_client_source(
        roots.codebuddy_ide.clone(),
        "VSCode",
        "vscode-ext",
        cutoff,
        &ide_projects,
        &mut attribution,
    );
    let jetbrains = ide_client_source(
        roots.codebuddy_ide.clone(),
        "JetBrains",
        "jetbrains",
        cutoff,
        &ide_projects,
        &mut attribution,
    );
    let codebuddy_ide = ide_client_source(
        roots.codebuddy_ide,
        "CodeBuddyIDE",
        "codebuddy-ide",
        cutoff,
        &ide_projects,
        &mut attribution,
    );
    // 全量统计之后顺带清一次死条目（条目对应的会话在所有客户端都不存在）。
    attribution.finish(&store_root);

    json!({
        "generatedAt": generated_at,
        "rangeDays": range_days,
        "sources": [workbuddy, workbuddy_ai, cli, vscode_ext, jetbrains, codebuddy_ide],
    })
}

/// 一次性回填的包一层：失败/已回填都不影响统计（`backfill_at` 内部只记日志）。
fn backfill_once_at(store_root: &Path) {
    let _ = usage_origin::backfill_at(store_root);
}

// ---------------------------------------------------------------------------
// 一次统计内的共享上下文（R6-07）
// ---------------------------------------------------------------------------

/// WorkBuddy JSONL 记录的指纹：`(时间戳, 输入, 输出, 读, 写, 模型)`。
///
/// 副本逐字重放父历史（时间戳与 usage 都不变），所以它可以直接当并集去重的键；
/// 模型名是 interned 的 `Arc<str>`，建键不额外分配。
pub(super) type WbFingerprint = (i64, u64, u64, u64, u64, Arc<str>);

/// 一次统计内的共享状态：归属索引 + 存活键 + 两份**跨来源共用**的指纹集合。
///
/// 共享是 R6-07 的核心：同一段历史散落在 CN/AI 两个档位（或 IDE 三个客户端）时，
/// 只有共用指纹集合才能「只计一次、归首次命中的账号」。
pub(super) struct Attribution {
    /// 归属索引（每轮只 load 一次，逐条记录按 ts 落段）。
    pub(super) origin: OriginIndex,
    /// 本轮在磁盘上真的见到的会话键（GC 用；空集合表示什么都没读到 → 跳过 GC）。
    pub(super) alive_keys: HashSet<String>,
    /// WorkBuddy 国内版 + 国际版共用。
    pub(super) workbuddy_seen: HashSet<WbFingerprint>,
    /// VS Code / JetBrains / CodeBuddy IDE 三个来源共用。
    pub(super) ide_seen: HashSet<String>,
}

impl Attribution {
    /// 用给定索引开一轮统计。
    pub(super) fn new(origin: OriginIndex) -> Self {
        Self {
            origin,
            alive_keys: HashSet::new(),
            workbuddy_seen: HashSet::new(),
            ide_seen: HashSet::new(),
        }
    }

    /// 空上下文（CLI 来源与单测用：没有归属、指纹集合独立）。
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn empty() -> Self {
        Self::new(OriginIndex::empty())
    }

    /// 记一个「这一轮确实存在」的会话键（GC 依据）。
    pub(super) fn note_alive(&mut self, namespace: &str, session_id: &str) {
        if !session_id.is_empty() {
            self.alive_keys.insert(key_of(namespace, session_id));
        }
    }

    /// 统计结束：GC 死条目并落盘。
    ///
    /// `alive_keys` 为空时**不 GC**：那说明这一轮一个会话都没读到（DB 被锁、目录被删、
    /// 沙箱为空…），据此删条目会把用户的归属一次性清空。
    fn finish(&mut self, store_root: &Path) {
        if self.alive_keys.is_empty() {
            return;
        }
        let removed = self.origin.gc(&self.alive_keys);
        if removed == 0 {
            return;
        }
        if let Err(error) = self.origin.save_at(store_root) {
            eprintln!("[usage-origin] 归属索引清理结果保存失败：{error}");
        }
    }

    /// 记录级归属：命中段就用段里的 uid，否则回退文件自身 uid。
    fn uid_for(
        &self,
        namespace: &str,
        session_id: &str,
        ts: Option<i64>,
        file_uid: &str,
    ) -> String {
        self.origin
            .attribute(namespace, session_id, ts)
            .unwrap_or_else(|| file_uid.to_string())
    }

    /// IDE 来源的命名空间：未知客户端返回 `None`（不做覆盖，回退文件 uid）。
    fn ide_namespace(client: &str) -> Option<&'static str> {
        IdeClientSource::from_client_dir(client).map(|source| source.namespace())
    }
}

/// 单测用的「完整统计次数」计数器（按 home 路径计数）。
///
/// 不能复用 `cache::PARSE_COUNTS`：文件解析缓存会让第二次统计也「零解析」，
/// 解析次数无法区分「并发合并成一次」和「扫了两遍但都命中文件缓存」。按路径计数
/// 而不是全局计数：单测并行跑，别的用例统计别的目录不该影响这里的断言。
#[cfg(test)]
static COLLECT_COUNTS: Mutex<Option<HashMap<PathBuf, usize>>> = Mutex::new(None);

#[cfg(test)]
fn note_collect_for_test(home: &Path) {
    let mut counts = COLLECT_COUNTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *counts
        .get_or_insert_with(HashMap::new)
        .entry(home.to_path_buf())
        .or_default() += 1;
}

#[cfg(test)]
fn collect_count(home: &Path) -> usize {
    COLLECT_COUNTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .and_then(|counts| counts.get(home).copied())
        .unwrap_or(0)
}

/// 单测用的串行守卫：memo 是**进程级单格**全局状态，并行用例会互相覆盖对方的条目，
/// 从而让「只算一次」的断言随机变红。所有触碰 `get_statistics*` 的用例都要先取它。
///
/// 它只是测试自身的互斥，中毒时取回内部值继续即可（`locks::recover` 的适用条件）。
#[cfg(test)]
fn memo_test_guard() -> MutexGuard<'static, ()> {
    static MEMO_TEST_GUARD: Mutex<()> = Mutex::new(());
    locks::recover(&MEMO_TEST_GUARD)
}

use cache::*;
use ide::*;
use jsonl::*;
use projection::*;

mod cache;
mod ide;
mod jsonl;
mod projection;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    include!("tests/fixtures.rs");
    include!("tests/decode.rs");
    include!("tests/detail.rs");
    include!("tests/accounts.rs");
    include!("tests/ide_roots.rs");
    include!("tests/origin.rs");
    include!("tests/cache.rs");
    include!("tests/memo.rs");
}
