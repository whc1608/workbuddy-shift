// ---------------------------------------------------------------------
// 第 7 条：Token 统计按账号筛选（uid 维度）
// ---------------------------------------------------------------------

fn account_root(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "workbuddy-shift-token-stats-account-{tag}-{}-{}",
        std::process::id(),
        crate::modules::config::now_ms()
    ));
    fs::create_dir_all(&base).expect("create fixture root");
    base
}

/// 一条可计入的 JSONL 记录（`input`/`output` 固定，便于按数字断言）。
fn jsonl_record(timestamp: i64, input: u64, model: &str) -> String {
    json!({
        "timestamp": timestamp,
        "model": model,
        "cwd": "/fixture/project-alpha",
        "message": { "usage": { "input_tokens": input, "output_tokens": 1 } }
    })
    .to_string()
        + "\n"
}

fn uid_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(session, uid)| ((*session).to_string(), (*uid).to_string()))
        .collect()
}

fn account_row(result: &Value, uid: &str) -> Value {
    result["accounts"]
        .as_array()
        .expect("accounts array")
        .iter()
        .find(|row| row["uid"] == uid)
        .unwrap_or_else(|| panic!("account row for uid {uid}"))
        .clone()
}

/// 复制会话：重放的历史请求归**最早出现**的文件（原账号），副本新增的请求归副本账号。
#[test]
fn copied_jsonl_replay_is_attributed_to_the_first_file_account() {
    let root = account_root("replay");
    let project = root.join("project-alpha");
    fs::create_dir_all(&project).expect("create project dir");
    let base_ts = 1_700_000_000_000i64;
    // 原账号：两条历史请求。
    fs::write(
        project.join("session-original.jsonl"),
        jsonl_record(base_ts, 100, "model-a") + &jsonl_record(base_ts + 1_000, 200, "model-a"),
    )
    .expect("write original session");
    pin_mtime(
        project.join("session-original.jsonl"),
        std::time::SystemTime::now() - std::time::Duration::from_secs(120),
    );
    // 副本：重放同样两条 + 复制后新增一条。
    fs::write(
        project.join("session-copy.jsonl"),
        jsonl_record(base_ts, 100, "model-a")
            + &jsonl_record(base_ts + 1_000, 200, "model-a")
            + &jsonl_record(base_ts + 60_000, 400, "model-a"),
    )
    .expect("write copied session");
    pin_mtime(
        project.join("session-copy.jsonl"),
        std::time::SystemTime::now() - std::time::Duration::from_secs(60),
    );

    let result = source_from_roots(
        std::slice::from_ref(&root),
        "workbuddy",
        None,
        false,
        &uid_map(&[
            ("session-original", "uid-original"),
            ("session-copy", "uid-copy"),
        ]),
        &mut Attribution::empty(),
        Some(namespace_for_variant(WbVariant::Cn)),
    );

    // 重放的两条只算一次，归原账号；副本只多算它新增的那条。
    assert_eq!(result["summary"]["records"], 3);
    assert_eq!(result["summary"]["input"], 700);
    let original = account_row(&result, "uid-original");
    assert_eq!(original["summary"]["records"], 2);
    assert_eq!(original["summary"]["input"], 300);
    let copy = account_row(&result, "uid-copy");
    assert_eq!(copy["summary"]["records"], 1);
    assert_eq!(copy["summary"]["input"], 400);

    // 会话行带 uid：原账号 2 条、副本 1 条。
    let sessions = result["sessions"].as_array().expect("sessions");
    assert_eq!(sessions.len(), 2);
    assert!(sessions
        .iter()
        .all(|session| session["uid"] == "uid-original" || session["uid"] == "uid-copy"));

    fs::remove_dir_all(root).expect("remove fixture");
}

/// 查不到归属（DB 缺行 / 孤儿文件）的记录进「未归属」桶，uid 为空串。
#[test]
fn unmapped_session_lands_in_the_unassigned_bucket() {
    let root = account_root("unassigned");
    let project = root.join("project-alpha");
    fs::create_dir_all(&project).expect("create project dir");
    fs::write(
        project.join("session-orphan.jsonl"),
        jsonl_record(1_700_000_000_000, 50, "model-a"),
    )
    .expect("write orphan session");

    let result = source_from_roots(
        std::slice::from_ref(&root),
        "workbuddy",
        None,
        false,
        &HashMap::new(),
        &mut Attribution::empty(),
        Some(namespace_for_variant(WbVariant::Cn)),
    );

    let accounts = result["accounts"].as_array().expect("accounts");
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0]["uid"], "");
    assert_eq!(accounts[0]["summary"]["records"], 1);
    assert_eq!(result["sessions"][0]["uid"], "");

    fs::remove_dir_all(root).expect("remove fixture");
}

