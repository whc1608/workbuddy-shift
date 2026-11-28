//! 归属索引单测：段合并/截断、淘汰、损坏降级、链式继承与一次性回填。

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::*;

fn temp_root(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "wb_shift_usage_origin_{}_{}_{}",
        std::process::id(),
        crate::modules::config::now_ms(),
        tag
    ));
    fs::create_dir_all(&base).expect("create temp root");
    base
}

fn uid(index: &OriginIndex, namespace: &str, session: &str, ts: i64) -> Option<String> {
    index.attribute(namespace, session, Some(ts))
}

/// 链式继承：A→B→C 能建出 `[(T1,A),(T2,B)]`，且各段时间点归属正确。
#[test]
fn inherit_chains_through_three_accounts() {
    let mut index = OriginIndex::empty();
    const T1: i64 = 1_790_000_000_000;
    const T2: i64 = 1_790_500_000_000;
    assert!(index.inherit(NS_WORKBUDDY_CN, "b", NS_WORKBUDDY_CN, "a", T1, "A"));
    assert!(index.inherit(NS_WORKBUDDY_CN, "c", NS_WORKBUDDY_CN, "b", T2, "B"));

    let entry = index.entry(NS_WORKBUDDY_CN, "c").expect("entry for c");
    assert_eq!(
        entry
            .segments
            .iter()
            .map(|segment| (segment.until_ms, segment.uid.as_str()))
            .collect::<Vec<_>>(),
        vec![(T1, "A"), (T2, "B")]
    );
    // 三段时间点分别命中三个账号：A 的历史归 A、B 自己的归 B、C 之后回退文件 uid。
    assert_eq!(
        uid(&index, NS_WORKBUDDY_CN, "c", T1 - 1).as_deref(),
        Some("A")
    );
    assert_eq!(uid(&index, NS_WORKBUDDY_CN, "c", T1).as_deref(), Some("A"));
    assert_eq!(uid(&index, NS_WORKBUDDY_CN, "c", T2).as_deref(), Some("B"));
    assert_eq!(uid(&index, NS_WORKBUDDY_CN, "c", T2 + 1), None);
}

/// 重复同步替换而不是追加：段不增长、也不重复计数。
#[test]
fn repeated_inherit_replaces_target_segments() {
    let mut index = OriginIndex::empty();
    assert!(index.inherit(NS_WORKBUDDY_CN, "b", NS_WORKBUDDY_CN, "a", 10, "A"));
    let before = index.entry(NS_WORKBUDDY_CN, "b").expect("entry").clone();
    // 第二次同步：boundary 更晚、来源仍是 A。
    assert!(index.inherit(NS_WORKBUDDY_CN, "b", NS_WORKBUDDY_CN, "a", 20, "A"));
    let after = index.entry(NS_WORKBUDDY_CN, "b").expect("entry").clone();
    assert_eq!(after.segments.len(), 1, "相邻同 uid 必须合并成一段");
    assert_eq!(after.segments[0].until_ms, 20);
    assert_ne!(before, after);
    // 完全相同的继承：不视为改动（避免重复落盘）。
    assert!(!index.inherit(NS_WORKBUDDY_CN, "b", NS_WORKBUDDY_CN, "a", 20, "A"));
}

/// 反向同步 B→A：A 的原始历史仍归 A。
#[test]
fn reverse_sync_keeps_original_history_with_its_account() {
    let mut index = OriginIndex::empty();
    // A → B（t1），此后 B 自己用了一段 → A 反向同步 B（t2）。
    assert!(index.inherit(NS_WORKBUDDY_CN, "b", NS_WORKBUDDY_CN, "a", 100, "A"));
    assert!(index.inherit(NS_WORKBUDDY_CN, "a", NS_WORKBUDDY_CN, "b", 200, "B"));
    let entry = index.entry(NS_WORKBUDDY_CN, "a").expect("entry");
    assert_eq!(
        entry
            .segments
            .iter()
            .map(|segment| (segment.until_ms, segment.uid.as_str()))
            .collect::<Vec<_>>(),
        vec![(100, "A"), (200, "B")]
    );
    assert_eq!(uid(&index, NS_WORKBUDDY_CN, "a", 50).as_deref(), Some("A"));
    assert_eq!(uid(&index, NS_WORKBUDDY_CN, "a", 150).as_deref(), Some("B"));
    // 反向同步之后 A 自己继续用：晚于 boundary 的记录回退文件 uid（A）。
    assert_eq!(uid(&index, NS_WORKBUDDY_CN, "a", 201), None);
}

