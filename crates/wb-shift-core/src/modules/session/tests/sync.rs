// ---------------------------------------------------------------------------
// 同步预览与执行契约（S5）
// ---------------------------------------------------------------------------

/// 追加 `count` 条有序记录（模拟用户在来源账号继续对话）。
fn append_records(path: &Path, cid: &str, from: usize, count: usize) {
    let mut text = std::fs::read_to_string(path).unwrap();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    for index in from..from + count {
        text.push_str(&json!({"type": "assistant", "sessionId": cid, "index": index}).to_string());
        text.push('\n');
    }
    std::fs::write(path, &text).unwrap();
}

fn preview(env: &Env, target_uid: &str) -> Value {
    session_links_preview_at(&env.paths(), WbVariant::Cn, &env.target(target_uid)).unwrap()
}

fn selection(group_id: &str, preview_token: &str, mode: SyncMode) -> SyncSelection {
    SyncSelection {
        group_id: group_id.to_string(),
        preview_token: preview_token.to_string(),
        mode,
    }
}

fn sync(env: &Env, target_uid: &str, selections: &[SyncSelection]) -> Value {
    sync_sessions_for_switch_at(
        &env.paths(),
        WbVariant::Cn,
        &env.target(target_uid),
        selections,
        |_| false,
    )
    .unwrap()
}

/// 组内所有配对基线的快照：同步不得推进任何关联版本（含第三方成员的配对）。
fn pair_snapshot(env: &Env) -> Vec<String> {
    let mut items: Vec<String> = env
        .store()
        .groups
        .iter()
        .flat_map(|group| {
            group
                .pair_bases
                .iter()
                .map(|pair| format!("{}/{}", pair.member_ids.join("+"), pair.baseline_ref))
        })
        .collect();
    items.sort();
    items
}

/// 造一个可快进的场景：A→B 复制后来源追加 3 条记录，返回 (groupId, 预览凭据, 目标会话 id)。
fn fast_forward_scene(env: &Env) -> (String, String, String) {
    let report = copy(env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    append_records(&env.body_path("sess-1"), "sess-1", 0, 3);

    let preview = preview(env, "uid-b");
    assert_eq!(preview["groups"].as_array().unwrap().len(), 1, "{preview}");
    let group = &preview["groups"][0];
    assert_eq!(group["verdict"], "fastForward", "{preview}");
    (
        group["groupId"].as_str().unwrap().to_string(),
        group["previewToken"].as_str().unwrap().to_string(),
        target_id,
    )
}

/// 预览报告 fastForward 与默认勾选；预览本身是只读的（不改正文与关联版本）。
#[test]
fn preview_reports_fast_forward_and_stays_read_only() {
    let env = ready_env("sync-preview-ff");
    let (_, _, target_id) = fast_forward_scene(&env);
    let target_body_before = std::fs::read_to_string(env.body_path(&target_id)).unwrap();
    let revision_before = env.store().revision;
    let baselines_before = env.baseline_files();

    let preview = preview(&env, "uid-b");
    assert_eq!(preview["supported"], true);
    assert_eq!(preview["storeStatus"], "ready");
    assert_eq!(preview["sourceUid"], "uid-a");
    assert_eq!(preview["targetUid"], "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["title"], "标题一");
    assert_eq!(group["cwd"], "/ws/a");
    assert_eq!(group["verdict"], "fastForward");
    assert_eq!(group["defaultChecked"], true);
    assert_eq!(group["extraA"], 3);
    assert_eq!(group["extraB"], 0);
    assert_eq!(group["common"], 2);
    assert_eq!(group["availableModes"], json!(["fastForward"]));
    assert_eq!(group["recordCount"]["source"], 5);
    assert_eq!(group["recordCount"]["target"], 2);
    assert_eq!(group["recordCount"]["baseline"], 2);
    assert_eq!(group["source"]["uid"], "uid-a");
    assert_eq!(group["target"]["sessionId"], target_id);
    assert_eq!(group["target"]["state"], "active");
    assert!(group["previewToken"].as_str().is_some());
    assert!(group["reason"].as_str().unwrap().contains("可以直接同步"));
    // 预览是只读的：目标正文与关联版本都不变。
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        target_body_before
    );
    assert_eq!(env.store().revision, revision_before);
    assert_eq!(env.baseline_files(), baselines_before);
}

/// 仅目标变化 → ahead：普通同步不可勾选，但为显式整组统一保留校验凭据。
#[test]
fn preview_reports_ahead_when_only_target_changed() {
    let env = ready_env("sync-preview-ahead");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    append_records(&env.body_path(&target_id), &target_id, 0, 5);
    let target_body_before = std::fs::read_to_string(env.body_path(&target_id)).unwrap();
    let revision_before = env.store().revision;

    let preview = preview(&env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["verdict"], "ahead");
    assert_eq!(group["defaultChecked"], false);
    assert_eq!(group["extraA"], 0);
    assert_eq!(group["extraB"], 5);
    assert_eq!(group["availableModes"], json!([]));
    assert!(
        group["previewToken"].as_str().is_some(),
        "显式整组统一需要绑定当前内容"
    );
    assert!(group["reason"]
        .as_str()
        .unwrap()
        .contains("只有目标账号新增"));

    // 预览不写目标：正文与关联版本都不变。
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        target_body_before
    );
    assert_eq!(env.store().revision, revision_before);
}

#[test]
fn explicit_group_unify_can_replace_ahead_but_ordinary_overwrite_cannot() {
    let env = ready_env("sync-unify-ahead");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    append_records(&env.body_path(&target_id), &target_id, 0, 5);
    let group = preview(&env, "uid-b")["groups"][0].clone();
    assert_eq!(group["verdict"], "ahead");
    assert_eq!(group["availableModes"], json!([]));
    let group_id = group["groupId"].as_str().unwrap();
    let token = group["previewToken"].as_str().unwrap();

    let rejected = sync(
        &env,
        "uid-b",
        &[selection(group_id, token, SyncMode::Overwrite)],
    );
    assert!(rejected["synced"].as_array().unwrap().is_empty());
    assert_eq!(rejected["errors"].as_array().unwrap().len(), 1);

    let applied = sync(
        &env,
        "uid-b",
        &[selection(group_id, token, SyncMode::UnifyOverwrite)],
    );
    assert_eq!(applied["synced"].as_array().unwrap().len(), 1, "{applied}");
    let source_text = std::fs::read_to_string(env.body_path("sess-1")).unwrap();
    let target_text = std::fs::read_to_string(env.body_path(&target_id)).unwrap();
    assert_eq!(
        session_link::normalize_jsonl(&source_text, "sess-1").unwrap(),
        session_link::normalize_jsonl(&target_text, &target_id).unwrap(),
    );
}

/// 两边各自新增不同内容（真分歧）→ 统一覆盖后重新预览必须变成一致。
///
/// 回归自 issue #3 最后一条：用户选「以此为准」后，组里仍显示「有分歧」消不掉。
/// 这里验证内核层：只要统一写入成功，重新判定就必须是 identical。
#[test]
fn unify_clears_divergence_after_both_sides_changed() {
    let env = ready_env("sync-unify-diverged");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);

    // 两边各自追加不同内容：条数相同、内容不同 = 真分歧。
    append_records(&env.body_path("sess-1"), "sess-1", 10, 2);
    append_records(&env.body_path(&target_id), &target_id, 20, 2);
    let group = preview(&env, "uid-b")["groups"][0].clone();
    assert_eq!(group["verdict"], "diverge", "{group}");

    let group_id = group["groupId"].as_str().unwrap();
    let token = group["previewToken"].as_str().unwrap();
    let applied = sync(
        &env,
        "uid-b",
        &[selection(group_id, token, SyncMode::Overwrite)],
    );
    assert_eq!(applied["synced"].as_array().unwrap().len(), 1, "{applied}");

    let after = preview(&env, "uid-b")["groups"][0].clone();
    assert_eq!(
        after["verdict"], "identical",
        "统一后仍报分歧，说明状态没有随写入收敛：{after}"
    );
}

