// ---------------------------------------------------------------------------
// 幂等复制与关联组（R1）
// ---------------------------------------------------------------------------

#[test]
fn copy_writes_body_row_and_link_group() {
    let env = ready_env("happy");
    let report = copy(&env, "uid-b", &["sess-1"]);

    assert_eq!(report["sourceUid"], "uid-a");
    assert_eq!(report["targetUid"], "uid-b");
    assert_eq!(report["copied"].as_array().unwrap().len(), 1);
    assert_eq!(report["alreadyLinked"].as_array().unwrap().len(), 0);
    assert!(report.get("errors").is_none());
    assert!(report.get("needsRecovery").is_none());

    let new_id = env.first_copy_id(&report);
    assert_ne!(new_id, "sess-1");
    assert_eq!(report["copied"][0]["id"], "sess-1");

    // 正文：新 id 文件存在、旧 id 引用已替换，源文件不动。
    let copied_body = std::fs::read_to_string(env.body_path(&new_id)).unwrap();
    assert!(copied_body.contains(&new_id));
    assert!(!copied_body.contains("sess-1"));
    assert_eq!(
        std::fs::read_to_string(env.body_path("sess-1")).unwrap(),
        body_text("sess-1")
    );

    // 数据库行归属目标账号。
    assert_eq!(env.rows_for("uid-b"), vec![new_id.clone()]);
    assert_eq!(env.rows_for("uid-a"), vec!["sess-1".to_string()]);

    // 不预写云端映射：登记交接给客户端（edge-sync 扩展迁移时自行写入）。
    assert_eq!(env.mapping_rows(), 0, "复制不得预写映射行");

    // 关联组：同一逻辑会话、两个账号各一个 active 成员、一对基线。
    let store = env.store();
    assert!(
        store.revision >= 1,
        "首次落地空存储 + 关联提交都会推进 revision"
    );
    assert_eq!(store.groups.len(), 1);
    let group = &store.groups[0];
    assert_eq!(group.variant, WbVariant::Cn);
    assert_eq!(group.members.len(), 2);
    assert!(group
        .members
        .iter()
        .all(|member| member.state == MemberState::Active));
    assert_eq!(group.pair_bases.len(), 1);
    assert!(env
        .paths
        .baselines_dir()
        .join(format!("{}.json", group.pair_bases[0].baseline_ref))
        .exists());
    // 来源账号的成员带上了 accountId（账号库缺失时为 None，不影响身份判定）。
    assert!(group
        .members
        .iter()
        .any(|member| member.uid == "uid-a" && member.session_id == "sess-1"));

    // 成功清理：临时备份已回收，报告不展示可还原路径，也没有维护记录残留。
    assert!(report["copied"][0]["backup"].is_null(), "{report}");
    assert_eq!(report["copied"][0]["cleanupState"], "cleaned", "{report}");
    assert_eq!(report["temporaryFiles"], json!([]), "{report}");
    let operation_id = session_link::scan_operations(&env.paths)
        .operations
        .first()
        .expect("操作日志必须保留")
        .operation_id
        .clone();
    assert!(
        !session_backup::transaction_dir(&env.paths, WbVariant::Cn, &operation_id)
            .unwrap()
            .exists(),
        "成功路径必须回收本次操作专属目录"
    );
    assert!(session_backup::scan_lifecycle(&env.paths)
        .records
        .is_empty());
}

/// 批量复制逐项清理：进入下一项之前，上一成功项的临时目录已经消失（不累积备份）。
#[test]
fn batch_copy_cleans_each_backup_before_next_item() {
    let env = ready_env("batch-cleanup");
    env.add_session("sess-2", "uid-a", "标题二");
    env.add_body("sess-2", &body_text("sess-2"));

    let report = copy(&env, "uid-b", &["sess-1", "sess-2"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 2, "{report}");
    for item in report["copied"].as_array().unwrap() {
        assert!(item["backup"].is_null(), "{report}");
        assert_eq!(item["cleanupState"], "cleaned", "{report}");
    }
    // 两个成功项都不留目录与维护记录：连续操作不累积成功备份。
    let transactions = env
        .paths
        .backup_root()
        .join(session_backup::TRANSACTIONS_DIR_NAME)
        .join(WbVariant::Cn.as_str());
    let leftovers = std::fs::read_dir(&transactions)
        .map(|entries| entries.flatten().count())
        .unwrap_or(0);
    assert_eq!(leftovers, 0, "成功批次不得累积操作专属目录");
    assert!(session_backup::scan_lifecycle(&env.paths)
        .records
        .is_empty());
    assert_eq!(
        env.body_files().len(),
        4,
        "两条来源内容与两个复制后的内容都在"
    );
}

/// 归属不可验证（临时目录路径被替换为符号链接）：业务成功保持不变、材料保留并上报，
/// 解除异常后由维护入口补清理。
#[cfg(unix)]
#[test]
fn cleanup_protects_business_success_when_transaction_path_is_symlinked() {
    let env = ready_env("cleanup-symlink");
    let transactions = env
        .paths
        .backup_root()
        .join(session_backup::TRANSACTIONS_DIR_NAME);
    std::fs::create_dir_all(&transactions).unwrap();
    let target = env.root.join("redirected-transactions");
    std::fs::create_dir_all(&target).unwrap();
    // 档位目录被替换为符号链接：删除前校验必须拒绝，且不得跟随链接删除。
    std::os::unix::fs::symlink(&target, transactions.join(WbVariant::Cn.as_str())).unwrap();

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    let new_id = env.first_copy_id(&report);
    // 业务成功不变：正文与数据库行都在，报告仍报成功，只是临时文件待处理。
    assert!(env.body_path(&new_id).exists());
    assert_eq!(env.rows_for("uid-b"), vec![new_id.clone()]);
    assert_eq!(report["copied"][0]["cleanupState"], "pending", "{report}");
    assert!(
        report["copied"][0]["backup"].as_str().is_some(),
        "待清理必须保留位置：{report}"
    );
    assert!(
        report["copied"][0]["cleanupError"]
            .as_str()
            .unwrap()
            .contains("符号链接"),
        "{report}"
    );
    let temporary_files = report["temporaryFiles"].as_array().expect("temporaryFiles");
    assert!(
        !temporary_files.is_empty(),
        "本轮清理受阻必须出现在报告级 temporaryFiles：{report}"
    );
    assert!(
        temporary_files.iter().any(|item| item["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("符号链接"))),
        "{report}"
    );
    assert_eq!(session_backup::scan_lifecycle(&env.paths).records.len(), 1);

    // 解除异常后：维护入口补清理，业务结果不变；链接目标不被当作本操作材料删除。
    std::fs::remove_file(transactions.join(WbVariant::Cn.as_str())).unwrap();
    let recovery = recover_pending_session_operations_at(&env.paths, WbVariant::Cn);
    assert!(recovery.is_clean(), "{:?}", recovery.needs_recovery);
    assert!(session_backup::scan_lifecycle(&env.paths)
        .records
        .is_empty());
    assert!(env.body_path(&new_id).exists());
    assert_eq!(
        session_link::scan_operations(&env.paths).operations[0]
            .cleanup_state
            .as_deref(),
        Some(session_backup::CLEANUP_STATE_CLEANED)
    );
}

#[test]
fn copy_retry_reuses_member_without_second_copy() {
    let env = ready_env("retry");
    let first = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&first);
    let revision_before_retry = env.store().revision;

    let second = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(second["copied"].as_array().unwrap().len(), 0);
    assert_eq!(second["alreadyLinked"].as_array().unwrap().len(), 1);
    assert_eq!(second["alreadyLinked"][0]["sessionId"], new_id);
    assert!(second.get("errors").is_none());

    assert_eq!(env.body_files().len(), 2, "重试不得产生第二个副本");
    assert_eq!(env.rows_for("uid-b").len(), 1);
    assert_eq!(
        env.store().revision,
        revision_before_retry,
        "幂等复用不写同步记录"
    );
    assert_eq!(env.store().groups[0].members.len(), 2);
}

#[test]
fn copy_back_reuses_original_session() {
    let env = ready_env("back");
    let first = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&first);

    // 目标账号成为当前账号，把副本复制回原账号：必须复用原件。
    env.set_login("uid-b");
    let back = copy(&env, "uid-a", &[&new_id]);
    assert_eq!(back["copied"].as_array().unwrap().len(), 0);
    assert_eq!(back["alreadyLinked"].as_array().unwrap().len(), 1);
    assert_eq!(back["alreadyLinked"][0]["sessionId"], "sess-1");

    assert_eq!(env.body_files().len(), 2, "B→A 不得新建副本");
    assert_eq!(env.rows_for("uid-a"), vec!["sess-1".to_string()]);
    assert_eq!(env.store().groups[0].members.len(), 2);
}

#[test]
fn chain_a_to_b_then_a_to_c_then_b_to_c_reuses_existing_copy() {
    let env = ready_env("chain");
    let to_b = copy(&env, "uid-b", &["sess-1"]);
    let b_id = env.first_copy_id(&to_b);

    // 同一来源再复制给 C：同组内新增一个成员，不是新组。
    let to_c = copy(&env, "uid-c", &["sess-1"]);
    let c_id = env.first_copy_id(&to_c);
    assert_eq!(env.store().groups.len(), 1);
    assert_eq!(env.store().groups[0].members.len(), 3);

    // B→C：组内已有 C 的有效副本，复用而不重复复制。
    env.set_login("uid-b");
    let b_to_c = copy(&env, "uid-c", &[&b_id]);
    assert_eq!(b_to_c["copied"].as_array().unwrap().len(), 0);
    assert_eq!(b_to_c["alreadyLinked"][0]["sessionId"], c_id);
    assert_eq!(env.body_files().len(), 3);

    // 配对基线按成员对保存：A/B、A/C 各有基线；C 加入时按 A/B 基线继承出 B/C。
    let group = &env.store().groups[0];
    assert_eq!(group.pair_bases.len(), 3);
    let member_id = |uid: &str| {
        group
            .members
            .iter()
            .find(|member| member.uid == uid)
            .unwrap()
            .member_id
            .clone()
    };
    let (a, b, c) = (member_id("uid-a"), member_id("uid-b"), member_id("uid-c"));
    let pair = |left: &str, right: &str| {
        session_link::find_pair_base(group, left, right)
            .unwrap_or_else(|| panic!("缺少成员对基线 {left}/{right}"))
            .baseline_ref
            .clone()
    };
    assert_eq!(pair(&a, &b), pair(&b, &c), "B/C 继承自 A/B 的共同基线");
    assert_ne!(pair(&a, &c), pair(&a, &b), "A/C 是各自新建的基线");
}

#[test]
fn rename_does_not_break_link() {
    let env = ready_env("rename");
    let first = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&first);

    // 用户在目标账号改名：关联不依赖标题。
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute(
        "UPDATE sessions SET title = '改过的标题', custom_title = '自定义名' WHERE id = ?1",
        [&new_id],
    )
    .unwrap();
    drop(conn);

    let again = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(again["alreadyLinked"][0]["sessionId"], new_id);
    assert_eq!(env.body_files().len(), 2);
    assert_eq!(env.store().groups[0].members.len(), 2);
}

#[test]
fn same_title_independent_sessions_stay_separate() {
    let env = ready_env("same-title");
    env.add_session("sess-2", "uid-a", "标题一");
    env.add_body("sess-2", &body_text("sess-2"));

    let report = copy(&env, "uid-b", &["sess-1", "sess-2"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 2);
    let ids: Vec<String> = report["copied"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["newId"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);

    // 同标题不建立关联：第二个会话各成一个组，且各自独立幂等。
    assert_eq!(env.store().groups.len(), 2);
    assert_eq!(env.rows_for("uid-b").len(), 2);
    assert_eq!(env.body_files().len(), 4);

    let again = copy(&env, "uid-b", &["sess-1", "sess-2"]);
    assert_eq!(again["alreadyLinked"].as_array().unwrap().len(), 2);
    assert_eq!(env.body_files().len(), 4);
}

#[test]
fn invalid_target_member_is_superseded_and_rebuilt_without_resurrection() {
    let env = ready_env("rebuild");
    let first = copy(&env, "uid-b", &["sess-1"]);
    let old_id = env.first_copy_id(&first);

    // 目标副本的正文丢失 → 旧成员失效，重建新成员。
    std::fs::remove_file(env.body_path(&old_id)).unwrap();
    let rebuilt = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(rebuilt["copied"].as_array().unwrap().len(), 1);
    let new_id = env.first_copy_id(&rebuilt);
    assert_ne!(new_id, old_id);
    assert_eq!(
        env.body_files().len(),
        2,
        "旧复制后的内容已删，只剩源与新建副本"
    );

    let group = &env.store().groups[0];
    assert_eq!(group.members.len(), 3);
    let actives: Vec<&str> = group
        .members
        .iter()
        .filter(|member| member.uid == "uid-b" && member.state == MemberState::Active)
        .map(|member| member.session_id.as_str())
        .collect();
    assert_eq!(actives, vec![new_id.as_str()], "每账号唯一 active");
    assert_eq!(
        group
            .members
            .iter()
            .find(|member| member.session_id == old_id)
            .unwrap()
            .state,
        MemberState::Superseded
    );

    // 旧正文恢复后不得自动争夺有效位置：仍是重建后的成员有效。
    env.add_body(&old_id, &body_text(&old_id));
    let after_restore = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(after_restore["alreadyLinked"][0]["sessionId"], new_id);
    assert_eq!(env.store().groups[0].members.len(), 3, "不新增成员");
}

#[test]
fn identity_uses_uid_and_never_rebinds_by_account_id() {
    let env = ready_env("identity");

    // 手工写入一个组：目标成员 uid=uid-b，accountId 是旧账号 id（重新导入前的 id）。
    let paths = env.paths();
    session_link::with_link_store_write(&paths, |store| {
        store.groups.push(LinkGroup {
            id: "g-1".to_string(),
            variant: WbVariant::Cn,
            created_at: 1,
            members: vec![
                LinkMember {
                    member_id: "m-a".to_string(),
                    account_id: Some("acc-uid-a".to_string()),
                    uid: "uid-a".to_string(),
                    session_id: "sess-1".to_string(),
                    variant: None,
                    state: MemberState::Active,
                    linked_at: 1,
                    last_synced_at: None,
                },
                LinkMember {
                    member_id: "m-b".to_string(),
                    account_id: Some("old-account-id".to_string()),
                    uid: "uid-b".to_string(),
                    session_id: "sess-b".to_string(),
                    variant: None,
                    state: MemberState::Active,
                    linked_at: 1,
                    last_synced_at: None,
                },
            ],
            pair_bases: Vec::new(),
        });
        Ok(())
    })
    .unwrap();
    env.add_body("sess-b", &body_text("sess-b"));
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, created_at, updated_at, deleted_at, is_playground)
         VALUES ('sess-b', 'uid-b', '旧副本', '/ws/a', 1, 2, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);

    // uid 相同、accountId 变了 → 仍复用（身份以 uid 为准）。
    let reused = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(reused["alreadyLinked"][0]["sessionId"], "sess-b");
    assert_eq!(env.body_files().len(), 2);

    // accountId 相同但 uid 不同 → 不得错误绑定，必须新建。
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute(
        "UPDATE sessions SET user_id = 'uid-x' WHERE id = 'sess-b'",
        [],
    )
    .unwrap();
    drop(conn);
    let fresh = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(fresh["copied"].as_array().unwrap().len(), 1);
    assert_eq!(env.store().groups[0].members.len(), 3);
}

#[test]
fn variant_isolation_keeps_groups_separate() {
    let env = ready_env("variants");
    let cn = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(cn["copied"].as_str(), None);
    assert_eq!(cn["copied"].as_array().unwrap().len(), 1);

    // 国际版数据根：单独一套数据（同 store 根），身份字符串相同但档位不同。
    let mut ai_paths = env.paths();
    ai_paths.data_root = env.root.join("data-ai");
    std::fs::create_dir_all(ai_paths.projects_dir().join("ws-a")).unwrap();
    let conn = Connection::open(ai_paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, title TEXT, custom_title TEXT, cwd TEXT, created_at INTEGER, updated_at INTEGER, deleted_at INTEGER, is_playground INTEGER);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, created_at, updated_at, deleted_at, is_playground)
         VALUES ('ai-sess-1', 'uid-a', 'AI 会话', '/ws/a', 1, 2, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);
    std::fs::write(
        ai_paths.projects_dir().join("ws-a").join("ai-sess-1.jsonl"),
        body_text("ai-sess-1"),
    )
    .unwrap();
    let conn = Connection::open(ai_paths.edge_sync_db(WbVariant::Ai)).unwrap();
    conn.execute_batch(
        "CREATE TABLE edge_sync_mapping (session_id TEXT, conversation_id TEXT, msg_channel TEXT, created_at INTEGER);",
    )
    .unwrap();
    drop(conn);

    let ai_report = copy_sessions_for_switch_at(
        &ai_paths,
        WbVariant::Ai,
        &json!({"id": "acc-uid-b", "uid": "uid-b", "variant": "ai"}),
        &["ai-sess-1".to_string()],
        false,
        |_| false,
    )
    .unwrap();
    assert_eq!(
        ai_report["copied"].as_array().unwrap().len(),
        1,
        "同档位身份不同，必须新复制"
    );

    let store = env.store();
    assert_eq!(store.groups.len(), 2);
    let variants: Vec<&str> = store
        .groups
        .iter()
        .map(|group| group.variant.as_str())
        .collect();
    assert!(
        variants.contains(&"cn") && variants.contains(&"ai"),
        "{variants:?}"
    );
    // 两档位不串数据：成员会话 id 不相交，国际版组里带着国际版来源会话。
    let mut seen = std::collections::HashSet::new();
    for group in &store.groups {
        for member in &group.members {
            assert!(
                seen.insert(member.session_id.clone()),
                "会话 {} 同时出现在两个档位的组里",
                member.session_id
            );
        }
    }
    let ai_group = store
        .groups
        .iter()
        .find(|group| group.variant == WbVariant::Ai)
        .unwrap();
    assert!(ai_group
        .members
        .iter()
        .any(|member| member.session_id == "ai-sess-1"));
}