/// 段数超过上限 → 降级为单 boundary 并标 `truncated`。
#[test]
fn segments_beyond_limit_collapse_and_mark_truncated() {
    let mut index = OriginIndex::empty();
    let total = MAX_SEGMENTS + 10;
    let segments: Vec<Segment> = (0..total)
        .map(|step| Segment {
            until_ms: 1_000 + step as i64,
            uid: format!("uid-{step}"),
        })
        .collect();
    // 直接经由 inherit 走规范化：来源条目先手工灌进索引。
    let source = Entry {
        segments: segments.clone(),
        updated_at: 1,
        heuristic: false,
        truncated: false,
    };
    index.entries.insert(key_of(NS_WORKBUDDY_CN, "src"), source);
    assert!(index.inherit(
        NS_WORKBUDDY_CN,
        "dst",
        NS_WORKBUDDY_CN,
        "src",
        9_999,
        "last"
    ));
    let entry = index.entry(NS_WORKBUDDY_CN, "dst").expect("entry");
    assert!(entry.truncated, "超过 {MAX_SEGMENTS} 段必须标记 truncated");
    assert_eq!(entry.segments.len(), 1);
    assert_eq!(entry.segments[0].until_ms, 9_999);
    assert_eq!(entry.segments[0].uid, "last");

    // 乱序输入也被收敛成单调递增的单段序列（用小样本：74 段会被判超限截断）。
    let reversed: Vec<Segment> = segments.iter().take(3).rev().cloned().collect();
    let (merged, truncated) = index::normalize_segments(&reversed);
    assert!(!truncated);
    assert!(merged
        .windows(2)
        .all(|pair| pair[0].until_ms <= pair[1].until_ms));
}

/// 条目上限：超出按 `updatedAt` 淘汰最旧。
#[test]
fn entries_over_limit_drop_the_oldest() {
    let root = temp_root("cap");
    let mut index = OriginIndex::empty();
    for step in 0..(MAX_ENTRIES + 5) {
        index.entries.insert(
            key_of(NS_WORKBUDDY_CN, &format!("session-{step}")),
            Entry {
                segments: vec![Segment {
                    until_ms: step as i64,
                    uid: "A".to_string(),
                }],
                // updatedAt 递增：`session-0` 最旧。
                updated_at: step as i64,
                heuristic: false,
                truncated: false,
            },
        );
    }
    index.save_at(&root).expect("save");
    let reloaded = OriginIndex::load_at(&root);
    assert_eq!(reloaded.len(), MAX_ENTRIES);
    assert!(
        reloaded.entry(NS_WORKBUDDY_CN, "session-0").is_none(),
        "最旧条目必须被淘汰"
    );
    assert!(reloaded
        .entry(NS_WORKBUDDY_CN, &format!("session-{}", MAX_ENTRIES + 4))
        .is_some());
    fs::remove_dir_all(root).expect("cleanup");
}

/// 索引缺失 / 损坏 / 版本不符 → 空索引，不 panic。
#[test]
fn missing_or_broken_file_degrades_to_empty_index() {
    let root = temp_root("broken");
    assert!(OriginIndex::load_at(&root).is_empty());

    fs::write(file_path(&root), "{not json").expect("write broken");
    assert!(OriginIndex::load_at(&root).is_empty());

    fs::write(
        file_path(&root),
        json!({"version": 99, "entries": {}}).to_string(),
    )
    .expect("write future version");
    assert!(OriginIndex::load_at(&root).is_empty());

    // 合法文件但段为空：条目被丢弃（没有可用归属）。
    fs::write(
        file_path(&root),
        json!({"version": 1, "entries": {"wb:cn:x": {"segments": [], "updatedAt": 1}}}).to_string(),
    )
    .expect("write empty segments");
    assert!(OriginIndex::load_at(&root).is_empty());
    fs::remove_dir_all(root).expect("cleanup");
}