/// 一方有效成员缺失（失效/被替换）→ 不提供写入动作，记录数按契约给 0 而不是 null。
#[test]
fn preview_reports_invalid_member_as_unknown_without_null_record_counts() {
    let env = ready_env("sync-preview-invalid-member");
    let (group_id, _token, target_id) = fast_forward_scene(&env);
    session_link::with_link_store_write(&env.paths(), |store| {
        let group = store
            .groups
            .iter_mut()
            .find(|group| group.id == group_id)
            .expect("组必须存在");
        let member_id = session_link::active_member_for(group, "uid-b")
            .expect("目标成员原本有效")
            .member_id
            .clone();
        assert!(session_link::set_member_state(
            group,
            &member_id,
            MemberState::Stale
        ));
        Ok(())
    })
    .unwrap();

    let preview = preview(&env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["verdict"], "unknown", "{preview}");
    assert_eq!(group["defaultChecked"], false);
    assert_eq!(group["availableModes"], json!([]));
    assert!(group.get("previewToken").is_none(), "不可执行的组不发凭据");
    assert!(
        group["reason"]
            .as_str()
            .unwrap()
            .contains("对应的会话已失效"),
        "{preview}"
    );
    // 契约：不可验证时 source/target 为 0、baseline 为 null（前端类型据此声明）。
    assert_eq!(
        group["recordCount"],
        json!({"source": 0, "target": 0, "baseline": null}),
        "{preview}"
    );
    // 两侧成员状态仍要展示（用户据此判断是哪一侧失效）。
    assert_eq!(group["target"]["sessionId"], target_id);
    assert_eq!(group["target"]["state"], "stale");
    assert_eq!(group["source"]["state"], "active");
}

/// 来源会话行已不存在（客户端清理过）→ 该组不再出现在预览列表里。
///
/// 这类组本来就不可能同步（判定必然 unknown），继续列出只会堆满 `(无标题)` +
/// 「无法确认」；过滤只发生在读路径，关联组与目标副本都不得被改动。
#[test]
fn preview_omits_group_when_source_row_is_gone() {
    let env = ready_env("sync-preview-source-row-gone");
    let (group_id, _, target_id) = fast_forward_scene(&env);
    let revision_before = env.store().revision;
    env.delete_row("sess-1");

    let preview = preview(&env, "uid-b");
    assert_eq!(preview["storeStatus"], "ready", "{preview}");
    assert!(
        preview["groups"].as_array().unwrap().is_empty(),
        "失效组不得出现在预览里：{preview}"
    );
    // 只过滤不清理：关联组、目标行与关联版本都保持原样。
    assert!(env.store().groups.iter().any(|group| group.id == group_id));
    assert_eq!(env.store().revision, revision_before);
    assert_eq!(
        session_row_owner(&env.paths(), &target_id).as_deref(),
        Some("uid-b")
    );
}

/// 目标会话行已不存在 → 同样不列出（两侧任一失效即整组剔除）。
#[test]
fn preview_omits_group_when_target_row_is_gone() {
    let env = ready_env("sync-preview-target-row-gone");
    let (group_id, _, target_id) = fast_forward_scene(&env);
    env.delete_row(&target_id);

    let preview = preview(&env, "uid-b");
    assert!(
        preview["groups"].as_array().unwrap().is_empty(),
        "{preview}"
    );
    assert!(env.store().groups.iter().any(|group| group.id == group_id));
}

/// 会话行被软删除（`deleted_at` 非空）→ 与行不存在同口径，不列出。
#[test]
fn preview_omits_group_when_row_is_soft_deleted() {
    // 来源软删除。
    let env = ready_env("sync-preview-soft-deleted-source");
    let (group_id, _, _) = fast_forward_scene(&env);
    env.soft_delete_row("sess-1");
    let source_deleted = preview(&env, "uid-b");
    assert!(
        source_deleted["groups"].as_array().unwrap().is_empty(),
        "{source_deleted}"
    );
    assert!(env.store().groups.iter().any(|group| group.id == group_id));

    // 目标软删除。
    let env = ready_env("sync-preview-soft-deleted-target");
    let (group_id, _, target_id) = fast_forward_scene(&env);
    env.soft_delete_row(&target_id);
    let target_deleted = preview(&env, "uid-b");
    assert!(
        target_deleted["groups"].as_array().unwrap().is_empty(),
        "{target_deleted}"
    );
    assert!(env.store().groups.iter().any(|group| group.id == group_id));
}

/// 行还在但归属改成了别的账号 → 成员失效，不列出。
#[test]
fn preview_omits_group_when_row_owner_changed() {
    let env = ready_env("sync-preview-owner-changed");
    let (group_id, _, _) = fast_forward_scene(&env);
    env.set_row_owner("sess-1", "uid-x");

    let preview = preview(&env, "uid-b");
    assert!(
        preview["groups"].as_array().unwrap().is_empty(),
        "{preview}"
    );
    assert!(env.store().groups.iter().any(|group| group.id == group_id));
}

