// ---------------------------------------------------------------------
// R6-07：按 `usage-origins.json` 的分段把用量归到「真正用它的账号」
// ---------------------------------------------------------------------

/// 键盘命名空间常量（测试里直接写值，避免与生产常量拼错时两边一起错）。
const NS_WORKBUDDY_CN: &str = crate::modules::usage_origin::NS_WORKBUDDY_CN;
const NS_IDE_VSCODE: &str = crate::modules::usage_origin::NS_IDE_VSCODE;

/// 建一个带「项目目录」的 fixture 根，返回 `(root, project)`。
fn origin_root(tag: &str) -> (PathBuf, PathBuf) {
    let root = account_root(&format!("origin-{tag}"));
    let project = root.join("project-alpha");
    fs::create_dir_all(&project).expect("create project dir");
    (root, project)
}

/// 一次统计的归属上下文：给定段的索引 + 空指纹集合。
fn attribution_with(index: OriginIndex) -> Attribution {
    Attribution::new(index)
}

/// `age_secs` 秒之前的时间点（越大越旧 → 越早被扫到）。
fn older_than_now(age_secs: u64) -> std::time::SystemTime {
    std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs)
}

/// **R6-07 关键回归**：源文件 mtime 比副本**新**（副本先被扫到）时，历史仍归源账号。
///
/// 这正是清单 §「现象」描述的翻转场景：复制/同步之后继续用源会话，源 mtime 立刻反超
/// 副本，按文件归属的旧实现会把整段历史判给副本账号。分段继承与文件顺序无关。
#[test]
fn newer_source_mtime_still_attributes_history_to_the_source() {
    const BASE: i64 = 1_700_000_000_000;
    let (root, project) = origin_root("newer-source");
    // 源账号 A：两条历史。
    fs::write(
        project.join("session-a.jsonl"),
        jsonl_record(BASE, 100, "model-a") + &jsonl_record(BASE + 1_000, 200, "model-a"),
    )
    .expect("write source session");
    // 副本 B：重放同样两条 + 复制后自己新增一条（时间戳更晚）。
    fs::write(
        project.join("session-b.jsonl"),
        jsonl_record(BASE, 100, "model-a")
            + &jsonl_record(BASE + 1_000, 200, "model-a")
            + &jsonl_record(BASE + 60_000, 400, "model-a"),
    )
    .expect("write copied session");
    let uids = uid_map(&[("session-a", "uid-a"), ("session-b", "uid-b")]);
    let mut index = OriginIndex::empty();
    index.inherit(
        NS_WORKBUDDY_CN,
        "session-b",
        NS_WORKBUDDY_CN,
        "session-a",
        BASE + 2_000,
        "uid-a",
    );

    // 场景 1（bug 现场）：副本更旧 → 副本先被处理。
    pin_mtime(project.join("session-a.jsonl"), older_than_now(10));
    pin_mtime(project.join("session-b.jsonl"), older_than_now(120));
    let scan = |index: OriginIndex| {
        source_from_roots(
            std::slice::from_ref(&root),
            "workbuddy",
            None,
            false,
            &uids,
            &mut attribution_with(index),
            Some(namespace_for_variant(WbVariant::Cn)),
        )
    };
    let copy_first = scan(index.clone());
    assert_eq!(copy_first["summary"]["records"], 3, "重放的两条只算一次");
    assert_eq!(copy_first["summary"]["input"], 700);
    let source_row = account_row(&copy_first, "uid-a");
    assert_eq!(source_row["summary"]["records"], 2, "历史必须留在源账号");
    assert_eq!(source_row["summary"]["input"], 300);
    let copy_row = account_row(&copy_first, "uid-b");
    assert_eq!(copy_row["summary"]["records"], 1, "副本只拿到自己新增的那条");
    assert_eq!(copy_row["summary"]["input"], 400);

    // 场景 2：把 mtime 反调（源更旧 → 源先被处理），数字必须完全相同。
    pin_mtime(project.join("session-a.jsonl"), older_than_now(120));
    pin_mtime(project.join("session-b.jsonl"), older_than_now(10));
    let source_first = scan(index);
    assert_eq!(source_first["summary"]["input"], 700);
    assert_eq!(account_row(&source_first, "uid-a")["summary"]["input"], 300);
    assert_eq!(account_row(&source_first, "uid-b")["summary"]["input"], 400);

    fs::remove_dir_all(root).expect("remove fixture");
}