/// 报告契约：copied / alreadyLinked / errors 同时出现时字段完整（桌面与 webui 同形）。
#[test]
fn copy_report_carries_copied_already_linked_and_errors_together() {
    let env = ready_env("contract");
    env.add_session("sess-2", "uid-a", "标题二");
    env.add_body("sess-2", &body_text("sess-2"));

    let first = copy(&env, "uid-b", &["sess-1", "sess-2"]);
    assert_eq!(first["copied"].as_array().unwrap().len(), 2);

    // 第二个会话的正文丢失：本次一个复用、一个失败。
    std::fs::remove_file(env.body_path("sess-2")).unwrap();
    let mixed = copy(&env, "uid-b", &["sess-1", "sess-2"]);
    assert_eq!(mixed["sourceUid"], "uid-a");
    assert_eq!(mixed["targetUid"], "uid-b");
    assert_eq!(mixed["copied"].as_array().unwrap().len(), 0);
    assert_eq!(mixed["alreadyLinked"].as_array().unwrap().len(), 1);
    assert_eq!(mixed["alreadyLinked"][0]["id"], "sess-1");
    assert_eq!(mixed["errors"].as_array().unwrap().len(), 1);
    assert_eq!(mixed["errors"][0]["id"], "sess-2");
    assert_eq!(mixed["errors"][0]["error"], "会话内容不存在，未复制");
    // 复用/失败都不算未完成写入。
    assert!(mixed.get("needsRecovery").is_none());
}

// ---------------------------------------------------------------------------
// 失败必须可见、可恢复（R1 / R5）
// ---------------------------------------------------------------------------

#[test]
fn missing_body_is_not_reported_as_success() {
    let env = ready_env("no-body");
    std::fs::remove_file(env.body_path("sess-1")).unwrap();

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0);
    assert_eq!(report["errors"][0]["error"], "会话内容不存在，未复制");
    assert_eq!(env.rows_for("uid-b").len(), 0, "不得写出半成品会话记录");
    assert_eq!(env.body_files().len(), 0);
}

#[test]
fn truncated_body_is_not_reported_as_success() {
    let env = ready_env("truncated");
    env.add_body("sess-1", "{\"sessionId\":\"sess-1\"\n");

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0);
    let error = report["errors"][0]["error"].as_str().unwrap();
    assert!(error.contains("会话内容无法验证"), "{error}");
    assert_eq!(env.rows_for("uid-b").len(), 0);
}

#[test]
fn missing_source_row_is_not_reported_as_success() {
    let env = ready_env("no-row");
    env.delete_row("sess-1");

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0);
    assert_eq!(
        report["errors"][0]["error"],
        "数据库中找不到源会话记录，未复制"
    );
    assert_eq!(env.body_files().len(), 1, "不得写出无数据库行的内容");
}

#[test]
fn source_row_of_another_account_is_rejected() {
    let env = ready_env("other-owner");
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute(
        "UPDATE sessions SET user_id = 'uid-x' WHERE id = 'sess-1'",
        [],
    )
    .unwrap();
    drop(conn);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0);
    assert_eq!(report["errors"][0]["error"], "源会话不属于当前账号，未复制");
}

/// 复制不再依赖云端映射库：库缺失也照常成功，不重建、不写行。
#[test]
fn copy_succeeds_without_edge_sync_db() {
    let env = ready_env("mapping-absent");
    std::fs::remove_file(env.paths.edge_sync_db(WbVariant::Cn)).unwrap();

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1);
    assert!(report.get("errors").is_none(), "{report}");
    assert_eq!(env.body_files().len(), 2);
    assert_eq!(env.rows_for("uid-b").len(), 1);
    assert!(
        !env.paths().edge_sync_db(WbVariant::Cn).exists(),
        "复制不得创建或写入映射库"
    );
    assert!(session_link::pending_operations(&env.paths(), WbVariant::Cn).is_empty());
}

/// 关联提交失败（存储目录不可写）→ 不报成功；恢复后同一 UUID 完成。
#[cfg(unix)]
#[test]
fn link_commit_failure_keeps_pending_then_recovers_with_same_uuid() {
    use std::os::unix::fs::PermissionsExt;

    let env = ready_env("link-commit");
    // 先跑一次成功，建立锁文件与存储文件，避免把「无法加锁」当成关联提交失败。
    copy(&env, "uid-c", &["sess-1"]);
    if std::fs::write(env.paths.store_root.join(".probe"), b"x").is_err() {
        return;
    }

    std::fs::set_permissions(
        &env.paths.store_root,
        std::fs::Permissions::from_mode(0o555),
    )
    .unwrap();
    if std::fs::write(env.paths.store_root.join(".probe"), b"x").is_ok() {
        // root / 特殊 ACL 环境写保护无效：跳过（不误报）。
        std::fs::set_permissions(
            &env.paths.store_root,
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        return;
    }

    let failed = copy(&env, "uid-b", &["sess-1"]);
    std::fs::set_permissions(
        &env.paths.store_root,
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert_eq!(failed["copied"].as_array().unwrap().len(), 0);
    let error = failed["errors"][0]["error"].as_str().unwrap();
    assert!(
        error.contains("保存失败") || error.contains("同步记录"),
        "{error}"
    );
    assert_eq!(failed["needsRecovery"], true);

    let pending = session_link::pending_operations(&env.paths(), WbVariant::Cn);
    let target = pending
        .iter()
        .find(|operation| operation.target.uid == "uid-b")
        .expect("应保留未完成操作");
    let new_id = target.target.session_id.clone();

    let retry = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(retry["copied"].as_array().unwrap().len(), 0);
    assert_eq!(retry["alreadyLinked"][0]["sessionId"], new_id);
    assert_eq!(
        env.body_files().len(),
        4,
        "两个目标各一份副本，重试不得新增"
    );
    assert_eq!(env.rows_for("uid-b"), vec![new_id]);
}

/// 正文写入失败：不得写出数据库行，也不报成功。
#[cfg(unix)]
#[test]
fn body_write_failure_reports_error_without_db_row() {
    use std::os::unix::fs::PermissionsExt;

    let env = ready_env("body-write");
    let ws = env.paths.projects_dir().join("ws-a");
    std::fs::set_permissions(&ws, std::fs::Permissions::from_mode(0o555)).unwrap();
    let writable = std::fs::write(ws.join(".probe"), b"x").is_ok();
    let report = copy(&env, "uid-b", &["sess-1"]);
    std::fs::set_permissions(&ws, std::fs::Permissions::from_mode(0o755)).unwrap();
    if writable {
        // root / 特殊 ACL 环境：写保护无效，跳过断言。
        return;
    }
    assert_eq!(report["copied"].as_array().unwrap().len(), 0);
    let error = report["errors"][0]["error"].as_str().unwrap();
    assert!(error.contains("复制后的内容保存失败"), "{error}");
    assert_eq!(env.rows_for("uid-b").len(), 0);
    assert_eq!(env.body_files().len(), 1);
}

#[test]
fn corrupt_store_blocks_copy_and_preserves_original() {
    let env = ready_env("corrupt-store");
    std::fs::create_dir_all(&env.paths.store_root).unwrap();
    std::fs::write(env.paths.session_links_file(), "not-json").unwrap();

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0);
    let error = report["errors"][0]["error"].as_str().unwrap();
    assert!(
        error.contains("同步记录") && error.contains("已阻止复制"),
        "{error}"
    );
    assert_eq!(env.rows_for("uid-b").len(), 0);
    assert_eq!(env.body_files().len(), 1);
    assert_eq!(
        std::fs::read_to_string(env.paths.session_links_file()).unwrap(),
        "not-json",
        "必须保留现场，不得当空表覆盖"
    );
}

#[test]
fn unknown_store_version_blocks_copy() {
    let env = ready_env("unknown-version");
    std::fs::create_dir_all(&env.paths.store_root).unwrap();
    std::fs::write(
        env.paths.session_links_file(),
        json!({"version": 99, "revision": 1, "groups": []}).to_string(),
    )
    .unwrap();

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0);
    let error = report["errors"][0]["error"].as_str().unwrap();
    assert!(error.contains("版本"), "{error}");
    assert_eq!(env.rows_for("uid-b").len(), 0);
}

/// 主文件被删但基线文件仍在 → 检测到痕迹，不得当首次使用重建空表，复制被阻止。
#[test]
fn copy_blocked_when_store_file_missing_but_baselines_remain() {
    let env = ready_env("baseline-trace");
    let first = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&first);
    assert!(env.baseline_files() > 0, "首次复制应留下基线文件");

    std::fs::remove_file(env.paths.session_links_file()).unwrap();
    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0);
    assert_eq!(report["alreadyLinked"].as_array().unwrap().len(), 0);
    let error = report["errors"][0]["error"].as_str().unwrap();
    assert!(error.contains("保留现场"), "{error}");
    assert!(
        !env.paths.session_links_file().exists(),
        "不得重建空表覆盖现场"
    );
    assert_eq!(env.body_files().len(), 2, "不得产生第二个副本");
    assert_eq!(env.rows_for("uid-b"), vec![new_id]);
}

/// 并发请求：档位操作锁被占用时直接拒绝，不产生第二个副本。
#[test]
fn concurrent_request_is_rejected_without_second_copy() {
    let env = ready_env("concurrent");
    let held = session_link::try_acquire_variant_ops_lock(&env.paths(), WbVariant::Cn).unwrap();

    let err = copy_sessions_for_switch_at(
        &env.paths(),
        WbVariant::Cn,
        &env.target("uid-b"),
        &["sess-1".to_string()],
        false,
        |_| false,
    )
    .expect_err("持锁期间必须拒绝");
    assert!(err.contains("会话操作"), "{err}");
    assert_eq!(env.body_files().len(), 1);

    drop(held);
    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1);
}

/// 目标账号不是当前登录账号（auth=uid-a）时，App 运行中不再拦截写入（S5 实测口径）；
/// 快拒与锁后复查仍各执行一次，且复查必须发生在持锁之后、写入之前。
#[test]
fn copy_runs_while_app_running_for_non_current_account() {
    let env = ready_env("app-running-non-current");
    let probes = std::cell::Cell::new(0usize);
    let lock_held_on_recheck = std::cell::Cell::new(false);
    let lock_path = env.paths.variant_ops_lock_file(WbVariant::Cn);
    let probe = |_: WbVariant| {
        let n = probes.get() + 1;
        probes.set(n);
        if n == 1 {
            true
        } else {
            // 第二次必须发生在持锁之后：此时再抢同一把锁应为 Busy。
            match session_link::try_lock_file(&lock_path) {
                Err(session_link::LockError::Busy) => lock_held_on_recheck.set(true),
                Err(session_link::LockError::Unavailable(reason)) => {
                    panic!("第二次探针时期望档位锁已被持有，实际 Unavailable: {reason}")
                }
                Ok(_) => panic!("第二次探针时期望档位锁已被持有，实际拿到了锁"),
            }
            true
        }
    };
    let report = copy_sessions_for_switch_at(
        &env.paths(),
        WbVariant::Cn,
        &env.target("uid-b"),
        &["sess-1".to_string()],
        false,
        probe,
    )
    .expect("目标非当前登录账号：运行中必须放行");
    assert_eq!(report["copied"].as_array().unwrap().len(), 1);
    assert_eq!(probes.get(), 2, "快拒与锁后复查各检查一次");
    assert!(
        lock_held_on_recheck.get(),
        "复查必须发生在已经拿到档位锁之后、任何写入之前"
    );
    assert_eq!(env.rows_for("uid-b").len(), 1, "副本行已写入");
    assert!(env.paths.session_links_file().exists(), "关联记录已建立");
}

// ---------------------------------------------------------------------------
// 恢复
// ---------------------------------------------------------------------------

#[test]
fn recovery_abandons_prepared_operation_when_source_is_gone() {
    let env = ready_env("abandon");
    let paths = env.paths();
    session_link::save_operation(
        &paths,
        &Operation {
            version: crate::modules::session_link::OPERATION_VERSION,
            operation_id: "op-gone".to_string(),
            kind: "copy".to_string(),
            variant: WbVariant::Cn,
            source_variant: None,
            group_id: "g-gone".to_string(),
            source: OperationMember {
                account_id: None,
                uid: "uid-a".to_string(),
                session_id: "sess-gone".to_string(),
            },
            target: OperationMember {
                account_id: None,
                uid: "uid-b".to_string(),
                session_id: "new-gone".to_string(),
            },
            expected_content_digest: "d".to_string(),
            expected_record_count: 1,
            phase: crate::modules::session_link::OpPhase::Prepared,
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

    let report = recover_pending_session_operations_at(&paths, WbVariant::Cn);
    assert_eq!(report.abandoned, vec!["op-gone".to_string()]);
    assert!(report.is_clean());
    assert!(session_link::pending_operations(&paths, WbVariant::Cn).is_empty());
    assert_eq!(env.body_files().len(), 1);
}

#[test]
fn recovery_stops_when_intermediate_body_was_modified() {
    let env = ready_env("recovery-stop");
    let first = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&first);
    let paths = env.paths();

    // 模拟"正文已写、后续阶段中断"：把已完成的操作日志回退到 BodyWritten。
    let mut operation = session_link::scan_operations(&paths)
        .operations
        .into_iter()
        .find(|operation| operation.target.session_id == new_id)
        .expect("应能找到该副本的操作记录");
    operation.phase = OpPhase::BodyWritten;
    session_link::save_operation(&paths, &operation).unwrap();

    // 中间产物被其它程序改动 → 停止恢复，不覆盖。
    let tampered = format!(
        "{}\n",
        json!({"type": "user", "sessionId": new_id, "text": "别人改的"})
    );
    std::fs::write(env.body_path(&new_id), &tampered).unwrap();

    let report = recover_pending_session_operations_at(&paths, WbVariant::Cn);
    assert!(!report.is_clean());
    let issue = &report.needs_recovery[0];
    assert!(!issue.retryable);
    assert!(
        issue.reason.contains("目标内容与操作记录不一致"),
        "{}",
        issue.reason
    );
    assert_eq!(
        std::fs::read_to_string(env.body_path(&new_id)).unwrap(),
        tampered
    );

    // 该会话再次请求时不新建副本，而是报告未完成。
    let again = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(again["copied"].as_array().unwrap().len(), 0);
    assert_eq!(again["errors"].as_array().unwrap().len(), 1);
    assert!(again["errors"][0]["error"]
        .as_str()
        .unwrap()
        .contains("上一次复制尚未完成"));
    assert_eq!(env.body_files().len(), 2, "不得产生第二个副本");
}

/// 复制完成后再次请求同一会话：报告 alreadyLinked，不新建副本。
#[test]
fn retry_after_completed_copy_does_not_duplicate() {
    let env = ready_env("partial-resume");
    let first = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&first);
    let body_count = env.body_files().len();
    assert_eq!(body_count, 2);

    let again = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(again["copied"].as_array().unwrap().len(), 0);
    assert_eq!(again["alreadyLinked"][0]["sessionId"], new_id);
    assert_eq!(env.body_files().len(), body_count, "不得产生第二个副本");
    assert_eq!(env.rows_for("uid-b").len(), 1);
}

