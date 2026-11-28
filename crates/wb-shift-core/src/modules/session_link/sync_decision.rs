//! 关联会话的纯同步判定规则。

use super::content::{is_strict_ordered_extension, ContentState};
use super::link_store::BaselineState;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// 同步判定（design §3.2）
/// 同步判定结果（design §3.2 优先级表的取值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncVerdict {
    /// 双方有序内容一致：不写正文。
    Identical,
    /// 目标内容被来源完整包含（目标为来源的严格有序前缀）：默认勾选快进。
    FastForward,
    /// 仅目标变化（来源等于共同基线），或来源是目标的严格有序前缀：
    /// 普通同步不写目标；组级统一可显式覆盖。
    Ahead,
    /// 双方都有变化，或来源重写/重排/压缩：默认不勾，可显式覆盖。
    Diverge,
    /// 成员/文件无效、内容不可验证，或双方互不为严格有序前缀且缺可验证基线：禁止同步。
    Unknown,
}

impl SyncVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            SyncVerdict::Identical => "identical",
            SyncVerdict::FastForward => "fastForward",
            SyncVerdict::Ahead => "ahead",
            SyncVerdict::Diverge => "diverge",
            SyncVerdict::Unknown => "unknown",
        }
    }

    /// 普通同步是否允许勾选；组级统一的显式覆盖独立于此。
    /// 单测用判定辅助（生产路径直接 match verdict，不走该 helper）。
    #[cfg(test)]
    pub fn is_actionable(self) -> bool {
        matches!(self, SyncVerdict::FastForward | SyncVerdict::Diverge)
    }

    /// 该判定允许的写入模式。unknown 不匹配任何模式——覆盖不能绕过未知。
    ///
    /// `StatusOnly` 恒为 false：它不属于任何正文判定，只能由「来源已归档 + 目标处于
    /// 终态」这一独立资格授权，走 `plan_sync_selection` 的单独分支，不得混进正文门禁。
    pub fn allows(self, mode: SyncMode) -> bool {
        match mode {
            SyncMode::FastForward => self == SyncVerdict::FastForward,
            SyncMode::Overwrite => self == SyncVerdict::Diverge,
            SyncMode::UnifyOverwrite => self == SyncVerdict::Ahead,
            SyncMode::StatusOnly => false,
        }
    }

    /// 前端可选的写入模式（空表示不可勾选）。
    ///
    /// 只描述**正文写入**模式；仅同步归档不进这个数组，由预览项的 `archiveAction`
    /// 单独表达，避免把状态动作误认成正文覆盖权限。
    pub fn available_modes(self) -> Vec<SyncMode> {
        match self {
            SyncVerdict::FastForward => vec![SyncMode::FastForward],
            SyncVerdict::Diverge => vec![SyncMode::Overwrite],
            _ => Vec::new(),
        }
    }
}

/// 同步写入模式（design §6 的 `mode`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncMode {
    /// 目标未偏离基线时，把来源的新增记录追加到目标（默认勾选）。
    FastForward,
    /// 用户显式选择的覆盖：必须仍为有效可比较的冲突。
    Overwrite,
    /// 用户选择整组保留来源副本时，明确覆盖仅目标有改动的副本。
    UnifyOverwrite,
    /// 仅同步会话生命周期状态（把目标对齐为已归档），正文零写入。
    ///
    /// 不进 `available_modes`：它的资格来自双方会话行的归档状态，与正文判定无关。
    StatusOnly,
}

impl SyncMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SyncMode::FastForward => "fastForward",
            SyncMode::Overwrite => "overwrite",
            SyncMode::UnifyOverwrite => "unifyOverwrite",
            SyncMode::StatusOnly => "statusOnly",
        }
    }

    /// 解析前端传入的模式；未知值直接拒绝，不回落默认值。
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim() {
            "fastForward" => Ok(SyncMode::FastForward),
            "overwrite" => Ok(SyncMode::Overwrite),
            "unifyOverwrite" => Ok(SyncMode::UnifyOverwrite),
            "statusOnly" => Ok(SyncMode::StatusOnly),
            other => Err(format!("未知的同步模式：{other}")),
        }
    }
}

/// 判定结果：verdict、默认勾选与解释性记录数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncDecision {
    pub verdict: SyncVerdict,
    /// 来源独有记录数（多重集差集）。只用于向用户解释，不参与判定。
    pub extra_a: usize,
    /// 目标独有记录数（多重集差集）。只用于向用户解释，不参与判定。
    pub extra_b: usize,
    /// 双方共有记录数（多重集交集）。只用于向用户解释，不参与判定。
    pub common: usize,
    /// 是否默认勾选：只有 fastForward 为 true。
    pub default_checked: bool,
    pub reason: String,
}

impl SyncDecision {
    /// 禁止同步：任何模式都不得写入。
    pub fn unknown(reason: impl Into<String>) -> Self {
        Self {
            verdict: SyncVerdict::Unknown,
            extra_a: 0,
            extra_b: 0,
            common: 0,
            default_checked: false,
            reason: reason.into(),
        }
    }