/// 不变量：全局 summary == 各账号之和（含未归属），模型/项目表同样逐项相等。
#[test]
fn global_summary_equals_the_sum_of_accounts() {
    let root = account_root("sum");
    let project = root.join("project-alpha");
    fs::create_dir_all(&project).expect("create project dir");
    let base_ts = 1_700_000_000_000i64;
    fs::write(
        project.join("session-one.jsonl"),
        jsonl_record(base_ts, 10, "model-a") + &jsonl_record(base_ts + 1, 20, "model-b"),
    )
    .expect("write session one");
    fs::write(
        project.join("session-two.jsonl"),
        jsonl_record(base_ts + 10, 30, "model-a"),
    )
    .expect("write session two");
    fs::write(
        project.join("session-three.jsonl"),
        jsonl_record(base_ts + 20, 40, "model-c"),
    )
    .expect("write session three");

    let result = source_from_roots(
        std::slice::from_ref(&root),
        "workbuddy",
        None,
        false,
        &uid_map(&[("session-one", "uid-a"), ("session-two", "uid-a")]),
        &mut Attribution::empty(),
        Some(namespace_for_variant(WbVariant::Cn)),
    );

    let accounts = result["accounts"].as_array().expect("accounts");
    // uid-a（两个会话）+ 未归属（session-three）。
    assert_eq!(accounts.len(), 2);
    let mut records = 0u64;
    let mut input = 0u64;
    let mut output = 0u64;
    let mut total = 0u64;
    for account in accounts {
        records += account["summary"]["records"].as_u64().unwrap_or(0);
        input += account["summary"]["input"].as_u64().unwrap_or(0);
        output += account["summary"]["output"].as_u64().unwrap_or(0);
        total += account["summary"]["total"].as_u64().unwrap_or(0);
    }
    assert_eq!(result["summary"]["records"].as_u64(), Some(records));
    assert_eq!(result["summary"]["input"].as_u64(), Some(input));
    assert_eq!(result["summary"]["output"].as_u64(), Some(output));
    assert_eq!(result["summary"]["total"].as_u64(), Some(total));
    assert_eq!(records, 4);
    assert_eq!(input, 100);

    // 模型的全局表同样是各账号之和。
    let model_total: u64 = accounts
        .iter()
        .flat_map(|account| {
            account["models"]
                .as_array()
                .map(|models| models.to_vec())
                .unwrap_or_default()
        })
        .filter(|model| model["key"] == "model-a")
        .map(|model| model["records"].as_u64().unwrap_or(0))
        .sum();
    let global_model_a = result["models"]
        .as_array()
        .expect("models")
        .iter()
        .find(|model| model["key"] == "model-a")
        .map(|model| model["records"].as_u64().unwrap_or(0))
        .unwrap_or(0);
    assert_eq!(global_model_a, model_total);
    // model-a 出现在 session-one 与 session-two 各一条，共 2 条。
    assert_eq!(global_model_a, 2);

    fs::remove_dir_all(root).expect("remove fixture");
}

/// 没有任何数据源 / 空账号列表时返回空数组，不 panic。
#[test]
fn empty_sources_return_empty_accounts() {
    let value = source_from_roots(
        &[],
        "workbuddy",
        None,
        false,
        &HashMap::new(),
        &mut Attribution::empty(),
        Some(namespace_for_variant(WbVariant::Cn)),
    );
    assert_eq!(value["accounts"].as_array().map(Vec::len), Some(0));
    assert_eq!(value["summary"]["records"], 0);
}

/// CLI 日志不记录账号：全部进「未归属」，明细行同样带空 uid。
#[test]
fn cli_source_only_has_the_unassigned_account() {
    let root = account_root("cli");
    let project = root.join("project-alpha");
    fs::create_dir_all(&project).expect("create project dir");
    let now = crate::modules::config::now_ms();
    fs::write(
        project.join("session-cli.jsonl"),
        jsonl_record(now, 7, "model-a"),
    )
    .expect("write cli session");

    let result = source(root.clone(), "codebuddy-cli", None, true);
    let accounts = result["accounts"].as_array().expect("accounts");
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0]["uid"], "");
    let requests = result["requests"].as_array().expect("requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["uid"], "");

    fs::remove_dir_all(root).expect("remove fixture");
}

/// 会话 id → uid 映射直接读 `workbuddy.db`：缺库/缺表返回空表，有表则逐行读回。
#[test]
fn session_uid_index_reads_workbuddy_db() {
    let root = account_root("db");
    // 先把「库不存在」的超前用例走一遍。
    assert!(session_uid_index_at(&root).is_empty());

    let db = root.join("workbuddy.db");
    let conn = rusqlite::Connection::open(&db).expect("create fixture db");
    conn.execute_batch(
        "CREATE TABLE sessions (id TEXT PRIMARY KEY, user_id TEXT);
             INSERT INTO sessions (id, user_id) VALUES ('conv-1', 'uid-a');
             INSERT INTO sessions (id, user_id) VALUES ('conv-2', NULL);",
    )
    .expect("seed fixture db");
    drop(conn);

    let index = session_uid_index_at(&root);
    assert_eq!(index.get("conv-1").map(String::as_str), Some("uid-a"));
    // 没有 user_id 的行不参与归因（归「未归属」）。
    assert!(!index.contains_key("conv-2"));

    fs::remove_dir_all(root).expect("remove fixture");
}
