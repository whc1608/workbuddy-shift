// ---------------------------------------------------------------------------
// 归档状态同步（statusOnly，#143）
// ---------------------------------------------------------------------------

/// 带状态列的可同步环境（会话表含 `status`）。
fn ready_env_with_status(name: &str) -> Env {
    let env = Env::new(name);
    env.create_db_with_status();
    env.create_edge_db(WbVariant::Cn);
    env.set_login("uid-a");
    env.add_session("sess-1", "uid-a", "标题一");
    env.add_body("sess-1", &body_text("sess-1"));
    env
}

fn member_last_synced_at(env: &Env, group_id: &str, uid: &str) -> Option<i64> {
    group_snapshot(env, group_id)
        .members
        .into_iter()
        .find(|member| member.uid == uid)
        .and_then(|member| member.last_synced_at)
}

/// 造一个「正文一致 + 来源已归档 + 目标为指定状态」的场景（不要求可勾选）。
fn archive_row_scene(env: &Env, target_status: &str) -> (String, String) {
    let report = copy(env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    env.set_status("sess-1", "archived");
    env.set_status(&target_id, target_status);
    let preview = preview(env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["verdict"], "identical", "{preview}");
    (group["groupId"].as_str().unwrap().to_string(), target_id)
}

/// 造一个「正文一致 + 来源已归档 + 目标在终态」的可勾选场景。
fn archive_scene(env: &Env, target_status: &str) -> (String, String, String) {
    let (group_id, target_id) = archive_row_scene(env, target_status);
    let preview = preview(env, "uid-b");
    let group = &preview["groups"][0];
    (
        group_id,
        group["previewToken"].as_str().unwrap().to_string(),
        target_id,
    )
}

/// 造一个「可快进 + 来源已归档」的场景。
fn fast_forward_archive_scene(env: &Env, target_status: &str) -> (String, String, String) {
    let report = copy(env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    append_records(&env.body_path("sess-1"), "sess-1", 2, 3);
    env.set_status("sess-1", "archived");
    env.set_status(&target_id, target_status);
    let preview = preview(env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["verdict"], "fastForward", "{preview}");
    (
        group["groupId"].as_str().unwrap().to_string(),
        group["previewToken"].as_str().unwrap().to_string(),
        target_id,
    )
}

/// 目标已归档且来源有可快进的新记录：正文照写、时间照更新，状态保持 archived。
#[test]
fn fast_forward_keeps_archived_target_archived() {
    let env = ready_env_with_status("status-ff-already-archived");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    append_records(&env.body_path("sess-1"), "sess-1", 2, 3);
    env.set_status("sess-1", "completed");
    env.set_status(&target_id, "archived");
    let preview = preview(&env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["verdict"], "fastForward", "{preview}");
    assert!(group["archiveAction"].is_null(), "{group}");
    let token = group["previewToken"].as_str().unwrap().to_string();
    let group_id = group["groupId"].as_str().unwrap().to_string();
    let updated_before = session_row(&env, &target_id).3;

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(env.status_of(&target_id).as_deref(), Some("archived"));
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        incoming_text(&env, "sess-1", &target_id)
    );
    assert_ne!(
        session_row(&env, &target_id).3,
        updated_before,
        "正文写入照旧更新时间"
    );
}

/// 造一个「双方都有改动（diverge）+ 来源已归档」的场景。
fn diverge_archive_scene(env: &Env, target_status: &str) -> (String, String, String) {
    let report = copy(env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    // 目标追加一条来源没有的记录（index 100 与来源的 2/3 都不同）→ 双方互不为前缀。
    append_records(&env.body_path("sess-1"), "sess-1", 2, 2);
    append_records(&env.body_path(&target_id), &target_id, 100, 1);
    env.set_status("sess-1", "archived");
    env.set_status(&target_id, target_status);
    let preview = preview(env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["verdict"], "diverge", "{preview}");
    (
        group["groupId"].as_str().unwrap().to_string(),
        group["previewToken"].as_str().unwrap().to_string(),
        target_id,
    )
}

/// 正文一致且归档资格成立：只改状态，正文字节、配对基线与 lastSyncedAt 全不动。
#[test]
fn status_sync_archives_target_without_touching_body() {
    let env = ready_env_with_status("status-identical");
    let (group_id, token, target_id) = archive_scene(&env, "completed");
    let preview = preview(&env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(
        group["availableModes"],
        json!([]),
        "正文模式仍为空：归档不是正文覆盖"
    );
    assert_eq!(group["archiveAction"], "statusOnly");
    assert_eq!(group["defaultChecked"], false);

    let body_before = body_bytes(&env, &target_id);
    let pairs_before = pair_snapshot(&env);
    let baselines_before = env.baseline_files();
    let synced_before = member_last_synced_at(&env, &group_id, "uid-b");

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::StatusOnly)],
    );
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    let item = &report["synced"][0];
    assert_eq!(item["mode"], "statusOnly");
    assert_eq!(item["verdict"], "identical");
    assert_eq!(item["recordCount"]["target"], 2, "记录数来自目标原正文");
    assert_eq!(env.status_of(&target_id).as_deref(), Some("archived"));
    assert_eq!(
        body_bytes(&env, &target_id),
        body_before,
        "正文字节不得变化"
    );
    assert_eq!(pair_snapshot(&env), pairs_before, "配对基线不得推进");
    assert_eq!(env.baseline_files(), baselines_before, "不得产生新基线");
    assert_eq!(
        member_last_synced_at(&env, &group_id, "uid-b"),
        synced_before,
        "正文 lastSyncedAt 保持原值"
    );
    assert_eq!(env.body_files().len(), 2, "不得产生第二份正文");
}

/// 终态白名单：completed / error / terminated 与**陈旧 active** 可归档；
/// `working`（生成中）与语义未定的值一律不授权。
///
/// `active` 放行是实测缺口：跨账号副本在目标侧常因历史中断停在 `active`（进程早已
/// 不在），来源侧归档后永远同步不过去。
#[test]
fn status_sync_accepts_only_terminal_target_statuses() {
    for allowed in ["completed", "error", "terminated", "active"] {
        let env = ready_env_with_status(&format!("status-allow-{allowed}"));
        let (group_id, token, target_id) = archive_scene(&env, allowed);
        let report = sync(
            &env,
            "uid-b",
            &[selection(&group_id, &token, SyncMode::StatusOnly)],
        );
        assert_eq!(
            report["synced"].as_array().unwrap().len(),
            1,
            "{allowed}: {report}"
        );
        assert_eq!(
            env.status_of(&target_id).as_deref(),
            Some("archived"),
            "{allowed}"
        );
    }
    for denied in ["working", "Pending", "unknown-value"] {
        let env = ready_env_with_status(&format!("status-deny-{denied}"));
        let (_, target_id) = archive_row_scene(&env, denied);
        let preview_report = preview(&env, "uid-b");
        let group = &preview_report["groups"][0];
        assert!(group["archiveAction"].is_null(), "{denied}: {group}");
        assert!(
            group["previewToken"].is_null(),
            "{denied}: 无动作即无凭据 {group}"
        );
        assert_eq!(
            env.status_of(&target_id).as_deref(),
            Some(denied),
            "{denied}: 不得写入状态"
        );
    }
}