/// 回归：两侧会话行都存活时照常列出，标题与判定不受过滤影响（不能过度过滤）。
#[test]
fn preview_keeps_live_group_with_title() {
    let env = ready_env("sync-preview-live-kept");
    let (group_id, _, target_id) = fast_forward_scene(&env);

    let preview = preview(&env, "uid-b");
    let groups = preview["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1, "{preview}");
    assert_eq!(groups[0]["groupId"], group_id.as_str());
    assert_eq!(groups[0]["title"], "标题一");
    assert_eq!(groups[0]["cwd"], "/ws/a");
    assert_eq!(groups[0]["verdict"], "fastForward");
    assert_eq!(groups[0]["target"]["sessionId"], target_id);
}

/// 同账号的旧成员（superseded）行已清理，不影响当前成员的组，且标题取**活成员**。
///
/// 真实顺序由 `add_active_member` 决定：旧 active 原位转 superseded、新成员 push 到末尾，
/// 所以「第一条该 uid 的成员」往往是被替换的旧副本。这里把旧成员放在 index 0 复现该顺序：
/// 若按第一条取代表成员，标题会变成 `(无标题)`、`source.state` 会报成 superseded。
#[test]
fn preview_keeps_group_and_takes_live_member_title_when_legacy_row_is_gone() {
    let env = ready_env("sync-preview-superseded-gone");
    let (group_id, _, _) = fast_forward_scene(&env);
    let paths = env.paths();
    session_link::with_link_store_write(&paths, |store| {
        let group = store
            .groups
            .iter_mut()
            .find(|group| group.id == group_id)
            .expect("组必须存在");
        group.members.insert(
            0,
            LinkMember {
                member_id: "m-uid-a-legacy".to_string(),
                account_id: None,
                uid: "uid-a".to_string(),
                session_id: "sess-a-legacy".to_string(),
                variant: None,
                state: MemberState::Superseded,
                linked_at: 1,
                last_synced_at: None,
            },
        );
        Ok(())
    })
    .unwrap();

    let preview = preview(&env, "uid-b");
    let groups = preview["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1, "旧成员行缺失不得过滤当前成员的组：{preview}");
    assert_eq!(groups[0]["groupId"], group_id.as_str());
    // 标题与摘要取代表成员（active），不是排在第一条的旧副本。
    assert_eq!(groups[0]["title"], "标题一", "{preview}");
    assert_eq!(groups[0]["cwd"], "/ws/a", "{preview}");
    assert_eq!(groups[0]["source"]["state"], "active", "{preview}");
    assert_eq!(groups[0]["source"]["sessionId"], "sess-1", "{preview}");
    assert_eq!(groups[0]["verdict"], "fastForward", "{preview}");
    assert_eq!(groups[0]["defaultChecked"], true, "{preview}");
}

/// 当前副本（active）行已缺失、只剩旧副本行存活 → 整组不列出。
///
/// 不能因为历史旧副本的行还在就继续展示：这种组判定必然 unknown，正是用户报告的
/// `(无标题)` + 「无法确认」形态。
#[test]
fn preview_omits_group_when_active_row_is_gone_but_legacy_row_lives() {
    let env = ready_env("sync-preview-active-gone-legacy-live");
    let (group_id, _, target_id) = fast_forward_scene(&env);
    // 给 uid-b 造一个「仍然存活」的旧副本行，并把对应的 superseded 成员放进组里。
    env.add_session("sess-b-legacy", "uid-b", "旧副本");
    let paths = env.paths();
    session_link::with_link_store_write(&paths, |store| {
        let group = store
            .groups
            .iter_mut()
            .find(|group| group.id == group_id)
            .expect("组必须存在");
        group.members.insert(
            0,
            LinkMember {
                member_id: "m-uid-b-legacy".to_string(),
                account_id: None,
                uid: "uid-b".to_string(),
                session_id: "sess-b-legacy".to_string(),
                variant: None,
                state: MemberState::Superseded,
                linked_at: 1,
                last_synced_at: None,
            },
        );
        Ok(())
    })
    .unwrap();
    // 旧副本行还活着时组照常列出（代表成员 active 的 0 行仍存活）。
    let before = preview(&env, "uid-b");
    assert_eq!(before["groups"].as_array().unwrap().len(), 1, "{before}");

    // 当前副本行消失 → 代表成员失效，整组剔除（即便旧副本行还在）。
    env.delete_row(&target_id);
    let after = preview(&env, "uid-b");
    assert!(
        after["groups"].as_array().unwrap().is_empty(),
        "当前副本行缺失的组不得再列出：{after}"
    );
    assert!(env.store().groups.iter().any(|group| group.id == group_id));
}

/// 无 active 成员时，代表成员退化为 `linked_at` 最大的成员（标题与存活判定同口径）。
///
/// 该账号有两条非 active 成员：`linked_at` 小的一条指向「旧」行、大的一条指向「新」行。
/// 代表成员必须取 `linked_at` 最大者——取第一条/最小者标题会变成旧行标题；而存活判定若
/// 退化成「任意一条成员存活」，当前（最新）副本行缺失时该组会被错误保留。
#[test]
fn preview_falls_back_to_latest_member_when_account_has_no_active_member() {
    let env = ready_env("sync-preview-latest-member-fallback");
    let (group_id, _, _) = fast_forward_scene(&env);
    env.add_session("sess-a-old", "uid-a", "旧标题");
    env.add_session("sess-a-new", "uid-a", "新标题");
    let paths = env.paths();
    session_link::with_link_store_write(&paths, |store| {
        let group = store
            .groups
            .iter_mut()
            .find(|group| group.id == group_id)
            .expect("组必须存在");
        // 该账号不再有 active 成员：换成两条非 active 副本，靠 linked_at 区分新旧。
        group.members.retain(|member| member.uid != "uid-a");
        group.pair_bases.clear();
        for (member_id, session_id, linked_at) in [
            ("m-uid-a-old", "sess-a-old", 1_i64),
            ("m-uid-a-new", "sess-a-new", 2_i64),
        ] {
            group.members.push(LinkMember {
                member_id: member_id.to_string(),
                account_id: None,
                uid: "uid-a".to_string(),
                session_id: session_id.to_string(),
                variant: None,
                state: MemberState::Stale,
                linked_at,
                last_synced_at: None,
            });
        }
        Ok(())
    })
    .unwrap();

    // 两条行都存活 → 组仍列出，标题与摘要取 linked_at 最大的成员（新行）。
    let listed = preview(&env, "uid-b");
    let groups = listed["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1, "{listed}");
    assert_eq!(groups[0]["title"], "新标题", "{listed}");
    assert_eq!(groups[0]["source"]["sessionId"], "sess-a-new", "{listed}");
    assert_eq!(groups[0]["source"]["state"], "stale", "{listed}");
    // 来源无 active 成员 → 判定 unknown、不可勾选（既有契约不变）。
    assert_eq!(groups[0]["verdict"], "unknown", "{listed}");
    assert_eq!(groups[0]["defaultChecked"], false, "{listed}");

    // 只保留 linked_at 小的旧行存活：代表成员的新行缺失 → 整组剔除，
    // 证明判定不是「任意一条成员存活」。
    env.delete_row("sess-a-new");
    let latest_gone = preview(&env, "uid-b");
    assert!(
        latest_gone["groups"].as_array().unwrap().is_empty(),
        "代表成员行缺失的组不得再列出：{latest_gone}"
    );
    assert!(env.store().groups.iter().any(|group| group.id == group_id));
}

/// 另一个数据根（共享同一工具存储根与登录态），用于构造「库形态异常」的场景。
fn build_probe_paths(env: &Env, name: &str) -> SessionPaths {
    let data_root = env.root.join(format!("data-{name}"));
    std::fs::create_dir_all(data_root.join("projects").join("ws-a")).unwrap();
    SessionPaths {
        store_root: env.paths.store_root.clone(),
        data_root,
        auth_file: env.paths.auth_file.clone(),
        link_namespace: LinkNamespace::WorkBuddy,
    }
}

/// 在探测数据根里建 `sessions` 表（列集合由调用方决定，模拟历史/异常库）。
fn create_sessions_table(paths: &SessionPaths, sql: &str) {
    let conn = Connection::open(paths.workbuddy_db()).unwrap();
    conn.execute_batch(sql).unwrap();
}

/// 在探测数据根里插入一条会话行（只含最小列集合）。
fn insert_probe_row(paths: &SessionPaths, id: &str, uid: &str) {
    let conn = Connection::open(paths.workbuddy_db()).unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd) VALUES (?1, ?2, ?3, '/ws/a')",
        rusqlite::params![id, uid, "标题一"],
    )
    .unwrap();
}

/// 存活索引不可用时 fail-open（列出但不参与过滤），不能静默清空整个列表。
///
/// 两种「没有判定依据」的库形态：缺 `user_id` 列（无法归属账号）与缺 `deleted_at` 列
/// （历史库没有软删除标记，按全部存活读）。都必须仍列出组，`storeStatus` 保持 ready。
#[test]
fn preview_lists_groups_when_row_index_is_unavailable() {
    let env = ready_env("sync-preview-index-unavailable");
    let (group_id, _, target_id) = fast_forward_scene(&env);

    // 缺 `user_id` 列：索引不可用 → 不参与过滤（组仍列出，判定退化为 unknown）。
    let no_user_id = build_probe_paths(&env, "no-user-id");
    create_sessions_table(
        &no_user_id,
        "CREATE TABLE sessions (id TEXT PRIMARY KEY, title TEXT, cwd TEXT);",
    );
    let report = session_links_preview_at(&no_user_id, WbVariant::Cn, &env.target("uid-b")).unwrap();
    assert_eq!(report["storeStatus"], "ready", "{report}");
    let groups = report["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1, "索引不可用时不得静默过滤：{report}");
    assert_eq!(groups[0]["groupId"], group_id.as_str());
    // 单条口径同样读不到行 → 判定 unknown（与改动前行为一致）。
    assert_eq!(groups[0]["verdict"], "unknown", "{report}");

    // 缺 `deleted_at` 列：历史库按「全部存活」读，两侧行都在 → 组照常列出。
    let no_deleted_at = build_probe_paths(&env, "no-deleted-at");
    create_sessions_table(
        &no_deleted_at,
        "CREATE TABLE sessions (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, title TEXT, cwd TEXT);",
    );
    insert_probe_row(&no_deleted_at, "sess-1", "uid-a");
    insert_probe_row(&no_deleted_at, &target_id, "uid-b");
    let report =
        session_links_preview_at(&no_deleted_at, WbVariant::Cn, &env.target("uid-b")).unwrap();
    assert_eq!(report["storeStatus"], "ready", "{report}");
    assert_eq!(
        report["groups"].as_array().unwrap().len(),
        1,
        "缺 deleted_at 列时不得额外过滤：{report}"
    );

    // 同一形态下目标行确实不在该库 → 索引生效（整组剔除），证明上一条不是「永远列出」。
    let only_source = build_probe_paths(&env, "only-source-row");
    create_sessions_table(
        &only_source,
        "CREATE TABLE sessions (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, title TEXT, cwd TEXT);",
    );
    insert_probe_row(&only_source, "sess-1", "uid-a");
    let report =
        session_links_preview_at(&only_source, WbVariant::Cn, &env.target("uid-b")).unwrap();
    assert!(
        report["groups"].as_array().unwrap().is_empty(),
        "目标行缺失时必须剔除：{report}"
    );
}

/// 预览后来源追加 → 执行时跳过该组（previewStale），不继承用户旧选择。
#[test]
fn sync_skips_when_source_changed_after_preview() {
    let env = ready_env("sync-stale-source");
    let (group_id, token, target_id) = fast_forward_scene(&env);
    let target_body_before = std::fs::read_to_string(env.body_path(&target_id)).unwrap();

    append_records(&env.body_path("sess-1"), "sess-1", 3, 1);
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert!(report["synced"].as_array().unwrap().is_empty());
    assert!(report["errors"].as_array().unwrap().is_empty(), "{report}");
    let skipped = &report["skipped"][0];
    assert_eq!(skipped["reasonCode"], REASON_PREVIEW_STALE);
    assert!(
        skipped["message"]
            .as_str()
            .unwrap()
            .contains("来源账号的内容已变化"),
        "{skipped}"
    );
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        target_body_before
    );
}

/// 预览后目标被改动 → 执行时跳过（显式覆盖也不能绕过版本校验）。
#[test]
fn sync_skips_when_target_changed_after_preview_even_with_overwrite() {
    let env = ready_env("sync-stale-target");
    let (group_id, token, target_id) = fast_forward_scene(&env);
    append_records(&env.body_path(&target_id), &target_id, 0, 1);
    let target_body_before = std::fs::read_to_string(env.body_path(&target_id)).unwrap();

    for mode in [SyncMode::FastForward, SyncMode::Overwrite] {
        let report = sync(&env, "uid-b", &[selection(&group_id, &token, mode)]);
        assert!(report["errors"].as_array().unwrap().is_empty(), "{report}");
        let skipped = &report["skipped"][0];
        assert_eq!(skipped["reasonCode"], REASON_PREVIEW_STALE, "{report}");
        assert!(
            skipped["message"]
                .as_str()
                .unwrap()
                .contains("目标账号的内容已变化"),
            "{skipped}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        target_body_before
    );
}

/// 预览后换了账号 → 执行时跳过（身份由后端从登录态读取，不沿用令牌里的身份）。
#[test]
fn sync_skips_when_account_changed_after_preview() {
    let env = ready_env("sync-stale-account");
    let (group_id, token, _) = fast_forward_scene(&env);
    let revision_before = env.store().revision;

    env.set_login("uid-c");
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert!(report["errors"].as_array().unwrap().is_empty(), "{report}");
    let skipped = &report["skipped"][0];
    assert_eq!(skipped["reasonCode"], REASON_PREVIEW_STALE);
    assert!(
        skipped["message"].as_str().unwrap().contains("账号已变化"),
        "{skipped}"
    );
    assert_eq!(env.store().revision, revision_before);
}

/// 预览后关联组或基线变化 → 执行时跳过。
#[test]
fn sync_skips_when_group_or_baseline_changed_after_preview() {
    // 组变化：同组新增成员（例如并发复制把第三方账号加进来）。
    let env = ready_env("sync-stale-group");
    let (group_id, token, _) = fast_forward_scene(&env);
    let paths = env.paths();
    session_link::with_link_store_write(&paths, |store| {
        let group = store
            .groups
            .iter_mut()
            .find(|group| group.id == group_id)
            .expect("组必须存在");
        session_link::add_active_member(
            group,
            LinkMember {
                member_id: "m-uid-c".to_string(),
                account_id: None,
                uid: "uid-c".to_string(),
                session_id: "sess-c".to_string(),
                variant: None,
                state: MemberState::Active,
                linked_at: 1,
                last_synced_at: None,
            },
        );
        Ok(())
    })
    .unwrap();
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    let skipped = &report["skipped"][0];
    assert_eq!(skipped["reasonCode"], REASON_PREVIEW_STALE, "{report}");
    assert!(
        skipped["message"]
            .as_str()
            .unwrap()
            .contains("会话的关联关系或同步记录已变化"),
        "{skipped}"
    );

    // 基线变化：同一个基线引用被改写成另一份内容。
    let env = ready_env("sync-stale-baseline");
    let (group_id, token, _) = fast_forward_scene(&env);
    let baseline_ref = env.store().groups[0].pair_bases[0].baseline_ref.clone();
    let drifted = session_link::BaselineRecord {
        version: session_link::BASELINE_VERSION,
        baseline_ref: baseline_ref.clone(),
        normalization_version: session_link::NORMALIZATION_VERSION,
        created_at: 1,
        record_count: 1,
        total_digest: session_link::total_digest_of(&["00".to_string()]),
        line_digests: vec!["00".to_string()],
    };
    std::fs::write(
        env.paths
            .baselines_dir()
            .join(format!("{baseline_ref}.json")),
        serde_json::to_string(&drifted).unwrap(),
    )
    .unwrap();
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    let skipped = &report["skipped"][0];
    assert_eq!(skipped["reasonCode"], REASON_PREVIEW_STALE, "{report}");
    assert!(
        skipped["message"]
            .as_str()
            .unwrap()
            .contains("上次同步的内容已变化"),
        "{skipped}"
    );
}

/// 伪造 token / 张冠李戴的组 → 拒绝，不静默执行。
#[test]
fn sync_rejects_forged_or_mismatched_preview_token() {
    let env = ready_env("sync-forged");
    let (group_id, token, target_id) = fast_forward_scene(&env);

    for forged in [
        "11111111-2222-3333-4444-555555555555",
        "../../../../etc/passwd",
        "sess-1.json",
    ] {
        let report = sync(
            &env,
            "uid-b",
            &[selection(&group_id, forged, SyncMode::FastForward)],
        );
        assert!(report["skipped"].as_array().unwrap().is_empty(), "{report}");
        let errors = report["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 1, "{report}");
        assert!(
            errors[0]["error"]
                .as_str()
                .unwrap()
                .contains("检查结果不存在"),
            "{report}"
        );
    }

    // 真实凭据 + 别的组 id：张冠李戴同样拒绝。
    let report = sync(
        &env,
        "uid-b",
        &[selection("g-other", &token, SyncMode::FastForward)],
    );
    assert!(report["skipped"].as_array().unwrap().is_empty(), "{report}");
    assert!(
        report["errors"][0]["error"]
            .as_str()
            .unwrap()
            .contains("不匹配"),
        "{report}"
    );

    // 未知模式在解析阶段就拒绝。
    assert!(parse_sync_selections(Some(
        &json!([{"groupId": group_id, "previewToken": token, "mode": "force"}])
    ))
    .unwrap_err()
    .contains("未知的同步模式"));

    // 全程没有任何写入。
    assert_eq!(env.body_files().len(), 2);
    assert_eq!(
        session_row_owner(&env.paths(), &target_id).as_deref(),
        Some("uid-b")
    );
}

/// 判定为 unknown 时强制覆盖 → 拒绝（不得绕过版本校验）。
#[test]
fn sync_rejects_forced_overwrite_when_verdict_is_unknown() {
    let env = ready_env("sync-unknown-overwrite");
    let (group_id, token, target_id) = fast_forward_scene(&env);
    // 目标加入独有内容：双方互不为前缀，删掉基线后无法用内容关系判定 → unknown。
    append_records(&env.body_path(&target_id), &target_id, 100, 2);
    let target_body_before = std::fs::read_to_string(env.body_path(&target_id)).unwrap();
    let revision_before = env.store().revision;

    // 基线文件被删除：共同基线不可验证 → 重新判定必然 unknown。
    let store = env.store();
    let group = store
        .groups
        .iter()
        .find(|group| group.id == group_id)
        .unwrap();
    let baseline_ref = group.pair_bases[0].baseline_ref.clone();
    std::fs::remove_file(
        env.paths
            .baselines_dir()
            .join(format!("{baseline_ref}.json")),
    )
    .unwrap();

    let paths = env.paths();
    let source_member = session_link::active_member_for(group, "uid-a").unwrap();
    let target_member = session_link::active_member_for(group, "uid-b").unwrap();
    let source_content = member_content_state(&paths, &source_member.session_id);
    let target_content = member_content_state(&paths, &target_member.session_id);
    let baseline = session_link::load_pair_baseline(
        &paths,
        group,
        &source_member.member_id,
        &target_member.member_id,
    );
    assert_eq!(
        session_link::decide_sync(&source_content, &target_content, &baseline).verdict,
        SyncVerdict::Unknown
    );
    // 前端只能回传凭据 id；这里直接构造一份「unknown + 空可选模式」的服务端凭据，
    // 模拟强行以覆盖模式执行。
    let forged = session_link::save_preview_token(
        &paths,
        live_preview_binding(
            group,
            source_member,
            target_member,
            &source_content,
            &target_content,
            &baseline,
            SyncVerdict::Unknown,
            &PreviewArchiveState::default(),
        ),
    )
    .unwrap();

    for mode in [SyncMode::Overwrite, SyncMode::FastForward] {
        let report = sync(&env, "uid-b", &[selection(&group_id, &forged, mode)]);
        assert!(
            report["skipped"].as_array().unwrap().is_empty(),
            "unknown 不得进入校验通过：{report}"
        );
        let errors = report["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 1, "{report}");
        assert!(
            errors[0]["error"].as_str().unwrap().contains("不允许以"),
            "{report}"
        );
    }

    // 以原凭据 + 覆盖模式 → 版本校验先拦下（判定已变），同样不写。
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::Overwrite)],
    );
    assert_eq!(report["skipped"][0]["reasonCode"], REASON_PREVIEW_STALE);

    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        target_body_before
    );
    assert_eq!(env.store().revision, revision_before);
}