/// 链式 A→B→C：A 的历史归 A、B 自己的归 B、C 新增归 C，与扫描顺序无关。
#[test]
fn chained_copies_split_three_accounts() {
    const BASE: i64 = 1_700_100_000_000;
    let (root, project) = origin_root("chain");
    fs::write(
        project.join("session-a.jsonl"),
        jsonl_record(BASE, 10, "model-a") + &jsonl_record(BASE + 1_000, 20, "model-a"),
    )
    .expect("write a");
    fs::write(
        project.join("session-b.jsonl"),
        jsonl_record(BASE, 10, "model-a")
            + &jsonl_record(BASE + 1_000, 20, "model-a")
            + &jsonl_record(BASE + 5_000, 40, "model-a"),
    )
    .expect("write b");
    fs::write(
        project.join("session-c.jsonl"),
        jsonl_record(BASE, 10, "model-a")
            + &jsonl_record(BASE + 5_000, 40, "model-a")
            + &jsonl_record(BASE + 9_000, 80, "model-a"),
    )
    .expect("write c");

    let mut index = OriginIndex::empty();
    // A → B（boundary = BASE+2000），B → C（boundary = BASE+6000）。
    index.inherit(NS_WORKBUDDY_CN, "session-b", NS_WORKBUDDY_CN, "session-a", BASE + 2_000, "uid-a");
    index.inherit(NS_WORKBUDDY_CN, "session-c", NS_WORKBUDDY_CN, "session-b", BASE + 6_000, "uid-b");
    assert_eq!(
        index
            .entry(NS_WORKBUDDY_CN, "session-c")
            .expect("entry for c")
            .segments
            .len(),
        2,
        "C 必须继承 B 的段再补一段，链式才不会断"
    );

    // 故意让 C 最旧（先被扫到）、A 最新：证明与扫描顺序无关。
    pin_mtime(project.join("session-a.jsonl"), older_than_now(10));
    pin_mtime(project.join("session-b.jsonl"), older_than_now(60));
    pin_mtime(project.join("session-c.jsonl"), older_than_now(120));

    let result = source_from_roots(
        std::slice::from_ref(&root),
        "workbuddy",
        None,
        false,
        &uid_map(&[
            ("session-a", "uid-a"),
            ("session-b", "uid-b"),
            ("session-c", "uid-c"),
        ]),
        &mut attribution_with(index),
        Some(namespace_for_variant(WbVariant::Cn)),
    );
    // 重放的历史只计一次（10+20），B 自己新增 40，C 自己新增 80。
    assert_eq!(result["summary"]["input"], 150);
    assert_eq!(account_row(&result, "uid-a")["summary"]["input"], 30);
    assert_eq!(account_row(&result, "uid-b")["summary"]["input"], 40);
    assert_eq!(account_row(&result, "uid-c")["summary"]["input"], 80);

    fs::remove_dir_all(root).expect("remove fixture");
}