/// 把会话行的 `status` 显式写成 NULL（列存在但没值——真实库的旧行会这样）。
fn set_status_null(env: &Env, id: &str) {
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute("UPDATE sessions SET status = NULL WHERE id = ?1", [id])
        .unwrap();
}

/// 可写 NULL 的 status 库。
///
/// 共享夹具 `create_db_with_status` 把该列声明成 `NOT NULL`，写不进 NULL；生产侧的
/// `read_member_status` 却按 `Option<String>` 读、并为 `Present(None)` 单独留了分支，
/// 说明 NULL 行要当成可能存在的现场。要覆盖那条分支只能另建一份 schema。
fn ready_env_with_nullable_status(name: &str) -> Env {
    let env = Env::new(name);
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            title TEXT,
            custom_title TEXT,
            status TEXT,
            cwd TEXT,
            created_at INTEGER,
            updated_at INTEGER,
            deleted_at INTEGER,
            is_playground INTEGER
        );",
    )
    .unwrap();
    drop(conn);
    env.create_edge_db(WbVariant::Cn);
    env.set_login("uid-a");
    env.add_session("sess-1", "uid-a", "标题一");
    env.add_body("sess-1", &body_text("sess-1"));
    env
}

/// 目标 `status` 为 NULL：白名单只认精确文本，NULL 不授予归档权限。
///
/// 这是「NULL 仍必须拒绝」的守护用例：`archive_qualifies` 用 `Present(Some(..))`
/// 解构目标状态，NULL 落进 `Present(None)` 分支被拒——行为对了，但没有用例它就会
/// 被人顺手改成「None 视为可归档」而无人发现。
#[test]
fn status_sync_rejects_null_target_status() {
    let env = ready_env_with_nullable_status("status-target-null");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    env.set_status("sess-1", "archived");
    set_status_null(&env, &target_id);

    let preview_report = preview(&env, "uid-b");
    let group = &preview_report["groups"][0];
    assert_eq!(group["verdict"], "identical", "{preview_report}");
    assert!(
        group["archiveAction"].is_null(),
        "目标状态为 NULL 时不得授权归档：{group}"
    );
    assert!(group["previewToken"].is_null(), "无动作即无凭据：{group}");
    assert_eq!(env.status_of(&target_id), None, "不得把状态写进目标行");
}

/// 来源 `status` 为 NULL：不是精确的 `archived`，同样不授权（不猜、不外推）。
#[test]
fn status_sync_rejects_null_source_status() {
    let env = ready_env_with_nullable_status("status-source-null");
    let (_, target_id) = archive_row_scene(&env, "completed");
    set_status_null(&env, "sess-1");

    let preview_report = preview(&env, "uid-b");
    let group = &preview_report["groups"][0];
    assert!(
        group["archiveAction"].is_null(),
        "来源状态为 NULL 时不得授权归档：{group}"
    );
    assert!(group["previewToken"].is_null(), "无动作即无凭据：{group}");
    assert_eq!(
        env.status_of(&target_id).as_deref(),
        Some("completed"),
        "不得写入状态"
    );
}

/// 单向粘滞：来源未归档、目标已归档时，绝不反向取消归档。
#[test]
fn status_sync_never_unarchives_target() {
    let env = ready_env_with_status("status-no-unarchive");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    env.set_status("sess-1", "completed");
    env.set_status(&target_id, "archived");

    let preview = preview(&env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["verdict"], "identical", "{preview}");
    assert!(group["archiveAction"].is_null(), "{group}");
    assert!(group["previewToken"].is_null(), "{group}");
    let updated_before = session_row(&env, &target_id).3;
    assert_eq!(env.status_of(&target_id).as_deref(), Some("archived"));
    assert_eq!(session_row(&env, &target_id).3, updated_before);
}

/// 双方都已归档：无动作、不写库、不抬时间戳。
#[test]
fn status_sync_is_noop_when_both_already_archived() {
    let env = ready_env_with_status("status-both-archived");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    env.set_status("sess-1", "archived");
    env.set_status(&target_id, "archived");
    let updated_before = session_row(&env, &target_id).3;

    let preview = preview(&env, "uid-b");
    let group = &preview["groups"][0];
    assert!(group["archiveAction"].is_null(), "{group}");
    assert!(group["previewToken"].is_null(), "{group}");
    assert_eq!(
        session_row(&env, &target_id).3,
        updated_before,
        "不得刷新时间"
    );
}

/// Ahead 只传导归档：目标独有内容完整保留，后续判定不因状态操作虚假变成一致。
#[test]
fn status_sync_preserves_ahead_target_content() {
    let env = ready_env_with_status("status-ahead");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    append_records(&env.body_path(&target_id), &target_id, 2, 2);
    env.set_status("sess-1", "archived");
    env.set_status(&target_id, "completed");

    let preview_report = preview(&env, "uid-b");
    let group = &preview_report["groups"][0];
    assert_eq!(group["verdict"], "ahead", "{preview_report}");
    assert_eq!(group["availableModes"], json!([]), "切号预览仍不给正文模式");
    assert_eq!(group["archiveAction"], "statusOnly");
    assert_eq!(group["defaultChecked"], false);
    let token = group["previewToken"].as_str().unwrap().to_string();
    let group_id = group["groupId"].as_str().unwrap().to_string();

    let body_before = std::fs::read_to_string(env.body_path(&target_id)).unwrap();
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::StatusOnly)],
    );
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(env.status_of(&target_id).as_deref(), Some("archived"));
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        body_before,
        "目标独有内容必须保留"
    );
    // 状态操作不得让后续判定虚假变成 identical。
    let again = preview(&env, "uid-b");
    assert_eq!(again["groups"][0]["verdict"], "ahead", "{again}");
}

/// 快进且来源已归档：提交的是 fastForward，归档作为副作用随正文写入一并生效。
#[test]
fn fast_forward_archives_target_as_side_effect() {
    let env = ready_env_with_status("status-ff");
    let (group_id, token, target_id) = fast_forward_archive_scene(&env, "completed");
    let preview = preview(&env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["availableModes"], json!(["fastForward"]));
    assert_eq!(group["archiveAction"], "statusOnly");

    let updated_before = session_row(&env, &target_id).3;
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(report["synced"][0]["mode"], "fastForward");
    assert_eq!(env.status_of(&target_id).as_deref(), Some("archived"));
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        incoming_text(&env, "sess-1", &target_id),
        "正文必须含来源新记录"
    );
    assert_ne!(
        session_row(&env, &target_id).3,
        updated_before,
        "正文写入照旧更新时间"
    );
}

/// 冲突：归档资格成立也不默认勾选，只有显式提交 overwrite 才随正文归档。
#[test]
fn diverge_still_requires_explicit_overwrite_then_archives() {
    let env = ready_env_with_status("status-diverge");
    let (group_id, token, target_id) = diverge_archive_scene(&env, "completed");
    let preview = preview(&env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["availableModes"], json!(["overwrite"]));
    assert_eq!(
        group["archiveAction"], "statusOnly",
        "预览时即固定，不随勾选变化"
    );
    assert_eq!(group["defaultChecked"], false, "冲突仍不默认勾选");

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::Overwrite)],
    );
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(env.status_of(&target_id).as_deref(), Some("archived"));
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        incoming_text(&env, "sess-1", &target_id)
    );
}

