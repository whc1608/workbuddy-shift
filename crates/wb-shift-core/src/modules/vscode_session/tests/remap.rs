    #[test]
    fn copy_backup_is_written_to_backup_root() {
        let fixture = Fixture::new("backup");
        seed_source(&fixture);
        seed_target_index(&fixture);

        let items = vec![CopyItem {
            workspace_hash: WS.to_string(),
            conversation_id: CONV_OLD.to_string(),
            project_key: None,
            store: None,
        }];
        copy_sessions_in(&fixture.root, &fixture.backup, SRC_UID, DST_UID, &items).unwrap();

        let backed_up = read_json(&fixture.backup.join(WS).join("index.json")).unwrap();
        assert_eq!(
            backed_up.get("current").and_then(Value::as_str),
            Some(CONV_EXISTING)
        );
    }

    #[test]
    fn failed_item_leaves_target_untouched_and_records_error() {
        let fixture = Fixture::new("fail");
        seed_source(&fixture);
        seed_target_index(&fixture);
        let original = std::fs::read(fixture.dst_ws_dir().join("index.json")).unwrap();

        let missing = "99999999999999999999999999999999";
        let items = vec![CopyItem {
            workspace_hash: WS.to_string(),
            conversation_id: missing.to_string(),
            project_key: None,
            store: None,
        }];
        let report = copy_sessions_in(&fixture.root, &fixture.backup, SRC_UID, DST_UID, &items)
            .expect("returns report");

        assert_eq!(
            report.get("copied").and_then(Value::as_array).map(Vec::len),
            Some(0)
        );
        let errors = report.get("errors").and_then(Value::as_array).unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].get("conversationId").and_then(Value::as_str),
            Some(missing)
        );

        // 目标索引未改动、无残留临时目录、源目录不变。
        let after = std::fs::read(fixture.dst_ws_dir().join("index.json")).unwrap();
        assert_eq!(after, original);
        assert!(std::fs::read_dir(fixture.dst_ws_dir())
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().starts_with(".tmp-")));
        assert!(fixture.src_ws_dir().join(CONV_OLD).is_dir());
    }

    #[test]
    fn partial_failure_keeps_successful_copy_and_preserves_current() {
        let fixture = Fixture::new("partial");
        seed_source(&fixture);
        seed_target_index(&fixture);

        let items = vec![
            CopyItem {
                workspace_hash: WS.to_string(),
                conversation_id: CONV_OLD.to_string(),
                project_key: None,
                store: None,
            },
            CopyItem {
                workspace_hash: WS.to_string(),
                conversation_id: "99999999999999999999999999999999".to_string(),
                project_key: None,
                store: None,
            },
        ];
        let report =
            copy_sessions_in(&fixture.root, &fixture.backup, SRC_UID, DST_UID, &items).unwrap();
        assert_eq!(
            report.get("copied").and_then(Value::as_array).map(Vec::len),
            Some(1)
        );
        assert_eq!(
            report.get("errors").and_then(Value::as_array).map(Vec::len),
            Some(1)
        );

        let dst_index = read_json(&fixture.dst_ws_dir().join("index.json")).unwrap();
        assert_eq!(
            dst_index
                .get("conversations")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(2)
        );
        assert_eq!(
            dst_index.get("current").and_then(Value::as_str),
            Some(CONV_EXISTING)
        );
    }

    #[test]
    fn rejects_same_source_and_target_uid() {
        let fixture = Fixture::new("same-uid");
        let items = vec![CopyItem {
            workspace_hash: WS.to_string(),
            conversation_id: CONV_OLD.to_string(),
            project_key: None,
            store: None,
        }];
        let error =
            copy_sessions_in(&fixture.root, &fixture.backup, SRC_UID, SRC_UID, &items).unwrap_err();
        assert!(error.contains("相同"));
    }

    #[test]
    fn rejects_default_and_public_target_uid() {
        let fixture = Fixture::new("bad-uid");
        let items = vec![CopyItem {
            workspace_hash: WS.to_string(),
            conversation_id: CONV_OLD.to_string(),
            project_key: None,
            store: None,
        }];
        assert!(
            copy_sessions_in(&fixture.root, &fixture.backup, SRC_UID, "default", &items).is_err()
        );
        assert!(
            copy_sessions_in(&fixture.root, &fixture.backup, SRC_UID, "Public", &items).is_err()
        );
    }

    /// D1 回归：复制后目标目录「零旧 id 残留」。
    ///
    /// 全目录递归扫描（跳过二进制附件），断言源旧会话 id、全部旧 messageId、全部旧 requestId
    /// 均不出现在目标工作区任何可解析文本文件的内容里；并反向确认 `responseId` 已指向消息新 id。
    #[test]
    fn copied_conversation_leaves_no_legacy_ids_in_target() {
        let fixture = Fixture::new("residue");
        seed_source(&fixture);
        seed_target_index(&fixture);

        let items = vec![CopyItem {
            workspace_hash: WS.to_string(),
            conversation_id: CONV_OLD.to_string(),
            project_key: None,
            store: None,
        }];
        let report = copy_sessions_in(&fixture.root, &fixture.backup, SRC_UID, DST_UID, &items)
            .expect("copy ok");
        assert_eq!(
            report.get("copied").and_then(Value::as_array).map(Vec::len),
            Some(1)
        );

        // 核心断言：目标工作区目录下，旧 conversationId / messageId / requestId 零残留。
        assert_no_legacy_ids(&fixture.dst_ws_dir(), &[CONV_OLD, MSG_1, MSG_2, REQ_1]);

        // 反向确认：responseId 已重映射为消息自身的新 id，且 extra 内嵌套 id 也已是新 id。
        let new_conv = report.get("copied").and_then(Value::as_array).unwrap()[0]
            .get("newId")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        let new_conv_dir = fixture.dst_ws_dir().join(&new_conv);
        let mut new_message_ids: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(new_conv_dir.join("messages"))
            .unwrap()
            .flatten()
        {
            let value: Value =
                serde_json::from_str(&std::fs::read_to_string(entry.path()).unwrap()).unwrap();
            let id = value.get("id").and_then(Value::as_str).unwrap().to_string();
            assert!(is_hex32(&id) && id != MSG_1 && id != MSG_2);
            let extra: Value =
                serde_json::from_str(value.get("extra").and_then(Value::as_str).unwrap()).unwrap();
            assert_eq!(
                extra.get("responseId").and_then(Value::as_str),
                Some(id.as_str()),
                "responseId 未重映射为消息自身新 id"
            );
            new_message_ids.push(id);
        }
        assert_eq!(new_message_ids.len(), 2);
    }

    /// D3 回归：某工作区所有条目最终都失败时，不得创建空的目标工作区目录。
    #[test]
    fn all_failed_items_leave_no_target_directory() {
        let fixture = Fixture::new("no-empty-dir");
        seed_source(&fixture); // 仅 seed 源；目标工作区不存在
        assert!(!fixture.dst_ws_dir().exists());

        let items = vec![
            CopyItem {
                workspace_hash: WS.to_string(),
                conversation_id: "bad-id".to_string(),
                project_key: None,
                store: None,
            },
            CopyItem {
                workspace_hash: WS.to_string(),
                conversation_id: "99999999999999999999999999999999".to_string(),
                project_key: None,
                store: None,
            },
        ];
        let report = copy_sessions_in(&fixture.root, &fixture.backup, SRC_UID, DST_UID, &items)
            .expect("returns report");
        assert_eq!(
            report.get("copied").and_then(Value::as_array).map(Vec::len),
            Some(0)
        );
        assert_eq!(
            report.get("errors").and_then(Value::as_array).map(Vec::len),
            Some(2)
        );

        assert!(
            !fixture.dst_ws_dir().exists(),
            "所有条目失败时不应创建空的目标工作区目录"
        );
    }

    /// 递归断言：`dir` 下所有可解析为 UTF-8 的文件内容都不含任何 `legacy` id
    /// （二进制附件按约定跳过文件名 / 内容比对）。
    fn assert_no_legacy_ids(dir: &Path, legacy: &[&str]) {
        let files = collect_files(dir);
        assert!(!files.is_empty(), "目标目录为空，扫描无意义");
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue; // 二进制附件：跳过
            };
            for id in legacy {
                assert!(!text.contains(id), "旧 id {id} 残留在 {}", path.display());
            }
        }
    }

    /// 递归收集目录下所有常规文件路径。
    fn collect_files(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    out.extend(collect_files(&path));
                } else if path.is_file() {
                    out.push(path);
                }
            }
        }
        out
    }