/// 反向同步 B→A：A 的原始历史仍归 A，B 的增量归 B，A 之后的新增归 A。
#[test]
fn reverse_sync_keeps_each_hop_with_its_owner() {
    const BASE: i64 = 1_700_200_000_000;
    let (root, project) = origin_root("reverse");
    // A 的正文被 B 覆盖后：A 的历史 + B 的增量 + A 之后的新增。
    fs::write(
        project.join("session-a.jsonl"),
        jsonl_record(BASE, 10, "model-a")
            + &jsonl_record(BASE + 5_000, 40, "model-a")
            + &jsonl_record(BASE + 9_000, 80, "model-a"),
    )
    .expect("write a");

    let mut index = OriginIndex::empty();
    // A → B（BASE+2000），随后 B → A（BASE+6000）。
    index.inherit(NS_WORKBUDDY_CN, "session-b", NS_WORKBUDDY_CN, "session-a", BASE + 2_000, "uid-a");
    index.inherit(NS_WORKBUDDY_CN, "session-a", NS_WORKBUDDY_CN, "session-b", BASE + 6_000, "uid-b");

    let result = source_from_roots(
        std::slice::from_ref(&root),
        "workbuddy",
        None,
        false,
        &uid_map(&[("session-a", "uid-a")]),
        &mut attribution_with(index),
        Some(namespace_for_variant(WbVariant::Cn)),
    );
    // 一段：BASE（<=2000）→ A；BASE+5000（<=6000）→ B；BASE+9000（>6000）→ 文件 uid A。
    assert_eq!(account_row(&result, "uid-a")["summary"]["input"], 90);
    assert_eq!(account_row(&result, "uid-b")["summary"]["input"], 40);
    assert_eq!(result["summary"]["input"], 130);

    fs::remove_dir_all(root).expect("remove fixture");
}

/// 源文件被删除：副本仍按段正确归属（索引不依赖源文件存在）。
#[test]
fn deleted_source_copy_still_attributes_history() {
    const BASE: i64 = 1_700_300_000_000;
    let (root, project) = origin_root("deleted-source");
    // 只剩副本：重放的历史 + 自己新增。
    fs::write(
        project.join("session-b.jsonl"),
        jsonl_record(BASE, 100, "model-a")
            + &jsonl_record(BASE + 1_000, 200, "model-a")
            + &jsonl_record(BASE + 60_000, 400, "model-a"),
    )
    .expect("write copy");

    let mut index = OriginIndex::empty();
    index.inherit(NS_WORKBUDDY_CN, "session-b", NS_WORKBUDDY_CN, "session-a", BASE + 2_000, "uid-a");

    let result = source_from_roots(
        std::slice::from_ref(&root),
        "workbuddy",
        None,
        false,
        &uid_map(&[("session-b", "uid-b")]),
        &mut attribution_with(index),
        Some(namespace_for_variant(WbVariant::Cn)),
    );
    assert_eq!(account_row(&result, "uid-a")["summary"]["input"], 300);
    assert_eq!(account_row(&result, "uid-b")["summary"]["input"], 400);

    fs::remove_dir_all(root).expect("remove fixture");
}

/// 索引里没有这个会话（条目缺失 / 索引损坏重建）：回退「文件自身 uid」的老行为。
#[test]
fn missing_entry_falls_back_to_the_file_uid() {
    const BASE: i64 = 1_700_400_000_000;
    let (root, project) = origin_root("no-entry");
    fs::write(project.join("session-a.jsonl"), jsonl_record(BASE, 100, "model-a"))
        .expect("write session");

    // 索引里有别的会话、也有同 id 但别的命名空间：都不能命中。
    let mut index = OriginIndex::empty();
    index.inherit(NS_WORKBUDDY_CN, "other", NS_WORKBUDDY_CN, "src", BASE, "uid-x");
    index.inherit(NS_IDE_VSCODE, "session-a", NS_IDE_VSCODE, "src", BASE, "uid-x");

    let result = source_from_roots(
        std::slice::from_ref(&root),
        "workbuddy",
        None,
        false,
        &uid_map(&[("session-a", "uid-a")]),
        &mut attribution_with(index),
        Some(namespace_for_variant(WbVariant::Cn)),
    );
    assert_eq!(account_row(&result, "uid-a")["summary"]["input"], 100);
    assert!(result["accounts"]
        .as_array()
        .expect("accounts")
        .iter()
        .all(|row| row["uid"] == "uid-a"));

    fs::remove_dir_all(root).expect("remove fixture");
}