/// 恢复不重放已完成的阶段：产物全部就位、只剩 Completed 未写时，只补写阶段标记，
/// 不重写关联存储与基线、不重复登记映射（design §5）。
#[test]
fn recovery_completes_without_replaying_finished_stages() {
    let env = ready_env("recover-no-replay");
    let first = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&first);
    let paths = env.paths();

    // 模拟「关联已提交、Completed 写入失败」：把已完成的操作日志回退到 LinksCommitted。
    let mut operation = session_link::scan_operations(&paths)
        .operations
        .into_iter()
        .find(|operation| operation.target.session_id == new_id)
        .expect("应能找到该副本的操作记录");
    operation.phase = OpPhase::LinksCommitted;
    session_link::save_operation(&paths, &operation).unwrap();
    let operation_id = operation.operation_id.clone();

    let before = env.store();
    let revision_before = before.revision;
    let pair_bases_before = before.groups[0].pair_bases.clone();
    let baselines_before = env.baseline_files();
    let members_before = before.groups[0].members.len();

    let report = recover_pending_session_operations_at(&paths, WbVariant::Cn);
    assert_eq!(report.recovered, vec![operation_id]);
    assert!(report.is_clean(), "{:?}", report.needs_recovery);

    let after = env.store();
    assert_eq!(after.revision, revision_before, "恢复不得重写同步记录");
    assert_eq!(after.groups[0].pair_bases.len(), pair_bases_before.len());
    assert_eq!(
        after.groups[0].pair_bases[0].baseline_ref, pair_bases_before[0].baseline_ref,
        "不得重写配对基线"
    );
    assert_eq!(after.groups[0].members.len(), members_before);
    assert_eq!(env.baseline_files(), baselines_before, "不得新增基线文件");
    assert_eq!(
        env.mapping_rows(),
        0,
        "复制不再预写云端映射，恢复也不得补写"
    );
    assert_eq!(env.body_files().len(), 2, "不得产生第二个副本");
    assert_eq!(env.rows_for("uid-b"), vec![new_id]);
    assert!(session_link::pending_operations(&paths, WbVariant::Cn).is_empty());
}

/// phase 已是 LinksCommitted，但关联主文件被删：不得跳过核验后标 Completed。
#[test]
fn recovery_stops_when_committed_links_are_missing() {
    let env = ready_env("recover-links-gone");
    let first = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&first);
    let paths = env.paths();

    let mut operation = session_link::scan_operations(&paths)
        .operations
        .into_iter()
        .find(|operation| operation.target.session_id == new_id)
        .expect("应能找到该副本的操作记录");
    operation.phase = OpPhase::LinksCommitted;
    session_link::save_operation(&paths, &operation).unwrap();
    std::fs::remove_file(paths.session_links_file()).unwrap();

    let report = recover_pending_session_operations_at(&paths, WbVariant::Cn);
    assert!(report.recovered.is_empty(), "{:?}", report.recovered);
    assert_eq!(report.needs_recovery.len(), 1);
    assert!(!report.needs_recovery[0].retryable);
    assert!(
        report.needs_recovery[0].reason.contains("同步记录"),
        "{}",
        report.needs_recovery[0].reason
    );
    assert!(
        !paths.session_links_file().exists(),
        "不得把缺失的主文件当成空表重建"
    );
    assert_eq!(env.body_files().len(), 2, "不得产生第二个副本");
    assert_eq!(
        session_link::pending_operations(&paths, WbVariant::Cn).len(),
        1,
        "必须保留未完成操作，不能标 Completed"
    );
}

// ---------------------------------------------------------------------------
// 账户/环境辅助
// ---------------------------------------------------------------------------

#[test]
fn unparseable_operation_log_blocks_copy_without_second_replica() {
    let env = ready_env("bad-op-json");
    std::fs::create_dir_all(env.paths.operations_dir()).unwrap();
    std::fs::write(env.paths.operations_dir().join("broken.json"), "not-json").unwrap();

    let recovery = recover_pending_session_operations_at(&env.paths(), WbVariant::Cn);
    assert!(!recovery.is_clean());
    assert_eq!(recovery.needs_recovery.len(), 1);
    assert!(!recovery.needs_recovery[0].retryable);
    assert!(
        recovery.needs_recovery[0]
            .reason
            .contains(UNPARSEABLE_OPERATION_REASON),
        "{}",
        recovery.needs_recovery[0].reason
    );

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 0);
    assert_eq!(report["alreadyLinked"].as_array().unwrap().len(), 0);
    assert_eq!(report["needsRecovery"], true);
    let error = report["errors"][0]["error"].as_str().unwrap();
    assert!(error.contains(UNPARSEABLE_OPERATION_REASON), "{error}");
    assert_eq!(
        env.body_files().len(),
        1,
        "不得绕过损坏的操作记录写出第二个副本"
    );
    assert_eq!(env.rows_for("uid-b").len(), 0);
}

#[test]
fn recovery_reports_store_unavailable_instead_of_writing() {
    let env = ready_env("store-broken-recovery");
    std::fs::create_dir_all(&env.paths.store_root).unwrap();
    std::fs::write(env.paths.session_links_file(), "not-json").unwrap();

    let report = recover_pending_session_operations_at(&env.paths(), WbVariant::Cn);
    assert!(report.is_clean(), "没有未完成操作时不做任何事");
    assert_eq!(
        std::fs::read_to_string(env.paths.session_links_file()).unwrap(),
        "not-json"
    );
}

#[test]
fn session_display_title_prefers_custom_title() {
    assert_eq!(
        session_display_title(Some("自动标题".into()), Some("美团每日自动领券".into())),
        "美团每日自动领券"
    );
    assert_eq!(
        session_display_title(None, Some("美团每日自动领券".into())),
        "美团每日自动领券"
    );
    assert_eq!(
        session_display_title(Some("汉字详情页".into()), None),
        "汉字详情页"
    );
    assert_eq!(session_display_title(None, None), "(无标题)");
    assert_eq!(
        session_display_title(Some("  ".into()), Some("".into())),
        "(无标题)"
    );
}

#[test]
fn claw_workspace_detected_by_folder_name() {
    assert!(is_claw_workspace("/Users/apple/WorkBuddy/Claw"));
    assert!(is_claw_workspace("/Users/apple/WorkBuddy/claw/"));
    assert!(is_claw_workspace(r"C:\Users\me\WorkBuddy\Claw"));
    assert!(!is_claw_workspace("/Users/apple/WorkBuddy/ClawBot"));
    assert!(!is_claw_workspace(
        "/Users/apple/Documents/AI-PROJECT/LetterTotTown"
    ));
}

#[test]
fn list_sessions_marks_has_history() {
    let env = ready_env("list-sessions");
    let sessions = list_sessions_for_user_at(&env.paths(), "uid-a");
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    assert_eq!(sessions[0]["id"], "sess-1");
    assert_eq!(sessions[0]["hasHistory"], true);

    std::fs::remove_file(env.body_path("sess-1")).unwrap();
    let sessions = list_sessions_for_user_at(&env.paths(), "uid-a");
    assert_eq!(sessions[0]["hasHistory"], false);
}

#[test]
fn insert_session_copy_duplicates_row_with_target_uid() {
    let env = ready_env("insert");
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, created_at, updated_at, deleted_at, is_playground)
         VALUES ('src-1', 'uid-a', '旧标题', '/ws', 1000, 2000, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);

    let outcome = insert_session_copy(
        &env.paths(),
        &env.paths(),
        "new-uuid-1",
        "src-1",
        "uid-a",
        "uid-b",
        false,
        false,
        true,
        None,
    )
    .unwrap();
    assert_eq!(
        outcome,
        DbCopyOutcome::Inserted {
            // R6-08 起默认不搬历史消耗，因此带这条诊断（本用例只关心 space 映射）。
            aux_note: Some(AUX_USAGE_LEFT_IN_SOURCE_NOTE),
            space_note: None
        }
    );

    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    let (id, user_id, title, deleted_at, is_playground): (String, String, String, Option<i64>, i64) =
        conn.query_row(
            "SELECT id, user_id, title, deleted_at, is_playground FROM sessions WHERE id = 'new-uuid-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(id, "new-uuid-1");
    assert_eq!(user_id, "uid-b");
    assert_eq!(title, "旧标题"); // 普通列原样保留
    assert_eq!(deleted_at, None);
    assert_eq!(is_playground, 0);
}

#[test]
fn insert_session_copy_reports_missing_source_and_db() {
    let env = ready_env("insert-missing");
    assert_eq!(
        insert_session_copy(
            &env.paths(),
            &env.paths(),
            "new-1",
            "missing",
            "uid-a",
            "uid-b",
            false,
            false,
            true,
            None,
        )
        .unwrap(),
        DbCopyOutcome::SourceRowMissing,
        "源行缺失必须显式上报，不能当成功（旧实现的假成功）"
    );

    std::fs::remove_file(env.paths.workbuddy_db()).unwrap();
    assert_eq!(
        insert_session_copy(
            &env.paths(),
            &env.paths(),
            "new-1",
            "sess-1",
            "uid-a",
            "uid-b",
            false,
            false,
            true,
            None,
        )
        .unwrap(),
        DbCopyOutcome::NoDb
    );
}

// ---------------------------------------------------------------------------
// 复制时的「末条 incomplete」归一
// ---------------------------------------------------------------------------

/// 末条停在 `status = "incomplete"` 的正文：前两行正常，第三行是「运行中」假象
/// （真实数据里的「Interrupted by user / 429 限流 / 500 报错」都是这个形态）。
fn incomplete_body(cid: &str) -> String {
    format!(
        "{}\n{}\n{}\n",
        json!({"type": "user", "sessionId": cid, "text": "你好"}),
        json!({
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "sessionId": cid,
            "content": [{"type": "output_text", "text": "hi"}]
        }),
        json!({
            "type": "message",
            "role": "assistant",
            "status": "incomplete",
            "content": [{"type": "output_text", "text": "Interrupted by user"}]
        })
    )
}

/// 正文行（去掉末尾换行后的逐行切片）。
fn body_lines(text: &str) -> Vec<&str> {
    text.trim_end_matches('\n').split('\n').collect()
}

/// 末条为 incomplete → 副本末条改为 completed：行数不变、其它行逐字不变，
/// 源正文一字不动，配对基线摘要 = 归一化后的正文摘要（后续同步不误判漂移）。
#[test]
fn copy_normalizes_incomplete_last_turn() {
    let env = ready_env("copy-normalize-incomplete");
    let source_text = incomplete_body("sess-1");
    env.add_body("sess-1", &source_text);

    let report = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&report);
    let target_text = std::fs::read_to_string(env.body_path(&new_id)).unwrap();

    let source_lines = body_lines(&source_text);
    let target_lines = body_lines(&target_text);
    assert_eq!(
        target_lines.len(),
        source_lines.len(),
        "归一不得增删行：{target_text}"
    );
    // 其它行只做 session id 替换，逐字不变（摘要按行算，任何多余改动都是「内容变了」）。
    for (source_line, target_line) in source_lines.iter().zip(target_lines.iter()).take(2) {
        assert_eq!(*target_line, source_line.replace("sess-1", &new_id));
    }
    let last: Value = serde_json::from_str(target_lines[2]).unwrap();
    assert_eq!(
        last["status"],
        json!("completed"),
        "末条必须归一到 completed"
    );
    assert_eq!(
        last["content"],
        json!([{"type": "output_text", "text": "Interrupted by user"}]),
        "正文内容不得被改写"
    );
    assert_eq!(last["role"], json!("assistant"));
    assert_eq!(
        last["_meta"]["incompleteNormalized"]["reason"],
        json!("copy normalize")
    );
    assert!(
        last["_meta"]["incompleteNormalized"]["at"].is_string(),
        "留痕必须带时间戳：{last}"
    );
    // 源会话（原会话）必须保持原样：归一只作用于副本。
    assert_eq!(
        std::fs::read_to_string(env.body_path("sess-1")).unwrap(),
        source_text
    );

    // 基线摘要必须是归一化后的值，否则下一次同步会把副本判成「内容漂移」。
    let baseline_ref = env.store().groups[0].pair_bases[0].baseline_ref.clone();
    let baseline = session_link::load_baseline(&env.paths(), &baseline_ref).unwrap();
    let target_snapshot =
        match session_link::read_content_snapshot(&env.body_path(&new_id), &new_id) {
            ContentState::Ready(snapshot) => snapshot,
            other => panic!("副本正文应可读，实际 {other:?}"),
        };
    assert_eq!(
        baseline.total_digest, target_snapshot.normalized.total_digest,
        "基线必须与落盘的归一化正文一致"
    );
    assert_eq!(baseline.record_count, 3, "基线记录数不变");
}

/// 末条已是 completed → 副本逐字等于「源正文替换 session id」，幂等不误改。
#[test]
fn copy_keeps_completed_last_turn_untouched() {
    let env = ready_env("copy-normalize-completed");
    let source_text = format!(
        "{}\n{}\n",
        json!({"type": "user", "sessionId": "sess-1", "text": "你好"}),
        json!({
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "sessionId": "sess-1",
            "content": [{"type": "output_text", "text": "hi"}]
        })
    );
    env.add_body("sess-1", &source_text);

    let report = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&report);
    let target_text = std::fs::read_to_string(env.body_path(&new_id)).unwrap();

    assert_eq!(
        target_text,
        source_text.replace("sess-1", &new_id),
        "已完成的会话不得被改写（幂等）"
    );
    assert!(
        !target_text.contains("_meta"),
        "没有归一动作就不该留痕：{target_text}"
    );
}