/// 预览后来源取消归档：整体判为预览失效，零状态写入。
#[test]
fn status_sync_skips_when_source_unarchived_after_preview() {
    let env = ready_env_with_status("status-stale-source");
    let (group_id, token, target_id) = archive_scene(&env, "completed");
    env.set_status("sess-1", "completed");

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::StatusOnly)],
    );
    assert!(report["synced"].as_array().unwrap().is_empty(), "{report}");
    assert_eq!(report["skipped"][0]["reasonCode"], REASON_PREVIEW_STALE);
    assert_eq!(env.status_of(&target_id).as_deref(), Some("completed"));
}

/// 预览后目标变为活跃：条件 UPDATE 拒绝，不得把活会话收进归档。
#[test]
fn status_sync_skips_when_target_becomes_active_after_preview() {
    let env = ready_env_with_status("status-stale-target");
    let (group_id, token, target_id) = archive_scene(&env, "completed");
    env.set_status(&target_id, "working");

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::StatusOnly)],
    );
    assert!(report["synced"].as_array().unwrap().is_empty(), "{report}");
    assert_eq!(report["skipped"][0]["reasonCode"], REASON_PREVIEW_STALE);
    assert_eq!(env.status_of(&target_id).as_deref(), Some("working"));
}

/// 伪造 statusOnly 提交：凭据里没有授权即拒绝，不静默执行。
#[test]
fn status_sync_rejects_unauthorized_status_only() {
    // 不带 status 列的库：双方状态读数恒为 None，与伪造凭据一致，
    // 因此走的是「凭据未授权归档」的拒绝路径，而不是预览过期。
    let env = ready_env("status-unauthorized");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    let preview = preview(&env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["verdict"], "identical", "{preview}");
    assert!(group["previewToken"].is_null(), "无动作即无凭据 {group}");

    // 手工伪造一份「identical + statusOnly」凭据：执行必须拒绝。
    let store = env.store();
    let group_id = store.groups[0].id.clone();
    let source_member = store.groups[0]
        .members
        .iter()
        .find(|member| member.uid == "uid-a")
        .unwrap()
        .clone();
    let target_member = store.groups[0]
        .members
        .iter()
        .find(|member| member.uid == "uid-b")
        .unwrap()
        .clone();
    let source_content = member_content_state(&env.paths(), &source_member.session_id);
    let target_content = member_content_state(&env.paths(), &target_member.session_id);
    let baseline = session_link::load_pair_baseline(
        &env.paths(),
        &store.groups[0],
        &source_member.member_id,
        &target_member.member_id,
    );
    let forged = session_link::save_preview_token(
        &env.paths(),
        live_preview_binding(
            &store.groups[0],
            &source_member,
            &target_member,
            &source_content,
            &target_content,
            &baseline,
            SyncVerdict::Identical,
            &PreviewArchiveState::default(),
        ),
    )
    .unwrap();
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &forged, SyncMode::StatusOnly)],
    );
    assert!(report["synced"].as_array().unwrap().is_empty(), "{report}");
    assert_eq!(report["errors"].as_array().unwrap().len(), 1, "{report}");
    assert!(
        report["errors"][0]["error"]
            .as_str()
            .unwrap()
            .contains("不提供仅同步归档"),
        "{report}"
    );
    let _ = target_id;
}

/// 数据库写入中断后恢复：补完归档，无正文写入、无新基线；重复恢复不二次写入。
#[test]
fn status_sync_recovers_after_interrupted_db_write() {
    let env = ready_env_with_status("status-recover");
    let (group_id, token, target_id) = archive_scene(&env, "completed");
    let body_before = body_bytes(&env, &target_id);
    let baselines_before = env.baseline_files();
    let pairs_before = pair_snapshot(&env);

    install_block_update_trigger(&env);
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::StatusOnly)],
    );
    assert!(report["synced"].as_array().unwrap().is_empty(), "{report}");
    assert_eq!(report["errors"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(env.status_of(&target_id).as_deref(), Some("completed"));
    drop_block_update_trigger(&env);

    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert!(
        recovery.needs_recovery.is_empty(),
        "{:?}",
        recovery.needs_recovery
    );
    assert_eq!(recovery.recovered.len(), 1, "{:?}", recovery);
    assert_eq!(env.status_of(&target_id).as_deref(), Some("archived"));
    assert_eq!(body_bytes(&env, &target_id), body_before, "恢复不得写正文");
    assert_eq!(env.baseline_files(), baselines_before, "恢复不得产生基线");
    assert_eq!(pair_snapshot(&env), pairs_before);

    // 重复恢复：已完成的不再重放，也不再刷新时间。
    let updated = session_row(&env, &target_id).3;
    let again = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert!(
        again.needs_recovery.is_empty(),
        "{:?}",
        again.needs_recovery
    );
    assert!(again.recovered.is_empty(), "{:?}", again);
    assert_eq!(session_row(&env, &target_id).3, updated);
}

/// 中断后目标正文被追加：保留并报告人工处理，不补写正文、不越权改状态。
#[test]
fn status_sync_reports_manual_handling_when_body_changed() {
    let env = ready_env_with_status("status-recover-tampered");
    let (group_id, token, target_id) = archive_scene(&env, "completed");
    install_block_update_trigger(&env);
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::StatusOnly)],
    );
    assert!(report["synced"].as_array().unwrap().is_empty(), "{report}");
    drop_block_update_trigger(&env);

    append_records(&env.body_path(&target_id), &target_id, 100, 1);
    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert!(recovery.recovered.is_empty(), "{:?}", recovery);
    assert_eq!(recovery.needs_recovery.len(), 1, "{:?}", recovery);
    assert!(!recovery.needs_recovery[0].retryable);
    assert_eq!(
        env.status_of(&target_id).as_deref(),
        Some("completed"),
        "正文已变即不得补写归档"
    );
}

/// 旧清单缺 status / newStatus：正常解析并按旧行为恢复，不自动归档。
#[test]
fn legacy_manifest_without_status_fields_parses_with_old_behavior() {
    let env = ready_env_with_status("legacy-manifest");
    let (group_id, token, _target_id) = fast_forward_archive_scene(&env, "completed");
    install_block_update_trigger(&env);
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert!(report["synced"].as_array().unwrap().is_empty(), "{report}");
    drop_block_update_trigger(&env);
    let operation = sync_operations(&env)[0].clone();
    let manifest_path = operation.backup.clone().unwrap();
    let text = std::fs::read_to_string(&manifest_path).unwrap();
    let mut value: Value = serde_json::from_str(&text).unwrap();
    {
        let db = value.get_mut("db").unwrap().as_object_mut().unwrap();
        db.remove("newStatus");
        if let Some(row) = db.get_mut("targetRow").and_then(Value::as_object_mut) {
            row.remove("status");
        }
    }
    let dir = env.root.join("legacy-manifest-dir");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(&value).unwrap(),
    )
    .unwrap();
    let parsed = load_sync_manifest(&dir).expect("旧清单必须能解析");
    assert_eq!(parsed.db.new_status, None, "旧清单不得被当成归档意图");
    assert!(parsed
        .db
        .target_row
        .as_ref()
        .and_then(|row| row.status.clone())
        .is_none());
    assert!(
        parsed.new_baseline_ref.is_some(),
        "旧清单的字符串基线引用仍读为 Some"
    );
    assert_eq!(parsed.mode, SyncMode::FastForward);
}