/// 无配对基线时，目标内容被来源完整包含 → 预览照常发放可勾选凭据，执行成功。
///
/// 覆盖轮换链的最后一跳（C 切回 A）：隔跳配对没有基线记录，但目标内容是来源内容的
/// 严格有序前缀，追加同步零覆盖，因此无需基线佐证（对齐 git fast-forward）。
#[test]
fn sync_fast_forward_without_baseline_when_target_is_ordered_prefix() {
    let env = ready_env("sync-ff-no-baseline");
    let (group_id, _, target_id) = fast_forward_scene(&env);
    let target_body_before = body_bytes(&env, &target_id);

    // 删除基线文件：等价于「隔跳配对从未登记过共同基线」的不可验证状态。
    let group = group_snapshot(&env, &group_id);
    let baseline_ref = group.pair_bases[0].baseline_ref.clone();
    std::fs::remove_file(
        env.paths
            .baselines_dir()
            .join(format!("{baseline_ref}.json")),
    )
    .unwrap();

    // 预览：没有可验证基线也照常判快进，并发放可勾选凭据。
    let preview = preview(&env, "uid-b");
    let item = &preview["groups"][0];
    assert_eq!(item["verdict"], "fastForward", "{preview}");
    assert_eq!(item["defaultChecked"], true, "{preview}");
    assert_eq!(item["availableModes"], json!(["fastForward"]), "{preview}");
    assert!(item["recordCount"]["baseline"].is_null(), "{preview}");
    assert!(
        item["reason"].as_str().unwrap().contains("新增 3 条"),
        "{preview}"
    );
    let token = item["previewToken"]
        .as_str()
        .expect("可勾选项必须发放凭据")
        .to_string();

    // 执行：无基线不阻塞写入，目标收敛到来源内容，并补建配对基线。
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert!(report["errors"].as_array().unwrap().is_empty(), "{report}");
    assert!(report["skipped"].as_array().unwrap().is_empty(), "{report}");
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    assert_ne!(body_bytes(&env, &target_id), target_body_before);
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        incoming_text(&env, "sess-1", &target_id)
    );
    let group = group_snapshot(&env, &group_id);
    let baseline = session_link::load_pair_baseline(
        &env.paths(),
        &group,
        &member_id_of(&group, "uid-a"),
        &member_id_of(&group, "uid-b"),
    );
    assert!(baseline.ready().is_some(), "执行后必须补建可验证的配对基线");
}