    fn decide(verdict: SyncVerdict, counts: (usize, usize, usize), reason: String) -> Self {
        Self {
            verdict,
            extra_a: counts.0,
            extra_b: counts.1,
            common: counts.2,
            default_checked: verdict == SyncVerdict::FastForward,
            reason,
        }
    }
}

/// 逐行摘要的多重集差集：`extra_a` 来源独有、`extra_b` 目标独有、`common` 双方共有。
///
/// 只用于向用户解释记录数（design §3.2）：即使 `extra_b == 0` 也不代表顺序与语义无损，
/// 因此本函数的结果不得参与自动勾选——没有「共同占比」之类的阈值判定。
fn multiset_counts(source: &[String], target: &[String]) -> (usize, usize, usize) {
    let mut remaining: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for digest in target {
        *remaining.entry(digest.as_str()).or_insert(0) += 1;
    }
    let mut common = 0usize;
    for digest in source {
        if let Some(count) = remaining.get_mut(digest.as_str()) {
            if *count > 0 {
                *count -= 1;
                common += 1;
            }
        }
    }
    (source.len() - common, target.len() - common, common)
}

/// 镜像前缀（目标已完整包含来源）的 ahead 文案：本次没有可写内容，并给出目标多出的条数。
fn mirror_ahead_reason(counts: (usize, usize, usize)) -> String {
    format!(
        "目标账号已包含当前账号的全部内容，另多出 {} 条，本次不会同步过去",
        counts.1
    )
}

/// 判定来源 A 与目标 B 能否同步（design §3.2，顺序不可调换）。
///
/// 1. 成员/文件无效或内容不可验证 → [`SyncVerdict::Unknown`]；
/// 2. A 与 B 有序一致 → [`SyncVerdict::Identical`]；
/// 3. B 是 A 的严格有序前缀 → [`SyncVerdict::FastForward`]（不依赖基线，
///    对齐 git fast-forward 的 ancestor 语义：目标内容被来源完整包含，追加同步零覆盖）；
/// 4. 无可验证共同基线：A 是 B 的严格有序前缀 → [`SyncVerdict::Ahead`]（目标已完整包含来源，
///    普通同步没有可写内容；同样不依赖基线、不授权任何写入），其余 → [`SyncVerdict::Unknown`]；
/// 5. A 等于基线、B 已变化 → [`SyncVerdict::Ahead`]；
/// 6. A 是 B 的严格有序前缀、B 已偏离基线 → [`SyncVerdict::Ahead`]（镜像规则；B 恰好停在
///    基线属于来源侧被删减/回滚，保持下一条的显式覆盖入口）；
/// 7. 其余（双方变化、来源重写/重排/压缩）→ [`SyncVerdict::Diverge`]。
///
/// 第 2～4、6 条的前缀/相等比较都走 `comparable_line_digests`（比较口径，见
/// [`super::content`] 模块注释）：**只**把「复制/同步写目标时对末条 assistant 消息做的
/// `incomplete` 归一留痕」折算成等价形式。归一本身是复制与同步的既定行为，副本因此与
/// 来源的原始摘要不同，但那不是用户改动——按原始摘要逐行硬比会把这类副本永久判成
/// `Diverge`（issue #3 最后一条：点「以此为准」也消不掉「存在分歧」）。约定：
/// 两边只差这条归一签名时判 [`SyncVerdict::Identical`]。
///
/// 第 5、6 条里与**基线**的比较仍走原始 `line_digests`：基线存的是写入当时的原始摘要，
/// 换成比较口径就等于放宽「目标是否已偏离基线」，这是本次刻意不改的部分。
///
/// 侧别称谓：判定原因与「内容不可验证」的提示共用同一套词，避免两处各说一套。
pub const SOURCE_ACCOUNT_LABEL: &str = "当前账号";
pub const TARGET_ACCOUNT_LABEL: &str = "目标账号";

/// 单侧内容状态的可读说明；`account` 只取上面两个常量之一。
///
/// 桌面端与 VS Code 插件端的同步入口共用这一份：同一个故障在两个入口必须是同一句话，
/// 否则用户换一个入口就认不出是同一个问题，排障时也会被措辞差异带偏。
pub(crate) fn side_content_reason(account: &str, state: &ContentState) -> String {
    match state {
        ContentState::Ready(_) => format!("{account}的内容可验证"),
        ContentState::Missing => format!("{account}的内容不存在，无法确认"),
        ContentState::Unavailable(reason) => format!("{account}的内容无法确认：{reason}"),
    }
}

/// 两侧内容状态的可读说明，供「内容不可验证」的兜底文案说明**是哪一侧**、**为什么**。
///
/// 只写确实不可验证的那一侧：两侧都不 Ready 时两侧都写，只有一侧不 Ready 时只写那一侧，
/// 免得把「另一侧没问题」也写进拒绝理由里误导用户。两侧都可验证时返回 `None`——调用方
/// 据此**不追加任何说明**：那种情况只是内容变多变少，没有「哪一侧坏了」可讲，多说反而误导。
pub(crate) fn unverifiable_sides(source: &ContentState, target: &ContentState) -> Option<String> {
    let mut parts: Vec<String> = Vec::with_capacity(2);
    if !matches!(source, ContentState::Ready(_)) {
        parts.push(format!(
            "来源：{}",
            side_content_reason(SOURCE_ACCOUNT_LABEL, source)
        ));
    }
    if !matches!(target, ContentState::Ready(_)) {
        parts.push(format!(
            "目标：{}",
            side_content_reason(TARGET_ACCOUNT_LABEL, target)
        ));
    }
    (!parts.is_empty()).then(|| parts.join("；"))
}