/// CN ↔ AI 共用一份指纹集合：同一段历史在两个档位各有一份时只计一次，且归首次命中的账号。
#[test]
fn cross_variant_history_is_counted_once_and_keeps_its_origin() {
    const BASE: i64 = 1_700_500_000_000;
    let cn_root = account_root("shared-cn");
    let ai_root = account_root("shared-ai");
    for root in [&cn_root, &ai_root] {
        let project = root.join("project-alpha");
        fs::create_dir_all(&project).expect("create project dir");
        // 两个档位的文件内容逐字相同（跨档复制/同步的形态）。
        fs::write(
            project.join("session-x.jsonl"),
            jsonl_record(BASE, 100, "model-a") + &jsonl_record(BASE + 1_000, 200, "model-a"),
        )
        .expect("write cross-variant session");
    }

    let mut attribution = Attribution::empty();
    let cn = source_from_roots(
        std::slice::from_ref(&cn_root),
        "workbuddy",
        None,
        false,
        &uid_map(&[("session-x", "uid-cn")]),
        &mut attribution,
        Some(namespace_for_variant(WbVariant::Cn)),
    );
    // 顺序固定 CN → AI：AI 遇到同指纹一律跳过。
    let ai = source_from_roots(
        std::slice::from_ref(&ai_root),
        "workbuddy-ai",
        None,
        false,
        &uid_map(&[("session-x", "uid-ai")]),
        &mut attribution,
        Some(namespace_for_variant(WbVariant::Ai)),
    );

    assert_eq!(cn["summary"]["input"], 300);
    assert_eq!(ai["summary"]["input"], 0, "跨档位同一历史不得双算");
    assert_eq!(ai["summary"]["records"], 0);

    fs::remove_dir_all(cn_root).expect("remove fixture");
    fs::remove_dir_all(ai_root).expect("remove fixture");
}

/// IDE 三来源共用一份指纹集合：同一会话在 VSCode 与 CodeBuddy IDE 各有一份时只计一次。
#[test]
fn ide_sources_share_one_fingerprint_set() {
    let now = crate::modules::config::now_ms();
    let root = std::env::temp_dir().join(format!(
        "workbuddy-shift-token-stats-origin-ide-shared-{}-{now}",
        std::process::id()
    ));
    let conversation = |client: &str, uid: &str, conv: &str| {
        root.join(uid)
            .join(client)
            .join(uid)
            .join("history")
            .join("workspace-hash")
            .join(conv)
    };
    let request = json!({
        "id": "req-1",
        "state": "complete",
        "startedAt": now,
        "usage": {"inputTokens": 100, "outputTokens": 0, "cacheTokens": 0, "cachedWriteTokens": 0},
    });
    for dir in [
        conversation("VSCode", "uid-a", "conv-a"),
        conversation("CodeBuddyIDE", "uid-b", "conv-a"),
    ] {
        fs::create_dir_all(&dir).expect("create conversation dir");
        fs::write(
            dir.join("index.json"),
            json!({ "requests": [request.clone()] }).to_string(),
        )
        .expect("write conversation");
    }

    let mut attribution = Attribution::empty();
    let empty = HashMap::new();
    let vscode = ide_client_source(
        root.clone(),
        "VSCode",
        "vscode-ext",
        None,
        &empty,
        &mut attribution,
    );
    let ide = ide_client_source(
        root.clone(),
        "CodeBuddyIDE",
        "codebuddy-ide",
        None,
        &empty,
        &mut attribution,
    );
    assert_eq!(vscode["summary"]["input"], 100);
    assert_eq!(ide["summary"]["input"], 0, "同一请求不得在第二个来源再计一次");

    fs::remove_dir_all(root).expect("remove fixture");
}

