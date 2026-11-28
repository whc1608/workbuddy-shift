//! 一次性回填：把历史上已经发生过的复制/同步关系转成归属段。
//!
//! 索引是本轮新增的，升级用户的历史副本没有任何段。这里有两条数据源：
//!
//! 1. **操作日志**（`session-links/operations/*.json`，每档位保留最近 20 条）：按
//!    `updatedAt` 从旧到新链式继承，`A→B`、`B→C` 能一路建出 `[(T1,A),(T2,B)]`；
//! 2. **关联组**（四份 link store）：没有操作日志更早的复制只能靠启发式——组内
//!    `linkedAt` 最早的成员视为来源，其余成员在各自 `linkedAt` 之前的历史归它，条目标
//!    `heuristic: true`（数据不如真实继承可靠，只作兜底）。
//!
//! 幂等：成功后写 `backfilledAt`，此后不再跑；回填不到（无日志、无组、解析失败）时
//! 不报错、也不写标记以外的东西——归属缺失一律回退「文件 uid」的老行为。

use std::path::{Path, PathBuf};

use crate::modules::config;
use crate::modules::session::{LinkNamespace, SessionPaths};
use crate::modules::session_link::{self, OpPhase, Operation, StoreState};
use crate::modules::variant::WbVariant;

use super::{namespace_for_ide, namespace_for_variant, store_root, IdeClientSource, OriginIndex};

/// 一条复制/同步操作里与本模块相关的事实（从 [`Operation`] 投影而来）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationFact {
    /// 目标侧键命名空间。
    pub target_ns: &'static str,
    /// 目标会话 id。
    pub target_sid: String,
    /// 来源侧键命名空间。
    pub source_ns: &'static str,
    /// 来源会话 id。
    pub source_sid: String,
    /// 本次写入的 boundary（操作完成时间）。
    pub boundary_ms: i64,
    /// 来源会话当前的归属账号。
    pub source_uid: String,
    /// 操作阶段：只有真正写过目标正文的才值得继承。
    pub phase: OpPhase,
}

impl OperationFact {
    /// 这条操作是否真的写过目标正文（`Prepared`/`Abandoned` 什么都没写）。
    fn wrote_target(&self) -> bool {
        matches!(
            self.phase,
            OpPhase::BodyWritten
                | OpPhase::DbWritten
                | OpPhase::MappingWritten
                | OpPhase::LinksCommitted
                | OpPhase::Completed
        )
    }
}

/// 启发式补洞的一条事实（组内某成员「在某时刻之前的历史归来源账号」）。
#[derive(Clone, Debug, PartialEq, Eq)]
struct HeuristicTip {
    namespace: &'static str,
    session_id: String,
    boundary_ms: i64,
    source_uid: String,
}

/// 首次启动（首次全量统计）时调用：已经回填过则直接返回 `None`。
///
/// 返回 `Some(写入条目数)` 表示本次真的跑过并落盘；`None` 表示跳过（已回填 / 写失败）。
pub fn backfill_once() -> Option<usize> {
    backfill_at(&store_root())
}

/// [`backfill_once`] 的注入式实现（家目录被重定向时按注入根回填）。
pub fn backfill_at(store_root: &Path) -> Option<usize> {
    let mut index = OriginIndex::load_at(store_root);
    if index.backfilled_at().is_some() {
        return None;
    }
    let mut applied = 0usize;
    // 1) 操作日志：链式继承（按 boundary 从旧到新，后一条能吃到前一条的段）。
    for fact in operation_facts(store_root) {
        if !fact.wrote_target() {
            continue;
        }
        if index.apply_operation(
            fact.target_ns,
            &fact.target_sid,
            fact.source_ns,
            &fact.source_sid,
            fact.boundary_ms,
            &fact.source_uid,
        ) {
            applied += 1;
        }
    }
    // 2) 关联组：更早的复制没有日志，用「组内最早成员 + linkedAt」补一段并标 heuristic。
    for tip in heuristic_tips(store_root) {
        if index.inherit_heuristic(
            tip.namespace,
            &tip.session_id,
            tip.boundary_ms,
            &tip.source_uid,
        ) {
            applied += 1;
        }
    }
    index.mark_backfilled(config::now_ms());
    if let Err(error) = index.save_at(store_root) {
        // 回填是尽力而为：写不进去就下一次启动再试，绝不影响其它功能。
        eprintln!("[usage-origin] 回填结果保存失败：{error}");
        return None;
    }
    Some(applied)
}