/// 最后一条 assistant 消息后面跟着 tool 行 → 仍归一那一行，tool 行逐字不变。
///
/// 定位口径是「最后一条 assistant 消息」（与存量修复脚本一致）：尾部跟
/// `function_call_result` 的会话，客户端会显示「正在执行工具」，同样要修。
#[test]
fn copy_normalizes_assistant_before_trailing_tool_rows() {
    let env = ready_env("copy-normalize-tool-tail");
    let source_text = format!(
        "{}\n{}\n{}\n",
        json!({
            "type": "message",
            "role": "user",
            "sessionId": "sess-1",
            "content": [{"type": "input_text", "text": "跑一下"}]
        }),
        json!({
            "type": "message",
            "role": "assistant",
            "status": "incomplete",
            "sessionId": "sess-1",
            "content": [{"type": "output_text", "text": "Interrupted by user"}]
        }),
        json!({
            "type": "function_call_result",
            "sessionId": "sess-1",
            "name": "read_file",
            "output": "ok"
        })
    );
    env.add_body("sess-1", &source_text);

    let report = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&report);
    let target_text = std::fs::read_to_string(env.body_path(&new_id)).unwrap();

    let source_lines = body_lines(&source_text);
    let target_lines = body_lines(&target_text);
    assert_eq!(target_lines.len(), 3, "行数必须不变：{target_text}");
    // 其它行（用户消息、tool 结果）逐字等于「源行替换 session id」。
    assert_eq!(target_lines[0], source_lines[0].replace("sess-1", &new_id));
    assert_eq!(
        target_lines[2],
        source_lines[2].replace("sess-1", &new_id),
        "尾部 tool 行必须逐字不变"
    );
    let assistant: Value = serde_json::from_str(target_lines[1]).unwrap();
    assert_eq!(assistant["status"], json!("completed"));
    assert_eq!(
        assistant["_meta"]["incompleteNormalized"]["reason"],
        json!("copy normalize")
    );
    // 源正文不受影响。
    assert_eq!(
        std::fs::read_to_string(env.body_path("sess-1")).unwrap(),
        source_text
    );
}

/// 正文里根本没有 assistant 记录 → 不动（哪怕别的记录带同名 status 文本）。
#[test]
fn copy_keeps_body_without_assistant_untouched() {
    let env = ready_env("copy-normalize-no-assistant");
    let source_text = format!(
        "{}\n{}\n",
        json!({"type": "user", "sessionId": "sess-1", "text": "只有用户消息"}),
        json!({
            "type": "function_call_result",
            "sessionId": "sess-1",
            "output": "ok"
        })
    );
    env.add_body("sess-1", &source_text);

    let report = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&report);
    let target_text = std::fs::read_to_string(env.body_path(&new_id)).unwrap();

    assert_eq!(
        target_text,
        source_text.replace("sess-1", &new_id),
        "没有 assistant 记录时一律不动"
    );
}

/// 复制归一 DB 行状态：`working` / `active` → `completed`，`error` / `completed` 原样。
#[test]
fn insert_session_copy_normalizes_running_row_status() {
    let env = Env::new("insert-status");
    env.create_db_with_status();
    env.set_login("uid-a");
    let insert = |id: &str, status: &str| {
        let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, user_id, title, cwd, status, created_at, updated_at, deleted_at, is_playground)
             VALUES (?1, 'uid-a', '标题', '/ws/a', ?2, 1000, 2000, NULL, 0)",
            rusqlite::params![id, status],
        )
        .unwrap();
    };
    insert("src-working", "working");
    insert("src-active", "active");
    insert("src-error", "error");
    insert("src-completed", "completed");
    // 终态常量之外的一切状态都算「可能被当成运行中」：合法但语义未定的默认值
    // `Pending`、未知新状态都在内。
    insert("src-pending", "Pending");
    insert("src-unknown", "queued");
    // 终态 `terminated` 不在真实库里常见，但必须证明保留集合没误伤它。
    insert("src-terminated", "terminated");
    // 归档态：用户显式做过的整理动作，复制不得抹掉（它不在归档动作的目标终态里）。
    insert("src-archived", "archived");

    for (source_id, new_id) in [
        ("src-working", "new-working"),
        ("src-active", "new-active"),
        ("src-error", "new-error"),
        ("src-completed", "new-completed"),
        ("src-pending", "new-pending"),
        ("src-unknown", "new-unknown"),
        ("src-terminated", "new-terminated"),
        ("src-archived", "new-archived"),
    ] {
        assert_eq!(
            insert_session_copy(
                &env.paths(),
                &env.paths(),
                new_id,
                source_id,
                "uid-a",
                "uid-b",
                false,
                false,
                true,
                None,
            )
            .unwrap(),
            // R6-08 起默认不搬历史消耗，因此每次复制都会带这条诊断（只影响「共消耗」展示）。
            DbCopyOutcome::Inserted {
                aux_note: Some(AUX_USAGE_LEFT_IN_SOURCE_NOTE),
                space_note: None
            }
        );
    }

    assert_eq!(
        env.status_of("new-working"),
        Some("completed".to_string()),
        "运行中的源状态必须归一为 completed（否则客户端仍显示思考中）"
    );
    assert_eq!(env.status_of("new-active"), Some("completed".to_string()));
    assert_eq!(
        env.status_of("new-pending"),
        Some("completed".to_string()),
        "Pending 不在终态常量里，必须被归一（白名单口径会漏掉它）"
    );
    assert_eq!(
        env.status_of("new-unknown"),
        Some("completed".to_string()),
        "未知状态同样归一：客户端可能把它当运行中"
    );
    assert_eq!(
        env.status_of("new-error"),
        Some("error".to_string()),
        "错误状态必须原样保留"
    );
    assert_eq!(
        env.status_of("new-completed"),
        Some("completed".to_string())
    );
    assert_eq!(
        env.status_of("new-terminated"),
        Some("terminated".to_string()),
        "terminated 是终态，保留集合不得误伤"
    );
    assert_eq!(
        env.status_of("new-archived"),
        Some("archived".to_string()),
        "归档态必须原样保留：复制不该让已归档会话变成未归档"
    );
    // 源行本身的 status 不受影响（只归一副本）。
    assert_eq!(env.status_of("src-working"), Some("working".to_string()));
    assert_eq!(env.status_of("src-pending"), Some("Pending".to_string()));
    assert_eq!(env.status_of("src-archived"), Some("archived".to_string()));
}

/// 回归：复制一个**已归档**会话，副本行必须仍是 `archived`（而不是被抹成
/// `completed`），因此预览也不会额外下发 `archiveAction: statusOnly`。
///
/// 抹成 `completed` 会让副本在目标账号里显示成未归档，还会让同步预览多出一个
/// 用户没请求的归档动作。
#[test]
fn copy_preserves_archived_row_status_without_archive_action() {
    let env = ready_env_with_status("copy-archived-row");
    env.set_status("sess-1", "archived");

    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);

    assert_eq!(
        env.status_of(&target_id).as_deref(),
        Some("archived"),
        "归档态必须原样继承"
    );
    let preview_report = preview(&env, "uid-b");
    let group = &preview_report["groups"][0];
    assert_eq!(group["verdict"], "identical", "{preview_report}");
    assert!(
        group["archiveAction"].is_null(),
        "副本已归档，不该再下发归档动作：{group}"
    );
}

/// 目标表没有 `status` 列（老库）：照常复制，不得因此报错。
#[test]
fn insert_session_copy_without_status_column_still_works() {
    let env = ready_env("insert-no-status");
    assert_eq!(
        insert_session_copy(
            &env.paths(),
            &env.paths(),
            "new-no-status",
            "sess-1",
            "uid-a",
            "uid-b",
            false,
            false,
            true,
            None,
        )
        .unwrap(),
        // R6-08 起默认不搬历史消耗，因此带这条诊断（与「目标库不支持账本」是两回事）。
        DbCopyOutcome::Inserted {
            aux_note: Some(AUX_USAGE_LEFT_IN_SOURCE_NOTE),
            space_note: None
        }
    );
    assert_eq!(env.status_of("new-no-status"), None);
}

/// 归一后的正文必须能通过 `write_copy_body` 的完整性校验。
///
/// 这是本次最容易踩的坑：`status` 一变，那一行的摘要与 `total_digest` 就变，
/// 期望值只能取「待写入文本」的摘要；拿源快照摘要去比必然不等。
#[test]
fn normalized_copy_body_passes_integrity_check() {
    let env = ready_env("copy-normalize-integrity");
    let source_text = incomplete_body("sess-1");
    env.add_body("sess-1", &source_text);
    let paths = env.paths();
    let source_path = env.body_path("sess-1");
    let snapshot = match session_link::read_content_snapshot(&source_path, "sess-1") {
        ContentState::Ready(snapshot) => snapshot,
        other => panic!("源正文应可读，实际 {other:?}"),
    };

    let body = copy_body(&snapshot, "sess-1", "new-cid-fixed").unwrap();
    // 期望摘要 == 对同一份待写入文本重新归一化算出的摘要（口径完全一致）。
    let recomputed = session_link::normalize_jsonl(&body.text, "new-cid-fixed").unwrap();
    assert_eq!(recomputed.total_digest, body.normalized.total_digest);
    // 源摘要与归一化摘要必须不同：否则这条用例证明不了「先写后改会判失败」这个坑。
    assert_ne!(
        snapshot.normalized.total_digest, body.normalized.total_digest,
        "归一必须真的改变了摘要，用例才有意义"
    );

    // 真正落盘 + 回读校验（校验逻辑本身没有放宽）：必须成功。
    let written = write_copy_body(&body, &source_path, &paths, &paths, "new-cid-fixed")
        .expect("归一后的正文必须通过写后完整性校验");
    assert_eq!(written, env.body_path("new-cid-fixed"));
    match session_link::read_content_snapshot(&written, "new-cid-fixed") {
        ContentState::Ready(read_back) => assert_eq!(
            read_back.normalized.total_digest, body.normalized.total_digest,
            "落盘正文与期望摘要必须一致"
        ),
        other => panic!("落盘正文应可读，实际 {other:?}"),
    }
}

/// 建一套「带项目归属」的库：sessions 有 `space_id` / `project_id`，另有 spaces 表。
///
/// 表结构与本机实测的真实库一致（`deleted_at` 默认 -1、`pinned`/`status`/`transport`
/// 都有默认值），这样才能验证复制的行是否真能被客户端当成项目会话。
fn create_db_with_spaces(env: &Env) {
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            title TEXT,
            custom_title TEXT,
            cwd TEXT,
            space_id TEXT,
            project_id TEXT,
            created_at INTEGER,
            updated_at INTEGER,
            deleted_at INTEGER,
            is_playground INTEGER
        );
        CREATE TABLE spaces (
            id TEXT PRIMARY KEY NOT NULL,
            user_id TEXT NOT NULL,
            device_id TEXT NOT NULL,
            name TEXT NOT NULL,
            primary_folder_path TEXT,
            create_folder_path TEXT,
            pinned INTEGER NOT NULL DEFAULT 0,
            sort_order INTEGER,
            status TEXT NOT NULL DEFAULT 'active',
            transport TEXT NOT NULL DEFAULT 'local',
            conversation_origin TEXT,
            deleted_at INTEGER NOT NULL DEFAULT -1,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            source_kind TEXT
        );",
    )
    .unwrap();
}

/// 复制后目标会话行的 (space_id, project_id)。
fn space_of(env: &Env, cid: &str) -> (Option<String>, Option<String>) {
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.query_row(
        "SELECT space_id, project_id FROM sessions WHERE id = ?1",
        [cid],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap()
}

/// a) 目标账号已有同名 space → 复制后 `space_id` 指向**目标账号**的那个 id。
///
/// 这是用户报告的现象的最小复现：同名不同 id，原样搬运会让目标行悬空。
#[test]
fn copy_remaps_space_id_to_target_account_space() {
    let env = Env::new("space-reuse");
    create_db_with_spaces(&env);
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "INSERT INTO spaces (id, user_id, device_id, name, source_kind, created_at, updated_at)
         VALUES ('space-a', 'uid-a', 'dev-1', 'ResumeForge', 'seed', 1, 2),
                ('space-b', 'uid-b', 'dev-1', 'ResumeForge', 'seed', 3, 4);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, space_id, project_id, created_at, updated_at, deleted_at, is_playground)
         VALUES ('src-1', 'uid-a', '会话', 'D:/ResumeForge', 'space-a', 'proj-a', 1000, 2000, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);

    let outcome = insert_session_copy(
        &env.paths(),
        &env.paths(),
        "new-1",
        "src-1",
        "uid-a",
        "uid-b",
        false,
        false,
        true,
        None,
    )
    .unwrap();
    assert_eq!(
        outcome,
        DbCopyOutcome::Inserted {
            // R6-08 起默认不搬历史消耗，因此带这条诊断（本用例只关心 space 映射）。
            aux_note: Some(AUX_USAGE_LEFT_IN_SOURCE_NOTE),
            space_note: None
        }
    );

    let (space_id, project_id) = space_of(&env, "new-1");
    assert_eq!(
        space_id.as_deref(),
        Some("space-b"),
        "必须落到目标账号自己的同名 space，不能留源账号的 space-a"
    );
    assert_eq!(
        project_id, None,
        "跨账号的 project_id 一定悬空，必须写 NULL"
    );

    let conn = Connection::open(env.paths().workbuddy_db()).unwrap();
    let spaces: i64 = conn
        .query_row("SELECT COUNT(*) FROM spaces", [], |row| row.get(0))
        .unwrap();
    assert_eq!(spaces, 2, "命中已有 space 时不得新建");
}

