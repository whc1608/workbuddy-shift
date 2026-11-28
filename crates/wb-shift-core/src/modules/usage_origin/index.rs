//! 归属索引的落盘与规范化：序列化格式、原子写、段合并/截断、条目上限淘汰。

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::modules::config;

use super::{Entry, OriginIndex, Segment, MAX_ENTRIES, MAX_SEGMENTS, VERSION};

/// 落盘结构与版本号。
///
/// 字段名与跨轨道契约（清单 0.1.11 第 2 条）逐字一致：`version` / `entries` /
/// `segments[].untilMs` / `segments[].uid` / `updatedAt` / `heuristic` / `truncated`。
/// `backfilledAt` 是回填的一次性标记（同文件、顶层）。
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileShape {
    version: u32,
    #[serde(default)]
    entries: BTreeMap<String, Entry>,
    #[serde(default)]
    backfilled_at: Option<i64>,
}

/// 读索引文件；缺失 / 不可读 / 损坏 / 版本不符 → 空索引。
///
/// 读到的条目会就地规范化（段排序、相邻同 uid 合并、超限截断）：手改过的文件不应该
/// 让统计进入「段乱序导致归属跳段」的诡异状态。
pub(super) fn load_file(path: &Path) -> OriginIndex {
    let Ok(text) = std::fs::read_to_string(path) else {
        return OriginIndex::empty();
    };
    let Ok(shape) = serde_json::from_str::<FileShape>(&text) else {
        eprintln!(
            "[usage-origin] 归属索引无法解析，已按空索引处理：{}",
            path.display()
        );
        return OriginIndex::empty();
    };
    if shape.version != VERSION {
        eprintln!(
            "[usage-origin] 归属索引版本 {} 不受支持（期望 {VERSION}），已按空索引处理",
            shape.version
        );
        return OriginIndex::empty();
    }
    let mut entries: BTreeMap<String, Entry> = BTreeMap::new();
    for (key, entry) in shape.entries {
        if key.trim().is_empty() {
            continue;
        }
        let (segments, truncated) = normalize_segments(&entry.segments);
        if segments.is_empty() {
            continue;
        }
        entries.insert(
            key,
            Entry {
                segments,
                updated_at: entry.updated_at,
                heuristic: entry.heuristic,
                truncated: truncated || entry.truncated,
            },
        );
    }
    trim_entries(&mut entries, MAX_ENTRIES);
    OriginIndex::from_parts(entries, shape.backfilled_at)
}

/// 原子写索引文件（临时文件 + rename，见 [`config::atomic_write`]）。
pub(super) fn save_file(path: &Path, index: &OriginIndex) -> Result<(), String> {
    let mut entries = index.entries().clone();
    trim_entries(&mut entries, MAX_ENTRIES);
    let shape = FileShape {
        version: VERSION,
        entries,
        backfilled_at: index.backfilled_at(),
    };
    let text = serde_json::to_string(&shape).map_err(|error| error.to_string())?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
    }
    config::atomic_write(path, &text).map_err(|error| error.to_string())
}

/// 段规范化：丢掉空 uid → 按 `untilMs` 升序 → 相邻同 uid 合并 → 超限降级为单 boundary。
///
/// 第二个返回值是「是否发生了截断」，写进条目的 `truncated` 标记。
///
/// 乱序输入（时钟回拨 / 手改文件）也在这里收敛：段的 `untilMs` 单调不减，后一段不会
/// 覆盖前一段，保证「按 ts 取第一个命中段」的查询语义不会跳段。
pub(super) fn normalize_segments(segments: &[Segment]) -> (Vec<Segment>, bool) {
    let mut sorted: Vec<Segment> = segments
        .iter()
        .filter(|segment| !segment.uid.trim().is_empty())
        .map(|segment| Segment {
            until_ms: segment.until_ms,
            uid: segment.uid.trim().to_string(),
        })
        .collect();
    sorted.sort_by_key(|segment| segment.until_ms);
    let mut merged: Vec<Segment> = Vec::with_capacity(sorted.len());
    for segment in sorted {
        match merged.last_mut() {
            Some(last) if last.uid == segment.uid => {
                // 相邻同 uid：合并成一段（上界取较大值）。
                last.until_ms = last.until_ms.max(segment.until_ms);
            }
            Some(last) => {
                let until_ms = segment.until_ms.max(last.until_ms);
                merged.push(Segment {
                    until_ms,
                    uid: segment.uid,
                });
            }
            None => merged.push(segment),
        }
    }
    if merged.len() <= MAX_SEGMENTS {
        return (merged, false);
    }
    // 超限：取最后一段（最新归属）作为单 boundary，历史段整体让位——不猜中间归属。
    let collapsed = merged.pop().map(|last| vec![last]).unwrap_or_default();
    (collapsed, true)
}

/// 条目上限淘汰：按 `updatedAt` 升序删最旧的，直到不超过 `limit`。
pub(super) fn trim_entries(entries: &mut BTreeMap<String, Entry>, limit: usize) {
    if entries.len() <= limit {
        return;
    }
    let mut order: Vec<(i64, &str)> = entries
        .iter()
        .map(|(key, entry)| (entry.updated_at, key.as_str()))
        .collect();
    order.sort();
    let remove = entries.len() - limit;
    let doomed: Vec<String> = order
        .into_iter()
        .take(remove)
        .map(|(_, key)| key.to_string())
        .collect();
    for key in doomed {
        entries.remove(&key);
    }
}
