    const WS: &str = "0123456789abcdef0123456789abcdef";
    const CONV_OLD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const CONV_EXISTING: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    const MSG_1: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const MSG_2: &str = "cccccccccccccccccccccccccccccccc";
    const REQ_1: &str = "dddddddddddddddddddddddddddddddd";
    const SRC_UID: &str = "uid-src-0001";
    const DST_UID: &str = "uid-dst-0002";

    struct Fixture {
        root: PathBuf,
        backup: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "wb_shift_vscode_{}_{}",
                uuid::Uuid::new_v4().simple(),
                name
            ));
            let root = base.join("Data");
            let backup = base.join("backup");
            std::fs::create_dir_all(&root).unwrap();
            std::fs::create_dir_all(&backup).unwrap();
            Self { root, backup }
        }

        fn src_ws_dir(&self) -> PathBuf {
            history_root(&self.root, SRC_UID).join(WS)
        }

        fn dst_ws_dir(&self) -> PathBuf {
            history_root(&self.root, DST_UID).join(WS)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Some(base) = self.root.parent() {
                let _ = std::fs::remove_dir_all(base);
            }
        }
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// 时序（D11 + design §3.1）：目标校验必须在**关闭编辑器之前**失败——
    /// 账号 / `access_token` / 数据目录 / `state.vscdb` 任一不满足时都不许先关编辑器。
    /// 这里用「账号不存在」走纯逻辑分支：`find_account` 先失败，不会触发任何进程操作。
    #[test]
    fn switch_with_copy_validates_before_closing_editor() {
        let missing = format!("no-such-account-{}", uuid::Uuid::new_v4().simple());
        let items = vec![CopyItem {
            workspace_hash: WS.to_string(),
            conversation_id: CONV_OLD.to_string(),
            project_key: None,
        }];
        let error = switch_vscode_ext_with_copy(&missing, true, &items, &[]).unwrap_err();
        assert!(error.contains("账号不存在"), "{error}");
    }

    /// 时序（design §7）：只勾同步（不勾复制）时也走编排 wrapper，
    /// 且目标校验同样必须早于 `close_vscode_for_switch`。
    #[test]
    fn switch_with_sync_only_validates_before_closing_editor() {
        let missing = format!("no-such-account-{}", uuid::Uuid::new_v4().simple());
        let selections = vec![SyncSelection {
            group_id: "group-1".to_string(),
            preview_token: "00000000-0000-4000-8000-000000000000".to_string(),
            mode: crate::modules::session_link::SyncMode::FastForward,
        }];
        let error = switch_vscode_ext_with_copy(&missing, true, &[], &selections).unwrap_err();
        assert!(error.contains("账号不存在"), "{error}");
    }

    fn seed_source(fixture: &Fixture) {
        let ws_dir = fixture.src_ws_dir();
        write(
            &ws_dir.join("index.json"),
            &format!(
                r#"{{"conversations":[{{"id":"{CONV_OLD}","type":"craft","name":"测试会话","createdAt":"2026-09-15T07:47:01.550Z","lastMessageAt":"2026-09-16T05:22:25.406Z","chatMode":"craft"}}],"current":"{CONV_OLD}"}}"#
            ),
        );
        let conv_dir = ws_dir.join(CONV_OLD);
        let conv_index = format!(
            r#"{{"messages":[{{"id":"{MSG_1}","type":"text","role":"user","isComplete":true}},{{"id":"{MSG_2}","type":"text","role":"assistant","isComplete":true}}],"requests":[{{"id":"{REQ_1}","type":"craft","messages":["{MSG_1}","{MSG_2}"],"state":"complete","startedAt":1789532362052}}]}}"#
        );
        write(&conv_dir.join("index.json"), &conv_index);
        // 会话级 `.index_bak.json`：结构与 index.json 同，用于覆盖「备份索引同样重映射」路径。
        write(&conv_dir.join(".index_bak.json"), &conv_index);
        // extra 为字符串化 JSON：
        //  - requestId（→ 请求映射表）；responseId（= 消息自身 id，→ 消息映射表，D1 回归点）；
        //  - 嵌套 tasks / sourceContentBlocks / selectionContexts（内含旧 messageId / requestId，
        //    用于覆盖递归精确匹配重映射）。
        write(
            &conv_dir.join(format!("messages/{MSG_1}.json")),
            &format!(
                r#"{{"role":"user","message":"{{\"role\":\"user\",\"content\":\"你好\"}}","id":"{MSG_1}","extra":"{{\"requestId\":\"{REQ_1}\",\"responseId\":\"{MSG_1}\",\"modelId\":\"deepseek-v4\",\"tasks\":[{{\"messageId\":\"{MSG_2}\",\"status\":\"done\"}}],\"sourceContentBlocks\":[\"{MSG_2}\"],\"selectionContexts\":[{{\"id\":\"{REQ_1}\"}}]}}","createdAt":"2026-09-16T05:05:29.751Z"}}"#
            ),
        );
        write(
            &conv_dir.join(format!("messages/{MSG_2}.json")),
            &format!(
                r#"{{"role":"assistant","message":"{{\"role\":\"assistant\",\"content\":\"在的\"}}","id":"{MSG_2}","extra":"{{\"requestId\":\"{REQ_1}\",\"responseId\":\"{MSG_2}\",\"modelId\":\"deepseek-v4\"}}","createdAt":"2026-09-16T05:05:31.001Z"}}"#
            ),
        );
        std::fs::create_dir_all(conv_dir.join("assets")).unwrap();
        std::fs::write(conv_dir.join("assets/图片.1.jpeg"), b"\x01\x02\x03binary").unwrap();
    }

    fn seed_target_index(fixture: &Fixture) {
        let ws_dir = fixture.dst_ws_dir();
        write(
            &ws_dir.join("index.json"),
            &format!(
                r#"{{"conversations":[{{"id":"{CONV_EXISTING}","type":"craft","name":"已存在"}}],"current":"{CONV_EXISTING}"}}"#
            ),
        );
    }

    #[test]
    fn gen_hex32_is_32_lower_hex_and_unique() {
        let mut seen = BTreeSet::new();
        for _ in 0..64 {
            let id = gen_hex32();
            assert_eq!(id.len(), 32);
            assert!(is_hex32(&id), "not lower hex: {id}");
            assert!(seen.insert(id), "duplicate id generated");
        }
    }

    #[test]
    fn safe_uid_rejects_default_public_and_paths() {
        assert!(is_safe_uid("3d3fbea0abcdef"));
        assert!(!is_safe_uid("default"));
        assert!(!is_safe_uid("Public"));
        assert!(!is_safe_uid("a/b"));
        assert!(!is_safe_uid("a\\b"));
        assert!(!is_safe_uid(".."));
        assert!(!is_safe_uid(""));
    }

    #[test]
    fn list_sessions_in_reports_title_and_history() {
        let fixture = Fixture::new("list");
        seed_source(&fixture);
        let result = list_sessions_in(&fixture.root, SRC_UID);
        let sessions = result.get("sessions").and_then(Value::as_array).unwrap();
        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        assert_eq!(session.get("id").and_then(Value::as_str), Some(CONV_OLD));
        assert_eq!(
            session.get("workspaceHash").and_then(Value::as_str),
            Some(WS)
        );
        assert_eq!(
            session.get("title").and_then(Value::as_str),
            Some("测试会话")
        );
        assert_eq!(
            session.get("hasHistory").and_then(Value::as_bool),
            Some(true)
        );
        assert!(session.get("updatedAt").and_then(Value::as_i64).unwrap() > 0);
        // F3：响应带上解析到的数据根目录（找不到根时为 null，由 list_vscode_sessions 兜底）。
        let expected_root = fixture.root.to_string_lossy().to_string();
        assert_eq!(
            result.get("dataRoot").and_then(Value::as_str),
            Some(expected_root.as_str())
        );
    }

    /// F5 回归：`messages/` 下的非 32-hex 文件名原样复制、原样命名、内文 id 不动；
    /// 正常 hex 消息文件仍按映射表重命名并同步内文 id，且不产生多余 hex 文件。
    /// F9 回归：目标工作区原本没有索引时，合并后必须写入 `current`。
    ///
    /// 实测（macOS，扩展 4.12）：缺 `current` 的工作区索引会被扩展判为损坏——改名为
    /// `index.json.corrupted.<ms>` 并重建，同时多出一条垃圾空会话（见 acceptance.md §3）。
    /// 目标原本 `current` 仍有效则保留的场景由
    /// `copy_conversation_remaps_ids_and_merges_index` 覆盖。
    /// R6-07 写入点 3/4：IDE（三客户端共用的 `copy_sessions_in_with`）复制成功后必须写
    /// 归属交接，索引落在**工具存储根**（由备份根 `<store>/backups/<kind>/<iso>` 反推）。
    #[test]
    fn copy_records_usage_origin_under_the_backup_store_root() {
        let fixture = Fixture::new("origin-index");
        seed_source(&fixture);
        // 生产侧备份根就是 `<工具存储根>/backups/<backup_kind>/<utc_iso>`。
        let store = fixture
            .root
            .parent()
            .expect("fixture base")
            .join(".workbuddy-shift");
        let backup = store
            .join("backups")
            .join(VSCODE_STORE.backup_kind)
            .join("2026-11-28T10-00-00Z");
        std::fs::create_dir_all(&backup).unwrap();

        let items = vec![CopyItem {
            workspace_hash: WS.to_string(),
            conversation_id: CONV_OLD.to_string(),
            project_key: None,
        }];
        let report =
            copy_sessions_in(&fixture.root, &backup, SRC_UID, DST_UID, &items).expect("copy ok");
        let new_conv = report["copied"]
            .as_array()
            .expect("copied")
            .first()
            .and_then(|item| item["newId"].as_str())
            .expect("newId")
            .to_string();
        assert_ne!(new_conv, CONV_OLD, "VS Code 路径恒取新会话 id");

        let index = crate::modules::usage_origin::OriginIndex::load_at(&store);
        let entry = index
            .entry(crate::modules::usage_origin::NS_IDE_VSCODE, &new_conv)
            .expect("IDE 复制必须写归属交接");
        assert_eq!(entry.segments.len(), 1);
        assert_eq!(entry.segments[0].uid, SRC_UID, "历史必须归来源账号");
        assert!(!entry.heuristic, "显式写入不是启发式");
        assert!(!entry.truncated);
    }

    #[test]
    fn copy_into_empty_workspace_writes_current() {
        let fixture = Fixture::new("empty-target-ws");
        seed_source(&fixture);

        let items = vec![CopyItem {
            workspace_hash: WS.to_string(),
            conversation_id: CONV_OLD.to_string(),
            project_key: None,
        }];
        let report = copy_sessions_in(&fixture.root, &fixture.backup, SRC_UID, DST_UID, &items)
            .expect("copy ok");
        let new_conv = report.get("copied").and_then(Value::as_array).unwrap()[0]
            .get("newId")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();

        let dst_index = read_json(&fixture.dst_ws_dir().join("index.json")).unwrap();
        assert_eq!(
            dst_index.get("current").and_then(Value::as_str),
            Some(new_conv.as_str()),
            "空目标工作区合并后必须写入 current"
        );
        let bak = read_json(&fixture.dst_ws_dir().join(".index_bak.json")).unwrap();
        assert_eq!(
            bak.get("current").and_then(Value::as_str),
            Some(new_conv.as_str())
        );
    }

    #[test]
    fn copy_keeps_non_hex_message_files_untouched() {
        let fixture = Fixture::new("non-hex-files");
        seed_source(&fixture);
        seed_target_index(&fixture);
        let src_conv_dir = fixture.src_ws_dir().join(CONV_OLD);
        // 故意用「带缩进 + 含旧 requestId 引用」的 JSON：若实现对非 hex 文件做解析重写，
        // 字节会变（缩进丢失、id 被替换），下面的字节级断言即会失败。
        let notes = format!(
            "{{\n  \"note\": \"调试笔记，不是消息\",\n  \"extra\": {{ \"requestId\": \"{REQ_1}\" }}\n}}\n"
        );
        let legacy = "{\n  \"kind\": \"legacy-index\"\n}\n".to_string();
        write(&src_conv_dir.join("messages/notes.json"), &notes);
        write(&src_conv_dir.join("messages/index.json"), &legacy);

        let items = vec![CopyItem {
            workspace_hash: WS.to_string(),
            conversation_id: CONV_OLD.to_string(),
            project_key: None,
        }];
        let report = copy_sessions_in(&fixture.root, &fixture.backup, SRC_UID, DST_UID, &items)
            .expect("copy ok");
        assert_eq!(
            report.get("copied").and_then(Value::as_array).map(Vec::len),
            Some(1)
        );
        // 报告的消息数量仍等于索引内的消息数（非 hex 文件不计入）。
        assert_eq!(
            report.get("copied").and_then(Value::as_array).unwrap()[0]
                .get("messages")
                .and_then(Value::as_u64),
            Some(2)
        );

        let new_conv = report.get("copied").and_then(Value::as_array).unwrap()[0]
            .get("newId")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        let target_messages = fixture.dst_ws_dir().join(&new_conv).join("messages");

        // 非 hex 文件：原名存在、内容未改。
        assert_eq!(
            std::fs::read_to_string(target_messages.join("notes.json")).unwrap(),
            notes
        );
        assert_eq!(
            std::fs::read_to_string(target_messages.join("index.json")).unwrap(),
            legacy
        );

        // 目录里恰好 4 个文件：2 个 hex 消息（重命名后）+ 2 个非 hex 原样文件，无多余 hex。
        let names: BTreeSet<String> = std::fs::read_dir(&target_messages)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names.len(), 4, "目标 messages 目录文件集不符：{names:?}");
        assert!(names.contains("notes.json") && names.contains("index.json"));
        for name in &names {
            if name == "notes.json" || name == "index.json" {
                continue;
            }
            let stem = name.strip_suffix(".json").expect("json 文件名");
            assert!(is_hex32(stem), "消息文件未被重命名为 hex：{name}");
            assert_ne!(stem, MSG_1);
            assert_ne!(stem, MSG_2);
            // 内文 id 与文件名同步。
            let value: Value =
                serde_json::from_str(&std::fs::read_to_string(target_messages.join(name)).unwrap())
                    .unwrap();
            assert_eq!(value.get("id").and_then(Value::as_str), Some(stem));
        }

        // 源目录不被改动（非 hex 文件仍在原位、原名）。
        assert_eq!(
            std::fs::read_to_string(src_conv_dir.join("messages/notes.json")).unwrap(),
            notes
        );
    }

    #[test]
    fn copy_conversation_remaps_ids_and_merges_index() {
        let fixture = Fixture::new("copy");
        seed_source(&fixture);
        seed_target_index(&fixture);

        let items = vec![CopyItem {
            workspace_hash: WS.to_string(),
            conversation_id: CONV_OLD.to_string(),
            project_key: None,
        }];
        let report = copy_sessions_in(&fixture.root, &fixture.backup, SRC_UID, DST_UID, &items)
            .expect("copy ok");

        let copied = report.get("copied").and_then(Value::as_array).unwrap();
        assert_eq!(copied.len(), 1);
        assert!(report.get("errors").is_none());
        let new_conv = copied[0]
            .get("newId")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        assert_ne!(new_conv, CONV_OLD);
        assert!(is_hex32(&new_conv), "new conversation id not lower hex");
        assert_eq!(
            copied[0].get("oldId").and_then(Value::as_str),
            Some(CONV_OLD)
        );
        assert_eq!(copied[0].get("messages").and_then(Value::as_u64), Some(2));

        // 目标工作区索引：已合并新会话且 `current` 保持不变。
        let dst_index = read_json(&fixture.dst_ws_dir().join("index.json")).unwrap();
        let conversations = dst_index
            .get("conversations")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(conversations.len(), 2);
        assert_eq!(
            dst_index.get("current").and_then(Value::as_str),
            Some(CONV_EXISTING)
        );
        let merged = conversations
            .iter()
            .find(|entry| entry.get("id").and_then(Value::as_str) == Some(new_conv.as_str()))
            .expect("merged entry");
        assert_eq!(merged.get("name").and_then(Value::as_str), Some("测试会话"));
        assert_eq!(
            merged.get("chatMode").and_then(Value::as_str),
            Some("craft")
        );

        // 会话索引：消息 / 请求 id 全部重映射，requests[].messages[] 与磁盘文件名一致。
        let new_conv_dir = fixture.dst_ws_dir().join(&new_conv);
        let conv_index = read_json(&new_conv_dir.join("index.json")).unwrap();
        let messages = conv_index
            .get("messages")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(messages.len(), 2);
        let mut message_ids: Vec<String> = Vec::new();
        for message in messages {
            let id = message
                .get("id")
                .and_then(Value::as_str)
                .unwrap()
                .to_string();
            assert!(is_hex32(&id));
            assert_ne!(id, MSG_1);
            assert_ne!(id, MSG_2);
            assert!(new_conv_dir.join(format!("messages/{id}.json")).is_file());
            message_ids.push(id);
        }
        let requests = conv_index
            .get("requests")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(requests.len(), 1);
        let new_req = requests[0]
            .get("id")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        assert!(is_hex32(&new_req));
        assert_ne!(new_req, REQ_1);
        let req_messages: Vec<String> = requests[0]
            .get("messages")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_string())
            .collect();
        assert_eq!(req_messages, message_ids);

        // 每条消息文件的 extra.requestId 已重映射到新的 request id。
        for id in &message_ids {
            let message = read_json(&new_conv_dir.join(format!("messages/{id}.json"))).unwrap();
            assert_eq!(message.get("id").and_then(Value::as_str), Some(id.as_str()));
            let extra_text = message.get("extra").and_then(Value::as_str).unwrap();
            let extra: Value = serde_json::from_str(extra_text).unwrap();
            assert_eq!(
                extra.get("requestId").and_then(Value::as_str),
                Some(new_req.as_str())
            );
        }

        // 附件按原名复制。
        let asset = new_conv_dir.join("assets/图片.1.jpeg");
        assert!(asset.is_file());
        assert_eq!(std::fs::read(&asset).unwrap(), b"\x01\x02\x03binary");

        // 源目录保持不变。
        assert!(fixture.src_ws_dir().join(CONV_OLD).is_dir());
        assert!(fixture
            .src_ws_dir()
            .join(CONV_OLD)
            .join(format!("messages/{MSG_1}.json"))
            .is_file());
        let src_index = read_json(&fixture.src_ws_dir().join("index.json")).unwrap();
        assert_eq!(
            src_index.get("current").and_then(Value::as_str),
            Some(CONV_OLD)
        );
    }