/// 落盘格式与跨轨道契约逐字一致（C 轨按同一份读）。
#[test]
fn saved_file_matches_the_frozen_contract() {
    let root = temp_root("contract");
    let mut index = OriginIndex::empty();
    index.inherit(
        NS_WORKBUDDY_CN,
        "sid",
        NS_WORKBUDDY_CN,
        "src",
        1_790_500_000_000,
        "A",
    );
    index.save_at(&root).expect("save");
    let text = fs::read_to_string(file_path(&root)).expect("read");
    let value: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(value["version"], 1);
    let entry = &value["entries"]["wb:cn:sid"];
    assert_eq!(entry["segments"][0]["untilMs"], 1_790_500_000_000i64);
    assert_eq!(entry["segments"][0]["uid"], "A");
    assert!(entry["updatedAt"].is_i64());
    assert_eq!(entry["heuristic"], false);
    assert_eq!(entry["truncated"], false);
    fs::remove_dir_all(root).expect("cleanup");
}

/// 无时间戳 / 无条目 / 未命中段 → `None`（调用方回退文件 uid）。
#[test]
fn attribute_returns_none_when_it_cannot_decide() {
    let mut index = OriginIndex::empty();
    index.inherit(NS_WORKBUDDY_CN, "sid", NS_WORKBUDDY_CN, "src", 100, "A");
    assert_eq!(index.attribute(NS_WORKBUDDY_CN, "sid", None), None);
    assert_eq!(index.attribute(NS_WORKBUDDY_CN, "unknown", Some(50)), None);
    assert_eq!(
        index.attribute(NS_IDE_VSCODE, "sid", Some(50)),
        None,
        "命名空间隔离：同名 id 不跨客户端串用"
    );
    assert_eq!(
        index.attribute(NS_WORKBUDDY_CN, "sid", Some(50)),
        Some("A".to_string())
    );
}

/// GC 只删「所有客户端都不存在」的条目。
#[test]
fn gc_drops_only_dead_entries() {
    let mut index = OriginIndex::empty();
    index.inherit(NS_WORKBUDDY_CN, "alive", NS_WORKBUDDY_CN, "src", 100, "A");
    index.inherit(NS_IDE_VSCODE, "dead", NS_IDE_VSCODE, "src", 100, "A");
    let alive: HashSet<String> = [key_of(NS_WORKBUDDY_CN, "alive")].into_iter().collect();
    assert_eq!(index.gc(&alive), 1);
    assert!(index.entry(NS_WORKBUDDY_CN, "alive").is_some());
    assert!(index.entry(NS_IDE_VSCODE, "dead").is_none());
}

/// 命名空间映射只有一份实现：客户端目录名 / 备份根名都指向同一命名空间。
#[test]
fn namespace_mapping_is_single_sourced() {
    assert_eq!(namespace_for_variant(WbVariant::Cn), "wb:cn");
    assert_eq!(namespace_for_variant(WbVariant::Ai), "wb:ai");
    assert_eq!(namespace_for_ide(IdeClientSource::VSCode), "ide:vscode");
    assert_eq!(
        namespace_for_ide(IdeClientSource::JetBrains),
        "ide:jetbrains"
    );
    assert_eq!(
        namespace_for_ide(IdeClientSource::CodeBuddyIde),
        "ide:codebuddy-ide"
    );
    for (client_dir, kind, namespace) in [
        ("VSCode", "vscode-sessions", "ide:vscode"),
        ("JetBrains", "jetbrains-sessions", "ide:jetbrains"),
        (
            "CodeBuddyIDE",
            "codebuddy-ide-sessions",
            "ide:codebuddy-ide",
        ),
    ] {
        let by_dir = IdeClientSource::from_client_dir(client_dir).expect("client dir");
        let by_kind = IdeClientSource::from_backup_kind(kind).expect("backup kind");
        assert_eq!(by_dir, by_kind);
        assert_eq!(by_dir.namespace(), namespace);
    }
    assert_eq!(key_of("wb:cn", "sid"), "wb:cn:sid");
}

/// 由备份根反推工具存储根：`<store>/backups/<kind>/<iso>`。
#[test]
fn store_root_is_derived_from_backup_root() {
    assert_eq!(
        store_root_from_backup_root(Path::new(
            "/home/u/.workbuddy-shift/backups/vscode-sessions/2026-11-28T10-00-00Z"
        )),
        Some(PathBuf::from("/home/u/.workbuddy-shift"))
    );
    assert_eq!(store_root_from_backup_root(Path::new("/tmp/backup")), None);
}

// ---------------------------------------------------------------------------
// 回填
// ---------------------------------------------------------------------------