/// 第三方账号：只处理 A 与 B 共同参与的组，同步只推进 A/B 的基线与正文。
#[test]
fn sync_never_touches_third_account_member_or_baseline() {
    let env = ready_env("sync-third-account");
    let a_to_b = copy(&env, "uid-b", &["sess-1"]);
    let b_id = env.first_copy_id(&a_to_b);
    let a_to_c = copy(&env, "uid-c", &["sess-1"]);
    let c_id = env.first_copy_id(&a_to_c);
    append_records(&env.body_path("sess-1"), "sess-1", 0, 2);
    append_records(&env.body_path(&c_id), &c_id, 0, 7);

    let pairs_before = pair_snapshot(&env);
    let baselines_before = env.baseline_files();
    let c_body_before = std::fs::read_to_string(env.body_path(&c_id)).unwrap();
    let b_body_before = std::fs::read_to_string(env.body_path(&b_id)).unwrap();

    // 只列出 A 与 B 共同参与的组；C 的改动不影响 A/B 的判定。
    let preview = preview(&env, "uid-b");
    let groups = preview["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1, "{preview}");
    assert_eq!(groups[0]["verdict"], "fastForward", "{preview}");
    assert_eq!(groups[0]["extraB"], 0);
    let group_id = groups[0]["groupId"].as_str().unwrap().to_string();
    let token = groups[0]["previewToken"].as_str().unwrap().to_string();

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
    assert!(report["errors"].as_array().unwrap().is_empty(), "{report}");
    assert!(report.get("needsRecovery").is_none(), "{report}");

    // 只有 A/B 的配对基线被推进（旧引用消失、新引用出现），A/C 与 B/C 原样保留。
    let store = env.store();
    let group = store
        .groups
        .iter()
        .find(|group| group.id == group_id)
        .unwrap();
    let pairs_after = pair_snapshot(&env);
    let removed: Vec<&String> = pairs_before
        .iter()
        .filter(|entry| !pairs_after.contains(entry))
        .collect();
    let added: Vec<&String> = pairs_after
        .iter()
        .filter(|entry| !pairs_before.contains(entry))
        .collect();
    assert_eq!(removed.len(), 1, "只有 A/B 的基线被改写：{pairs_after:?}");
    let (a_member, b_member) = (member_id_of(group, "uid-a"), member_id_of(group, "uid-b"));
    assert!(
        removed[0].starts_with(&format!("{a_member}+{b_member}"))
            || removed[0].starts_with(&format!("{b_member}+{a_member}")),
        "被改写的必须是 A/B 成员对：{}",
        removed[0]
    );
    assert_eq!(added.len(), 1);
    assert_eq!(env.baseline_files(), baselines_before + 1, "只新增一份基线");

    // C 的正文、基线与同步时间都不动；来源账号也不被标成已同步。
    assert_eq!(
        std::fs::read_to_string(env.body_path(&c_id)).unwrap(),
        c_body_before,
        "C 的内容不得被同步改写"
    );
    assert_ne!(
        std::fs::read_to_string(env.body_path(&b_id)).unwrap(),
        b_body_before,
        "B 的内容应被同步替换"
    );
    let group = env
        .store()
        .groups
        .into_iter()
        .find(|group| group.id == group_id)
        .unwrap();
    assert!(group
        .members
        .iter()
        .find(|member| member.uid == "uid-c")
        .unwrap()
        .last_synced_at
        .is_none());
    assert!(session_link::active_member_for(&group, "uid-a")
        .unwrap()
        .last_synced_at
        .is_none());
    assert!(session_link::active_member_for(&group, "uid-b")
        .unwrap()
        .last_synced_at
        .is_some());
}

fn member_id_of(group: &LinkGroup, uid: &str) -> String {
    group
        .members
        .iter()
        .find(|member| member.uid == uid)
        .unwrap()
        .member_id
        .clone()
}

/// 关联存储不可用/缺失 → 不把校验当成通过。
#[test]
fn sync_reports_store_problems_instead_of_passing_checks() {
    // 损坏：保留现场，全部选择项报错并要求恢复。
    let env = ready_env("sync-store-broken");
    let (group_id, token, _) = fast_forward_scene(&env);
    std::fs::write(env.paths.session_links_file(), "not-json").unwrap();
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert!(report["skipped"].as_array().unwrap().is_empty(), "{report}");
    assert!(
        report["errors"][0]["error"]
            .as_str()
            .unwrap()
            .contains("同步记录"),
        "{report}"
    );
    assert_eq!(report["needsRecovery"], true, "关系表损坏必须要求恢复");

    // 主文件与基线都被清掉（首次使用）：凭据无从校验 → 跳过。
    let env = ready_env("sync-store-missing");
    let (group_id, token, target_id) = fast_forward_scene(&env);
    std::fs::remove_file(env.paths.session_links_file()).unwrap();
    std::fs::remove_dir_all(env.paths.baselines_dir()).unwrap();
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert!(report["errors"].as_array().unwrap().is_empty(), "{report}");
    assert_eq!(report["skipped"][0]["reasonCode"], REASON_PREVIEW_STALE);
    assert_eq!(env.body_files().len(), 2);
    assert!(env.body_path(&target_id).exists());
}

/// 预览的能力与状态上报：档位不支持、关联存储未初始化、入参错误。
#[test]
fn preview_reports_supported_and_store_status() {
    let env = Env::new("sync-preview-status");
    env.set_login("uid-a");

    // 关联存储还没建立 → missing，不是错误。
    let report = preview(&env, "uid-b");
    assert_eq!(report["supported"], true);
    assert_eq!(report["storeStatus"], "missing");
    assert_eq!(report["groups"].as_array().unwrap().len(), 0);

    // 国际版数据根不同构 → 不支持，前端据此隐藏同步区块。
    let ai_paths = SessionPaths {
        store_root: env.root.join("store-ai"),
        data_root: env.root.join("data-ai"),
        auth_file: env.paths.auth_file.clone(),
        link_namespace: LinkNamespace::WorkBuddy,
    };
    std::fs::create_dir_all(&ai_paths.data_root).unwrap();
    let report = session_links_preview_at(
        &ai_paths,
        WbVariant::Ai,
        &json!({"id": "ai-1", "uid": "uid-b", "variant": "ai"}),
    )
    .unwrap();
    assert_eq!(report["supported"], false);
    assert_eq!(report["storeStatus"], "unsupported");
    assert_eq!(report["groups"].as_array().unwrap().len(), 0);

    // 入参错误：缺 uid、同账号。
    assert!(
        session_links_preview_at(&env.paths(), WbVariant::Cn, &json!({"uid": " "}))
            .unwrap_err()
            .contains("缺少 uid")
    );
    assert!(
        session_links_preview_at(&env.paths(), WbVariant::Cn, &env.target("uid-a"))
            .unwrap_err()
            .contains("当前账号与目标账号相同")
    );
}

/// 执行入口的生命周期与入参保护。
#[test]
fn sync_requires_app_stopped_and_valid_target() {
    let env = ready_env("sync-lifecycle");
    let (group_id, token, target_id) = fast_forward_scene(&env);
    let target_body_before = std::fs::read_to_string(env.body_path(&target_id)).unwrap();
    let one = [selection(&group_id, &token, SyncMode::FastForward)];

    // 目标账号不是当前登录账号（auth=uid-a）：App 运行中不再拦截，正常同步（S5 实测口径）。
    let report = sync_sessions_for_switch_at(
        &env.paths(),
        WbVariant::Cn,
        &env.target("uid-b"),
        &one,
        |_| true,
    )
    .unwrap();
    assert_eq!(report["synced"].as_array().unwrap().len(), 1);
    let target_body_after = std::fs::read_to_string(env.body_path(&target_id)).unwrap();
    assert_ne!(target_body_before, target_body_after, "目标正文已追平");

    // 空选择项 → 什么都不做，也不要求关闭 App。
    let report = sync_sessions_for_switch_at(
        &env.paths(),
        WbVariant::Cn,
        &env.target("uid-b"),
        &[],
        |_| true,
    )
    .unwrap();
    assert!(report["skipped"].as_array().unwrap().is_empty());
    assert!(report["errors"].as_array().unwrap().is_empty());
    assert!(report["synced"].as_array().unwrap().is_empty());

    // 缺 uid / 同账号。
    let err = sync_sessions_for_switch_at(
        &env.paths(),
        WbVariant::Cn,
        &json!({"uid": "  "}),
        &one,
        |_| false,
    )
    .unwrap_err();
    assert_eq!(err, "目标账号缺少 uid，无法同步会话");
    let err = sync_sessions_for_switch_at(
        &env.paths(),
        WbVariant::Cn,
        &env.target("uid-a"),
        &one,
        |_| false,
    )
    .unwrap_err();
    assert!(err.contains("当前账号与目标账号相同"), "{err}");

    // 失败的两条（缺 uid / 同账号）都不应再触碰目标正文。
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        target_body_after
    );
}