/// 仅同步归档的清单不落新基线引用；正文模式必须有。
#[test]
fn status_only_manifest_has_no_new_baseline_ref() {
    let env = ready_env_with_status("status-manifest-baseline");
    let (group_id, token, _target_id) = archive_scene(&env, "completed");
    install_block_update_trigger(&env);
    let _ = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::StatusOnly)],
    );
    drop_block_update_trigger(&env);
    let operation = sync_operations(&env)[0].clone();
    let manifest = load_sync_manifest(
        Path::new(&operation.backup.clone().unwrap())
            .parent()
            .unwrap(),
    )
    .expect("清单必须可读");
    assert_eq!(manifest.mode, SyncMode::StatusOnly);
    assert_eq!(manifest.new_baseline_ref, None, "状态同步不得写假基线引用");
    assert_eq!(manifest.db.new_status.as_deref(), Some("archived"));
    assert_eq!(
        manifest
            .db
            .target_row
            .as_ref()
            .and_then(|row| row.status.clone()),
        Some("completed".to_string())
    );
}

/// 会话表缺 status 列：不归档、不报列名错误，正文同步照旧可用。
#[test]
fn status_sync_degrades_when_status_column_missing() {
    let env = ready_env("status-no-column");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    append_records(&env.body_path("sess-1"), "sess-1", 2, 3);

    let preview = preview(&env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["verdict"], "fastForward", "{preview}");
    assert!(
        group["archiveAction"].is_null(),
        "缺列即不提供归档动作 {group}"
    );
    let token = group["previewToken"].as_str().unwrap().to_string();
    let group_id = group["groupId"].as_str().unwrap().to_string();

    // 正文同步不受影响。
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        incoming_text(&env, "sess-1", &target_id)
    );
}

/// `read_session_row` 在 custom_title × status 四种列组合下索引都正确。
#[test]
fn read_session_row_handles_every_column_combination() {
    for (has_custom_title, has_status) in
        [(true, true), (true, false), (false, true), (false, false)]
    {
        let env = Env::new(&format!("row-cols-{has_custom_title}-{has_status}"));
        let mut columns = vec![
            "id TEXT PRIMARY KEY".to_string(),
            "user_id TEXT NOT NULL".to_string(),
            "title TEXT".to_string(),
        ];
        if has_custom_title {
            columns.push("custom_title TEXT".to_string());
        }
        if has_status {
            columns.push("status TEXT".to_string());
        }
        columns.push("updated_at INTEGER".to_string());
        columns.push("deleted_at INTEGER".to_string());
        let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
        conn.execute_batch(&format!("CREATE TABLE sessions ({});", columns.join(", ")))
            .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, user_id, title, updated_at, deleted_at) \
             VALUES ('s1', 'uid-a', '标题', 2000, NULL)",
            [],
        )
        .unwrap();
        if has_custom_title {
            conn.execute(
                "UPDATE sessions SET custom_title = '自定义' WHERE id = 's1'",
                [],
            )
            .unwrap();
        }
        if has_status {
            conn.execute(
                "UPDATE sessions SET status = 'completed' WHERE id = 's1'",
                [],
            )
            .unwrap();
        }
        let row = read_session_row(&conn, "s1").unwrap().expect("行必须可读");
        assert_eq!(row.session_id, "s1");
        assert_eq!(row.user_id, "uid-a");
        assert_eq!(row.title.as_deref(), Some("标题"));
        assert_eq!(
            row.custom_title.as_deref(),
            has_custom_title.then_some("自定义"),
            "{has_custom_title}/{has_status}"
        );
        assert_eq!(
            row.status.as_deref(),
            has_status.then_some("completed"),
            "{has_custom_title}/{has_status}"
        );
        assert_eq!(
            row.updated_at,
            Some(2000),
            "{has_custom_title}/{has_status}"
        );
        assert_eq!(row.deleted_at, None);
    }
}

/// `statusOnly` 只能由归档资格授权，不属于任何正文判定的权限。
#[test]
fn status_only_mode_is_not_a_body_permission() {
    assert_eq!(SyncMode::parse("statusOnly").unwrap(), SyncMode::StatusOnly);
    assert_eq!(SyncMode::StatusOnly.as_str(), "statusOnly");
    for verdict in [
        SyncVerdict::Identical,
        SyncVerdict::FastForward,
        SyncVerdict::Ahead,
        SyncVerdict::Diverge,
        SyncVerdict::Unknown,
    ] {
        assert!(!verdict.allows(SyncMode::StatusOnly), "{verdict:?}");
        assert!(
            !verdict.available_modes().contains(&SyncMode::StatusOnly),
            "{verdict:?} 的正文模式里不得出现 statusOnly"
        );
    }
    // 原有模式权限不变。
    assert!(SyncVerdict::FastForward.allows(SyncMode::FastForward));
    assert!(SyncVerdict::Diverge.allows(SyncMode::Overwrite));
    assert!(SyncVerdict::Ahead.allows(SyncMode::UnifyOverwrite));
}

/// 预览绑定的归档字段：旧凭据可反序列化，状态或动作变化必须报过期。
#[test]
fn preview_binding_archive_fields_round_trip_and_are_verified() {
    let env = ready_env_with_status("preview-binding-archive");
    let (group_id, token, _target_id) = archive_scene(&env, "completed");
    let stored = session_link::load_preview_token(&env.paths(), &token).expect("凭据必须存在");
    assert_eq!(stored.version, session_link::PREVIEW_TOKEN_VERSION);
    assert_eq!(stored.binding.source_status.as_deref(), Some("archived"));
    assert_eq!(stored.binding.target_status.as_deref(), Some("completed"));
    assert_eq!(stored.binding.archive_action.as_deref(), Some("statusOnly"));

    // 旧格式（无归档字段）仍能反序列化，且一律按「未授权」处理。
    let mut value = serde_json::to_value(&stored.binding).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("sourceStatus");
    object.remove("targetStatus");
    object.remove("archiveAction");
    let legacy: PreviewBinding = serde_json::from_value(value).unwrap();
    assert_eq!(legacy.source_status, None);
    assert_eq!(legacy.target_status, None);
    assert_eq!(legacy.archive_action, None);

    // 状态或动作任一项变化都必须报过期。
    let mut drifted_status = stored.binding.clone();
    drifted_status.target_status = Some("active".to_string());
    let stale = session_link::verify_preview(&stored, &drifted_status);
    assert!(
        stale.iter().any(|reason| reason.contains("归档状态")),
        "{stale:?}"
    );
    let mut drifted_action = stored.binding.clone();
    drifted_action.archive_action = None;
    let stale = session_link::verify_preview(&stored, &drifted_action);
    assert!(
        stale.iter().any(|reason| reason.contains("归档")),
        "{stale:?}"
    );
    let _ = group_id;
}