fn write_operation(root: &Path, id: &str, value: serde_json::Value) {
    let dir = root.join("session-links").join("operations");
    fs::create_dir_all(&dir).expect("operations dir");
    fs::write(dir.join(format!("{id}.json")), value.to_string()).expect("write operation");
}

fn operation(
    id: &str,
    source: &str,
    target: &str,
    created: i64,
    updated: i64,
) -> serde_json::Value {
    json!({
        "version": 1,
        "operationId": id,
        "kind": "copy",
        "variant": "cn",
        "groupId": "g1",
        "source": {"uid": source, "sessionId": format!("session-{source}")},
        "target": {"uid": target, "sessionId": format!("session-{target}")},
        "expectedContentDigest": "digest",
        "expectedRecordCount": 1,
        "phase": "completed",
        "auxSkipped": false,
        "createdAt": created,
        "updatedAt": updated,
    })
}

/// 操作日志链式回填：A→B→C 建出 `[(T1,A),(T2,B)]`。
#[test]
fn backfill_chains_operation_log() {
    let root = temp_root("backfill_chain");
    const T1: i64 = 1_790_000_000_000;
    const T2: i64 = 1_790_500_000_000;
    write_operation(&root, "op-1", operation("op-1", "A", "B", T1, T1));
    write_operation(&root, "op-2", operation("op-2", "B", "C", T2, T2));

    let applied = backfill_at(&root).expect("first backfill runs");
    assert_eq!(applied, 2);
    let index = OriginIndex::load_at(&root);
    let entry = index
        .entry(NS_WORKBUDDY_CN, "session-C")
        .expect("entry for C");
    assert_eq!(
        entry
            .segments
            .iter()
            .map(|segment| (segment.until_ms, segment.uid.as_str()))
            .collect::<Vec<_>>(),
        vec![(T1, "A"), (T2, "B")]
    );
    assert!(!entry.heuristic);
    assert!(index.backfilled_at().is_some());

    // 幂等：第二次不再跑。
    assert_eq!(backfill_at(&root), None);
    fs::remove_dir_all(root).expect("cleanup");
}

/// 只有未完成（`prepared`）的操作不产生段。
#[test]
fn backfill_skips_unwritten_operations() {
    let root = temp_root("backfill_prepared");
    let mut value = operation("op-1", "A", "B", 10, 10);
    value["phase"] = json!("prepared");
    write_operation(&root, "op-1", value);
    assert_eq!(backfill_at(&root), Some(0));
    let index = OriginIndex::load_at(&root);
    assert!(index.is_empty());
    fs::remove_dir_all(root).expect("cleanup");
}

/// 缺操作日志时用关联组启发式补洞，并标 `heuristic`。
#[test]
fn backfill_marks_heuristic_entries() {
    let root = temp_root("backfill_heuristic");
    let store = json!({
        "version": 1,
        "revision": 1,
        "groups": [{
            "id": "g1",
            "variant": "cn",
            "createdAt": 100,
            "members": [
                {
                    "memberId": "m1",
                    "uid": "A",
                    "sessionId": "session-a",
                    "state": "active",
                    "linkedAt": 100,
                },
                {
                    "memberId": "m2",
                    "uid": "B",
                    "sessionId": "session-b",
                    "state": "active",
                    "linkedAt": 500,
                }
            ],
        }],
    });
    fs::write(root.join("session_links.json"), store.to_string()).expect("write link store");

    assert_eq!(backfill_at(&root), Some(1));
    let index = OriginIndex::load_at(&root);
    let entry = index
        .entry(NS_WORKBUDDY_CN, "session-b")
        .expect("heuristic entry for B");
    assert!(entry.heuristic, "启发式条目必须标记 heuristic");
    assert_eq!(entry.segments.len(), 1);
    assert_eq!(entry.segments[0].until_ms, 500);
    assert_eq!(entry.segments[0].uid, "A");
    // 组内最早成员自身不需要段（回退文件 uid 就是它）。
    assert!(index.entry(NS_WORKBUDDY_CN, "session-a").is_none());
    fs::remove_dir_all(root).expect("cleanup");
}

/// 什么都没有时回填也是「成功但零条」，不会反复重跑。
#[test]
fn backfill_without_data_marks_done() {
    let root = temp_root("backfill_empty");
    assert_eq!(backfill_at(&root), Some(0));
    let index = OriginIndex::load_at(&root);
    assert!(index.backfilled_at().is_some());
    fs::remove_dir_all(root).expect("cleanup");
}