/// syncSelections 解析：缺字段/未知模式/非数组一律拒绝。
#[test]
fn sync_selection_parsing_rejects_invalid_input() {
    assert!(parse_sync_selections(None).unwrap().is_empty());
    assert!(parse_sync_selections(Some(&json!(null)))
        .unwrap()
        .is_empty());
    assert!(parse_sync_selections(Some(&json!("x")))
        .unwrap_err()
        .contains("必须是数组"));

    let parsed = parse_sync_selections(Some(&json!([
        {"groupId": "g-1", "previewToken": "11111111-2222-3333-4444-555555555555", "mode": "overwrite"}
    ])))
    .unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].group_id, "g-1");
    assert_eq!(parsed[0].mode, SyncMode::Overwrite);

    for (name, item) in [
        (
            "缺 groupId",
            json!({"previewToken": "t", "mode": "fastForward"}),
        ),
        (
            "缺 previewToken",
            json!({"groupId": "g-1", "mode": "fastForward"}),
        ),
        ("缺 mode", json!({"groupId": "g-1", "previewToken": "t"})),
        (
            "空白 groupId",
            json!({"groupId": "  ", "previewToken": "t", "mode": "fastForward"}),
        ),
    ] {
        let error = parse_sync_selections(Some(&json!([item]))).unwrap_err();
        assert!(!error.is_empty(), "{name}");
    }
}

/// 遗留库（`sessions` 没有 `deleted_at` 列）也要读得到会话行信息。
///
/// 硬套 `deleted_at IS NULL` 会让 `prepare` 失败 → 行被判缺失 → 预览标题退化成
/// `(无标题)`，而存活索引（缺列按全部存活读）却认为该行还在：同一场景两套结论。
#[test]
fn session_row_info_reads_a_legacy_table_without_deleted_at() {
    let env = Env::new("row-info-legacy");
    std::fs::create_dir_all(env.paths.data_root.clone()).unwrap();
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            title TEXT,
            custom_title TEXT,
            cwd TEXT
        );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, custom_title, cwd)
         VALUES ('sess-1', 'uid-a', '标题一', '自定义标题', '/ws/a')",
        [],
    )
    .unwrap();
    drop(conn);

    // 缺列 + 行存在：按未删除读，标题仍是 custom_title 优先。
    assert_eq!(
        session_row_info(&env.paths(), "sess-1"),
        Some(("自定义标题".to_string(), "/ws/a".to_string()))
    );
    // 缺列 + 行不存在：仍然是 None（不能因为缺列就把「不存在」当成「存在」）。
    assert_eq!(session_row_info(&env.paths(), "missing"), None);
}

// ---------------------------------------------------------------------------
// R5-16 切号弹窗预览的摘要 / 判定缓存
// ---------------------------------------------------------------------------

use crate::modules::session_link::real_read_count;