/// IDE 来源按段归属：副本目录在 B 账号下，历史仍归段里的 A。
#[test]
fn ide_copy_history_follows_the_origin_segment() {
    let now = crate::modules::config::now_ms();
    let root = std::env::temp_dir().join(format!(
        "workbuddy-shift-token-stats-origin-ide-{}-{now}",
        std::process::id()
    ));
    let request = |id: &str, started_at: i64, input: u64| {
        json!({
            "id": id,
            "state": "complete",
            "startedAt": started_at,
            "usage": {
                "inputTokens": input,
                "outputTokens": 0,
                "cacheTokens": 0,
                "cachedWriteTokens": 0
            }
        })
    };
    // 副本落在 uid-b 目录下：重放了一条历史（更早） + 自己新增一条（更晚）。
    let dir = root
        .join("uid-b")
        .join("VSCode")
        .join("uid-b")
        .join("history")
        .join("workspace-hash")
        .join("conv-copy");
    fs::create_dir_all(&dir).expect("create conversation dir");
    fs::write(
        dir.join("index.json"),
        json!({
            "requests": [
                request("req-hist", now - 60_000, 100),
                request("req-new", now - 1_000, 400),
            ]
        })
        .to_string(),
    )
    .expect("write conversation");

    let mut index = OriginIndex::empty();
    index.inherit(
        NS_IDE_VSCODE,
        "conv-copy",
        NS_IDE_VSCODE,
        "conv-src",
        now - 30_000,
        "uid-a",
    );
    let result = ide_client_source(
        root.clone(),
        "VSCode",
        "vscode-ext",
        None,
        &HashMap::new(),
        &mut attribution_with(index),
    );
    assert_eq!(result["summary"]["input"], 500);
    let accounts = result["accounts"].as_array().expect("accounts");
    let uid_a = accounts
        .iter()
        .find(|row| row["uid"] == "uid-a")
        .expect("uid-a 必须拿到历史");
    assert_eq!(uid_a["summary"]["input"], 100);
    let uid_b = accounts
        .iter()
        .find(|row| row["uid"] == "uid-b")
        .expect("uid-b 必须拿到自己新增的");
    assert_eq!(uid_b["summary"]["input"], 400);

    fs::remove_dir_all(root).expect("remove fixture");
}

/// 索引文件损坏时统计照常（`OriginIndex::load` 退化成空索引，全部回退文件 uid）。
#[test]
fn broken_index_file_degrades_to_file_attribution() {
    const BASE: i64 = 1_700_600_000_000;
    let (root, project) = origin_root("broken-index");
    fs::write(project.join("session-a.jsonl"), jsonl_record(BASE, 100, "model-a"))
        .expect("write session");
    let store_root = crate::modules::usage_origin::store_root_at(&root);
    fs::create_dir_all(&store_root).expect("create store root");
    fs::write(
        crate::modules::usage_origin::file_path(&store_root),
        "{ this is not json",
    )
    .expect("write broken index");

    let index = OriginIndex::load_at(&store_root);
    assert!(index.is_empty(), "损坏文件必须退化成空索引，不 panic");
    let result = source_from_roots(
        std::slice::from_ref(&root),
        "workbuddy",
        None,
        false,
        &uid_map(&[("session-a", "uid-a")]),
        &mut attribution_with(index),
        Some(namespace_for_variant(WbVariant::Cn)),
    );
    assert_eq!(account_row(&result, "uid-a")["summary"]["input"], 100);

    fs::remove_dir_all(root).expect("remove fixture");
}

/// 一轮统计只 `load()` 一次索引：同一份索引喂给 6 个来源，各自都能命中同一段。
#[test]
fn one_index_serves_every_source_in_a_round() {
    const BASE: i64 = 1_700_700_000_000;
    let (root, project) = origin_root("single-load");
    fs::write(
        project.join("session-a.jsonl"),
        jsonl_record(BASE, 100, "model-a"),
    )
    .expect("write session");
    let mut index = OriginIndex::empty();
    index.inherit(NS_WORKBUDDY_CN, "session-a", NS_WORKBUDDY_CN, "src", BASE + 1_000, "uid-a");
    let mut attribution = attribution_with(index);

    // CN 命中段 → uid-a；随后把同一轮上下文给 AI（不同根），索引仍然可用。
    let cn = source_from_roots(
        std::slice::from_ref(&root),
        "workbuddy",
        None,
        false,
        &uid_map(&[("session-a", "uid-b")]),
        &mut attribution,
        Some(namespace_for_variant(WbVariant::Cn)),
    );
    assert_eq!(account_row(&cn, "uid-a")["summary"]["input"], 100);
    let ai = source_from_roots(
        &[],
        "workbuddy-ai",
        None,
        false,
        &HashMap::new(),
        &mut attribution,
        Some(namespace_for_variant(WbVariant::Ai)),
    );
    assert_eq!(ai["summary"]["records"], 0);

    fs::remove_dir_all(root).expect("remove fixture");
}