/// 旧清单在**带 status 列的库**上必须仍能按旧行为恢复完成。
///
/// 回归 guarding：旧清单反序列化后 `targetRow.status` 为 None，若拿它去和现场读出的
/// `Some("completed")` 比对，会恒不相等并把正常恢复误判成「状态被改动」。
#[test]
fn legacy_manifest_recovers_on_database_with_status_column() {
    let env = ready_env_with_status("legacy-manifest-status-db");
    let (group_id, token, target_id) = fast_forward_archive_scene(&env, "completed");
    install_block_update_trigger(&env);
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert!(report["synced"].as_array().unwrap().is_empty(), "{report}");
    drop_block_update_trigger(&env);

    // 把清单改写成改动之前的旧格式：去掉 status 与 newStatus。
    let operation = sync_operations(&env)[0].clone();
    let manifest_path = operation.backup.clone().unwrap();
    let text = std::fs::read_to_string(&manifest_path).unwrap();
    let mut value: Value = serde_json::from_str(&text).unwrap();
    {
        let db = value.get_mut("db").unwrap().as_object_mut().unwrap();
        db.remove("newStatus");
        if let Some(row) = db.get_mut("targetRow").and_then(Value::as_object_mut) {
            row.remove("status");
        }
    }
    std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&value).unwrap(),
    )
    .unwrap();

    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert!(
        recovery.needs_recovery.is_empty(),
        "旧清单必须能恢复完成：{:?}",
        recovery.needs_recovery
    );
    assert_eq!(recovery.recovered.len(), 1, "{:?}", recovery);
    // 旧清单不含归档意图：正文补完即可，不得顺手归档。
    assert_eq!(env.status_of(&target_id).as_deref(), Some("completed"));
}

/// 只归档遇到正文文件被删：放弃本次归档，不写库、不补正文，也不永久阻断启动。
#[test]
fn status_sync_abandons_when_target_body_gone() {
    let env = ready_env_with_status("status-body-gone");
    let (group_id, token, target_id) = archive_scene(&env, "completed");
    install_block_update_trigger(&env);
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::StatusOnly)],
    );
    assert!(report["synced"].as_array().unwrap().is_empty(), "{report}");
    drop_block_update_trigger(&env);
    std::fs::remove_file(env.body_path(&target_id)).unwrap();

    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert!(
        recovery.needs_recovery.is_empty(),
        "正文缺失不得变成需要人工处理：{:?}",
        recovery.needs_recovery
    );
    assert_eq!(recovery.abandoned.len(), 1, "{:?}", recovery);
    assert_eq!(
        env.status_of(&target_id).as_deref(),
        Some("completed"),
        "放弃时不得写状态"
    );
    assert!(!env.body_path(&target_id).exists(), "不得重建正文文件");
}

/// 不可验证（unknown）时不下发 archiveAction：没有凭据就勾不了，发了只会误导。
#[test]
fn unknown_verdict_never_offers_archive_action() {
    let env = ready_env_with_status("status-unknown");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    env.set_status("sess-1", "archived");
    env.set_status(&target_id, "completed");
    // 删掉目标正文：内容不可验证 → unknown。
    std::fs::remove_file(env.body_path(&target_id)).unwrap();

    let preview = preview(&env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["verdict"], "unknown", "{preview}");
    assert!(group["archiveAction"].is_null(), "{group}");
    assert!(group["previewToken"].is_null(), "{group}");
    assert_eq!(env.status_of(&target_id).as_deref(), Some("completed"));
}

/// 归档已提交、完成标记未落盘，之后正文丢失：放弃本次，保留已提交的状态。
///
/// `DbWritten` 晚于 `BodyWritten`，若放弃分支排在「已写正文却丢失」的通用拦截
/// 之后，这个现场会先被通用分支截走并永久阻断启动。
#[test]
fn status_sync_abandons_when_body_lost_after_db_commit() {
    let env = ready_env_with_status("status-gone-after-commit");
    let (group_id, token, target_id) = archive_scene(&env, "completed");
    install_block_update_trigger(&env);
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::StatusOnly)],
    );
    assert!(report["synced"].as_array().unwrap().is_empty(), "{report}");
    drop_block_update_trigger(&env);

    // 手工推进到「数据库已提交、完成标记未落盘」的现场：状态置为本次新值，
    // 操作阶段停在 DbWritten。
    env.set_status(&target_id, "archived");
    let mut operation = sync_operations(&env)[0].clone();
    operation.phase = OpPhase::DbWritten;
    session_link::save_operation(&env.paths, &operation).unwrap();
    std::fs::remove_file(env.body_path(&target_id)).unwrap();

    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert!(
        recovery.needs_recovery.is_empty(),
        "归档已提交后正文丢失不得变成人工处理：{:?}",
        recovery.needs_recovery
    );
    assert_eq!(recovery.abandoned.len(), 1, "{:?}", recovery);
    assert_eq!(
        env.status_of(&target_id).as_deref(),
        Some("archived"),
        "已提交的状态必须保留"
    );
    assert!(!env.body_path(&target_id).exists(), "不得补写或重建正文");
}

/// 不写状态的正文恢复：目标状态被改成 active 也照常补完，不被状态变化阻断。
#[test]
fn body_only_recovery_tolerates_unrelated_status_change() {
    let env = ready_env_with_status("body-recovery-status-changed");
    let (group_id, token, target_id) = fast_forward_archive_scene(&env, "completed");
    install_block_update_trigger(&env);
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert!(report["synced"].as_array().unwrap().is_empty(), "{report}");
    drop_block_update_trigger(&env);
    // 中断期间目标被打开并开始生成：状态变为 active（与本次正文同步无关）。
    env.set_status(&target_id, "active");

    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert!(
        recovery.needs_recovery.is_empty(),
        "与状态无关的正文恢复不得被阻断：{:?}",
        recovery.needs_recovery
    );
    assert_eq!(recovery.recovered.len(), 1, "{:?}", recovery);
    assert_eq!(
        env.status_of(&target_id).as_deref(),
        Some("active"),
        "本次不写状态，必须保留当前状态"
    );
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        incoming_text(&env, "sess-1", &target_id),
        "正文必须补完"
    );
}

/// 跳过归档副作用后「提交数据库 → 完成标记落盘前」再次崩溃：恢复必须能补完，
/// 不能把自己合法产生的「本次时间戳 + active」判成外部修改。
#[test]
fn body_mode_recovery_survives_second_crash_after_archive_skipped() {
    let env = ready_env_with_status("body-skip-second-crash");
    let (group_id, token, target_id) = fast_forward_archive_scene(&env, "completed");
    install_block_update_trigger(&env);
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert!(report["synced"].as_array().unwrap().is_empty(), "{report}");
    drop_block_update_trigger(&env);
    // 目标被打开并开始生成：归档副作用将被合法跳过。
    env.set_status(&target_id, "active");

    // 手工构造「已跳过归档并提交数据库、完成标记未落盘」的现场。
    let mut operation = sync_operations(&env)[0].clone();
    let manifest = load_sync_manifest(
        Path::new(operation.backup.as_ref().unwrap())
            .parent()
            .unwrap(),
    )
    .expect("清单必须可读");
    env.set_updated_at(&target_id, manifest.db.new_updated_at);
    operation.phase = OpPhase::DbWritten;
    session_link::save_operation(&env.paths, &operation).unwrap();

    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert!(
        recovery.needs_recovery.is_empty(),
        "跳过归档后的二次崩溃不得阻断恢复：{:?}",
        recovery.needs_recovery
    );
    assert_eq!(recovery.recovered.len(), 1, "{:?}", recovery);
    assert_eq!(
        env.status_of(&target_id).as_deref(),
        Some("active"),
        "活跃会话绝不能被归档"
    );
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        incoming_text(&env, "sess-1", &target_id)
    );
}