/// 预览项里除一次性凭据之外的全部字段（凭据每次都新发，不参与比对）。
fn groups_without_token(preview: &Value) -> Vec<Value> {
    preview["groups"]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .map(|item| {
                    let mut item = item.clone();
                    if let Some(object) = item.as_object_mut() {
                        object.remove("previewToken");
                    }
                    item
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 等长改写正文（用于构造「长度没变、只有 mtime 变了」的失效场景）。
fn rewrite_same_length(path: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(path).unwrap();
    assert_eq!(from.len(), to.len(), "等长改写的前提");
    assert!(text.contains(from), "正文里应包含 {from}");
    let patched = text.replacen(from, to, 1);
    assert_eq!(patched.len(), text.len());
    std::fs::write(path, &patched).unwrap();
}

/// 把 mtime 推到 1 小时后：与刚写入的时刻一定可区分，不受系统时钟粒度影响。
fn bump_mtime(path: &Path) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
        .unwrap();
}

/// 第二次打开切号弹窗不再读取任何正文，且判定结果与首次逐字段一致。
#[test]
fn preview_second_open_reuses_cached_decisions_without_reading_bodies() {
    // `fast_forward_scene` 内部已经预览过一次，判定缓存是温的。
    let env = ready_env("preview-cache-hit");
    fast_forward_scene(&env);

    let reads_before = real_read_count();
    let first = preview(&env, "uid-b");
    assert_eq!(
        real_read_count(),
        reads_before,
        "缓存命中时预览不得读取正文"
    );
    let reads_after_first = real_read_count();
    let second = preview(&env, "uid-b");
    assert_eq!(
        real_read_count(),
        reads_after_first,
        "再次预览仍不得读取正文"
    );
    assert_eq!(
        groups_without_token(&first),
        groups_without_token(&second),
        "命中缓存必须给出与首次一致的判定"
    );
    // 凭据仍然每次新发（缓存不缓存凭据）。
    assert!(second["groups"][0]["previewToken"].as_str().is_some());
}

/// 预览之间来源追加记录（len 变化）→ 判定必须重新计算，不得沿用旧判定。
#[test]
fn preview_decision_cache_invalidates_when_body_grows() {
    let env = ready_env("preview-cache-append");
    fast_forward_scene(&env);
    let warm = preview(&env, "uid-b");
    assert_eq!(warm["groups"][0]["recordCount"]["source"], 5, "{warm}");

    append_records(&env.body_path("sess-1"), "sess-1", 10, 2);
    let reads_before = real_read_count();
    let after = preview(&env, "uid-b");
    assert!(
        real_read_count() > reads_before,
        "len 变化必须回落到真实读取"
    );
    assert_eq!(after["groups"][0]["verdict"], "fastForward", "{after}");
    assert_eq!(
        after["groups"][0]["recordCount"]["source"], 7,
        "记录数必须来自本次读取，不能是缓存里的 5：{after}"
    );
    assert_eq!(after["groups"][0]["extraA"], 5, "{after}");
}

/// 预览之间等长改写目标正文（只有 mtime 变化）→ 判定同样必须失效。
#[test]
fn preview_decision_cache_invalidates_on_mtime_only_change() {
    let env = ready_env("preview-cache-mtime");
    let (_, _, target_id) = fast_forward_scene(&env);
    let warm = preview(&env, "uid-b");
    assert_eq!(warm["groups"][0]["verdict"], "fastForward", "{warm}");

    let target_path = env.body_path(&target_id);
    rewrite_same_length(&target_path, "\"text\":\"hi\"", "\"text\":\"ho\"");
    bump_mtime(&target_path);

    let reads_before = real_read_count();
    let after = preview(&env, "uid-b");
    assert!(
        real_read_count() > reads_before,
        "mtime 变化必须回落到真实读取"
    );
    assert_eq!(
        after["groups"][0]["verdict"], "diverge",
        "目标内容已变，判定必须跟着变：{after}"
    );
    assert_eq!(after["groups"][0]["recordCount"]["target"], 2, "{after}");
}

/// 执行路径不复用预览缓存：预览之后手工改目标正文，同步必须报「检查结果已失效」。
///
/// 如果执行路径命中了预览缓存，它拿到的还是预览时刻的摘要，会认为「什么都没变」并
/// 直接写入——所以「被拦下」正是执行路径真实读取的证据。
#[test]
fn sync_execution_ignores_preview_decision_cache() {
    let env = ready_env("preview-cache-exec");
    let (group_id, token, target_id) = fast_forward_scene(&env);
    // 预热判定缓存（同时证明命中路径不会把结果写进任何执行侧结构）。
    let warm = preview(&env, "uid-b");
    assert_eq!(warm["groups"][0]["verdict"], "fastForward", "{warm}");

    let target_path = env.body_path(&target_id);
    rewrite_same_length(&target_path, "\"text\":\"hi\"", "\"text\":\"ho\"");
    bump_mtime(&target_path);
    let body_before = std::fs::read_to_string(&target_path).unwrap();

    let reads_before = real_read_count();
    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert!(
        real_read_count() > reads_before,
        "执行路径必须重新读取正文"
    );
    assert!(report["synced"].as_array().unwrap().is_empty(), "{report}");
    assert!(report["errors"].as_array().unwrap().is_empty(), "{report}");
    assert_eq!(report["skipped"][0]["reasonCode"], REASON_PREVIEW_STALE);
    // 零写入：目标正文原样保留。
    assert_eq!(
        std::fs::read_to_string(&target_path).unwrap(),
        body_before
    );
}

/// 执行侧的读取原语（`member_content_state`）永远实读，且拿得到真实正文。
#[test]
fn execution_content_read_is_never_served_from_cache() {
    let env = ready_env("preview-cache-primitive");
    let (_, _, target_id) = fast_forward_scene(&env);
    let _ = preview(&env, "uid-b");

    let reads_before = real_read_count();
    let state = member_content_state(&env.paths(), &target_id);
    assert_eq!(
        real_read_count(),
        reads_before + 1,
        "执行路径的读取原语必须实读，不得命中预览缓存"
    );
    match state {
        ContentState::Ready(snapshot) => {
            assert!(!snapshot.text.is_empty(), "执行路径要拿到真实正文落盘");
            assert_eq!(snapshot.normalized.record_count, 2);
        }
        other => panic!("目标正文应可验证，实际 {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 末条 incomplete 归一不得让组永久「存在分歧」（issue #3 最后一条）
// ---------------------------------------------------------------------------
//
// 复制与同步写目标时都会把末条 assistant 的 `incomplete` 归一到 `completed` 并留痕，
// 于是「刚复制完」「刚点过以此为准」的组在原始摘要上永远与来源差一行 → 判定恒为
// diverge，用户消不掉「存在分歧」。下面三条覆盖这条链路的三个入口：
// 复制 → 预览 / 覆盖 → 预览 / 快进 → 预览。

/// 造一个「来源会话已结束、末条停在 incomplete」的环境（复制与同步都按终态归一）。
fn settled_incomplete_env(name: &str) -> Env {
    let env = ready_env_with_status(name);
    // `source_settled_for_normalize` 只认终态集合（completed/error/terminated/archived）。
    env.set_status("sess-1", "completed");
    env.add_body("sess-1", &incomplete_body("sess-1"));
    env
}

/// 复制出一个「末条已归一」的副本 → 立即预览必须是 identical（改造前恒为 diverge）。
#[test]
fn copy_incomplete_tail_previews_as_identical() {
    let env = settled_incomplete_env("sync-copy-incomplete-identical");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);

    // 前提：副本确实被归一了（末条 completed + 留痕），否则这个用例证明不了什么。
    let target_text = std::fs::read_to_string(env.body_path(&target_id)).unwrap();
    assert!(
        target_text.contains("incompleteNormalized"),
        "副本末条应带归一留痕：{target_text}"
    );
    assert!(
        !target_text.contains("\"status\":\"incomplete\""),
        "副本末条不应再停在 incomplete：{target_text}"
    );

    let preview = preview(&env, "uid-b");
    let group = &preview["groups"][0];
    assert_eq!(group["verdict"], "identical", "{preview}");
    assert_eq!(group["availableModes"], json!([]), "{preview}");
    assert_eq!(group["defaultChecked"], false, "{preview}");
    assert!(
        group.get("previewToken").is_none(),
        "一致且无归档动作的组不发凭据：{preview}"
    );
    assert!(
        group["reason"].as_str().unwrap().contains("归一"),
        "理由是用户能读懂的说明：{preview}"
    );
    // 解释性计数必须与判定同口径：判成「一致」的组不能在详情里显示「独有 1 条」。
    assert_eq!(group["extraA"], 0, "一致的组不该报告来源独有记录：{preview}");
    assert_eq!(group["extraB"], 0, "一致的组不该报告目标独有记录：{preview}");
    assert_eq!(
        group["common"], group["recordCount"]["source"],
        "共有条数应等于来源条数：{preview}"
    );
}

/// 两边真分歧 → 「以此为准（overwrite）」写入成功后，再预览必须收敛为 identical。
#[test]
fn overwrite_incomplete_tail_converges_to_identical() {
    let env = settled_incomplete_env("sync-overwrite-incomplete-tail");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);

    // 两侧各自追加不同内容：条数相同、内容不同 = 真分歧（用户点「以此为准」的场景）。
    append_records(&env.body_path("sess-1"), "sess-1", 10, 2);
    append_records(&env.body_path(&target_id), &target_id, 20, 2);
    let group = preview(&env, "uid-b")["groups"][0].clone();
    assert_eq!(group["verdict"], "diverge", "{group}");

    let group_id = group["groupId"].as_str().unwrap();
    let token = group["previewToken"].as_str().unwrap();
    let applied = sync(
        &env,
        "uid-b",
        &[selection(group_id, token, SyncMode::Overwrite)],
    );
    assert_eq!(applied["synced"].as_array().unwrap().len(), 1, "{applied}");

    let after = preview(&env, "uid-b")["groups"][0].clone();
    assert_eq!(
        after["verdict"], "identical",
        "覆盖后仍报分歧，说明「以此为准」消不掉分歧：{after}"
    );
    assert_eq!(after["availableModes"], json!([]), "{after}");
    assert_eq!(after["defaultChecked"], false, "{after}");
}

/// 来源在归一副本之后又追加 → 快进写入成功后，再预览必须收敛为 identical。
#[test]
fn fast_forward_incomplete_tail_converges_to_identical() {
    let env = settled_incomplete_env("sync-ff-incomplete-tail");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);

    // 目标仍是来源的严格有序前缀 → 可快进（尾部两行只有来源有）。
    append_records(&env.body_path("sess-1"), "sess-1", 10, 2);
    let group = preview(&env, "uid-b")["groups"][0].clone();
    assert_eq!(group["verdict"], "fastForward", "{group}");

    let group_id = group["groupId"].as_str().unwrap();
    let token = group["previewToken"].as_str().unwrap();
    let applied = sync(
        &env,
        "uid-b",
        &[selection(group_id, token, SyncMode::FastForward)],
    );
    assert_eq!(applied["synced"].as_array().unwrap().len(), 1, "{applied}");

    // 写入确实落到了目标：末条留痕是同步场景的，且不再停在 incomplete。
    let target_text = std::fs::read_to_string(env.body_path(&target_id)).unwrap();
    assert!(
        target_text.contains("incompleteNormalized"),
        "目标末条应带归一留痕：{target_text}"
    );
    assert!(
        !target_text.contains("\"status\":\"incomplete\""),
        "目标末条不应再停在 incomplete：{target_text}"
    );

    let after = preview(&env, "uid-b")["groups"][0].clone();
    assert_eq!(
        after["verdict"], "identical",
        "快进后仍报分歧，说明写入把归一签名当成了内容差异：{after}"
    );
    assert_eq!(after["availableModes"], json!([]), "{after}");
    assert_eq!(after["defaultChecked"], false, "{after}");
}

/// 三成员组：两两判定都已收敛，组级判定不得因为「末条归一留痕」另造一条独立分支。
///
/// 场景（真实可达，只需 3 个成员）：uid-b 的副本停在 base；来源末条变成 `incomplete`；
/// 来源行仍是终态，再复制出的 uid-c 副本末条是 `completed` + `_meta.incompleteNormalized`。
/// 组级判定（`session_groups::graph::common_base_branches`）若仍比原始摘要，会把来源与 uid-c 的
/// 副本当成两条独立尖端，`view.rs` 随即把 `summaryStatus` 强制改成 `diverge`——用户看到的还是
/// 「存在分歧」（issue #3 最后一条的第二个出口）。
///
/// 这里断言的是组级判定的**全部输入**：同一组 3 个 active 成员 + 六个有序对的两两判定里没有
/// 任何一个 `Diverge`。徽标覆盖条件本身（`divergence.is_some()` → `diverge`）及
/// `common_base_branches` 对这种内容形态返回 `None` 由
/// `session_groups::tests::normalized_copy_is_not_an_independent_branch` 覆盖——组级载荷
/// （`session_groups::detail`）无法在这里断言：它对 WorkBuddy 走
/// `session::list_sessions_for_user` / `member_content_state(SessionPaths::for_variant(..))`，
/// 路径不可注入，单测里调用会读写真实 `~/.workbuddy-shift`。
#[test]
fn three_member_group_is_not_marked_divergent_by_tail_normalization() {
    let env = ready_env_with_status("sync-group-3-member-normalized-tail");
    // 来源会话已结束（复制不看状态，但同步的归一资格看它，场景要真实）。
    env.set_status("sess-1", "completed");

    // 1) 把来源正文换成 `incomplete_body` 的**前两行**（user + 已收尾的 assistant），
    //    再复制出停在 base 的副本（uid-b，末条无留痕）。
    let full_body = incomplete_body("sess-1");
    let base_text = body_lines(&full_body)[..2].join("\n");
    std::fs::write(env.body_path("sess-1"), format!("{base_text}\n")).unwrap();
    let report = copy(&env, "uid-b", &["sess-1"]);
    let base_copy = env.first_copy_id(&report);
    // 2) 来源末条变成 incomplete（用户中断 / 限流 / 报错留下的真实形态）。
    std::fs::write(env.body_path("sess-1"), &full_body).unwrap();
    // 3) 再复制出一个末条已归一的副本（uid-c）。
    let report = copy(&env, "uid-c", &["sess-1"]);
    let normalized_copy = env.first_copy_id(&report);

    // 前提一：两次复制登记进同一个组，且三个成员都 active（否则触发不到 N 路识别）。
    let group = preview(&env, "uid-b")["groups"][0].clone();
    let group_id = group["groupId"].as_str().unwrap().to_string();
    let store = env.store();
    let link_group = store
        .groups
        .iter()
        .find(|candidate| candidate.id == group_id)
        .expect("两次复制应在同一组里");
    assert_eq!(link_group.members.len(), 3, "该场景只需 3 个成员");
    assert!(
        link_group
            .members
            .iter()
            .all(|member| member.state == MemberState::Active),
        "三个成员都应有效"
    );

    // 前提二：两两判定已收敛（uid-b 落后可快进、uid-c 与来源一致）。
    assert_eq!(group["verdict"], "fastForward", "{group}");
    let normalized_preview = preview(&env, "uid-c");
    assert_eq!(
        normalized_preview["groups"][0]["verdict"], "identical",
        "{normalized_preview}"
    );

    // 组级判定的输入矩阵：六个有序对，一个 Diverge 都不许有。
    let states = [
        member_content_state(&env.paths(), "sess-1"),
        member_content_state(&env.paths(), &base_copy),
        member_content_state(&env.paths(), &normalized_copy),
    ];
    for (source, target) in [(0, 1), (1, 0), (0, 2), (2, 0), (1, 2), (2, 1)] {
        let decision = session_link::decide_sync(
            &states[source],
            &states[target],
            &session_link::BaselineState::Missing,
        );
        assert_ne!(
            decision.verdict,
            crate::modules::session_link::SyncVerdict::Diverge,
            "成员 {source}→{target} 不该判成分歧（否则组徽标会被强制改成 diverge）：{}",
            decision.reason
        );
    }
    // 具体口径：来源 → 停在 base 的副本 = 可快进；来源 ↔ 归一副本 = 一致。
    let verdict = |source: usize, target: usize| {
        session_link::decide_sync(
            &states[source],
            &states[target],
            &session_link::BaselineState::Missing,
        )
        .verdict
    };
    assert_eq!(
        verdict(0, 1),
        crate::modules::session_link::SyncVerdict::FastForward
    );
    assert_eq!(
        verdict(0, 2),
        crate::modules::session_link::SyncVerdict::Identical
    );
    assert_eq!(
        verdict(2, 0),
        crate::modules::session_link::SyncVerdict::Identical
    );

    // 组徽标：用户在列表里看到「存在分歧」的那个字段。`view.rs` 只在
    // `common_base_branches` 识别出「共同旧版 + 独立尖端」时才把 `summaryStatus` 覆盖成
    // `diverge`，所以必须真的走一遍组载荷装配（`detail_at` 注入临时数据根），
    // 只断言两两判定是推断，证明不了这条合成链路。
    let detail = crate::modules::session_groups::detail_at(
        &env.paths(),
        &|_: WbVariant| env.paths(),
        crate::modules::session_groups::SessionClient::Workbuddy,
        // 只有 CodeBuddy IDE 支持档位筛选（`validate_scope`），WorkBuddy 必须传 None。
        None,
        &group_id,
    )
    .expect("组详情应可读");
    assert_eq!(
        detail["summaryStatus"], "behind",
        "组徽标不该被末条归一留痕带成 diverge：{detail}"
    );
    assert!(
        detail.get("divergence").is_none(),
        "不该识别出独立分支：{detail}"
    );
    assert_eq!(detail["hasSafeSource"], true, "{detail}");
    assert_eq!(detail["activeMemberCount"], 3, "{detail}");
    // 反「假绿」锚点：徽标结论必须建立在**真读到的正文**上。镜像注入路径漏传时三个成员会
    // 全变成 `Missing`，那时 `common_base_branches` 永远返回 `None`，徽标断言不红也说明不了
    // 任何问题（改了 bug 也照样绿）。
    let members = detail["members"]
        .as_array()
        .expect("详情载荷应带成员列表");
    assert_eq!(members.len(), 3, "{detail}");
    for member in members {
        assert_eq!(
            member["contentState"], "ready",
            "成员正文必须真读到，否则徽标断言是假绿：{detail}"
        );
        assert!(
            member["recordCount"].as_u64().is_some_and(|count| count > 0),
            "成员记录数必须为正：{detail}"
        );
    }
}

// ---------------------------------------------------------------------------
// R6-08：同步不搬会话账本（现状保持），复制默认也不搬
// ---------------------------------------------------------------------------

/// 同步只写正文与 `sessions` 行：两侧账本在同步前后逐字段相同。
///
/// R6-07/R6-08 的口径都要求「两个账本各管一件事」——`session_usage` 的数值不参与
/// Token 归属计算，归属索引也不参与 `session_usage` 的写入。这条用例守的是后半句。
#[test]
fn sync_leaves_both_ledgers_untouched() {
    let env = ready_env("sync-ledger-untouched");
    let db = env.paths.workbuddy_db();
    create_ledger_tables(&db);
    write_ledger(
        &db,
        "sess-1",
        100,
        10,
        r#"{"r1":1.0}"#,
        &[("req-1", None, None)],
    );
    let (group_id, token, target_id) = fast_forward_scene(&env);
    // 目标账本属于目标账号自己：同步不得搬来源数值，也不得把目标清零。
    write_ledger(
        &db,
        &target_id,
        7,
        8,
        r#"{"r2":3.5}"#,
        &[("req-9", None, None)],
    );
    let source_before = ledger_usage(&db, "sess-1");
    let target_before = ledger_usage(&db, &target_id);
    let target_interruptions_before = ledger_interruptions(&db, &target_id);

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
    assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");

    assert_eq!(
        ledger_usage(&db, "sess-1"),
        source_before,
        "来源账本不得被同步改写"
    );
    assert_eq!(
        ledger_usage(&db, &target_id),
        target_before,
        "同步既不得搬来源账本，也不得清零目标账本"
    );
    assert_eq!(
        ledger_interruptions(&db, &target_id),
        target_interruptions_before,
        "同步不得改写目标的中断记录"
    );
}