/// 从操作日志投影出回填事实（按 boundary 从旧到新排序）。
fn operation_facts(store_root: &Path) -> Vec<OperationFact> {
    let paths = paths_for(store_root, LinkNamespace::WorkBuddy);
    let scan = session_link::scan_operations(&paths);
    if !scan.complete {
        // 扫描不完整时仍然用拿到的部分：宁可少回填，也不能因为一个坏文件整体跳过。
        eprintln!(
            "[usage-origin] 操作记录扫描不完整，回填仅使用 {} 条记录",
            scan.operations.len()
        );
    }
    let mut facts: Vec<OperationFact> = scan
        .operations
        .iter()
        .filter_map(fact_of_operation)
        .collect();
    facts.sort_by(|left, right| {
        left.boundary_ms
            .cmp(&right.boundary_ms)
            .then_with(|| left.target_sid.cmp(&right.target_sid))
    });
    facts
}

/// 单条操作 → 回填事实。只认复制/同步（其它 kind 不会写目标会话）。
fn fact_of_operation(operation: &Operation) -> Option<OperationFact> {
    if !matches!(operation.kind.as_str(), "copy" | "sync") {
        return None;
    }
    if operation.target.session_id.trim().is_empty()
        || operation.source.session_id.trim().is_empty()
    {
        return None;
    }
    let boundary_ms = if operation.updated_at > 0 {
        operation.updated_at
    } else {
        operation.created_at
    };
    if boundary_ms <= 0 {
        return None;
    }
    let source_variant = operation.source_variant.unwrap_or(operation.variant);
    Some(OperationFact {
        target_ns: namespace_for_variant(operation.variant),
        target_sid: operation.target.session_id.clone(),
        source_ns: namespace_for_variant(source_variant),
        source_sid: operation.source.session_id.clone(),
        boundary_ms,
        source_uid: operation.source.uid.clone(),
        phase: operation.phase,
    })
}

/// 从四份 link store 收集启发式事实。
fn heuristic_tips(store_root: &Path) -> Vec<HeuristicTip> {
    let mut tips: Vec<HeuristicTip> = Vec::new();
    for link_namespace in [
        LinkNamespace::WorkBuddy,
        LinkNamespace::VscodeExt,
        LinkNamespace::CodeBuddyIde,
        LinkNamespace::Jetbrains,
    ] {
        let paths = paths_for(store_root, link_namespace);
        let StoreState::Ready(store) = session_link::load_store(&paths) else {
            // 首次使用 / 保留现场：没有可回填的数据，不是错误。
            continue;
        };
        for group in &store.groups {
            // 组内最早建立关联的成员视为「原始来源」；同刻按 member_id 定序，保证可重复。
            let Some(origin) = group.members.iter().min_by(|left, right| {
                left.linked_at
                    .cmp(&right.linked_at)
                    .then_with(|| left.member_id.cmp(&right.member_id))
            }) else {
                continue;
            };
            if origin.uid.trim().is_empty() {
                continue;
            }
            for member in &group.members {
                if member.member_id == origin.member_id || member.uid.trim().is_empty() {
                    continue;
                }
                // boundary 取该成员的 `linkedAt`（副本落地时刻）：之前的记录属于来源。
                // 旧存储缺 linkedAt 时退回 `lastSyncedAt`；两者都没有就不猜。
                let boundary_ms = if member.linked_at > 0 {
                    member.linked_at
                } else {
                    member.last_synced_at.unwrap_or(0)
                };
                if boundary_ms <= 0 || member.session_id.trim().is_empty() {
                    continue;
                }
                tips.push(HeuristicTip {
                    namespace: namespace_of(
                        link_namespace,
                        session_link::member_variant(group, member),
                    ),
                    session_id: member.session_id.clone(),
                    boundary_ms,
                    source_uid: origin.uid.clone(),
                });
            }
        }
    }
    tips.sort_by(|left, right| {
        left.boundary_ms
            .cmp(&right.boundary_ms)
            .then_with(|| left.namespace.cmp(right.namespace))
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    tips
}

/// 关联存储命名空间 + 成员档位 → 键命名空间。
fn namespace_of(link_namespace: LinkNamespace, variant: WbVariant) -> &'static str {
    match link_namespace {
        LinkNamespace::WorkBuddy => namespace_for_variant(variant),
        LinkNamespace::VscodeExt => namespace_for_ide(IdeClientSource::VSCode),
        LinkNamespace::CodeBuddyIde => namespace_for_ide(IdeClientSource::CodeBuddyIde),
        LinkNamespace::Jetbrains => namespace_for_ide(IdeClientSource::JetBrains),
    }
}

/// 只带 `store_root` 的 [`SessionPaths`]：回填只读存储根下的操作日志与关联表，
/// 不读档位数据根（会话文件不参与回填）。
fn paths_for(store_root: &Path, link_namespace: LinkNamespace) -> SessionPaths {
    SessionPaths {
        store_root: store_root.to_path_buf(),
        data_root: PathBuf::new(),
        auth_file: PathBuf::new(),
        link_namespace,
    }
}
