    /// design §6.1 步 6 / §9：「索引已改、基线未提交」的窗口（进程被 kill、没走回滚）
    /// → 下次判定内容一致，且不需要任何修复动作。
    #[test]
    fn index_written_before_baseline_commit_settles_identical() {
        let fixture = Fixture::new("index-ahead-of-baseline");
        seed_source(&fixture);
        let target_conv = copy_and_register(&fixture);
        append_round(&fixture, MSG_3, MSG_4, REQ_2, "新增内容");
        let target_dir = fixture.dst_conv_dir(&target_conv);
        let store_backup = fixture.base.join("store-before-sync");
        copy_tree(&fixture.store, &store_backup);

        // 正常同步一次：索引补齐到 4 条、基线同步提交。
        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        let group = &preview["groups"][0];
        assert_eq!(group["verdict"], "fastForward");
        let group_id = group["groupId"].as_str().unwrap().to_string();
        let token = group["previewToken"].as_str().unwrap().to_string();
        let report = sync_selected_at(
            &fixture.root,
            &fixture.paths(),
            SRC_UID,
            &target_acc(),
            &[selection(&group_id, &token, SyncMode::FastForward)],
        )
        .unwrap();
        assert_eq!(report["synced"].as_array().unwrap().len(), 1, "{report}");
        assert_eq!(records(&target_dir).len(), 4, "造数：第一次同步成功");

        // 只把关联存储回滚到同步前：索引已含新记录、基线还是旧的 2 条
        // ——等价于「索引改完、基线提交前进程被 kill」，且没有任何回滚动作。
        copy_tree(&store_backup, &fixture.store);
        let store_state = dir_snapshot(&fixture.store);

        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        let group = &preview["groups"][0];
        assert_eq!(
            group["verdict"], "identical",
            "双方摘要已相等（decide_sync 先比摘要再看基线）：{group}"
        );
        assert_eq!(group["availableModes"], json!([]), "无需动作：不可勾选");
        assert_eq!(group["defaultChecked"], false);
        assert!(group.get("previewToken").is_none(), "不可勾选项不发凭据");
        assert_eq!(
            group["recordCount"],
            json!({"source": 4, "target": 4, "baseline": 2}),
            "基线仍是旧的 2 条，但判定与它无关"
        );
        // 无需任何修复动作：预览是只读的，不会去「补」那条落后的基线。
        assert_eq!(
            dir_snapshot(&fixture.store),
            store_state,
            "预览不得改动关联存储"
        );
        assert_eq!(records(&target_dir).len(), 4);
        assert_eq!(
            digests(&target_dir, &target_conv),
            digests(&fixture.src_conv_dir(), CONV_SRC)
        );
    }

    /// `already_applied` 的另一半：重跑时**不重写**已存在的消息文件
    /// （`fast_forward_converges_after_interrupted_index_write` 只锁了「不换 salt / 不留孤儿」）。
    ///
    /// 判据用 mtime 而不是 `sleep`：先把两条已落盘文件的 mtime 设成 2000 年，重写会让它变成
    /// 当前时间 —— 与文件系统时钟粒度无关，三平台一致，也不引入等待。
    #[test]
    fn already_applied_does_not_rewrite_existing_message_files() {
        let fixture = Fixture::new("already-applied-mtime");
        seed_source(&fixture);
        let target_conv = copy_and_register(&fixture);
        append_round(&fixture, MSG_3, MSG_4, REQ_2, "新增内容");
        let target_dir = fixture.dst_conv_dir(&target_conv);
        let orphans = interrupted_index_write_scene(&fixture, &target_conv);

        let landed: Vec<PathBuf> = orphans
            .iter()
            .map(|name| target_dir.join("messages").join(name))
            .collect();
        let old = UNIX_EPOCH + Duration::from_secs(946_684_800); // 2000-01-01
        for path in &landed {
            set_mtime(path, old);
        }

        // 重跑：目标内容仍等于旧基线 → 判定可快进，两条新增文件命中 already_applied。
        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        let group = &preview["groups"][0];
        assert_eq!(group["verdict"], "fastForward");
        let group_id = group["groupId"].as_str().unwrap().to_string();
        let token = group["previewToken"].as_str().unwrap().to_string();
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

        // 复用而不是重写：mtime 仍是 2000 年（重写会变成当前时间）。
        for path in &landed {
            let mtime = std::fs::metadata(path).unwrap().modified().unwrap();
            assert!(
                mtime < UNIX_EPOCH + Duration::from_secs(1_600_000_000),
                "已存在的消息文件不得被重写：{path:?} 的 mtime 变成了 {mtime:?}"
            );
        }
        assert_eq!(records(&target_dir).len(), 4);
    }

    /// 覆盖模式回滚失败：`restore_dir` 必须上报失败并给出备份路径，调用点的文案不得声称已回滚。
    ///
    /// 造数不依赖平台语义：用一个不存在的备份目录（「备份读不到」是恢复失败的唯一原因，
    /// 与 chmod / 只读句柄这类平台差异无关）。
    #[test]
    fn restore_dir_reports_failure_with_backup_path() {
        let fixture = Fixture::new("restore-failure");
        let target_dir = fixture.base.join("target-conv");
        write(&target_dir.join("index.json"), "{}");
        let missing_backup = fixture.base.join("missing-backup");

        let error = restore_dir(&missing_backup, &target_dir).unwrap_err();
        assert!(
            error.contains("未恢复"),
            "文案必须明说副本目录未恢复：{error}"
        );
        assert!(
            error.contains(&missing_backup.to_string_lossy().to_string()),
            "文案必须给出可手工恢复的备份路径：{error}"
        );
        // 失败的真实后果：目标目录已先被清空，只剩 `copy_dir_recursive` 建出的空目录。
        assert!(
            target_dir.is_dir() && dir_snapshot(&target_dir).is_empty(),
            "恢复失败后副本目录是空壳，所以提示不能写「已回滚」"
        );

        // 调用点合成文案：恢复成功时与原来的文案逐字一致；失败时追加说明、不掩盖。
        let success = "重建后的内容与来源不一致，已整目录回滚";
        assert_eq!(rollback_message(Ok(()), success), success);
        let composed = rollback_message(Err(error.clone()), success);
        assert!(composed.contains(success), "{composed}");
        assert!(composed.contains("未恢复"), "{composed}");
        assert!(
            composed.contains(&missing_backup.to_string_lossy().to_string()),
            "{composed}"
        );
    }

    /// 只为 `rollback_index` 造一个计划：其余字段填空，仅 `target_dir` 指向测试给定路径。
    fn plan_pointing_at(target_dir: &Path) -> SyncItemPlan {
        let member = |uid: &str| LinkMember {
            member_id: format!("{uid}:{CONV_SRC}"),
            account_id: Some(uid.to_string()),
            uid: uid.to_string(),
            session_id: CONV_SRC.to_string(),
            variant: None,
            state: MemberState::Active,
            linked_at: 0,
            last_synced_at: None,
        };
        let snapshot = ContentSnapshot {
            text: String::new(),
            full_digest: String::new(),
            normalized: crate::modules::session_link::NormalizedContent {
                record_count: 0,
                line_digests: Vec::new(),
                comparable_line_digests: Vec::new(),
                total_digest: String::new(),
            },
            latest_at: None,
            list_facts: None,
        };
        SyncItemPlan {
            group_id: "group-under-test".to_string(),
            mode: SyncMode::FastForward,
            verdict: SyncVerdict::FastForward,
            source_member: member(SRC_UID),
            target_member: member(DST_UID),
            source_dir: PathBuf::new(),
            target_dir: target_dir.to_path_buf(),
            workspace_hash: WS.to_string(),
            source_conversation_id: CONV_SRC.to_string(),
            target_conversation_id: CONV_SRC.to_string(),
            target_uid: DST_UID.to_string(),
            backup_kind: VSCODE_STORE.backup_kind,
            // R6-07：夹具按 VS Code 数据仓构造，命名空间与生产侧同源映射。
            ns: Some(crate::modules::usage_origin::IdeClientSource::VSCode.namespace()),
            source_content: snapshot.clone(),
            target_content: snapshot,
        }
    }

    /// 快进回滚失败必须上报：`rollback_index` 返回失败说明（含备份路径），调用点据此追加提示。
    ///
    /// 造数不依赖平台语义：把目标路径做成一个**普通文件**（不是目录），恢复索引时的 `copy`
    /// 在任何平台都无法在文件下建出子路径 → 必败；与 chmod / 只读句柄 / `set_readonly`
    /// 这类跨平台语义不同的手段无关。
    #[test]
    fn rollback_index_reports_failure_with_backup_path() {
        let fixture = Fixture::new("rollback-index-failure");
        let backup_dir = fixture.base.join("index-backup");
        write(&backup_dir.join("index.json"), "{\"messages\":[]}");
        let blocked = fixture.base.join("target-as-file");
        write(&blocked, "不是目录");
        let plan = plan_pointing_at(&blocked);
        let created = vec![blocked.join("messages/never-written.json")];

        let error = rollback_index(&plan, &backup_dir, &created).unwrap_err();
        assert!(error.contains("索引 index.json 未恢复"), "{error}");
        assert!(
            error.contains(&backup_dir.to_string_lossy().to_string()),
            "文案必须给出可手工恢复的备份路径：{error}"
        );

        // 成功路径一：目标与备份都没有索引文件 → 幂等删除（NotFound）不算失败。
        let clean = fixture.base.join("clean-conv");
        std::fs::create_dir_all(&clean).unwrap();
        let empty_backup = fixture.base.join("empty-backup");
        std::fs::create_dir_all(&empty_backup).unwrap();
        let clean_plan = plan_pointing_at(&clean);
        let rolled_back = rollback_index(&clean_plan, &empty_backup, &[]);
        assert!(rolled_back.is_ok(), "文件本就不在不算失败：{rolled_back:?}");

        // 成功路径二：调用点合成文案时，用户可见文案必须逐字不变。
        let original = "写入副本消息失败：磁盘空间不足";
        assert_eq!(
            rollback_message(rollback_index(&clean_plan, &empty_backup, &[]), original),
            original
        );
        let composed = rollback_message(Err(error), original);
        assert!(composed.contains(original), "{composed}");
        assert!(
            composed.contains(&backup_dir.to_string_lossy().to_string()),
            "{composed}"
        );
    }