/// b) 目标账号没有同名 space → 自动新建一条 spaces 行，会话指向它。
#[test]
fn copy_creates_space_for_target_when_missing() {
    let env = Env::new("space-create");
    create_db_with_spaces(&env);
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "INSERT INTO spaces (id, user_id, device_id, name, source_kind, created_at, updated_at)
         VALUES ('space-a', 'uid-a', 'dev-src', 'ResumeForge', 'seed', 1, 2),
                ('space-other', 'uid-b', 'dev-dst', '其它项目', 'local', 3, 4);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, space_id, created_at, updated_at, deleted_at, is_playground)
         VALUES ('src-1', 'uid-a', '会话', 'D:/ResumeForge', 'space-a', 1000, 2000, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);

    let outcome = insert_session_copy(
        &env.paths(),
        &env.paths(),
        "new-1",
        "src-1",
        "uid-a",
        "uid-b",
        false,
        false,
        true,
        None,
    )
    .unwrap();
    assert_eq!(
        outcome,
        DbCopyOutcome::Inserted {
            // R6-08 起默认不搬历史消耗，因此带这条诊断（本用例只关心 space 映射）。
            aux_note: Some(AUX_USAGE_LEFT_IN_SOURCE_NOTE),
            space_note: None
        }
    );

    let (space_id, _) = space_of(&env, "new-1");
    let space_id = space_id.expect("目标账号没有同名 space 时应当新建并回填");
    assert_ne!(space_id, "space-a", "不得复用源账号的 space");
    assert_ne!(space_id, "space-other", "不得挂到不同名的 space 上");

    let conn = Connection::open(env.paths().workbuddy_db()).unwrap();
    let (user_id, device_id, name, status, transport, pinned, deleted_at, source_kind): (
        String,
        String,
        String,
        String,
        String,
        i64,
        i64,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT user_id, device_id, name, status, transport, pinned, deleted_at, source_kind
             FROM spaces WHERE id = ?1",
            [&space_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(user_id, "uid-b", "新 space 必须属于目标账号");
    assert_eq!(device_id, "dev-src", "设备标识沿用源 space");
    assert_eq!(name, "ResumeForge");
    assert_eq!(source_kind.as_deref(), Some("seed"), "来源标记沿用源 space");
    assert_eq!(status, "active");
    assert_eq!(transport, "local");
    assert_eq!(pinned, 0);
    assert_eq!(deleted_at, -1);
    let created_at: i64 = conn
        .query_row(
            "SELECT created_at FROM spaces WHERE id = ?1",
            [&space_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(created_at > 0, "时间戳必须写入当前时间，不能是 0");
}

/// 源 space 的置顶标记（`spaces.pinned`）；列不存在或值为 NULL 时为 None。
fn pinned_of(env: &Env, space_id: &str) -> Option<i64> {
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.query_row("SELECT pinned FROM spaces WHERE id = ?1", [space_id], |row| {
        row.get::<_, Option<i64>>(0)
    })
    .ok()
    .flatten()
}

/// 源 space 已置顶 → 目标账号新建的同构 space 也必须置顶。
///
/// 置顶是用户显式做的整理动作；复制过去的新项目若丢掉它，用户会以为置顶被重置了。
#[test]
fn copy_passes_source_space_pinned_to_new_space() {
    let env = Env::new("space-pinned");
    create_db_with_spaces(&env);
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "INSERT INTO spaces (id, user_id, device_id, name, source_kind, pinned, created_at, updated_at)
         VALUES ('space-a', 'uid-a', 'dev-src', 'ResumeForge', 'seed', 1, 1, 2);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, space_id, created_at, updated_at, deleted_at, is_playground)
         VALUES ('src-1', 'uid-a', '会话', 'D:/ResumeForge', 'space-a', 1000, 2000, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);

    insert_session_copy(
        &env.paths(),
        &env.paths(),
        "new-1",
        "src-1",
        "uid-a",
        "uid-b",
        false,
        false,
        true,
        None,
    )
    .unwrap();

    let (space_id, _) = space_of(&env, "new-1");
    let space_id = space_id.expect("目标账号没有同名 space 时应当新建");
    assert_ne!(space_id, "space-a", "不得复用源账号的 space");
    assert_eq!(
        pinned_of(&env, &space_id),
        Some(1),
        "源 space 已置顶，新建的 space 必须同样置顶"
    );
}

/// 源 space 取不到置顶值（老库缺列 / 值为 NULL）时仍写 0：该列 NOT NULL，不能写 NULL。
#[test]
fn copy_writes_zero_pinned_when_source_space_has_no_pin() {
    let env = Env::new("space-pinned-none");
    create_db_with_spaces(&env);
    // 缺列：`SELECT *` 读出的行里根本没有这个键。
    let without_column = vec![(
        "name".to_string(),
        rusqlite::types::Value::Text("ResumeForge".to_string()),
    )];
    assert_eq!(
        space_meta_from_row(&without_column).unwrap().pinned,
        None,
        "缺列必须读成 None，而不是 0"
    );
    // 有列但值为 NULL。
    let mut with_null = without_column.clone();
    with_null.push(("pinned".to_string(), rusqlite::types::Value::Null));
    assert_eq!(
        space_meta_from_row(&with_null).unwrap().pinned,
        None,
        "NULL 必须读成 None"
    );

    let meta = SpaceMeta {
        name: "ResumeForge".to_string(),
        device_id: Some("dev-1".to_string()),
        source_kind: Some("seed".to_string()),
        create_folder_path: None,
        folder_paths: Vec::new(),
        pinned: None,
    };
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    let new_id = insert_target_space(&conn, &env.paths(), "uid-b", &meta, "D:/ResumeForge")
        .unwrap()
        .expect("设备标识齐全时应当新建 space");
    let pinned: i64 = conn
        .query_row("SELECT pinned FROM spaces WHERE id = ?1", [&new_id], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(pinned, 0, "取不到置顶值时写 0（列 NOT NULL）");
}

/// c) 源 `space_id` 为 NULL → 目标仍为 NULL，且不新建任何 spaces 行。
#[test]
fn copy_keeps_null_space_id_without_creating_space() {
    let env = Env::new("space-null");
    create_db_with_spaces(&env);
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "INSERT INTO spaces (id, user_id, device_id, name, source_kind, created_at, updated_at)
         VALUES ('space-b', 'uid-b', 'dev-1', 'ResumeForge', 'seed', 1, 2);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, space_id, created_at, updated_at, deleted_at, is_playground)
         VALUES ('src-1', 'uid-a', '无归属会话', 'D:/ResumeForge', NULL, 1000, 2000, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);

    let outcome = insert_session_copy(
        &env.paths(),
        &env.paths(),
        "new-1",
        "src-1",
        "uid-a",
        "uid-b",
        false,
        false,
        true,
        None,
    )
    .unwrap();
    assert_eq!(
        outcome,
        DbCopyOutcome::Inserted {
            // R6-08 起默认不搬历史消耗，因此带这条诊断（本用例只关心 space 映射）。
            aux_note: Some(AUX_USAGE_LEFT_IN_SOURCE_NOTE),
            space_note: None
        }
    );

    let (space_id, _) = space_of(&env, "new-1");
    assert_eq!(space_id, None, "源无项目归属时不得凭空塞进同名 space");
    let conn = Connection::open(env.paths().workbuddy_db()).unwrap();
    let spaces: i64 = conn
        .query_row("SELECT COUNT(*) FROM spaces", [], |row| row.get(0))
        .unwrap();
    assert_eq!(spaces, 1, "不得新建 spaces 行");
}

/// QA-E1 回归：复制被拒时不得残留本次新建的 space 行。
///
/// 错误路径是「Claw 工作区」：校验必须早于建 space，否则目标账号会多出一条
/// 0 会话的空项目。这里刻意让目标账号**没有**同名 space（即会走新建分支），
/// 才能证明失败路径上确实什么都没落地。
#[test]
fn copy_rejection_leaves_no_orphan_space() {
    let env = Env::new("space-orphan");
    create_db_with_spaces(&env);
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "INSERT INTO spaces (id, user_id, device_id, name, source_kind, create_folder_path, created_at, updated_at)
         VALUES ('space-a', 'uid-a', 'dev-1', 'ResumeForge', 'seed', 'D:/ResumeForge', 1, 2);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, space_id, created_at, updated_at, deleted_at, is_playground)
         VALUES ('src-1', 'uid-a', 'Claw 会话', 'D:/proj/claw', 'space-a', 1000, 2000, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);

    let error = insert_session_copy(
        &env.paths(),
        &env.paths(),
        "new-1",
        "src-1",
        "uid-a",
        "uid-b",
        false,
        false,
        true,
        None,
    )
    .unwrap_err();
    assert!(error.contains("Claw"), "{error}");

    let conn = Connection::open(env.paths().workbuddy_db()).unwrap();
    let spaces: i64 = conn
        .query_row("SELECT COUNT(*) FROM spaces", [], |row| row.get(0))
        .unwrap();
    assert_eq!(spaces, 1, "错误路径不得残留孤儿 spaces 行");
    let sessions: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(sessions, 1, "错误路径不得写入会话行");
}

/// 同一不变量的第二条路径：会话行插入失败（UUID 撞库）时，新建的 space 必须回滚。
///
/// 这是事务存在的意义——只把校验提前挡不住 INSERT 失败，必须整体回滚。
#[test]
fn copy_insert_failure_rolls_back_new_space() {
    let env = Env::new("space-rollback");
    create_db_with_spaces(&env);
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "INSERT INTO spaces (id, user_id, device_id, name, source_kind, created_at, updated_at)
         VALUES ('space-a', 'uid-a', 'dev-1', 'ResumeForge', 'seed', 1, 2);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, space_id, created_at, updated_at, deleted_at, is_playground)
         VALUES ('src-1', 'uid-a', '会话', 'D:/ResumeForge', 'space-a', 1000, 2000, NULL, 0)",
        [],
    )
    .unwrap();
    // 目标账号先占住 'new-1' 这个 id：普通 INSERT 必然撞主键失败。
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, created_at, updated_at, deleted_at, is_playground)
         VALUES ('new-1', 'uid-b', '占位', 'D:/ResumeForge', 1, 2, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);

    let error = insert_session_copy(
        &env.paths(),
        &env.paths(),
        "new-1",
        "src-1",
        "uid-a",
        "uid-b",
        false,
        false,
        true,
        None,
    )
    .unwrap_err();
    assert!(error.contains("会话记录保存失败"), "{error}");

    let conn = Connection::open(env.paths().workbuddy_db()).unwrap();
    let spaces: i64 = conn
        .query_row("SELECT COUNT(*) FROM spaces", [], |row| row.get(0))
        .unwrap();
    assert_eq!(spaces, 1, "会话行写入失败时，新建的 space 必须随事务回滚");
    let sessions: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(sessions, 2, "原有两行会话不得被改动");
}

/// 新建 space 必须继承源 space 的 `create_folder_path`；源为 NULL 时保持 NULL。
///
/// 实测真实行：`primary_folder_path` 是 NULL、`create_folder_path` 有值
/// （如 `D:\ResumeForge`）。客户端靠后者展示/打开项目，漏写会让新项目显示异常。
#[test]
fn copy_new_space_inherits_create_folder_path() {
    // 源有值 → 新行沿用。
    let env = Env::new("space-folder");
    create_db_with_spaces(&env);
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "INSERT INTO spaces (id, user_id, device_id, name, source_kind, create_folder_path, created_at, updated_at)
         VALUES ('space-a', 'uid-a', 'dev-1', 'ResumeForge', 'seed', 'D:/ResumeForge', 1, 2);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, space_id, created_at, updated_at, deleted_at, is_playground)
         VALUES ('src-1', 'uid-a', '会话', 'D:/ResumeForge', 'space-a', 1000, 2000, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);
    insert_session_copy(
        &env.paths(),
        &env.paths(),
        "new-1",
        "src-1",
        "uid-a",
        "uid-b",
        false,
        false,
        true,
        None,
    )
    .unwrap();
    let (space_id, _) = space_of(&env, "new-1");
    let space_id = space_id.unwrap();
    let conn = Connection::open(env.paths().workbuddy_db()).unwrap();
    let folder: Option<String> = conn
        .query_row(
            "SELECT create_folder_path FROM spaces WHERE id = ?1",
            [&space_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        folder.as_deref(),
        Some("D:/ResumeForge"),
        "新建 space 必须沿用源 space 的 create_folder_path"
    );

    // 源为 NULL → 新行也是 NULL（不得拿 cwd 硬猜）。
    let env2 = Env::new("space-folder-null");
    create_db_with_spaces(&env2);
    let conn = Connection::open(env2.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "INSERT INTO spaces (id, user_id, device_id, name, source_kind, created_at, updated_at)
         VALUES ('space-a', 'uid-a', 'dev-1', 'ResumeForge', 'seed', 1, 2);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, cwd, space_id, created_at, updated_at, deleted_at, is_playground)
         VALUES ('src-1', 'uid-a', '会话', 'D:/ResumeForge/sub', 'space-a', 1000, 2000, NULL, 0)",
        [],
    )
    .unwrap();
    drop(conn);
    insert_session_copy(
        &env2.paths(),
        &env2.paths(),
        "new-1",
        "src-1",
        "uid-a",
        "uid-b",
        false,
        false,
        true,
        None,
    )
    .unwrap();
    let (space_id, _) = space_of(&env2, "new-1");
    let space_id = space_id.unwrap();
    let conn = Connection::open(env2.paths().workbuddy_db()).unwrap();
    let folder: Option<String> = conn
        .query_row(
            "SELECT create_folder_path FROM spaces WHERE id = ?1",
            [&space_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(folder, None, "源没有文件夹时不得用会话 cwd 硬猜");
}

#[test]
fn backup_failure_is_propagated_instead_of_claimed_success() {
    let env = ready_env("backup-fail");
    std::fs::remove_file(env.paths.workbuddy_db()).unwrap();
    let err = backup_workbuddy_db(&env.paths(), &env.paths.backup_root()).unwrap_err();
    assert!(err.contains("会话数据不存在"), "{err}");

    // 正常备份返回主库路径且大小一致。
    env.create_db();
    env.add_session("sess-1", "uid-a", "标题一");
    let backup = backup_workbuddy_db(&env.paths(), &env.paths.backup_root()).unwrap();
    assert!(backup.ends_with("workbuddy.db"));
    assert_eq!(
        std::fs::metadata(&backup).unwrap().len(),
        std::fs::metadata(env.paths.workbuddy_db()).unwrap().len()
    );
}

#[test]
fn session_row_owner_reads_target_uid() {
    let env = ready_env("row-owner");
    assert_eq!(
        session_row_owner(&env.paths(), "sess-1").as_deref(),
        Some("uid-a")
    );
    assert_eq!(session_row_owner(&env.paths(), "missing"), None);
    env.delete_row("sess-1");
    assert_eq!(session_row_owner(&env.paths(), "sess-1"), None);
}

/// 遗留库（`sessions` 没有 `deleted_at` 列）的归属判定：
///
/// 硬套 `deleted_at IS NULL` 会让 `prepare` 失败 → 所有行都被判成「行缺失」，与存活索引
/// 「缺列按全部存活读」的结论分叉（组会列出但内容永远 unknown）。缺列时按未删除处理。
#[test]
fn session_row_owner_tolerates_a_legacy_table_without_deleted_at() {
    let env = Env::new("row-owner-legacy");
    std::fs::create_dir_all(env.paths.data_root.clone()).unwrap();
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, title TEXT);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title) VALUES ('sess-1', 'uid-a', '标题一')",
        [],
    )
    .unwrap();
    drop(conn);

    // 缺列 + 行存在：仍然读得到归属（而不是一律判缺失）。
    assert_eq!(
        session_row_owner(&env.paths(), "sess-1").as_deref(),
        Some("uid-a")
    );
    // 缺列 + 行不存在：仍然返回 None。
    assert_eq!(session_row_owner(&env.paths(), "missing"), None);
}

// ---------------------------------------------------------------------------
// 源正文容错：末行截断可修复，且修复动作必须报告给用户（R1 / issue #1）
// ---------------------------------------------------------------------------

/// 源会话末行被截断（进程被杀留下的半条记录）：复制路径容错成功，并把修复动作报告出来。
///
/// 严格校验（`read_content_snapshot` → `normalize_jsonl`）仍然拒绝截断内容——这一条
/// 与 `truncated_body_is_not_reported_as_success` 是同一个不变量的两面：判据不变，
/// 只是复制路径多了一步「末行截断且前面有完整记录」的可解释修复。
#[test]
fn truncated_tail_body_is_repaired_and_reported() {
    let env = ready_env("repair-tail");
    // 两条完整记录 + 一条半截记录（第 3 行）。
    let source = format!(
        "{}{}",
        body_text("sess-1"),
        "{\"type\":\"user\",\"sessionId\":\"sess-1\",\"text\":\"半"
    );
    env.add_body("sess-1", &source);
    assert!(
        matches!(
            session_link::read_content_snapshot(&env.body_path("sess-1"), "sess-1"),
            session_link::ContentState::Unavailable(_)
        ),
        "严格校验必须仍然判为不可验证"
    );

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    assert!(report.get("errors").is_none(), "{report}");
    let note = report["copied"][0]["sourceRepaired"]
        .as_str()
        .unwrap_or_default();
    assert!(note.contains("第 3 行"), "修复说明必须指出行号：{note}");
    assert!(note.contains("已跳过"), "{note}");

    // 副本 = 修复后的正文：两条记录、新 id 引用、能通过严格读取。
    let new_id = env.first_copy_id(&report);
    let copied = std::fs::read_to_string(env.body_path(&new_id)).unwrap();
    assert_eq!(copied.lines().count(), 2, "{copied}");
    assert!(copied.contains(&new_id));
    assert!(!copied.contains("sess-1"), "{copied}");
    assert!(
        matches!(
            session_link::read_content_snapshot(&env.body_path(&new_id), &new_id),
            session_link::ContentState::Ready(snapshot) if snapshot.normalized.record_count == 2
        ),
        "副本必须能通过严格校验"
    );

    // 源文件一字不改（修复只在内存里做）。
    assert_eq!(
        std::fs::read_to_string(env.body_path("sess-1")).unwrap(),
        source
    );
    assert_eq!(env.rows_for("uid-b").len(), 1);
}