/// 纯函数：不读文件、不写文件、无时间依赖。
pub fn decide_sync(
    source: &ContentState,
    target: &ContentState,
    baseline: &BaselineState,
) -> SyncDecision {
    let (source, target) = match (source, target) {
        (ContentState::Ready(source), ContentState::Ready(target)) => (source, target),
        (ContentState::Missing, _) => {
            return SyncDecision::unknown(side_content_reason(SOURCE_ACCOUNT_LABEL, source))
        }
        (ContentState::Unavailable(_), _) => {
            return SyncDecision::unknown(side_content_reason(SOURCE_ACCOUNT_LABEL, source))
        }
        (_, ContentState::Missing) => {
            return SyncDecision::unknown(side_content_reason(TARGET_ACCOUNT_LABEL, target))
        }
        (_, ContentState::Unavailable(_)) => {
            return SyncDecision::unknown(side_content_reason(TARGET_ACCOUNT_LABEL, target))
        }
    };
    // 计数是给用户看的**解释**，因此与判定同一口径（比较口径）：否则判成一致（identical）的组
    // 仍会显示「目标账号独有 1 条」，自相矛盾。没有归一签名的内容两者逐元素相等，既有计数不变。
    let counts = multiset_counts(
        &source.normalized.comparable_line_digests,
        &target.normalized.comparable_line_digests,
    );

    // 比较口径：原始摘要不同、但只差「末条归一签名」时仍判一致（见函数文档第 2 条）。
    if source.normalized.comparable_line_digests == target.normalized.comparable_line_digests {
        // 两侧原始摘要也相同时沿用原文案；只在新增的等价路径上用能自解释的那句，
        // 让用户明白「副本末条被复制规则归一过」不是分歧。
        let reason = if source.normalized.line_digests == target.normalized.line_digests {
            format!(
                "两边的内容一致（各 {} 条），不需要同步",
                source.normalized.record_count
            )
        } else {
            format!(
                "两边内容一致（副本末条已按复制规则归一，各 {} 条），不需要同步",
                source.normalized.record_count
            )
        };
        return SyncDecision::decide(SyncVerdict::Identical, counts, reason);
    }
    // 目标内容全部包含在来源里（B 是 A 的严格有序前缀）：追加同步零覆盖，
    // 不依赖基线即可判定（对齐 git fast-forward 的 ancestor 语义）。
    if is_strict_ordered_extension(
        &target.normalized.comparable_line_digests,
        &source.normalized.comparable_line_digests,
    ) {
        let added = source.normalized.record_count - target.normalized.record_count;
        return SyncDecision::decide(
            SyncVerdict::FastForward,
            counts,
            format!("目标账号没有独有改动，来源账号新增 {added} 条，可以直接同步"),
        );
    }
    let Some(record) = baseline.ready() else {
        // 目标已完整包含来源（A ⊊ B 的镜像）：普通同步没有可写内容，
        // 与祖先快进同一原则，不依赖基线即可判定，且不授权任何写入。
        if is_strict_ordered_extension(
            &source.normalized.comparable_line_digests,
            &target.normalized.comparable_line_digests,
        ) {
            return SyncDecision::decide(SyncVerdict::Ahead, counts, mirror_ahead_reason(counts));
        }
        return SyncDecision::unknown(
            baseline
                .unusable_reason()
                .unwrap_or_else(|| "找不到上次同步的记录，无法确认两边内容".to_string()),
        );
    };
    // 基线比较用原始摘要：基线存的是写入当时的原始字节摘要（口径不随判定放宽）。
    if source.normalized.line_digests == record.line_digests {
        return SyncDecision::decide(
            SyncVerdict::Ahead,
            counts,
            format!("只有目标账号新增 {} 条，这次不会同步过去", counts.1),
        );
    }
    // A 是 B 的严格有序前缀（目标已完整包含来源）：本次没有可写内容 → ahead。
    // B 恰好停在基线 → 是来源侧被删减/回滚，维持 diverge 的显式覆盖入口。
    if is_strict_ordered_extension(
        &source.normalized.comparable_line_digests,
        &target.normalized.comparable_line_digests,
    ) && target.normalized.line_digests != record.line_digests
    {
        return SyncDecision::decide(SyncVerdict::Ahead, counts, mirror_ahead_reason(counts));
    }
    SyncDecision::decide(
        SyncVerdict::Diverge,
        counts,
        format!(
            "两边都有改动（目标账号独有的 {} 条会被替换）；覆盖会替换目标账号的完整内容",
            counts.1
        ),
    )
}