/// 正文模式下「数据库已提交、归档曾被跳过、目标又回到终态」：恢复不得重做归档。
///
/// 跳过与否是现场状态的纯函数，但**纯函数不等于跨次恢复幂等**——输入状态会变，
/// 重新推导就可能从「跳过」翻成「可归档」。数据库步骤必须恰好执行一次。
#[test]
fn body_mode_recovery_does_not_reapply_skipped_archive() {
    let env = ready_env_with_status("body-no-rearchive");
    let (group_id, token, target_id) = fast_forward_archive_scene(&env, "completed");
    install_block_update_trigger(&env);
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert!(report["synced"].as_array().unwrap().is_empty(), "{report}");
    drop_block_update_trigger(&env);

    // 手工构造「数据库已提交且归档被跳过、完成标记未落盘」的现场：时间戳是本次值，
    // 状态仍是 completed（归档跳过后回到终态，或用户取消了归档）。
    let mut operation = sync_operations(&env)[0].clone();
    let manifest = load_sync_manifest(
        Path::new(operation.backup.as_ref().unwrap())
            .parent()
            .unwrap(),
    )
    .expect("清单必须可读");
    env.set_updated_at(&target_id, manifest.db.new_updated_at);
    operation.phase = OpPhase::DbWritten;
    session_link::save_operation(&env.paths, &operation).unwrap();

    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert!(
        recovery.needs_recovery.is_empty(),
        "数据库已提交后恢复不得再被阻断：{:?}",
        recovery.needs_recovery
    );
    assert_eq!(recovery.recovered.len(), 1, "{:?}", recovery);
    assert_eq!(
        env.status_of(&target_id).as_deref(),
        Some("completed"),
        "恢复不得重做已跳过的归档"
    );
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        incoming_text(&env, "sess-1", &target_id),
        "正文必须已补完"
    );
}

/// 造一个「正文已写入、数据库未更新」的中断现场：返回 (目标会话 id, 覆盖前正文, 本次写入正文)。
fn interrupted_sync_scene(env: &Env) -> (String, Vec<u8>, String) {
    let (group_id, token, target_id) = fast_forward_scene(env);
    let target_before = body_bytes(env, &target_id);
    install_block_update_trigger(env);
    let report = sync(
        env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert_eq!(report["needsRecovery"], true, "{report}");
    drop_block_update_trigger(env);
    let written = std::fs::read_to_string(env.body_path(&target_id)).unwrap();
    assert_eq!(written, incoming_text(env, "sess-1", &target_id));
    assert_eq!(
        session_link::pending_operations(&env.paths, WbVariant::Cn)[0].phase,
        OpPhase::BodyWritten
    );
    (target_id, target_before, written)
}

/// 恢复不覆盖后续无关修改：目标正文被改动过 → 停止恢复并要求人工处理。
#[test]
fn sync_recovery_stops_when_target_changed_after_interrupted_write() {
    let env = ready_env("sync-recovery-unknown");
    let (target_id, _, _) = interrupted_sync_scene(&env);
    let baselines_before = env.baseline_files();
    let pairs_before = pair_snapshot(&env);

    // 官方 App / 用户对中间产物继续追加了内容：属于「未知内容」，不得覆盖。
    append_records(&env.body_path(&target_id), &target_id, 100, 1);
    let tampered = std::fs::read_to_string(env.body_path(&target_id)).unwrap();

    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert!(recovery.recovered.is_empty(), "{:?}", recovery.recovered);
    assert_eq!(recovery.needs_recovery.len(), 1);
    assert!(
        !recovery.needs_recovery[0].retryable,
        "未知内容必须暂停启动：{:?}",
        recovery.needs_recovery[0]
    );
    assert!(
        recovery.needs_recovery[0].reason.contains("不一致")
            || recovery.needs_recovery[0].reason.contains("无法验证"),
        "{}",
        recovery.needs_recovery[0].reason
    );
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        tampered,
        "不得覆盖后续无关修改"
    );
    // 数据库与组表都保持原样，未完成操作保留待人工处理。
    assert_eq!(env.baseline_files(), baselines_before);
    assert_eq!(pair_snapshot(&env), pairs_before);
    assert_eq!(
        session_link::pending_operations(&env.paths, WbVariant::Cn).len(),
        1
    );
    // 编排层据此暂停切换与启动 App。
    assert!(crate::modules::switch::recovery_blocks_startup(&recovery));
}

/// 目标行更新时间被其它程序改动 → 停止恢复（不覆盖未知修改）。
#[test]
fn sync_recovery_stops_when_target_row_changed_after_write() {
    let env = ready_env("sync-recovery-row-drift");
    let (target_id, _, written) = interrupted_sync_scene(&env);

    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute(
        "UPDATE sessions SET updated_at = 987654321 WHERE id = ?1",
        [&target_id],
    )
    .unwrap();
    drop(conn);

    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert!(recovery.recovered.is_empty(), "{:?}", recovery.recovered);
    assert_eq!(recovery.needs_recovery.len(), 1);
    assert!(!recovery.needs_recovery[0].retryable);
    assert!(
        recovery.needs_recovery[0].reason.contains("更新时间"),
        "{}",
        recovery.needs_recovery[0].reason
    );
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        written
    );
}

/// 目标行归属不符（被改到别的账号）→ 停止恢复，不报成功。
#[test]
fn sync_recovery_stops_when_target_row_owner_changed() {
    let env = ready_env("sync-recovery-owner");
    let (target_id, _, written) = interrupted_sync_scene(&env);

    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute(
        "UPDATE sessions SET user_id = 'uid-x' WHERE id = ?1",
        [&target_id],
    )
    .unwrap();
    drop(conn);

    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert!(recovery.recovered.is_empty(), "{:?}", recovery.recovered);
    assert_eq!(recovery.needs_recovery.len(), 1);
    assert!(!recovery.needs_recovery[0].retryable);
    assert!(
        recovery.needs_recovery[0].reason.contains("归属"),
        "{}",
        recovery.needs_recovery[0].reason
    );
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        written,
        "归属异常时不得继续改写内容"
    );
    assert_eq!(
        session_link::pending_operations(&env.paths, WbVariant::Cn).len(),
        1
    );
}