/// 两个对象粘在同一行（上次退出没写换行）：无损拆分后复制成功，报告里说明「内容未丢失」。
#[test]
fn glued_records_are_split_and_reported() {
    let env = ready_env("repair-glued");
    let glued = format!(
        "{}{}",
        json!({"type": "user", "sessionId": "sess-1", "text": "你好"}),
        json!({"type": "assistant", "sessionId": "sess-1", "text": "hi"})
    );
    env.add_body("sess-1", &format!("{glued}\n"));

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    let note = report["copied"][0]["sourceRepaired"]
        .as_str()
        .unwrap_or_default();
    assert!(note.contains("第 1 行"), "{note}");
    assert!(note.contains("拆分"), "{note}");
    assert!(!note.contains("已跳过"), "无损拆分不得报告丢记录：{note}");

    let new_id = env.first_copy_id(&report);
    let copied = std::fs::read_to_string(env.body_path(&new_id)).unwrap();
    assert_eq!(copied.lines().count(), 2, "{copied}");
}

/// 末行「完整记录 + 半截记录」：同行的完整记录必须保留（只丢末尾半截），
/// 报告要把「丢了半截」和「完整记录还在」两件事都说清楚，不能让用户以为丢了一条。
#[test]
fn truncated_tail_with_a_complete_record_keeps_that_record() {
    let env = ready_env("repair-tail-keep");
    let source = format!(
        "{}{}{}",
        body_text("sess-1"),
        json!({"type": "user", "sessionId": "sess-1", "text": "第三条"}),
        "{\"type\":\"assistant\",\"sessionId\":\"sess-1\",\"text\":\"半"
    );
    env.add_body("sess-1", &source);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    let note = report["copied"][0]["sourceRepaired"]
        .as_str()
        .unwrap_or_default();
    assert!(note.contains("第 3 行"), "必须指出坏行行号：{note}");
    assert!(note.contains("已跳过"), "{note}");
    assert!(
        note.contains("同行的 1 条完整记录已保留"),
        "保留了记录就必须说明，不能只说丢了一条：{note}"
    );

    let new_id = env.first_copy_id(&report);
    let copied = std::fs::read_to_string(env.body_path(&new_id)).unwrap();
    assert_eq!(copied.lines().count(), 3, "{copied}");
    assert!(
        copied.contains("第三条"),
        "同行已写完整的记录不得被丢掉：{copied}"
    );
    match session_link::read_content_snapshot(&env.body_path(&new_id), &new_id) {
        session_link::ContentState::Ready(snapshot) => {
            assert_eq!(snapshot.normalized.record_count, 3)
        }
        other => panic!("副本必须能通过严格校验，实际 {other:?}"),
    }
    // 源文件一字不改。
    assert_eq!(
        std::fs::read_to_string(env.body_path("sess-1")).unwrap(),
        source
    );
}

/// 没有做修复时报告里不出现 `sourceRepaired`（其它字段逐字不变）。
#[test]
fn clean_source_copy_has_no_source_repaired_field() {
    let env = ready_env("repair-clean");
    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1);
    assert!(
        report["copied"][0].get("sourceRepaired").is_none(),
        "{report}"
    );
}

// ---------------------------------------------------------------------------
// R6-08：复制默认不搬 `session_usage`（「谁用的记在谁头上」）
// ---------------------------------------------------------------------------

/// 默认口径（`copy_usage = false`）：**绝不搬来源数值**——源账本逐字段不变，副本只有
/// 零值占位行；中断记录与积分无关，默认仍照搬。
#[test]
fn copy_leaves_source_usage_untouched_and_writes_a_zero_row() {
    let env = ready_env("aux-r6-08-default");
    let db = env.paths.workbuddy_db();
    create_ledger_tables(&db);
    write_ledger(
        &db,
        "sess-1",
        12_345,
        678,
        r#"{"r1":1.25}"#,
        &[("req-1", None, None)],
    );
    let source_usage_before = ledger_usage(&db, "sess-1");
    let source_interruptions_before = ledger_interruptions(&db, "sess-1");

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    let new_id = env.first_copy_id(&report);

    // 源账号的历史消耗必须原样留在源账号。
    assert_eq!(
        ledger_usage(&db, "sess-1"),
        source_usage_before,
        "源账本必须逐字段不变"
    );
    assert_eq!(
        ledger_interruptions(&db, "sess-1"),
        source_interruptions_before,
        "源中断记录必须逐字段不变"
    );

    // 副本只有零值占位行（客户端要求「有行」时的兜底），不是来源数值。
    let (used, size, credit) = ledger_usage(&db, &new_id).expect("默认口径必须留零值占位行");
    assert_eq!((used, size), (0, 0));
    assert_eq!(credit.as_deref(), Some("{}"));
    assert_eq!(
        report["copied"][0]["auxNote"],
        json!(AUX_USAGE_LEFT_IN_SOURCE_NOTE),
        "必须让用户知道「共消耗」为什么是 0"
    );
    // 中断记录默认仍搬（与积分无关）。
    assert_eq!(ledger_interruptions(&db, &new_id).len(), 1);
}

/// 零值占位行是「只增不改」：客户端已经给副本写下的真实用量绝不被覆盖。
#[test]
fn zero_usage_row_never_overwrites_client_written_values() {
    let env = ready_env("aux-r6-08-zero-keep");
    let db = env.paths.workbuddy_db();
    create_ledger_tables(&db);
    write_ledger(&db, "sess-1", 999, 9, r#"{"r1":7.5}"#, &[]);
    // 目标库已存在同 session_id 的用量行（客户端在复制落库后自己写过的形态）。
    let conn = Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO session_usage (session_id, used, size, updated_at, credit_json) \
         VALUES ('new-zero', 42, 4, 1, '{\"r9\":6.5}')",
        [],
    )
    .unwrap();
    drop(conn);

    insert_session_copy(
        &env.paths(),
        &env.paths(),
        "new-zero",
        "sess-1",
        "uid-a",
        "uid-b",
        false,
        false,
        true,
        None,
    )
    .unwrap();

    let (used, size, credit) = ledger_usage(&db, "new-zero").expect("行仍在");
    assert_eq!((used, size), (42, 4), "零值行不得覆盖客户端真实用量");
    assert_eq!(credit.as_deref(), Some(r#"{"r9":6.5}"#));
}

// ---------------------------------------------------------------------------
// R5-14：会话账本（session_usage + session_turn_interruptions）随副本搬运
// ---------------------------------------------------------------------------

/// 给环境加上两张账本表（真实 0.2.3 客户端库里有，测试夹具默认没有）。
fn create_ledger_tables(db: &Path) {
    let conn = Connection::open(db).unwrap();
    conn.execute_batch(
        "CREATE TABLE session_usage (
            session_id TEXT PRIMARY KEY,
            used INTEGER NOT NULL,
            size INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            credit_json TEXT
        );
        CREATE TABLE session_turn_interruptions (
            session_id TEXT NOT NULL,
            request_id TEXT NOT NULL,
            reason_code TEXT,
            presentation TEXT,
            PRIMARY KEY (session_id, request_id)
        );",
    )
    .unwrap();
}

fn write_ledger(
    db: &Path,
    sid: &str,
    used: i64,
    size: i64,
    credit_json: &str,
    interruptions: &[(&str, Option<&str>, Option<&str>)],
) {
    let conn = Connection::open(db).unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO session_usage (session_id, used, size, updated_at, credit_json) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![sid, used, size, 111, credit_json],
    )
    .unwrap();
    for (request_id, reason, presentation) in interruptions {
        conn.execute(
            "INSERT OR REPLACE INTO session_turn_interruptions \
             (session_id, request_id, reason_code, presentation) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![sid, request_id, reason, presentation],
        )
        .unwrap();
    }
}

