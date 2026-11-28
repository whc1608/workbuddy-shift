    #[test]
    fn plugin_cross_region_copy_registration_preview_and_sync() {
        for (source_variant, target_variant) in [
            (WbVariant::Cn, WbVariant::Ai),
            (WbVariant::Ai, WbVariant::Cn),
        ] {
            let fixture = Fixture::new("cross-region");
            seed_source(&fixture);
            let target =
                json!({ "id": "acc-dst", "uid": DST_UID, "variant": target_variant.as_str() });
            write(&account::accounts_file_in(&fixture.store), &json!([
                { "id": "acc-src", "uid": SRC_UID, "variant": source_variant.as_str() }, target.clone()
            ]).to_string());
            let report = copy_sessions_in(
                &fixture.root,
                &fixture.backup,
                SRC_UID,
                DST_UID,
                &[CopyItem {
                    workspace_hash: WS.to_string(),
                    conversation_id: CONV_SRC.to_string(),
                    project_key: None,
                }],
            )
            .unwrap();
            let target_conv = report["copied"][0]["newId"].as_str().unwrap();
            assert!(register_copied_sessions(
                &fixture.root,
                &fixture.paths(),
                target_variant,
                &report
            )
            .is_empty());
            // Re-registration must reuse the group even when its region differs from the caller.
            assert!(register_copied_sessions(
                &fixture.root,
                &fixture.paths(),
                source_variant,
                &report
            )
            .is_empty());
            let StoreState::Ready(store) = session_link::load_store(&fixture.paths()) else {
                panic!("store unavailable")
            };
            assert_eq!(store.groups.len(), 1);
            let group = &store.groups[0];
            assert_eq!(
                group
                    .members
                    .iter()
                    .find(|m| m.uid == SRC_UID)
                    .unwrap()
                    .variant,
                Some(source_variant)
            );
            assert_eq!(
                group
                    .members
                    .iter()
                    .find(|m| m.uid == DST_UID)
                    .unwrap()
                    .variant,
                Some(target_variant)
            );
            append_round(&fixture, MSG_3, MSG_4, REQ_2, "cross region append");
            let preview =
                links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target).unwrap();
            assert_eq!(preview["groups"].as_array().unwrap().len(), 1);
            let item = &preview["groups"][0];
            assert_eq!(item["verdict"], "fastForward");
            let result = sync_selected_at(
                &fixture.root,
                &fixture.paths(),
                SRC_UID,
                &target,
                &[selection(
                    item["groupId"].as_str().unwrap(),
                    item["previewToken"].as_str().unwrap(),
                    SyncMode::FastForward,
                )],
            )
            .unwrap();
            assert_eq!(result["synced"].as_array().unwrap().len(), 1, "{result}");
            assert!(result["errors"].as_array().unwrap().is_empty(), "{result}");
            assert_eq!(
                digests(&fixture.dst_conv_dir(target_conv), target_conv),
                digests(&fixture.src_conv_dir(), CONV_SRC)
            );
        }
    }

    /// AC1：复制成功后出现 1 个组、2 个 active 成员、1 条配对基线；重复复制不产生重复组。
    #[test]
    fn copy_registers_one_group_and_does_not_duplicate() {
        let fixture = Fixture::new("register");
        seed_source(&fixture);
        let first_copy = copy_and_register(&fixture);

        // 命名空间隔离：只写 VS Code 专属文件，WorkBuddy 的关联表不出现（AC6 的存储面）。
        assert!(fixture.store.join("vscode_session_links.json").is_file());
        assert!(!fixture.store.join("session_links.json").exists());
        assert!(fixture
            .store
            .join("vscode-session-links/baselines")
            .is_dir());
        assert!(!fixture.store.join("session-links").exists());

        let store = match session_link::load_store(&fixture.paths()) {
            StoreState::Ready(store) => store,
            other => panic!("关联表不可用：{other:?}"),
        };
        assert_eq!(store.groups.len(), 1);
        let group = &store.groups[0];
        assert_eq!(group.members.len(), 2);
        assert_eq!(group.pair_bases.len(), 1);
        assert!(group
            .members
            .iter()
            .all(|member| member.state == MemberState::Active));
        assert!(group.members.iter().any(|member| member.uid == SRC_UID));
        assert!(group.members.iter().any(|member| member.uid == DST_UID));

        // 再复制同一会话：仍然只有一个组（同账号旧的 active 成员转 superseded）。
        let second_copy = copy_and_register(&fixture);
        assert_ne!(first_copy, second_copy);
        let store = match session_link::load_store(&fixture.paths()) {
            StoreState::Ready(store) => store,
            other => panic!("关联表不可用：{other:?}"),
        };
        assert_eq!(store.groups.len(), 1, "重复复制不得产生第二个组");
        assert_eq!(store.groups[0].members.len(), 3);
        assert_eq!(
            store.groups[0]
                .members
                .iter()
                .filter(|member| member.state == MemberState::Active)
                .count(),
            2
        );
    }

    /// 无关联表 → storeStatus 为 missing（前端据此提示「先复制一次」）。
    #[test]
    fn preview_reports_missing_store() {
        let fixture = Fixture::new("missing-store");
        seed_source(&fixture);
        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        assert_eq!(preview["storeStatus"], "missing");
        assert_eq!(preview["supported"], true);
        assert_eq!(preview["groups"].as_array().unwrap().len(), 0);
    }

    /// AC3：可快进 → 同步后副本记录数与源一致；重复执行不产生重复消息（幂等）。
    #[test]
    fn fast_forward_sync_is_idempotent() {
        let fixture = Fixture::new("fast-forward");
        seed_source(&fixture);
        let target_conv = copy_and_register(&fixture);
        append_round(&fixture, MSG_3, MSG_4, REQ_2, "新增内容");

        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        assert_eq!(preview["storeStatus"], "ready");
        let groups = preview["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["verdict"], "fastForward");
        assert_eq!(groups[0]["defaultChecked"], true);
        assert_eq!(groups[0]["availableModes"], json!(["fastForward"]));
        assert_eq!(groups[0]["title"], "测试会话");
        assert_eq!(groups[0]["recordCount"]["source"], 4);
        assert_eq!(groups[0]["recordCount"]["target"], 2);
        assert_eq!(groups[0]["recordCount"]["baseline"], 2);
        let group_id = groups[0]["groupId"].as_str().unwrap().to_string();
        let token = groups[0]["previewToken"].as_str().unwrap().to_string();

        let report = sync_selected_at(
            &fixture.root,
            &fixture.paths(),
            SRC_UID,
            &target_acc(),
            &[selection(&group_id, &token, SyncMode::FastForward)],
        )
        .unwrap();
        assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
        assert!(report["errors"].as_array().unwrap().is_empty(), "{report}");
        assert_eq!(report["synced"][0]["recordCount"]["target"], 4);
        assert!(!report["synced"][0]["backup"].as_str().unwrap().is_empty());

        let target_dir = fixture.dst_conv_dir(&target_conv);
        let target_records = records(&target_dir);
        assert_eq!(target_records.len(), 4, "副本记录数必须与源一致");
        let unique: BTreeSet<&String> = target_records.iter().collect();
        assert_eq!(unique.len(), 4, "不得出现重复消息 id");
        // 派生 id 必须走真实写入路径（不是只在单测里算一遍）：新增两条的 id
        // 等于 `derive_message_id(目标 uid, 目标会话 id, 序号, 源摘要, salt=0)`。
        let source_digests = digests(&fixture.src_conv_dir(), CONV_SRC);
        assert_eq!(
            target_records[2],
            vscode_session_link::derive_message_id(DST_UID, &target_conv, 2, &source_digests[2], 0)
        );
        assert_eq!(
            target_records[3],
            vscode_session_link::derive_message_id(DST_UID, &target_conv, 3, &source_digests[3], 0)
        );
        // 最强断言：归一化后逐条摘要与源完全一致。
        assert_eq!(digests(&target_dir, &target_conv), source_digests);
        // 附件补入（源侧新增的附件出现在副本里）。
        assert!(target_dir.join("assets/新附件.txt").is_file());

        // 幂等：同一凭据再执行一次 → 跳过（previewStale，零新增），不产生重复消息。
        // 写入层 already_applied 正向路径见 `fast_forward_converges_after_interrupted_index_write`。
        let again = sync_selected_at(
            &fixture.root,
            &fixture.paths(),
            SRC_UID,
            &target_acc(),
            &[selection(&group_id, &token, SyncMode::FastForward)],
        )
        .unwrap();
        assert_eq!(again["synced"].as_array().unwrap().len(), 0, "{again}");
        assert_eq!(again["skipped"].as_array().unwrap().len(), 1, "{again}");
        assert_eq!(again["skipped"][0]["reasonCode"], REASON_PREVIEW_STALE);
        assert_eq!(records(&target_dir).len(), 4);

        // 之后重新预览：判定为内容一致且不可勾选、不发凭据。
        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        let group = &preview["groups"][0];
        assert_eq!(group["verdict"], "identical");
        assert_eq!(group["availableModes"], json!([]));
        assert!(group.get("previewToken").is_none());
    }

    /// AC5：副本本地改动 → ahead；双方都改动 → diverge；覆盖后判定一致，且覆盖前有整目录备份。
    #[test]
    fn overwrite_converges_after_diverge_with_backup() {
        let fixture = Fixture::new("overwrite");
        seed_source(&fixture);
        let target_conv = copy_and_register(&fixture);
        let target_dir = fixture.dst_conv_dir(&target_conv);

        // 副本侧本地改动（改写最后一条助手消息）→ 只有目标变化。
        let assistant = target_dir.join(format!("messages/{}.json", records(&target_dir)[1]));
        let text = std::fs::read_to_string(&assistant)
            .unwrap()
            .replace("在的", "在的（副本本地改动）");
        std::fs::write(&assistant, text).unwrap();
        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        assert_eq!(preview["groups"][0]["verdict"], "ahead");
        assert_eq!(preview["groups"][0]["availableModes"], json!([]));

        // 源侧也新增一轮 → 双方都有更新 → 分叉，需显式覆盖。
        append_round(&fixture, MSG_3, MSG_4, REQ_2, "新增内容");
        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        let group = &preview["groups"][0];
        assert_eq!(group["verdict"], "diverge");
        assert_eq!(group["defaultChecked"], false, "覆盖必须手动勾选");
        assert_eq!(group["availableModes"], json!(["overwrite"]));
        let group_id = group["groupId"].as_str().unwrap().to_string();
        let token = group["previewToken"].as_str().unwrap().to_string();

        let before = dir_snapshot(&target_dir);
        let report = sync_selected_at(
            &fixture.root,
            &fixture.paths(),
            SRC_UID,
            &target_acc(),
            &[selection(&group_id, &token, SyncMode::Overwrite)],
        )
        .unwrap();
        assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");

        // 覆盖前存在整目录备份，且内容与覆盖前逐字节一致。
        let backup = PathBuf::from(report["synced"][0]["backup"].as_str().unwrap());
        assert!(backup.is_dir(), "覆盖前必须有整目录备份");
        assert_eq!(dir_snapshot(&backup), before);

        // 覆盖后内容与源一致（保留目标会话 id）→ 判定变为内容一致。
        assert_eq!(records(&target_dir).len(), 4);
        assert_eq!(
            digests(&target_dir, &target_conv),
            digests(&fixture.src_conv_dir(), CONV_SRC)
        );
        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        assert_eq!(preview["groups"][0]["verdict"], "identical");

        // 目标工作区索引未被改动（会话 id 保留，仍指向原会话）。
        let ws_index = vscode_session::read_json(
            &vscode_session::history_root(&fixture.root, DST_UID)
                .join(WS)
                .join("index.json"),
        )
        .unwrap();
        assert_eq!(ws_index["current"], target_conv);
    }

    /// AC5：覆盖重建失败 → 目标目录内容与操作前逐字节一致。
    #[test]
    fn overwrite_failure_keeps_target_intact() {
        let fixture = Fixture::new("overwrite-failure");
        seed_source(&fixture);
        let target_conv = copy_and_register(&fixture);
        let target_dir = fixture.dst_conv_dir(&target_conv);
        let assistant = target_dir.join(format!("messages/{}.json", records(&target_dir)[1]));
        let text = std::fs::read_to_string(&assistant)
            .unwrap()
            .replace("在的", "在的（副本本地改动）");
        std::fs::write(&assistant, text).unwrap();
        append_round(&fixture, MSG_3, MSG_4, REQ_2, "新增内容");

        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        let group = &preview["groups"][0];
        let group_id = group["groupId"].as_str().unwrap().to_string();
        let token = group["previewToken"].as_str().unwrap().to_string();

        // 让重建的临时目录位置被一个文件占住 → 重建必然失败。
        let tmp = target_dir.with_file_name(format!(".tmp-{target_conv}{OVERWRITE_SUFFIX}"));
        write(&tmp, "blocked");

        let before = dir_snapshot(&target_dir);
        let report = sync_selected_at(
            &fixture.root,
            &fixture.paths(),
            SRC_UID,
            &target_acc(),
            &[selection(&group_id, &token, SyncMode::Overwrite)],
        )
        .unwrap();
        assert_eq!(report["synced"].as_array().unwrap().len(), 0, "{report}");
        let error = report["errors"][0]["error"].as_str().unwrap();
        assert!(error.contains("重建副本会话目录失败"), "{error}");
        assert_eq!(
            dir_snapshot(&target_dir),
            before,
            "失败后目录必须与操作前一致"
        );
    }

    /// design §6.1 步 6：基线提交失败 → 恢复索引并删除本次新增的消息文件。
    #[test]
    fn baseline_commit_failure_rolls_back_index_and_new_files() {
        let fixture = Fixture::new("rollback-baseline");
        seed_source(&fixture);
        let target_conv = copy_and_register(&fixture);
        append_round(&fixture, MSG_3, MSG_4, REQ_2, "新增内容");

        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        let group = &preview["groups"][0];
        let group_id = group["groupId"].as_str().unwrap().to_string();
        let token = group["previewToken"].as_str().unwrap().to_string();

        // 让关联存储锁无法建立（路径被目录占住）→ 提交基线必定失败。
        let lock = fixture.store.join("locks/vscode-session-links.lock");
        let _ = std::fs::remove_file(&lock);
        std::fs::create_dir_all(&lock).unwrap();

        let target_dir = fixture.dst_conv_dir(&target_conv);
        let before_index = std::fs::read(target_dir.join("index.json")).unwrap();
        let before_messages = dir_snapshot(&target_dir.join("messages"));
        let report = sync_selected_at(
            &fixture.root,
            &fixture.paths(),
            SRC_UID,
            &target_acc(),
            &[selection(&group_id, &token, SyncMode::FastForward)],
        )
        .unwrap();
        assert_eq!(report["synced"].as_array().unwrap().len(), 0, "{report}");
        let error = report["errors"][0]["error"].as_str().unwrap();
        assert!(error.contains("同步记录"), "{error}");
        assert!(
            !error.contains("；且"),
            "回滚成功时用户可见文案必须与原始错误逐字一致：{error}"
        );
        assert_eq!(
            std::fs::read(target_dir.join("index.json")).unwrap(),
            before_index,
            "回滚后索引必须回到操作前状态"
        );
        assert_eq!(
            dir_snapshot(&target_dir.join("messages")),
            before_messages,
            "回滚后新增消息文件必须被删除"
        );
        // 附件是纯增量：回滚不涉及（design §6.1 步 6 只恢复索引 + 删本次新增文件）。
    }

    /// 写入中途失败 → 已写入的新增文件被删除、索引回滚（覆盖 design §6.1 步 6 的另一半）。
    #[test]
    fn write_failure_rolls_back_written_files() {
        let fixture = Fixture::new("rollback-write");
        seed_source(&fixture);
        let target_conv = copy_and_register(&fixture);
        append_round(&fixture, MSG_3, MSG_4, REQ_2, "新增内容");

        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        let group = &preview["groups"][0];
        let group_id = group["groupId"].as_str().unwrap().to_string();
        let token = group["previewToken"].as_str().unwrap().to_string();

        // 第二个新增记录的落点被目录占住：第一个记录会先写入成功，第二个失败 → 必须回滚。
        let source_digests = digests(&fixture.src_conv_dir(), CONV_SRC);
        let blocked =
            vscode_session_link::derive_message_id(DST_UID, &target_conv, 3, &source_digests[3], 0);
        let target_dir = fixture.dst_conv_dir(&target_conv);
        std::fs::create_dir_all(target_dir.join(format!("messages/{blocked}.json"))).unwrap();

        let before = dir_snapshot(&target_dir);
        let report = sync_selected_at(
            &fixture.root,
            &fixture.paths(),
            SRC_UID,
            &target_acc(),
            &[selection(&group_id, &token, SyncMode::FastForward)],
        )
        .unwrap();
        assert_eq!(report["synced"].as_array().unwrap().len(), 0, "{report}");
        assert_eq!(report["errors"].as_array().unwrap().len(), 1, "{report}");
        let error = report["errors"][0]["error"].as_str().unwrap();
        assert!(error.contains("写入副本消息失败"), "{error}");
        assert!(
            !error.contains("；且"),
            "回滚成功时用户可见文案必须与原始错误逐字一致：{error}"
        );
        assert_eq!(
            dir_snapshot(&target_dir),
            before,
            "已写入的新增消息文件必须被删除"
        );
    }

    // -----------------------------------------------------------------------
    // 关联预览：代表成员口径与存活过滤
    // -----------------------------------------------------------------------

    /// 预览展示名与存活判定都只看**代表成员**（active 优先），不看排在前面的历史副本。
    ///
    /// 真实顺序是 `[旧 superseded, 新 active]`：按「第一条该 uid 的成员」取会让可同步的组
    /// 显示成 `(无标题)`，存活过滤还会把整组误剔除。
    #[test]
    fn preview_reads_the_representative_member_and_keeps_the_group() {
        let fixture = Fixture::new("preview-representative");
        seed_source(&fixture);
        let target_conv = copy_and_register(&fixture);
        let store_paths = fixture.paths();
        session_link::with_link_store_write(&store_paths, |store| {
            let group = store.groups.first_mut().ok_or_else(|| "缺少组".to_string())?;
            let source_active = group
                .members
                .iter()
                .find(|member| member.uid == SRC_UID)
                .cloned()
                .ok_or_else(|| "缺少来源成员".to_string())?;
            // 历史副本（已删除的旧会话）排在前面：旧口径会取到它。
            let mut superseded = source_active.clone();
            superseded.member_id = "m-superseded".to_string();
            superseded.session_id = "ffffffffffffffffffffffffffffffff".to_string();
            superseded.state = MemberState::Superseded;
            superseded.linked_at = 0;
            group.members.insert(0, superseded);
            Ok(())
        })
        .unwrap();
        assert!(fixture.dst_conv_dir(&target_conv).is_dir());

        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();

        let groups = preview["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 1, "组必须保留：{preview}");
        assert_eq!(groups[0]["title"], "测试会话", "{preview}");
        assert_eq!(groups[0]["source"]["state"], "active", "{preview}");
    }

    /// 会话在客户端被删除（索引条目与 `history/<ws>/<会话 id>/` 目录都没了）后，整组剔除。
    ///
    /// 光删索引条目不算「消失」：目录还在就说明会话确实存在（见 `liveness` 的目录证据），
    /// 那种现场预览必须保留该组。
    #[test]
    fn preview_hides_groups_whose_index_entries_are_gone() {
        let fixture = Fixture::new("preview-dead-group");
        seed_source(&fixture);
        copy_and_register(&fixture);
        // 模拟客户端删除会话：索引条目与正文目录一起消失。
        let ws_dir = vscode_session::history_root(&fixture.root, SRC_UID).join(WS);
        write(&ws_dir.join("index.json"), &json!({ "conversations": [] }).to_string());
        std::fs::remove_dir_all(fixture.src_conv_dir()).unwrap();

        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();

        assert_eq!(preview["storeStatus"], "ready");
        assert_eq!(preview["groups"].as_array().unwrap().len(), 0, "{preview}");
    }

    /// 索引读不到（历史目录缺失）时没有判定依据：预览必须 fail-open，保持改造前行为。
    #[test]
    fn preview_fails_open_when_the_conversation_index_is_unreadable() {
        let fixture = Fixture::new("preview-fail-open");
        seed_source(&fixture);
        copy_and_register(&fixture);
        // 数据根整体不可读：既读不到索引，也不能据此判定会话已删除。
        std::fs::remove_dir_all(vscode_session::uid_data_dir_in(
            vscode_session::VSCODE_STORE,
            &fixture.root,
            SRC_UID,
        ))
        .unwrap();

        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();

        let groups = preview["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 1, "判定不可用时不得剔除任何组：{preview}");
        assert_eq!(groups[0]["verdict"], "unknown", "{preview}");
    }

    /// 判定为 diverge 时用快进模式 → 拒绝文案必须带上判定自己算出的原因。
    ///
    /// 前缀 `该组判定为 X，不允许以 Y 模式同步` 是前端提示与既有用例的匹配锚点，
    /// 逐字保留；原因只能追加在它后面。
    #[test]
    fn sync_reject_message_appends_verdict_reason() {
        let fixture = Fixture::new("reject-reason");
        seed_source(&fixture);
        let target_conv = copy_and_register(&fixture);
        // 副本本地改动 + 源侧新增一轮 → 两边都有改动 = 真分歧。
        let target_dir = fixture.dst_conv_dir(&target_conv);
        let assistant = target_dir.join(format!("messages/{}.json", records(&target_dir)[1]));
        let text = std::fs::read_to_string(&assistant)
            .unwrap()
            .replace("在的", "在的（副本本地改动）");
        std::fs::write(&assistant, text).unwrap();
        append_round(&fixture, MSG_3, MSG_4, REQ_2, "新增内容");

        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        let group = &preview["groups"][0];
        assert_eq!(group["verdict"], "diverge", "{preview}");
        let report = sync_selected_at(
            &fixture.root,
            &fixture.paths(),
            SRC_UID,
            &target_acc(),
            &[selection(
                group["groupId"].as_str().unwrap(),
                group["previewToken"].as_str().unwrap(),
                SyncMode::FastForward,
            )],
        )
        .unwrap();

        assert!(
            report["skipped"].as_array().unwrap().is_empty(),
            "模式越权是拒绝而不是跳过：{report}"
        );
        let errors = report["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 1, "{report}");
        let message = errors[0]["error"].as_str().unwrap();
        assert!(
            message.starts_with("该组判定为 diverge，不允许以 fastForward 模式同步"),
            "前缀必须逐字保留：{message}"
        );
        assert!(
            message.contains("两边都有改动"),
            "判定原因必须追加在前缀之后：{message}"
        );
    }

    /// 预览之后来源正文损坏 → 跳过文案要说清是哪一侧、为什么（不是只说「内容已变化」）。
    #[test]
    fn stale_preview_explains_corrupted_source() {
        let fixture = Fixture::new("stale-explain");
        seed_source(&fixture);
        let target_conv = copy_and_register(&fixture);
        append_round(&fixture, MSG_3, MSG_4, REQ_2, "新增内容");

        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        let group = &preview["groups"][0];
        assert_eq!(group["verdict"], "fastForward", "{preview}");
        let group_id = group["groupId"].as_str().unwrap().to_string();
        let token = group["previewToken"].as_str().unwrap().to_string();

        // 预览之后把来源的一条消息文件删掉：索引仍指着它 → 内容不可验证。
        let src_dir = fixture.src_conv_dir();
        let victim = records(&src_dir)[1].clone();
        std::fs::remove_file(src_dir.join("messages").join(format!("{victim}.json"))).unwrap();

        let report = sync_selected_at(
            &fixture.root,
            &fixture.paths(),
            SRC_UID,
            &target_acc(),
            &[selection(&group_id, &token, SyncMode::FastForward)],
        )
        .unwrap();

        assert!(
            report["errors"].as_array().unwrap().is_empty(),
            "预览过期是跳过而不是拒绝：{report}"
        );
        let skipped = report["skipped"].as_array().unwrap();
        assert_eq!(skipped.len(), 1, "{report}");
        let message = skipped[0]["message"].as_str().unwrap();
        assert!(message.contains("检查结果已失效："), "{message}");
        assert!(message.contains("当前内容状态："), "{message}");
        assert!(message.contains("当前账号的内容无法确认"), "{message}");
        assert!(message.contains(&victim), "原因应指出是哪条消息：{message}");
        assert_eq!(skipped[0]["reasonCode"], "previewStale", "{report}");
        // 目标副本未被打动。
        assert_eq!(
            records(&fixture.dst_conv_dir(&target_conv)).len(),
            2,
            "跳过不得写入"
        );
    }