/// 恢复期间数据库不可读：不覆盖正文/行/基线，保留 pending 并暂停启动；解除后恢复成功。
#[test]
fn sync_recovery_preserves_committed_state_on_database_read_failure() {
    for fault in ["open", "schema", "query"] {
        let env = ready_env(&format!("sync-read-failure-{fault}"));
        // 写入阶段被故障中断：操作未完成、临时备份按契约保留（只在完成后才清理）。
        let (target_id, _, _) = interrupted_sync_scene(&env);
        let operation = session_link::pending_operations(&env.paths, WbVariant::Cn)
            .into_iter()
            .next()
            .expect("故障后必须保留未完成操作");
        let body_before = body_bytes(&env, &target_id);
        let row_before = session_row(&env, &target_id);
        let pairs_before = pair_snapshot(&env);
        let baselines_before = env.baseline_files();
        let db = env.paths.workbuddy_db();
        let saved_db = db.with_extension("saved");
        match fault {
            "open" => std::fs::rename(&db, &saved_db).unwrap(),
            "schema" => Connection::open(&db)
                .unwrap()
                .execute_batch("ALTER TABLE sessions RENAME TO unavailable_sessions")
                .unwrap(),
            "query" => Connection::open(&db)
                .unwrap()
                .execute_batch(
                    "ALTER TABLE sessions RENAME COLUMN updated_at TO unavailable_updated_at",
                )
                .unwrap(),
            _ => unreachable!(),
        }
        let existing_db = if fault == "open" { &saved_db } else { &db };
        let db_before = std::fs::read(existing_db).unwrap();
        let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
        assert!(recovery.recovered.is_empty(), "{fault}");
        assert!(recovery.abandoned.is_empty(), "{fault}");
        assert_eq!(recovery.needs_recovery.len(), 1, "{fault}");
        assert!(
            crate::modules::switch::recovery_blocks_startup(&recovery),
            "{fault}"
        );
        assert_eq!(body_bytes(&env, &target_id), body_before, "{fault}");
        assert_eq!(std::fs::read(existing_db).unwrap(), db_before, "{fault}");
        assert_eq!(pair_snapshot(&env), pairs_before, "{fault}");
        assert_eq!(env.baseline_files(), baselines_before, "{fault}");
        let pending = session_link::pending_operations(&env.paths, WbVariant::Cn);
        assert_eq!(pending.len(), 1, "{fault}");
        assert_eq!(pending[0].operation_id, operation.operation_id);
        assert_eq!(pending[0].phase, OpPhase::BodyWritten);

        match fault {
            "open" => std::fs::rename(&saved_db, &db).unwrap(),
            "schema" => Connection::open(&db)
                .unwrap()
                .execute_batch("ALTER TABLE unavailable_sessions RENAME TO sessions")
                .unwrap(),
            "query" => Connection::open(&db)
                .unwrap()
                .execute_batch(
                    "ALTER TABLE sessions RENAME COLUMN unavailable_updated_at TO updated_at",
                )
                .unwrap(),
            _ => unreachable!(),
        }
        let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
        assert!(
            recovery.is_clean(),
            "{fault}: {:?}",
            recovery.needs_recovery
        );
        assert_eq!(recovery.recovered, vec![operation.operation_id.clone()]);
        assert_eq!(body_bytes(&env, &target_id), body_before);
        // 补完之后才更新目标行与基线：只改 updated_at，身份/标题不动。
        let row_after = session_row(&env, &target_id);
        assert_eq!(row_after.0, row_before.0);
        assert_eq!(row_after.1, row_before.1);
        assert_eq!(row_after.2, row_before.2);
        assert!(row_after.3.unwrap() > row_before.3.unwrap());
        assert_eq!(row_after.4, None);
        assert_eq!(env.baseline_files(), baselines_before + 1);
        assert_ne!(pair_snapshot(&env), pairs_before);
        assert_eq!(sync_operations(&env)[0].phase, OpPhase::Completed);
        assert!(
            !session_backup::transaction_dir(&env.paths, WbVariant::Cn, &operation.operation_id)
                .unwrap()
                .exists(),
            "{fault}: 恢复完成后必须回收临时备份"
        );
    }
}

#[test]
fn sync_row_snapshot_supports_legacy_schema_and_distinguishes_missing_row() {
    let conn = Connection::open_in_memory().unwrap();
    assert!(read_session_row(&conn, "session").is_err());
    conn.execute_batch(
        "CREATE TABLE sessions (id TEXT, user_id TEXT, title TEXT, updated_at INTEGER, deleted_at INTEGER);
         INSERT INTO sessions VALUES ('session', 'uid', 'title', 123, NULL);",
    ).unwrap();
    let row = read_session_row(&conn, "session").unwrap().unwrap();
    assert_eq!(row.custom_title, None);
    assert_eq!(row.title.as_deref(), Some("title"));
    assert!(read_session_row(&conn, "absent").unwrap().is_none());
    conn.execute_batch("UPDATE sessions SET updated_at = 'invalid'")
        .unwrap();
    assert!(read_session_row(&conn, "session").is_err());
}

/// 目标行已不存在（硬删除）→ 无法补完，按备份回滚正文，不留无行的半成品。
#[test]
fn sync_recovery_rolls_back_body_when_target_row_is_gone() {
    let env = ready_env("sync-recovery-row-gone");
    let (target_id, target_before, _) = interrupted_sync_scene(&env);
    let baselines_before = env.baseline_files();
    env.delete_row(&target_id);

    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert_eq!(recovery.abandoned.len(), 1, "{:?}", recovery.recovered);
    assert!(recovery.is_clean(), "{:?}", recovery.needs_recovery);
    assert_eq!(
        body_bytes(&env, &target_id),
        target_before,
        "必须按备份回滚成覆盖前内容"
    );
    assert_eq!(env.baseline_files(), baselines_before, "基线未被提交");
    assert!(session_link::pending_operations(&env.paths, WbVariant::Cn).is_empty());
    assert!(try_session_row(&env, &target_id).is_none(), "目标行已删除");
}

/// 目标行被软删除（deleted_at 非空）→ 同样无法补完，回滚正文并记为放弃。
#[test]
fn sync_recovery_rolls_back_body_when_target_row_is_soft_deleted() {
    let env = ready_env("sync-recovery-row-deleted");
    let (target_id, target_before, _) = interrupted_sync_scene(&env);
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute(
        "UPDATE sessions SET deleted_at = 123456 WHERE id = ?1",
        [&target_id],
    )
    .unwrap();
    drop(conn);

    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert_eq!(recovery.abandoned.len(), 1, "{:?}", recovery.recovered);
    assert!(recovery.is_clean(), "{:?}", recovery.needs_recovery);
    assert_eq!(body_bytes(&env, &target_id), target_before);
    assert!(session_link::pending_operations(&env.paths, WbVariant::Cn).is_empty());
}

/// 同一请求里重复勾选同一组：第一次写入后凭据即失效，第二次跳过，不重复写。
#[test]
fn sync_duplicate_selection_in_one_request_writes_once() {
    let env = ready_env("sync-duplicate-selection");
    let (group_id, token, target_id) = fast_forward_scene(&env);
    let baselines_before = env.baseline_files();
    let expected = incoming_text(&env, "sess-1", &target_id);
    let once = selection(&group_id, &token, SyncMode::FastForward);

    let report = sync(&env, "uid-b", &[once.clone(), once]);
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    assert!(report["errors"].as_array().unwrap().is_empty(), "{report}");
    let skipped = report["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 1, "{report}");
    assert_eq!(skipped[0]["reasonCode"], REASON_PREVIEW_STALE);
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        expected
    );
    assert_eq!(env.body_files().len(), 2, "不得产生第二份副本");
    assert_eq!(env.baseline_files(), baselines_before + 1, "只提交一次基线");
    assert_eq!(sync_operations(&env).len(), 1, "只留一条同步操作");
}

