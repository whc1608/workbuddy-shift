    /// 目标工作区已有自己的会话：合并是加法，保留目标 `current`。
    #[test]
    fn copy_is_additive_and_preserves_target_state() {
        let fixture = Fixture::new("additive");
        seed_source(&fixture);
        seed_target_index(&fixture);
        write(
            &fixture.dst_ws_dir().join(".index_bak.json"),
            r#"{"conversations":[],"current":"keep-me"}"#,
        );

        copy_once(&fixture, WS);
        let dst_index = read_json(&fixture.dst_ws_dir().join("index.json")).unwrap();
        assert_eq!(
            dst_index.get("current").and_then(Value::as_str),
            Some(CONV_EXISTING)
        );
        assert_eq!(
            dst_index
                .get("conversations")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(2)
        );
        // 目标侧 `.index_bak.json` 保持原样（IDE 侧不写工作区索引备份）。
        assert_eq!(
            std::fs::read_to_string(fixture.dst_ws_dir().join(".index_bak.json")).unwrap(),
            r#"{"conversations":[],"current":"keep-me"}"#
        );
        // 备份根目录留下被修改前的目标索引，供人工回滚。
        let backup_index = read_json(&fixture.backup.join(WS).join("index.json")).unwrap();
        assert_eq!(
            backup_index.get("current").and_then(Value::as_str),
            Some(CONV_EXISTING)
        );
    }

    /// 源会话缺失 / id 非法：逐条记错，不创建目标目录。
    #[test]
    fn failed_items_leave_no_target_directory() {
        let fixture = Fixture::new("missing");
        seed_source(&fixture);
        let report = copy_codebuddy_ide_sessions_in(
            &fixture.root,
            &fixture.backup,
            SRC_UID,
            DST_UID,
            &[
                CopyItem {
                    workspace_hash: WS.to_string(),
                    conversation_id: "99999999999999999999999999999999".to_string(),
                    project_key: None,
                    store: None,
                },
                CopyItem {
                    workspace_hash: WS.to_string(),
                    conversation_id: "bad-id".to_string(),
                    project_key: None,
                    store: None,
                },
            ],
        )
        .expect("returns report");
        assert_eq!(
            report.get("copied").and_then(Value::as_array).map(Vec::len),
            Some(0)
        );
        assert_eq!(
            report.get("errors").and_then(Value::as_array).map(Vec::len),
            Some(2)
        );
        assert!(!fixture.dst_ws_dir().exists(), "全部失败时不应建出目标目录");
    }

    /// 目标工作区被一个同名文件占位（无法建目录）：该条失败、错误可见，其余条目不受影响。
    #[test]
    fn unwritable_target_workspace_records_error() {
        let fixture = Fixture::new("unwritable");
        seed_source(&fixture);
        let target_history = history_root_in(CODEBUDDY_IDE_STORE, &fixture.root, DST_UID);
        std::fs::create_dir_all(&target_history).unwrap();
        // `<ws>` 位置放一个文件：写临时目录必然失败。
        std::fs::write(target_history.join(WS), b"not a directory").unwrap();

        let report = copy_codebuddy_ide_sessions_in(
            &fixture.root,
            &fixture.backup,
            SRC_UID,
            DST_UID,
            &[CopyItem {
                workspace_hash: WS.to_string(),
                conversation_id: CONV_OLD.to_string(),
                project_key: None,
                store: None,
            }],
        )
        .expect("returns report");
        assert_eq!(
            report.get("copied").and_then(Value::as_array).map(Vec::len),
            Some(0)
        );
        let errors = report.get("errors").and_then(Value::as_array).unwrap();
        assert_eq!(errors.len(), 1);
        let message = errors[0].get("error").and_then(Value::as_str).unwrap();
        assert!(message.contains("写入临时会话目录失败"), "{message}");
    }

    /// 时序：目标校验必须在关闭 IDE **之前**失败——账号不存在时不触发任何进程操作。
    /// （关闭/注入本身要跑真实进程与 Keychain，单测不触碰；顺序由代码结构与这两条断言锁定。）
    #[test]
    fn switch_with_copy_validates_before_closing_ide() {
        let missing = format!("no-such-account-{}", uuid::Uuid::new_v4().simple());
        let items = [CopyItem {
            workspace_hash: WS.to_string(),
            conversation_id: CONV_OLD.to_string(),
            project_key: None,
            store: None,
        }];
        let error = switch_codebuddy_cn_ide_with_copy(&missing, true, &items, &[]).unwrap_err();
        assert!(error.contains("账号不存在"), "{error}");
    }

    /// 只勾同步（不勾复制）时也走编排 wrapper，且同样先校验账号再决定关闭。
    #[test]
    fn switch_with_sync_only_validates_before_closing_ide() {
        let missing = format!("no-such-account-{}", uuid::Uuid::new_v4().simple());
        let selections = [SyncSelection {
            group_id: "group-1".to_string(),
            preview_token: "00000000-0000-4000-8000-000000000000".to_string(),
            mode: crate::modules::session_link::SyncMode::FastForward,
        }];
        let error =
            switch_codebuddy_cn_ide_with_copy(&missing, true, &[], &selections).unwrap_err();
        assert!(error.contains("账号不存在"), "{error}");
    }

    /// 国际版入口与国内版共用同一编排内核：账号不存在时同样在关闭 IDE **之前**失败
    /// （复制与仅同步两条入口都断言）。
    #[test]
    fn intl_switch_with_copy_validates_before_closing_ide() {
        let missing = format!("no-such-account-{}", uuid::Uuid::new_v4().simple());
        let items = [CopyItem {
            workspace_hash: WS.to_string(),
            conversation_id: CONV_OLD.to_string(),
            project_key: None,
            store: None,
        }];
        let error = switch_codebuddy_intl_ide_with_copy(&missing, true, &items, &[]).unwrap_err();
        assert!(error.contains("账号不存在"), "{error}");

        let selections = [SyncSelection {
            group_id: "group-1".to_string(),
            preview_token: "00000000-0000-4000-8000-000000000000".to_string(),
            mode: crate::modules::session_link::SyncMode::FastForward,
        }];
        let error =
            switch_codebuddy_intl_ide_with_copy(&missing, true, &[], &selections).unwrap_err();
        assert!(error.contains("账号不存在"), "{error}");
    }

    /// 空参数（既不复制的也不同步）直通国际版 `switch_account`，不进入复制编排。
    #[test]
    fn intl_switch_without_selection_passes_through() {
        let missing = format!("no-such-account-{}", uuid::Uuid::new_v4().simple());
        let error = switch_codebuddy_intl_ide_with_copy(&missing, true, &[], &[]).unwrap_err();
        assert!(error.contains("账号不存在"), "{error}");
    }

    #[test]
    fn copy_rejects_unsafe_or_same_uids() {
        let fixture = Fixture::new("bad-uid");
        let items = [CopyItem {
            workspace_hash: WS.to_string(),
            conversation_id: CONV_OLD.to_string(),
            project_key: None,
            store: None,
        }];
        // uid 白名单 / 路径穿越：`default` / `Public` / 分隔符 / `..` / 空串一律拒绝。
        for uid in ["default", "Public", "a/b", "a\\b", "..", ""] {
            let error = copy_codebuddy_ide_sessions_in(
                &fixture.root,
                &fixture.backup,
                SRC_UID,
                uid,
                &items,
            )
            .unwrap_err();
            assert!(error.contains("非法"), "{uid}: {error}");
        }
        let error = copy_codebuddy_ide_sessions_in(
            &fixture.root,
            &fixture.backup,
            SRC_UID,
            SRC_UID,
            &items,
        )
        .unwrap_err();
        assert!(error.contains("相同"), "{error}");
    }

    /// 多次复制同一会话：第一次沿用 id，之后每次都取新 id（加法，不覆盖既有副本）。
    #[test]
    fn repeated_copy_never_overwrites_existing_copy() {
        let fixture = Fixture::new("repeat");
        seed_source(&fixture);
        let first = copy_once(&fixture, WS);
        let second = copy_once(&fixture, WS);

        let first_id = first.get("copied").and_then(Value::as_array).unwrap()[0]
            .get("newId")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        let second_id = second.get("copied").and_then(Value::as_array).unwrap()[0]
            .get("newId")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        assert_ne!(first_id, second_id);
        let index = read_json(&fixture.dst_ws_dir().join("index.json")).unwrap();
        let ids: BTreeSet<String> = index
            .get("conversations")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .map(|entry| entry.get("id").and_then(Value::as_str).unwrap().to_string())
            .collect();
        assert_eq!(ids.len(), 2, "两次复制生成两条独立条目");
        assert!(ids.contains(&first_id) && ids.contains(&second_id));
    }