fn ledger_usage(db: &Path, sid: &str) -> Option<(i64, i64, Option<String>)> {
    let conn = Connection::open(db).unwrap();
    conn.query_row(
        "SELECT used, size, credit_json FROM session_usage WHERE session_id = ?1",
        [sid],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .ok()
}

fn ledger_interruptions(db: &Path, sid: &str) -> Vec<(String, Option<String>, Option<String>)> {
    let conn = Connection::open(db).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT request_id, reason_code, presentation FROM session_turn_interruptions \
             WHERE session_id = ?1 ORDER BY request_id",
        )
        .unwrap();
    let rows = stmt
        .query_map([sid], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .flatten()
        .collect();
    rows
}

/// 用量与中断记录原样搬到副本（**仅在显式打开 `copy_usage` 时**）：`credit_json` 逐字节
/// 相等（含浮点尾数）、`used`/`size` 逐字段相等、中断记录的 `request_id` 集合完全一致。
#[test]
fn copy_carries_usage_and_interruptions_verbatim() {
    let env = ready_env("aux-happy");
    let db = env.paths.workbuddy_db();
    create_ledger_tables(&db);
    // 真实库里 credit_json 的键是请求 id，值与账号无关；故意带浮点尾数验证不格式化。
    let credit = r#"{"01a1f90d":8.569999999999999,"01a1f914":6.47}"#;
    write_ledger(
        &db,
        "sess-1",
        123_456,
        7_890,
        credit,
        &[
            ("req-b", Some("rate_limited"), Some("限流")),
            ("req-a", None, None),
        ],
    );

    let report = copy_with_usage(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    let new_id = env.first_copy_id(&report);

    let (used, size, copied_credit) = ledger_usage(&db, &new_id).expect("副本必须有用量行");
    assert_eq!((used, size), (123_456, 7_890));
    assert_eq!(
        copied_credit.as_deref(),
        Some(credit),
        "credit_json 必须逐字相等"
    );
    // 核对合计：json_each 求和相等（这就是客户端「共消耗」的口径）。
    let conn = Connection::open(&db).unwrap();
    let total: f64 = conn
        .query_row(
            "SELECT SUM(value) FROM session_usage, json_each(credit_json) WHERE session_id = ?1",
            [&new_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!((total - (8.569999999999999 + 6.47)).abs() < 1e-9);

    let interruptions = ledger_interruptions(&db, &new_id);
    assert_eq!(interruptions.len(), 2);
    assert_eq!(interruptions[0].0, "req-a");
    assert_eq!(interruptions[1].0, "req-b");
    // 未知 reason_code 原样保留，不做枚举校验。
    assert_eq!(interruptions[1].1.as_deref(), Some("rate_limited"));

    // 两表时序一致：账本 updated_at 与同一事务写的 sessions.updated_at 相等。
    let (usage_updated, session_updated): (i64, i64) = conn
        .query_row(
            "SELECT u.updated_at, s.updated_at FROM session_usage u \
             JOIN sessions s ON s.id = u.session_id WHERE u.session_id = ?1",
            [&new_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(usage_updated, session_updated);
    assert!(
        report["copied"][0].get("auxNote").is_none(),
        "有账本可搬时不应有诊断"
    );
}

/// 源库有账本表但该会话没有用量行：不算失败，也不写空行。
#[test]
fn copy_without_usage_rows_is_not_a_failure() {
    let env = ready_env("aux-empty");
    let db = env.paths.workbuddy_db();
    create_ledger_tables(&db);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    let new_id = env.first_copy_id(&report);
    // R6-08：默认口径写**零值行**（客户端要求副本有行），绝不写来源数值。
    let (used, size, credit) = ledger_usage(&db, &new_id).expect("默认口径必须留零值占位行");
    assert_eq!((used, size), (0, 0));
    assert_eq!(credit.as_deref(), Some("{}"));
    assert_eq!(
        report["copied"][0]["auxNote"],
        json!(AUX_USAGE_LEFT_IN_SOURCE_NOTE),
        "必须让用户知道「共消耗」为什么是 0"
    );
}

/// 源库连账本表都没有（旧客户端）：跳过，复制照旧成功。
#[test]
fn copy_skips_when_source_has_no_ledger_tables() {
    let env = ready_env("aux-legacy");
    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(report["errors"].as_array().map_or(0, Vec::len), 0);
}

/// 目标库缺账本表（跨档/跨库场景）：跳过并写诊断，不算失败。
#[test]
fn copy_reports_aux_note_when_target_lacks_ledger_tables() {
    let source = ready_env("aux-target-legacy-src");
    let target = ready_env("aux-target-legacy-dst");
    let source_db = source.paths.workbuddy_db();
    create_ledger_tables(&source_db);
    write_ledger(&source_db, "sess-1", 10, 20, r#"{"r1":1.5}"#, &[]);

    let outcome = insert_session_copy(
        &source.paths(),
        &target.paths(),
        "new-legacy",
        "sess-1",
        "uid-a",
        "uid-b",
        false,
        false,
        true,
        None,
    )
    .unwrap();
    assert!(
        matches!(
            outcome,
            DbCopyOutcome::Inserted {
                aux_note: Some(_),
                space_note: None
            }
        ),
        "{outcome:?}"
    );
    // 会话行照写，只是没有账本。
    assert!(ledger_usage(&target.paths.workbuddy_db(), "new-legacy").is_none());
}

/// 账本写入报错 → 整次失败且事务回滚（目标库不留 sessions 行）。
#[test]
fn aux_write_failure_rolls_back_the_session_row() {
    let env = ready_env("aux-fail");
    let db = env.paths.workbuddy_db();
    create_ledger_tables(&db);
    write_ledger(&db, "sess-1", 5, 6, r#"{"r1":2.5}"#, &[]);
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER aux_boom BEFORE INSERT ON session_usage \
         BEGIN SELECT RAISE(ABORT, 'ledger down'); END;",
    )
    .unwrap();
    drop(conn);

    let error = insert_session_copy(
        &env.paths(),
        &env.paths(),
        "new-boom",
        "sess-1",
        "uid-a",
        "uid-b",
        false,
        // 只有「连账本一起复制」时才会真的写来源数值，本用例要复现写入报错。
        true,
        true,
        None,
    )
    .unwrap_err();
    assert!(
        is_aux_write_failure(&error),
        "错误必须带可忽略前缀：{error}"
    );
    let conn = Connection::open(&db).unwrap();
    let rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE id = 'new-boom'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0, "账本失败必须回滚会话行");
}

/// 忽略账本：两张表零写入，操作记录落 `auxSkipped`。
#[test]
fn ignore_aux_writes_no_ledger_and_marks_operation() {
    let env = ready_env("aux-ignore");
    let db = env.paths.workbuddy_db();
    create_ledger_tables(&db);
    write_ledger(
        &db,
        "sess-1",
        7,
        8,
        r#"{"r1":3.5}"#,
        &[("req-1", None, None)],
    );

    let report = copy_sessions_for_switch_at(
        &env.paths(),
        WbVariant::Cn,
        &env.target("uid-b"),
        &["sess-1".to_string()],
        true,
        |_| false,
    )
    .unwrap();
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(report["auxSkipped"], true);
    let new_id = env.first_copy_id(&report);
    assert!(ledger_usage(&db, &new_id).is_none(), "忽略模式不得写用量");
    assert!(ledger_interruptions(&db, &new_id).is_empty());

    // 操作记录必须持久化忽略标记，恢复重放才不会在同一处再失败。
    let scan = session_link::scan_operations(&env.paths());
    let operation = scan
        .operations
        .iter()
        .find(|operation| operation.target.session_id == new_id)
        .expect("必须有本次复制的操作记录");
    assert!(operation.aux_skipped);
}

/// 重放幂等：同一目标 id 再写一次不产生重复中断行，`credit_json` 不被重复累加。
#[test]
fn aux_replay_is_idempotent() {
    let env = ready_env("aux-replay");
    let db = env.paths.workbuddy_db();
    create_ledger_tables(&db);
    write_ledger(
        &db,
        "sess-1",
        9,
        10,
        r#"{"r1":4.25}"#,
        &[("req-1", None, None), ("req-2", Some("aborted"), None)],
    );

    insert_session_copy(
        &env.paths(),
        &env.paths(),
        "new-replay",
        "sess-1",
        "uid-a",
        "uid-b",
        false,
        // 重放幂等只在「搬账本」时才有意义（默认口径根本不写来源数值）。
        true,
        true,
        None,
    )
    .unwrap();
    // 模拟恢复重放：会话行先被清掉，再按同一目标 id 补写一次。
    let conn = Connection::open(&db).unwrap();
    conn.execute("DELETE FROM sessions WHERE id = 'new-replay'", [])
        .unwrap();
    drop(conn);
    insert_session_copy(
        &env.paths(),
        &env.paths(),
        "new-replay",
        "sess-1",
        "uid-a",
        "uid-b",
        false,
        // 重放幂等只在「搬账本」时才有意义（默认口径根本不写来源数值）。
        true,
        true,
        None,
    )
    .unwrap();

    assert_eq!(
        ledger_interruptions(&db, "new-replay").len(),
        2,
        "重放不得重复"
    );
    let (used, _, credit) = ledger_usage(&db, "new-replay").unwrap();
    assert_eq!(used, 9);
    assert_eq!(credit.as_deref(), Some(r#"{"r1":4.25}"#));
}

// ---------------------------------------------------------------------------
// R5-20：复制时的空间归属（路径优先 → 同名兜底 → 新建，补 space_folders）
// ---------------------------------------------------------------------------

/// 带 `space_id` 的会话表 + `spaces` / `space_folders`（真实 5.11 客户端库结构）。
fn create_space_schema(db: &Path) {
    let conn = Connection::open(db).unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            title TEXT,
            custom_title TEXT,
            cwd TEXT,
            created_at INTEGER,
            updated_at INTEGER,
            deleted_at INTEGER,
            is_playground INTEGER,
            space_id TEXT
        );
        CREATE TABLE spaces (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            name TEXT,
            device_id TEXT,
            source_kind TEXT,
            create_folder_path TEXT,
            primary_folder_path TEXT,
            status TEXT,
            transport TEXT,
            pinned INTEGER,
            deleted_at INTEGER,
            created_at INTEGER,
            updated_at INTEGER
        );
        CREATE TABLE space_folders (
            space_id TEXT NOT NULL,
            path TEXT NOT NULL,
            sort_order INTEGER,
            PRIMARY KEY (space_id, path)
        );",
    )
    .unwrap();
}

fn insert_space(db: &Path, id: &str, uid: &str, name: &str, folder: &str, device: &str) {
    let conn = Connection::open(db).unwrap();
    conn.execute(
        "INSERT INTO spaces (id, user_id, name, device_id, source_kind, create_folder_path, \
         primary_folder_path, status, transport, pinned, deleted_at, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, 'seed', ?5, NULL, 'active', 'local', 0, -1, 1, 1)",
        rusqlite::params![id, uid, name, device, folder],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO space_folders (space_id, path, sort_order) VALUES (?1, ?2, 0)",
        rusqlite::params![id, folder],
    )
    .unwrap();
}

fn insert_session_with_space(db: &Path, id: &str, uid: &str, cwd: &str, space_id: &str) {
    let conn = Connection::open(db).unwrap();
    conn.execute(
        "INSERT INTO sessions (id, user_id, title, custom_title, cwd, created_at, updated_at, \
         deleted_at, is_playground, space_id) VALUES (?1, ?2, ?3, NULL, ?4, 1000, 2000, NULL, 0, ?5)",
        rusqlite::params![id, uid, format!("标题-{id}"), cwd, space_id],
    )
    .unwrap();
}

fn space_row_ids(db: &Path, uid: &str) -> Vec<String> {
    let conn = Connection::open(db).unwrap();
    let mut stmt = conn
        .prepare("SELECT id FROM spaces WHERE user_id = ?1 ORDER BY id")
        .unwrap();
    let rows: Vec<String> = stmt
        .query_map([uid], |row| row.get(0))
        .unwrap()
        .flatten()
        .collect();
    rows
}

fn session_space(db: &Path, sid: &str) -> Option<String> {
    let conn = Connection::open(db).unwrap();
    conn.query_row(
        "SELECT space_id FROM sessions WHERE id = ?1",
        [sid],
        |row| row.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
}

fn folder_rows(db: &Path, space_id: &str) -> Vec<(String, i64)> {
    let conn = Connection::open(db).unwrap();
    let mut stmt = conn
        .prepare("SELECT path, COALESCE(sort_order, -1) FROM space_folders WHERE space_id = ?1 ORDER BY sort_order")
        .unwrap();
    let rows: Vec<(String, i64)> = stmt
        .query_map([space_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .flatten()
        .collect();
    rows
}

/// 源账号 uid-a 有一条绑定 src-space 的会话，cwd 为 `cwd`，源空间目录 `folder`。
fn space_env(name: &str, cwd: &str, folder: &str) -> Env {
    let env = Env::new(name);
    let db = env.paths.workbuddy_db();
    create_space_schema(&db);
    env.create_edge_db(WbVariant::Cn);
    env.set_login("uid-a");
    env.add_body("sess-1", &body_text("sess-1"));
    insert_space(&db, "src-space", "uid-a", "ResumeForge", folder, "dev-src");
    insert_session_with_space(&db, "sess-1", "uid-a", cwd, "src-space");
    env
}

/// 路径键：分隔符、大小写、尾分隔符都不参与比较；不做 canonicalize。
#[test]
fn space_path_key_normalizes_separators_case_and_trailing_slash() {
    assert_eq!(space_path_key("D:\\Proj\\A\\"), space_path_key("d:/proj/a"));
    assert_eq!(space_path_key("D:/Proj/A"), "d:/proj/a");
    assert_eq!(space_path_key(""), "");
    // 不做符号链接解析：不同路径就是不同键（保守失败，宁可多建一个组）。
    assert_ne!(space_path_key("D:/a"), space_path_key("D:/b"));
}

/// 目标账号有**名字不同**但绑定同一目录的空间 → 复用（证明是按路径匹配）。
#[test]
fn copy_reuses_target_space_by_path_even_with_different_name() {
    let env = space_env("space-path", "D:\\Proj\\A", "D:\\Proj");
    let db = env.paths.workbuddy_db();
    // 目标账号的空间名字完全不同，只有目录相同（还用 / 与大小写差异验证归一）。
    insert_space(&db, "tgt-a", "uid-b", "另一个名字", "d:/proj/A/", "dev-tgt");

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    let new_id = env.first_copy_id(&report);
    assert_eq!(session_space(&db, &new_id).as_deref(), Some("tgt-a"));
    assert_eq!(space_row_ids(&db, "uid-b"), vec!["tgt-a"], "不得新建空间");
    assert!(report["copied"][0].get("spaceNote").is_none());
}

/// 目标账号无该目录、但有同名空间 → 同名兜底复用。
#[test]
fn copy_falls_back_to_same_name_space() {
    let env = space_env("space-name", "D:\\Proj\\A", "D:\\Proj");
    let db = env.paths.workbuddy_db();
    insert_space(
        &db,
        "tgt-name",
        "uid-b",
        "ResumeForge",
        "E:\\别处",
        "dev-tgt",
    );

    let report = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&report);
    assert_eq!(session_space(&db, &new_id).as_deref(), Some("tgt-name"));
    assert_eq!(space_row_ids(&db, "uid-b"), vec!["tgt-name"]);
}

/// 同一目录命中两个空间 → 判 ambiguous：不猜，按目录新建并写诊断。
#[test]
fn copy_treats_shared_cwd_as_ambiguous_and_creates_new_space() {
    let env = space_env("space-ambiguous", "D:\\Proj\\A", "D:\\Proj");
    let db = env.paths.workbuddy_db();
    insert_space(
        &db,
        "tgt-1",
        "uid-b",
        "ResumeForge-主",
        "D:\\Proj",
        "dev-tgt",
    );
    insert_space(&db, "tgt-2", "uid-b", "ResumeForge", "D:\\Proj", "dev-tgt");

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    let new_id = env.first_copy_id(&report);
    let mapped = session_space(&db, &new_id).expect("必须落到某个空间");
    assert_ne!(mapped, "tgt-1", "ambiguous 时不得静默绑到其中一个");
    assert_ne!(mapped, "tgt-2", "ambiguous 时不得静默绑到其中一个");
    assert_eq!(space_row_ids(&db, "uid-b").len(), 3, "应新建第三个空间");
    assert_eq!(
        report["copied"][0]["spaceNote"], SPACE_AMBIGUOUS_NOTE,
        "必须写诊断"
    );
}

/// 目标账号一个空间都没有 → 用设备级 device-id 建组，并写 `space_folders`。
#[test]
fn copy_creates_space_with_device_id_file_and_binds_folder() {
    let env = space_env("space-device", "D:\\Proj\\A", "D:\\Proj");
    // 源空间没有 device_id 时才轮到设备文件兜底。
    Connection::open(env.paths.workbuddy_db())
        .unwrap()
        .execute(
            "UPDATE spaces SET device_id = NULL WHERE id = 'src-space'",
            [],
        )
        .unwrap();
    // store_root 的父目录 = 测试临时根；写入设备标识文件模拟真实 `~/.workbuddy/device-id`。
    let home = env.root.clone();
    std::fs::create_dir_all(home.join(".workbuddy")).unwrap();
    std::fs::write(home.join(".workbuddy").join("device-id"), "dev-from-file\n").unwrap();

    let report = copy(&env, "uid-b", &["sess-1"]);
    let new_id = env.first_copy_id(&report);
    let db = env.paths.workbuddy_db();
    let created = session_space(&db, &new_id).expect("target must get a space");
    let conn = Connection::open(&db).unwrap();
    let (uid, device): (String, String) = conn
        .query_row(
            "SELECT user_id, device_id FROM spaces WHERE id = ?1",
            [&created],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(uid, "uid-b");
    assert_eq!(device, "dev-from-file");
    // 会话 cwd 与源 create_folder_path 不同 → 两条目录行（0 与 1）。
    let folders = folder_rows(&db, &created);
    assert_eq!(folders.len(), 2, "{folders:?}");
    assert_eq!(folders[0], ("D:\\Proj\\A".to_string(), 0));
    assert_eq!(folders[1], ("D:\\Proj".to_string(), 1));
}

/// 同一批三条同 cwd 会话 → 只建 1 个空间、`space_folders` 不重复。
#[test]
fn copy_same_cwd_many_sessions_creates_one_space() {
    let env = space_env("space-idempotent", "D:\\Proj\\A", "D:\\Proj\\A");
    let db = env.paths.workbuddy_db();
    for id in ["sess-1", "sess-2", "sess-3"] {
        if id != "sess-1" {
            env.add_body(id, &body_text(id));
            insert_session_with_space(&db, id, "uid-a", "D:\\Proj\\A", "src-space");
        }
    }

    let report = copy(&env, "uid-b", &["sess-1", "sess-2", "sess-3"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 3, "{report}");
    let ids = space_row_ids(&db, "uid-b");
    assert_eq!(ids.len(), 1, "三条会话只能建一个空间：{ids:?}");
    assert_eq!(folder_rows(&db, &ids[0]).len(), 1, "同 cwd 只绑一条目录");
}

/// 目标库没有 `space_folders` 表（旧客户端）→ 建空间但不报错。
#[test]
fn copy_skips_space_folders_when_table_missing() {
    let env = space_env("space-old-client", "D:\\Proj\\A", "D:\\Proj");
    let db = env.paths.workbuddy_db();
    let conn = Connection::open(&db).unwrap();
    conn.execute("DROP TABLE space_folders", []).unwrap();
    drop(conn);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    let new_id = env.first_copy_id(&report);
    assert!(session_space(&db, &new_id).is_some(), "空间本身仍要建");
}

/// 空间写入失败 → 整条复制回滚：不残留空分组。
#[test]
fn space_insert_failure_rolls_back_without_orphan_space() {
    let env = space_env("space-rollback", "D:\\Proj\\A", "D:\\Proj");
    let db = env.paths.workbuddy_db();
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER space_boom BEFORE INSERT ON spaces \
         BEGIN SELECT RAISE(ABORT, 'spaces down'); END;",
    )
    .unwrap();
    drop(conn);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(
        report["copied"].as_array().map_or(0, Vec::len),
        0,
        "{report}"
    );
    let errors = report["errors"].as_array().expect("必须报错");
    assert!(
        errors[0]["error"].as_str().unwrap().contains("项目空间"),
        "{errors:?}"
    );
    assert!(space_row_ids(&db, "uid-b").is_empty(), "不得残留空分组");
    assert_eq!(env.rows_for("uid-b").len(), 0, "会话行也要回滚");
}

// ---------------------------------------------------------------------------
// R5-13：专家绑定判定（只提示，不改变复制结果）
// ---------------------------------------------------------------------------

/// 会话带一个目标账号没有的专家 → 报告带 `expert.state = missing`，但**照常复制**。
#[test]
fn copy_reports_missing_expert_but_still_copies() {
    let env = ready_env("expert-missing");
    let db = env.paths.workbuddy_db();
    let conn = Connection::open(&db).unwrap();
    conn.execute("ALTER TABLE sessions ADD COLUMN expert_id TEXT", [])
        .unwrap();
    conn.execute(
        "ALTER TABLE sessions ADD COLUMN expert_runtime_identity TEXT",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE sessions SET expert_id = 'ex_NOT_EXIST', expert_runtime_identity = ?1 WHERE id = 'sess-1'",
        [r#"expert-v2:{"v":2,"kind":"expert","expertId":"ex_NOT_EXIST","pluginRegisteredName":"no-such-plugin","agentName":"不存在的专家","source":"custom"}"#],
    )
    .unwrap();
    drop(conn);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(report["copied"][0]["expert"]["state"], "missing");
    assert_eq!(
        report["copied"][0]["expert"]["plugin"], "no-such-plugin",
        "必须按 pluginName 报，不按 expert_id"
    );
    // 决策：missing 只提示，不跳过。
    let new_id = env.first_copy_id(&report);
    assert_eq!(env.rows_for("uid-b"), vec![new_id]);
}

/// 插件已装在目标数据根 → 判 `available`（零误报），且列按原样搬运不被改写。
#[test]
fn copy_keeps_expert_columns_untouched_when_available() {
    let env = ready_env("expert-available");
    let db = env.paths.workbuddy_db();
    // 目标档数据根里放一个插件缓存目录（测试用临时 data_root，不碰真实 ~/.workbuddy）。
    std::fs::create_dir_all(
        env.paths
            .data_root
            .join("plugins")
            .join("cache")
            .join("experts")
            .join("software-company"),
    )
    .unwrap();
    let identity = r#"expert-v2:{"v":2,"kind":"expert","expertId":"SoftwareCompany","marketExpertId":"ex_jPdPfIC04OKD","pluginRegisteredName":"software-company","agentName":"SoftwareCompany","source":"builtin"}"#;
    let conn = Connection::open(&db).unwrap();
    conn.execute("ALTER TABLE sessions ADD COLUMN expert_id TEXT", [])
        .unwrap();
    conn.execute(
        "ALTER TABLE sessions ADD COLUMN expert_runtime_identity TEXT",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE sessions SET expert_id = 'SoftwareCompany', expert_runtime_identity = ?1 WHERE id = 'sess-1'",
        [identity],
    )
    .unwrap();
    drop(conn);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(
        report["copied"][0]["expert"]["state"], "available",
        "{report}"
    );
    // 原样搬运：副本的两列与源逐字相同。
    let conn = Connection::open(&db).unwrap();
    let (expert_id, copied_identity): (String, String) = conn
        .query_row(
            "SELECT expert_id, expert_runtime_identity FROM sessions WHERE id = ?1",
            [env.first_copy_id(&report)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(expert_id, "SoftwareCompany");
    assert_eq!(copied_identity, identity);
}

// ---------------------------------------------------------------------------
// 复制保留源创建时间（批量复制后目标账号的时间排序与统计不失真）
// ---------------------------------------------------------------------------

/// 目标会话行的 (created_at, updated_at)。
fn copied_times(env: &Env, cid: &str) -> (Option<i64>, Option<i64>) {
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.query_row(
        "SELECT created_at, updated_at FROM sessions WHERE id = ?1",
        [cid],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap()
}

/// 副本沿用源的 `created_at`（源行是 1000/2000），`updated_at` 必须是本次复制时刻。
///
/// 一处放宽会连带两件事：创建时间全挤在切号那一分钟（排序/统计失真），或更新时间
/// 沿用旧值让副本在客户端「最近更新」里沉底。两个断言合在一个用例里，防止有人
/// 顺手把两列一起改成「保留源值」。
#[test]
fn copy_preserves_source_created_at() {
    let env = ready_env("copy-created-at");
    let before = crate::modules::config::now_ms();
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);

    let (created_at, updated_at) = copied_times(&env, &target_id);
    assert_eq!(created_at, Some(1000), "必须保留源创建时间");
    let updated_at = updated_at.expect("更新时间必须有值");
    assert!(
        updated_at >= before,
        "更新时间必须落在本次复制之后：{updated_at} < {before}"
    );
    assert_ne!(updated_at, 2000, "更新时间不得沿用源的旧值");
}

/// 源 `created_at` 为 NULL（旧库可能没写过）时回退成本次时间戳，不写 NULL。
#[test]
fn copy_falls_back_to_copy_time_when_source_created_at_is_null() {
    let env = ready_env("copy-created-at-null");
    let conn = Connection::open(env.paths.workbuddy_db()).unwrap();
    conn.execute("UPDATE sessions SET created_at = NULL WHERE id = 'sess-1'", [])
        .unwrap();
    drop(conn);

    let before = crate::modules::config::now_ms();
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);

    let (created_at, _) = copied_times(&env, &target_id);
    let created_at = created_at.expect("没有可保留的值时必须回退成时间戳，不能写 NULL");
    assert!(
        created_at >= before,
        "回退值必须落在本次复制之后：{created_at} < {before}"
    );
}

// ---------------------------------------------------------------------------
// 复制时的「最后活跃时间」（`sessions.last_activity_at`）
// ---------------------------------------------------------------------------
//
// 客户端侧栏每条会话的「活跃时间」读的就是这一列。正文一直在更新，这一列却从来
// 没人维护，于是副本的活跃时间冻结在源行的旧值上，正文却是最新的。

/// 正文里两条记录的毫秒时间戳（2023-11，一定早于「现在」）。
const ACTIVITY_BODY_EARLY: i64 = 1_700_000_000_000;
const ACTIVITY_BODY_NEWEST: i64 = 1_700_000_009_000;
/// 源行停更的旧值（2020-09）。
const ACTIVITY_SOURCE_STALE: i64 = 1_600_000_000_000;

/// 带毫秒时间戳的正文：先一条**没有** `timestamp` 的用户消息（真实正文里大量如此），
/// 再按 `stamps` 逐行写记录。
fn body_with_timestamps(cid: &str, stamps: &[i64]) -> String {
    let mut text = format!(
        "{}\n",
        json!({"type": "user", "sessionId": cid, "text": "你好"})
    );
    for stamp in stamps {
        text.push_str(
            &json!({
                "type": "message",
                "role": "assistant",
                "sessionId": cid,
                "timestamp": stamp
            })
            .to_string(),
        );
        text.push('\n');
    }
    text
}

/// 正文最新时间晚于源行 → 副本取正文时间（缺陷主现场：源列停在很早以前）。
#[test]
fn copy_uses_newest_body_timestamp_for_activity() {
    let env = ready_env("activity-body-newer");
    env.add_activity_column();
    env.add_body(
        "sess-1",
        &body_with_timestamps("sess-1", &[ACTIVITY_BODY_EARLY, ACTIVITY_BODY_NEWEST]),
    );
    env.set_activity("sess-1", ACTIVITY_SOURCE_STALE);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    let target_id = env.first_copy_id(&report);
    assert_eq!(
        env.activity_of(&target_id),
        Some(ACTIVITY_BODY_NEWEST),
        "活跃时间必须以正文最后一条记录为准，不能沿用源行的旧值"
    );
    assert_eq!(
        env.activity_of("sess-1"),
        Some(ACTIVITY_SOURCE_STALE),
        "源行一字不动：维护活跃时间只作用于副本"
    );
}

/// 正文最新时间早于源行 → 取源值，不回退（源行可能比正文更新，改小就是丢信息）。
#[test]
fn copy_keeps_source_activity_when_body_is_older() {
    let env = ready_env("activity-body-older");
    env.add_activity_column();
    env.add_body(
        "sess-1",
        &body_with_timestamps("sess-1", &[ACTIVITY_BODY_EARLY, ACTIVITY_BODY_NEWEST]),
    );
    env.set_activity("sess-1", ACTIVITY_BODY_NEWEST + 500);

    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    assert_eq!(
        env.activity_of(&target_id),
        Some(ACTIVITY_BODY_NEWEST + 500),
        "正文更旧时必须保留源值（只前进不回退）"
    );
}

/// 正文时间戳超过当前时刻（源机时钟超前）→ 被本次复制时间夹住，不能顶到未来。
#[test]
fn copy_clamps_future_body_timestamp_to_copy_time() {
    let env = ready_env("activity-future");
    env.add_activity_column();
    let future = now_ms() + 3_600_000;
    env.add_body("sess-1", &body_with_timestamps("sess-1", &[future]));
    let before = now_ms();

    let report = copy(&env, "uid-b", &["sess-1"]);
    let after = now_ms();
    let activity = env
        .activity_of(&env.first_copy_id(&report))
        .expect("正文有时间戳时必须写值");
    assert!(
        activity >= before && activity <= after,
        "超过当前时刻的时间戳必须被复制时刻夹住：{activity} 不在 [{before}, {after}]"
    );
    assert!(activity < future, "副本的活跃时间不得停在未来");
}

/// 正文一个时间戳都没有（旧客户端只写正文不写时间）→ 沿用源值，不报错。
#[test]
fn copy_falls_back_to_source_activity_without_body_timestamps() {
    let env = ready_env("activity-no-timestamps");
    env.add_activity_column();
    env.set_activity("sess-1", ACTIVITY_SOURCE_STALE);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(
        env.activity_of(&env.first_copy_id(&report)),
        Some(ACTIVITY_SOURCE_STALE),
        "正文取不到时间时保持原有行为（沿用源值）"
    );
}

/// 目标库是老库（没有 `last_activity_at` 列）→ 不报错，其它列照常写入。
#[test]
fn copy_into_legacy_db_without_activity_column_still_works() {
    let source = ready_env("activity-legacy-src");
    let target = ready_env("activity-legacy-dst");
    source.add_activity_column();
    source.add_body(
        "sess-1",
        &body_with_timestamps("sess-1", &[ACTIVITY_BODY_NEWEST]),
    );
    source.set_activity("sess-1", ACTIVITY_SOURCE_STALE);
    // 目标库确实没有这一列（默认建表就没有，正是老库形态）。
    let target_conn = Connection::open(target.paths.workbuddy_db()).unwrap();
    assert!(
        !column_exists(&target_conn, "sessions", "last_activity_at"),
        "本用例的前提是目标库没有该列"
    );
    drop(target_conn);

    let outcome = insert_session_copy(
        &source.paths(),
        &target.paths(),
        "new-legacy-activity",
        "sess-1",
        "uid-a",
        "uid-b",
        false,
        false,
        true,
        None,
    )
    .unwrap();
    assert!(
        matches!(outcome, DbCopyOutcome::Inserted { .. }),
        "{outcome:?}"
    );
    let conn = Connection::open(target.paths.workbuddy_db()).unwrap();
    let (title, updated_at): (String, i64) = conn
        .query_row(
            "SELECT title, updated_at FROM sessions WHERE id = 'new-legacy-activity'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(title, "标题一", "缺列不得影响其它列的搬运");
    assert!(updated_at > 2000, "updated_at 仍写本次复制时刻");
}

/// 源行的活跃时间是负数（列被人手改坏 / 早期写入缺陷）→ 当成「没有候选」，用正文时间。
///
/// 负数经过 `min(copy_now)` 之后仍是负数，副本会显示成 1969 年；这里要求它被丢弃，
/// 而不是原样搬过去。
#[test]
fn copy_ignores_negative_source_activity() {
    let env = ready_env("activity-negative");
    env.add_activity_column();
    env.add_body(
        "sess-1",
        &body_with_timestamps("sess-1", &[ACTIVITY_BODY_NEWEST]),
    );
    env.set_activity("sess-1", -1);

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(
        env.activity_of(&env.first_copy_id(&report)),
        Some(ACTIVITY_BODY_NEWEST),
        "负值必须当成缺失，退回正文时间"
    );
}

/// 正文只有 `timestamp = 0`（1970）→ 与同步侧同一口径：当成「没有可用时间」，
/// 沿用源行的值，不写 0。
///
/// 这条钉的是两条写路径的口径一致性：解析层只认正数，所以复制与同步都不会把
/// 「1970」写进侧栏的活跃时间；反面断言（不得写 0）比「等于源值」更重要。
#[test]
fn copy_treats_zero_timestamp_as_missing() {
    let env = ready_env("activity-zero");
    env.add_activity_column();
    env.add_body("sess-1", &body_with_timestamps("sess-1", &[0]));
    env.set_activity("sess-1", ACTIVITY_SOURCE_STALE);

    let report = copy(&env, "uid-b", &["sess-1"]);
    let copied = env.first_copy_id(&report);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(
        env.activity_of(&copied),
        Some(ACTIVITY_SOURCE_STALE),
        "0 不是活动时间：必须沿用源值，不能写成 0（侧栏会显示 1970 年）"
    );
    assert_ne!(env.activity_of(&copied), Some(0), "不得写 0");
}

/// 复制在「正文未写、行未写」处中断 → 恢复补写正文后，活跃时间按**本次补写的那份
/// 正文**取值，而不是沿用源行的旧值；恢复只跑一次，不产生第二份副本。
///
/// 这是 `recover_copy_operation` 里唯一带正文时间的分支（`BodyCheck::Absent`）：
/// 另两个分支拿不到待写入正文，只能沿用源值（见该函数注释）。缺这条用例时，
/// 「恢复了正文、活跃时间却仍冻结」不会有任何断言报警。
#[test]
fn copy_recovery_backfills_activity_from_restored_body() {
    let env = ready_env("activity-copy-recovery");
    env.add_activity_column();
    env.add_body(
        "sess-1",
        &body_with_timestamps("sess-1", &[ACTIVITY_BODY_EARLY, ACTIVITY_BODY_NEWEST]),
    );
    env.set_activity("sess-1", ACTIVITY_SOURCE_STALE);
    let paths = env.paths();
    // 空关联存储：真实现场是「首次复制刚建好操作记录就被中断」，此时同步记录主文件
    // 还不存在，恢复会按「有未完成痕迹但主文件缺失」直接拒绝，走不到补写分支。
    session_link::with_link_store_write(&paths, |_| Ok(())).unwrap();

    // 手工留下一条「刚建好、正文与数据库行都还没写」的操作记录：比先跑一次完整复制
    // 再把阶段回退更接近真实中断现场（此时关联存储里也还没有任何组）。
    session_link::save_operation(
        &paths,
        &Operation {
            version: session_link::OPERATION_VERSION,
            operation_id: "op-copy-activity".to_string(),
            kind: "copy".to_string(),
            variant: WbVariant::Cn,
            source_variant: None,
            group_id: "g-copy-activity".to_string(),
            source: OperationMember {
                account_id: None,
                uid: "uid-a".to_string(),
                session_id: "sess-1".to_string(),
            },
            target: OperationMember {
                account_id: None,
                uid: "uid-b".to_string(),
                session_id: "new-copy-activity".to_string(),
            },
            // 待写入正文的摘要由恢复自己按当前源内容重算，这里不用预先写对。
            expected_content_digest: String::new(),
            expected_record_count: 0,
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

    let recovery = recover_pending_session_operations_at(&paths, WbVariant::Cn);
    assert_eq!(
        recovery.recovered.len(),
        1,
        "{:?}",
        recovery.needs_recovery
    );
    assert!(recovery.is_clean(), "{:?}", recovery.needs_recovery);
    assert_eq!(
        env.activity_of("new-copy-activity"),
        Some(ACTIVITY_BODY_NEWEST),
        "恢复补写的正文里有更新的时间戳，活跃时间必须跟着正文走（不得停在源行旧值）"
    );
    assert_eq!(
        env.activity_of("sess-1"),
        Some(ACTIVITY_SOURCE_STALE),
        "源行一字不动"
    );

    let again = recover_pending_session_operations_at(&paths, WbVariant::Cn);
    assert!(again.is_clean(), "{:?}", again.needs_recovery);
    assert!(again.recovered.is_empty(), "已完成的操不得被再重放一次");
    assert_eq!(env.body_files().len(), 2, "恢复不得产生第二份副本");
    assert_eq!(env.rows_for("uid-b"), vec!["new-copy-activity".to_string()]);
    assert_eq!(
        env.activity_of("new-copy-activity"),
        Some(ACTIVITY_BODY_NEWEST),
        "重放幂等：值不变"
    );
}

// ---------------------------------------------------------------------------
// 项目归属失败不再静默（复制报告带 `spaceNote`）
// ---------------------------------------------------------------------------

/// 源会话指着源账号的 space，但源 space 在两边都查不到（老库没有 spaces 表 / 库损坏）
/// → 副本如实写 NULL，并在报告里说明「未归入任何项目」。
#[test]
fn copy_reports_space_note_when_source_space_missing() {
    let env = space_env("space-gone", "D:\\Proj\\A", "D:\\Proj");
    let db = env.paths.workbuddy_db();
    // 源 space 行消失（会话仍指着它的 id）：这正是「目标里看不到副本在哪个项目」的成因。
    Connection::open(&db)
        .unwrap()
        .execute("DELETE FROM spaces WHERE id = 'src-space'", [])
        .unwrap();

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    let new_id = env.first_copy_id(&report);
    assert_eq!(session_space(&db, &new_id), None, "取不到归属就如实写 NULL");
    assert_eq!(
        report["copied"][0]["spaceNote"], SPACE_UNASSIGNED_NOTE,
        "必须让用户知道副本没进任何项目"
    );
}

/// 源 space 的 `device_id` 取不到、目标账号又没有可兜底的设备标识 → 建不出分组，
/// 同样要带上诊断（另一种「副本不在项目里」的成因）。
#[test]
fn copy_reports_space_note_when_target_space_cannot_be_created() {
    let env = space_env("space-no-device", "D:\\Proj\\A", "D:\\Proj");
    let db = env.paths.workbuddy_db();
    // 空字符串按缺失处理（`row_text` 会 trim 后判空）；目标账号一个 space 都没有，
    // 临时目录下也没有 `.workbuddy/device-id` → 三级兜底全部失效。
    Connection::open(&db)
        .unwrap()
        .execute("UPDATE spaces SET device_id = '' WHERE id = 'src-space'", [])
        .unwrap();

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    let new_id = env.first_copy_id(&report);
    assert_eq!(session_space(&db, &new_id), None);
    assert!(
        space_row_ids(&db, "uid-b").is_empty(),
        "设备标识取不到时不得造可疑空项目"
    );
    assert_eq!(
        report["copied"][0]["spaceNote"], SPACE_UNASSIGNED_NOTE,
        "建不出分组也要说明原因"
    );
}

/// 源会话本来就没有项目（`space_id` 为 NULL）→ 正常情况，**不**发诊断。
#[test]
fn copy_keeps_quiet_when_source_has_no_project() {
    let env = space_env("space-none", "D:\\Proj\\A", "D:\\Proj");
    let db = env.paths.workbuddy_db();
    Connection::open(&db)
        .unwrap()
        .execute("UPDATE sessions SET space_id = NULL WHERE id = 'sess-1'", [])
        .unwrap();

    let report = copy(&env, "uid-b", &["sess-1"]);
    assert_eq!(report["copied"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(session_space(&db, &env.first_copy_id(&report)), None);
    assert!(
        report["copied"][0].get("spaceNote").is_none(),
        "源本来没有项目属于正常情况，不该报警：{report}"
    );
}