/// 同一目标仍有未完成写入时，本轮不得再写一次（等恢复完成）。
#[test]
fn sync_refuses_to_write_while_previous_operation_is_unfinished() {
    let env = ready_env("sync-pending-guard");
    let (group_id, token, target_id) = fast_forward_scene(&env);
    let target_before = body_bytes(&env, &target_id);
    let paths = env.paths();
    // 手工留下一条缺备份清单的未完成同步操作：恢复无法完成它，只能保持 pending。
    session_link::save_operation(
        &paths,
        &Operation {
            version: OPERATION_VERSION,
            operation_id: "op-sync-pending".to_string(),
            kind: OPERATION_KIND_SYNC.to_string(),
            variant: WbVariant::Cn,
            source_variant: None,
            group_id: group_id.clone(),
            source: OperationMember {
                account_id: None,
                uid: "uid-a".to_string(),
                session_id: "sess-1".to_string(),
            },
            target: OperationMember {
                account_id: None,
                uid: "uid-b".to_string(),
                session_id: target_id.clone(),
            },
            expected_content_digest: "d".to_string(),
            expected_record_count: 1,
            phase: OpPhase::Prepared,
            backup: None,
            lifecycle_version: None,
            cleanup_state: None,
            last_error: None,
            aux_skipped: false,
            created_at: 1,
            updated_at: 1,
        },
    )
    .unwrap();

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert!(report["synced"].as_array().unwrap().is_empty(), "{report}");
    assert_eq!(report["needsRecovery"], true, "{report}");
    assert_eq!(report["errors"].as_array().unwrap().len(), 1, "{report}");
    let error = report["errors"][0]["error"].as_str().unwrap();
    assert!(error.contains("上一次会话保存尚未完成"), "{error}");
    assert_eq!(body_bytes(&env, &target_id), target_before);
    assert_eq!(env.body_files().len(), 2);
    assert_eq!(
        session_link::pending_operations(&paths, WbVariant::Cn).len(),
        1,
        "旧操作保留待恢复，不新增待写操作"
    );
}

/// 同步的备份清单被删/为空目录时不得盲目重放。
#[test]
fn sync_recovery_stops_without_usable_backup() {
    let env = ready_env("sync-recovery-no-backup");
    let (group_id, _token, target_id) = fast_forward_scene(&env);
    let target_before = body_bytes(&env, &target_id);
    let paths = env.paths();
    session_link::save_operation(
        &paths,
        &Operation {
            version: OPERATION_VERSION,
            operation_id: "op-sync-no-backup".to_string(),
            kind: OPERATION_KIND_SYNC.to_string(),
            variant: WbVariant::Cn,
            source_variant: None,
            group_id,
            source: OperationMember {
                account_id: None,
                uid: "uid-a".to_string(),
                session_id: "sess-1".to_string(),
            },
            target: OperationMember {
                account_id: None,
                uid: "uid-b".to_string(),
                session_id: target_id.clone(),
            },
            expected_content_digest: "d".to_string(),
            expected_record_count: 1,
            phase: OpPhase::BodyWritten,
            backup: Some(
                env.paths
                    .backup_root()
                    .join("session-transactions/cn/does-not-exist/manifest.json")
                    .to_string_lossy()
                    .to_string(),
            ),
            lifecycle_version: Some(OPERATION_LIFECYCLE_VERSION),
            cleanup_state: None,
            last_error: None,
            aux_skipped: false,
            created_at: 1,
            updated_at: 1,
        },
    )
    .unwrap();

    let recovery = recover_pending_session_operations_at(&paths, WbVariant::Cn);
    assert!(recovery.recovered.is_empty(), "{:?}", recovery.recovered);
    assert_eq!(recovery.needs_recovery.len(), 1);
    assert!(!recovery.needs_recovery[0].retryable);
    assert!(
        recovery.needs_recovery[0].reason.contains("备份"),
        "{}",
        recovery.needs_recovery[0].reason
    );
    assert_eq!(body_bytes(&env, &target_id), target_before);
}

/// 结束一条「目标已被改写」的未完成同步：只改记录、保留备份，且之后不再挡住同步。
#[test]
fn discard_unfinished_sync_ends_stuck_record_and_keeps_backup() {
    let env = ready_env_with_status("discard-stuck");
    let (group_id, token, target_id) = archive_scene(&env, "completed");
    install_block_update_trigger(&env);
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::StatusOnly)],
    );
    assert!(report["synced"].as_array().unwrap().is_empty(), "{report}");
    drop_block_update_trigger(&env);
    // 中断期间目标被打开并追加：现场变成「不可验证」，自动恢复会拒绝。
    append_records(&env.body_path(&target_id), &target_id, 100, 1);
    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert_eq!(recovery.needs_recovery.len(), 1, "{:?}", recovery);

    let operation = sync_operations(&env)[0].clone();
    let backup_dir = PathBuf::from(operation.backup.clone().unwrap())
        .parent()
        .unwrap()
        .to_path_buf();
    assert!(backup_dir.exists(), "前提：中断现场留有备份");

    let result = discard_unfinished_sync(&env.paths, &operation.operation_id).expect("应允许结束");
    assert!(result.backup_kept, "目标可能被改写时必须保留备份");
    assert!(backup_dir.exists(), "备份必须留在原地");

    let after = sync_operations(&env)[0].clone();
    assert_eq!(after.phase, OpPhase::Abandoned);
    assert_eq!(after.cleanup_state, None, "不得给维护入口删除授权");
    assert!(after
        .last_error
        .as_deref()
        .unwrap_or_default()
        .contains("用户手动结束"));

    // 记录已不再是「未完成」：后续恢复不再上报，同步也不会再被它挡住。
    let again = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert!(again.needs_recovery.is_empty(), "{:?}", again);
    assert!(again.recovered.is_empty(), "{:?}", again);

    // 重复结束：明确拒绝，而不是默默成功。
    let second = discard_unfinished_sync(&env.paths, &operation.operation_id);
    assert!(second.is_err(), "已结束的记录不应再次结束");
}

/// 未知 id 与「不完整扫描」之外的常规错误：给可读文案，不 panic。
#[test]
fn discard_unfinished_sync_rejects_unknown_operation() {
    let env = ready_env_with_status("discard-unknown-id");
    let error = discard_unfinished_sync(&env.paths, "not-a-real-operation").unwrap_err();
    assert!(error.contains("找不到这条操作记录"), "{error}");
}

/// 列表：只列未完成项，标题取自操作自身档位的数据根（单测注入同一个临时目录）。
#[test]
fn pending_session_writes_lists_unfinished_with_title() {
    let env = ready_env_with_status("pending-list");
    let (group_id, token, target_id) = archive_scene(&env, "completed");
    install_block_update_trigger(&env);
    let _ = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::StatusOnly)],
    );
    drop_block_update_trigger(&env);

    let items = pending_session_operations_with(&env.paths, |_| env.paths.clone())
        .expect("列表应可读取");
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].target_session_id, target_id);
    assert_eq!(items[0].kind, "sync");
    assert!(items[0].discardable, "同步的中断记录应可结束");
    assert!(items[0].title.is_some(), "标题应从 sessions 表解析出来");

    // 结束后列表清空（记录仍是 Abandoned，但不再是「未完成」）。
    discard_unfinished_sync(&env.paths, &items[0].operation_id).expect("应可结束");
    let after = pending_session_operations_with(&env.paths, |_| env.paths.clone()).unwrap();
    assert!(after.is_empty(), "{after:?}");
}
