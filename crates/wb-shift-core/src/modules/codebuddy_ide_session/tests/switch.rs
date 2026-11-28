    const WS: &str = "0123456789abcdef0123456789abcdef";
    const WS2: &str = "fedcba9876543210fedcba9876543210";
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
                "wb_shift_codebuddy_ide_{}_{name}",
                uuid::Uuid::new_v4().simple()
            ));
            let root = base.join("Data");
            let backup = base.join("backup");
            std::fs::create_dir_all(&root).unwrap();
            std::fs::create_dir_all(&backup).unwrap();
            Self { root, backup }
        }

        fn src_ws_dir(&self) -> PathBuf {
            ide_ws_dir(&self.root, SRC_UID, WS)
        }

        fn dst_ws_dir(&self) -> PathBuf {
            ide_ws_dir(&self.root, DST_UID, WS)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Some(base) = self.root.parent() {
                let _ = std::fs::remove_dir_all(base);
            }
        }
    }

    /// 测试内定位工作区目录：与生产同构（`<root>/<uid>/CodeBuddyIDE/<uid>/history/<ws>`）。
    fn ide_ws_dir(root: &Path, uid: &str, workspace_hash: &str) -> PathBuf {
        history_root_in(CODEBUDDY_IDE_STORE, root, uid).join(workspace_hash)
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// 造一条源会话：`index.json` + 两条消息（含 `extra.requestId` 引用）+ 一个附件。
    fn seed_source(fixture: &Fixture) {
        let ws_dir = fixture.src_ws_dir();
        write(
            &ws_dir.join("index.json"),
            &format!(
                r#"{{"conversations":[{{"id":"{CONV_OLD}","type":"craft","name":"测试会话","createdAt":"2026-09-23T16:22:52.034Z","lastMessageAt":"2026-09-23T16:23:14.088Z"}}],"current":"{CONV_OLD}"}}"#
            ),
        );
        let conv_dir = ws_dir.join(CONV_OLD);
        write(
            &conv_dir.join("index.json"),
            &format!(
                r#"{{"messages":[{{"id":"{MSG_1}","type":"text","role":"user","isComplete":true}},{{"id":"{MSG_2}","type":"text","role":"assistant","isComplete":false}}],"requests":[{{"id":"{REQ_1}","type":"craft","messages":["{MSG_1}","{MSG_2}"],"state":"complete","startedAt":1790180582570}}]}}"#
            ),
        );
        write(
            &conv_dir.join(format!("messages/{MSG_1}.json")),
            &format!(
                r#"{{"role":"user","message":"{{\"role\":\"user\",\"content\":\"你好\"}}","id":"{MSG_1}","extra":"{{\"requestId\":\"{REQ_1}\",\"modelId\":\"hy3\"}}","createdAt":"2026-09-23T16:23:11.563Z"}}"#
            ),
        );
        write(
            &conv_dir.join(format!("messages/{MSG_2}.json")),
            &format!(
                r#"{{"role":"assistant","message":"{{\"role\":\"assistant\",\"content\":\"在的\"}}","id":"{MSG_2}","extra":"{{\"requestId\":\"{REQ_1}\",\"modelId\":\"hy3\"}}","createdAt":"2026-09-23T16:23:12.001Z"}}"#
            ),
        );
        std::fs::create_dir_all(conv_dir.join("assets")).unwrap();
        std::fs::write(conv_dir.join("assets/图片.1.jpeg"), b"\x01\x02\x03binary").unwrap();
    }

    /// 造目标账号既有的会话（另一条 id）。
    fn seed_target_index(fixture: &Fixture) {
        write(
            &fixture.dst_ws_dir().join("index.json"),
            &format!(
                r#"{{"conversations":[{{"id":"{CONV_EXISTING}","type":"craft","name":"目标既有"}}],"current":"{CONV_EXISTING}"}}"#
            ),
        );
    }

    fn copy_once(fixture: &Fixture, workspace_hash: &str) -> Value {
        copy_codebuddy_ide_sessions_in(
            &fixture.root,
            &fixture.backup,
            SRC_UID,
            DST_UID,
            &[CopyItem {
                workspace_hash: workspace_hash.to_string(),
                conversation_id: CONV_OLD.to_string(),
                project_key: None,
                store: None,
            }],
        )
        .expect("copy ok")
    }

    #[test]
    fn list_reports_sessions_and_skips_corrupted_index() {
        let fixture = Fixture::new("list");
        seed_source(&fixture);
        // 第二个工作区：损坏的 index.json（应计入 skipped）；第三个：合法但无 current。
        let broken = history_root_in(CODEBUDDY_IDE_STORE, &fixture.root, SRC_UID).join(WS2);
        write(&broken.join("index.json"), "{ not json");
        let no_current = history_root_in(CODEBUDDY_IDE_STORE, &fixture.root, SRC_UID)
            .join("11111111111111111111111111111111");
        write(
            &no_current.join("index.json"),
            &format!(r#"{{"conversations":[{{"id":"{CONV_EXISTING}","name":"无 current"}}]}}"#),
        );

        let result =
            vscode_session::list_sessions_in_store(CODEBUDDY_IDE_STORE, &fixture.root, SRC_UID);
        let sessions = result.get("sessions").and_then(Value::as_array).unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(result.get("skipped").and_then(Value::as_u64), Some(1));
        let first = sessions
            .iter()
            .find(|item| item.get("id").and_then(Value::as_str) == Some(CONV_OLD))
            .expect("含正文的会话在列表里");
        assert_eq!(first.get("workspaceHash").and_then(Value::as_str), Some(WS));
        assert_eq!(first.get("title").and_then(Value::as_str), Some("测试会话"));
        assert_eq!(first.get("hasHistory").and_then(Value::as_bool), Some(true));
        assert!(first.get("updatedAt").and_then(Value::as_i64).unwrap() > 0);
    }

    #[test]
    fn list_rejects_default_and_public_uid() {
        let fixture = Fixture::new("list-bad-uid");
        seed_source(&fixture);
        for uid in ["default", "Public", "a/b", "..", ""] {
            let result =
                vscode_session::list_sessions_in_store(CODEBUDDY_IDE_STORE, &fixture.root, uid);
            assert_eq!(
                result
                    .get("sessions")
                    .and_then(Value::as_array)
                    .map(Vec::len),
                Some(0),
                "{uid} 不应列出会话"
            );
        }
    }

    /// 未登录空列表也必须带 `dataRoot`（与「已登录但无会话」区分）；缺字段会让前端落到错误空态。
    #[test]
    fn empty_list_payload_always_includes_data_root() {
        let payload = empty_session_list(None, None);
        assert!(payload.get("sourceUid").unwrap().is_null());
        assert!(payload.get("dataRoot").unwrap().is_null());
        assert_eq!(
            payload
                .get("sessions")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0)
        );
        assert_eq!(payload.get("skipped").and_then(Value::as_u64), Some(0));

        let root = std::env::temp_dir();
        let payload = empty_session_list(None, Some(root.as_path()));
        assert!(payload.get("sourceUid").unwrap().is_null());
        assert_eq!(
            payload.get("dataRoot").and_then(Value::as_str),
            Some(root.to_string_lossy().as_ref())
        );
    }

    /// 空目标工作区：沿用源会话 id，直接可读；合并后必须写入 `current`。
    #[test]
    fn copy_into_empty_workspace_keeps_id_and_writes_current() {
        let fixture = Fixture::new("empty");
        seed_source(&fixture);

        let report = copy_once(&fixture, WS);
        let copied = report.get("copied").and_then(Value::as_array).unwrap();
        assert_eq!(copied.len(), 1);
        assert_eq!(
            copied[0].get("oldId").and_then(Value::as_str),
            Some(CONV_OLD)
        );
        assert_eq!(
            copied[0].get("newId").and_then(Value::as_str),
            Some(CONV_OLD),
            "无冲突时沿用源会话 id"
        );
        assert_eq!(copied[0].get("messages").and_then(Value::as_u64), Some(2));

        let dst_index = read_json(&fixture.dst_ws_dir().join("index.json")).unwrap();
        assert_eq!(
            dst_index.get("current").and_then(Value::as_str),
            Some(CONV_OLD)
        );
        assert_eq!(
            dst_index
                .get("conversations")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        // IDE 侧不写工作区级 `.index_bak.json`（目标原本没有就不新建）。
        assert!(!fixture.dst_ws_dir().join(".index_bak.json").exists());
    }

    /// 沿用 id 路径必须**字节级**复制：会话索引与消息文件内容与源逐字节一致。
    #[test]
    fn copy_keeps_conversation_files_byte_exact() {
        let fixture = Fixture::new("byte-exact");
        seed_source(&fixture);
        copy_once(&fixture, WS);

        let src_conv = fixture.src_ws_dir().join(CONV_OLD);
        let dst_conv = fixture.dst_ws_dir().join(CONV_OLD);
        for relative in [
            "index.json",
            &format!("messages/{MSG_1}.json"),
            &format!("messages/{MSG_2}.json"),
            "assets/图片.1.jpeg",
        ] {
            let src = std::fs::read(src_conv.join(relative)).unwrap();
            let dst = std::fs::read(dst_conv.join(relative)).unwrap();
            assert_eq!(src, dst, "{relative} 不是字节级复制");
        }
        // 源账号树保持不变。
        let src_index = read_json(&fixture.src_ws_dir().join("index.json")).unwrap();
        assert_eq!(
            src_index.get("current").and_then(Value::as_str),
            Some(CONV_OLD)
        );
    }

    /// 目标已有同 id：复制体取新 id、改写内部引用；目标既有会话与源会话都不被覆盖。
    #[test]
    fn copy_on_conflict_reassigns_id_and_rewrites_references() {
        let fixture = Fixture::new("conflict");
        seed_source(&fixture);
        // 目标工作区已有同 id 会话（内容与源不同，用于确认不被覆盖），且预置一条引用旧 id 的痕迹。
        write(
            &fixture.dst_ws_dir().join(CONV_OLD).join("index.json"),
            &format!(r#"{{"messages":[],"requests":[],"sessionId":"{CONV_OLD}"}}"#),
        );
        let target_original =
            std::fs::read(fixture.dst_ws_dir().join(CONV_OLD).join("index.json")).unwrap();
        seed_target_index(&fixture);

        let report = copy_once(&fixture, WS);
        let copied = report.get("copied").and_then(Value::as_array).unwrap();
        let new_id = copied[0]
            .get("newId")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        assert_ne!(new_id, CONV_OLD, "冲突时必须重随机会话 id");
        assert!(vscode_session::is_hex32(&new_id));

        // 目标原有会话目录（同 id）保持原样。
        assert_eq!(
            std::fs::read(fixture.dst_ws_dir().join(CONV_OLD).join("index.json")).unwrap(),
            target_original
        );
        // 目标工作区索引：既有条目 + 新条目，`current` 保留目标原值。
        let dst_index = read_json(&fixture.dst_ws_dir().join("index.json")).unwrap();
        let ids: Vec<String> = dst_index
            .get("conversations")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .map(|entry| entry.get("id").and_then(Value::as_str).unwrap().to_string())
            .collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.iter().any(|id| id == CONV_EXISTING));
        assert!(ids.contains(&new_id));
        assert_eq!(
            dst_index.get("current").and_then(Value::as_str),
            Some(CONV_EXISTING)
        );

        // 复制体内的 id 引用被改写到新 id，消息文件仍一一对应。
        let new_conv_dir = fixture.dst_ws_dir().join(&new_id);
        let conv_index = read_json(&new_conv_dir.join("index.json")).unwrap();
        let message_ids: Vec<String> = conv_index
            .get("messages")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .map(|m| m.get("id").and_then(Value::as_str).unwrap().to_string())
            .collect();
        assert!(new_conv_dir.join("index.json").is_file());
        for id in &message_ids {
            assert!(new_conv_dir.join(format!("messages/{id}.json")).is_file());
        }
        // 源会话目录原封不动（真源只读）。
        assert!(fixture
            .src_ws_dir()
            .join(CONV_OLD)
            .join("index.json")
            .is_file());
    }

    /// 「重写内部引用」的定向性：副本内只有真正出现旧会话 id 的文件被改写，其余字节不变。
    #[test]
    fn conflict_rewrite_only_touches_files_with_references() {
        let fixture = Fixture::new("conflict-rewrite");
        seed_source(&fixture);
        let src_conv = fixture.src_ws_dir().join(CONV_OLD);
        // 让消息文件里真的带一处会话 id 引用（字符串化 JSON 的 extra）。
        let msg2 = format!(
            r#"{{"role":"assistant","message":"{{\"role\":\"assistant\",\"content\":\"在的\"}}","id":"{MSG_2}","extra":"{{\"requestId\":\"{REQ_1}\",\"sessionId\":\"{CONV_OLD}\"}}","createdAt":"2026-09-23T16:23:12.001Z"}}"#
        );
        write(&src_conv.join(format!("messages/{MSG_2}.json")), &msg2);
        // 目标同 id 冲突。
        write(
            &fixture.dst_ws_dir().join(CONV_OLD).join("index.json"),
            r#"{"messages":[],"requests":[]}"#,
        );

        let report = copy_once(&fixture, WS);
        let new_id = report.get("copied").and_then(Value::as_array).unwrap()[0]
            .get("newId")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        let dst_conv = fixture.dst_ws_dir().join(&new_id);

        // 有引用的文件：引用改成新 id，其余字段保留。
        let rewritten: Value = serde_json::from_str(
            &std::fs::read_to_string(dst_conv.join(format!("messages/{MSG_2}.json"))).unwrap(),
        )
        .unwrap();
        let extra: Value =
            serde_json::from_str(rewritten.get("extra").and_then(Value::as_str).unwrap()).unwrap();
        assert_eq!(
            extra.get("sessionId").and_then(Value::as_str),
            Some(new_id.as_str())
        );
        assert_eq!(extra.get("requestId").and_then(Value::as_str), Some(REQ_1));
        // 无引用的文件：逐字节与源一致。
        assert_eq!(
            std::fs::read(dst_conv.join(format!("messages/{MSG_1}.json"))).unwrap(),
            std::fs::read(src_conv.join(format!("messages/{MSG_1}.json"))).unwrap()
        );
    }
