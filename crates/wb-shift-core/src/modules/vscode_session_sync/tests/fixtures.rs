    const WS: &str = "0123456789abcdef0123456789abcdef";
    const SRC_UID: &str = "uid-src-0001";
    const DST_UID: &str = "uid-dst-0002";
    const CONV_SRC: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const MSG_1: &str = "11111111111111111111111111111111";
    const MSG_2: &str = "22222222222222222222222222222222";
    const MSG_3: &str = "33333333333333333333333333333333";
    const MSG_4: &str = "44444444444444444444444444444444";
    const REQ_1: &str = "55555555555555555555555555555555";
    const REQ_2: &str = "66666666666666666666666666666666";
    const MSG_5: &str = "77777777777777777777777777777777";
    const MSG_6: &str = "88888888888888888888888888888888";
    const REQ_3: &str = "99999999999999999999999999999999";

    struct Fixture {
        base: PathBuf,
        /// 扩展数据根（`<base>/Data`）。
        root: PathBuf,
        /// 复制备份根（注入给 `copy_sessions_in`）。
        backup: PathBuf,
        /// 工具存储根（注入给 `SessionPaths`）。
        store: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "wb_shift_vscode_sync_{}_{name}",
                uuid::Uuid::new_v4().simple()
            ));
            let root = base.join("Data");
            let backup = base.join("copy-backup");
            let store = base.join("store");
            for dir in [&root, &backup, &store] {
                std::fs::create_dir_all(dir).unwrap();
            }
            Self {
                base,
                root,
                backup,
                store,
            }
        }

        fn paths(&self) -> SessionPaths {
            SessionPaths::for_vscode_ext_at(self.store.clone())
        }

        fn src_conv_dir(&self) -> PathBuf {
            vscode_session::history_root(&self.root, SRC_UID)
                .join(WS)
                .join(CONV_SRC)
        }

        fn dst_conv_dir(&self, conv_id: &str) -> PathBuf {
            vscode_session::history_root(&self.root, DST_UID)
                .join(WS)
                .join(conv_id)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn target_acc() -> Value {
        json!({ "id": "acc-dst", "uid": DST_UID, "nickname": "目标账号" })
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// 写一条消息文件：`extra` 为字符串化 JSON（实测形态）。
    fn write_message(conv_dir: &Path, id: &str, role: &str, request_id: &str, text: &str) {
        write(
            &conv_dir.join(format!("messages/{id}.json")),
            &json!({
                "role": role,
                "message": json!({"role": role, "content": text}).to_string(),
                "id": id,
                "extra": json!({"requestId": request_id, "responseId": id, "modelId": "deepseek-v4"})
                    .to_string(),
                "createdAt": "2026-09-16T05:05:29.751Z",
            })
            .to_string(),
        );
    }

    /// 造源账号的会话：工作区索引 + 2 条记录（user + assistant）+ 一个附件。
    fn seed_source(fixture: &Fixture) {
        let ws_dir = vscode_session::history_root(&fixture.root, SRC_UID).join(WS);
        write(
            &ws_dir.join("index.json"),
            &json!({
                "conversations": [{
                    "id": CONV_SRC, "type": "craft", "name": "测试会话",
                    "createdAt": "2026-09-15T07:47:01.550Z",
                    "lastMessageAt": "2026-09-16T05:22:25.406Z", "chatMode": "craft",
                }],
                "current": CONV_SRC,
            })
            .to_string(),
        );
        let conv_dir = ws_dir.join(CONV_SRC);
        write(
            &conv_dir.join("index.json"),
            &json!({
                "messages": [
                    {"id": MSG_1, "type": "text", "role": "user", "isComplete": true},
                    // 助手消息的 isComplete 恒为 false（实测常态，不得据此判断写完）
                    {"id": MSG_2, "type": "text", "role": "assistant", "isComplete": false},
                ],
                "requests": [{
                    "id": REQ_1, "type": "craft", "messages": [MSG_1, MSG_2],
                    "state": "complete", "startedAt": 1789532362052_i64,
                }],
            })
            .to_string(),
        );
        write_message(&conv_dir, MSG_1, "user", REQ_1, "你好");
        write_message(&conv_dir, MSG_2, "assistant", REQ_1, "在的");
        std::fs::create_dir_all(conv_dir.join("assets")).unwrap();
        std::fs::write(conv_dir.join("assets/图片.1.jpeg"), b"\x01\x02\x03binary").unwrap();
    }

    /// 源账号追加一轮对话（user + assistant + 一条请求）与一个新附件。
    fn append_round(
        fixture: &Fixture,
        user_id: &str,
        assistant_id: &str,
        request_id: &str,
        text: &str,
    ) {
        let conv_dir = fixture.src_conv_dir();
        let mut index = vscode_session::read_json(&conv_dir.join("index.json")).unwrap();
        index["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": user_id, "type": "text", "role": "user", "isComplete": true}));
        index["messages"].as_array_mut().unwrap().push(
            json!({"id": assistant_id, "type": "text", "role": "assistant", "isComplete": false}),
        );
        index["requests"].as_array_mut().unwrap().push(json!({
            "id": request_id, "type": "craft", "messages": [user_id, assistant_id],
            "state": "complete", "startedAt": 1789532363000_i64,
        }));
        write(&conv_dir.join("index.json"), &index.to_string());
        write_message(&conv_dir, user_id, "user", request_id, text);
        write_message(&conv_dir, assistant_id, "assistant", request_id, "收到");
        std::fs::write(conv_dir.join("assets/新附件.txt"), b"new asset").unwrap();
    }

    /// 复制一次并登记关联，返回副本会话 id（登记失败即断言失败）。
    fn copy_and_register(fixture: &Fixture) -> String {
        let items = vec![CopyItem {
            workspace_hash: WS.to_string(),
            conversation_id: CONV_SRC.to_string(),
            project_key: None,
            store: None,
        }];
        let report =
            copy_sessions_in(&fixture.root, &fixture.backup, SRC_UID, DST_UID, &items).unwrap();
        let errors =
            register_copied_sessions(&fixture.root, &fixture.paths(), WbVariant::Cn, &report);
        assert!(errors.is_empty(), "登记失败：{errors:?}");
        report["copied"][0]["newId"].as_str().unwrap().to_string()
    }

    fn records(conv_dir: &Path) -> Vec<String> {
        let index = vscode_session::read_json(&conv_dir.join("index.json")).unwrap();
        vscode_session_link::message_ids_of(&index)
    }

    fn digests(conv_dir: &Path, conv_id: &str) -> Vec<String> {
        match vscode_session_link::read_session_content(conv_dir, conv_id) {
            ContentState::Ready(snapshot) => snapshot.normalized.line_digests,
            other => panic!("内容不可验证：{other:?}"),
        }
    }

    /// 目录快照：相对路径 → 文件字节（比较「逐字节一致」与增删）。
    fn dir_snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
        fn collect(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect(root, &path, out);
                } else if path.is_file() {
                    let key = path
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .to_string();
                    out.insert(key, std::fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        collect(dir, dir, &mut out);
        out
    }

    fn selection(group_id: &str, token: &str, mode: SyncMode) -> SyncSelection {
        SyncSelection {
            group_id: group_id.to_string(),
            preview_token: token.to_string(),
            mode,
        }
    }

    /// `messages/` 下的文件名集合（含未被索引引用的「孤儿」文件）。
    fn message_files(conv_dir: &Path) -> BTreeSet<String> {
        std::fs::read_dir(conv_dir.join("messages"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect()
    }

    /// 整树拷贝目录（造现场 / 还原现场用；`from` 不存在时把 `to` 清空）。
    fn copy_tree(from: &Path, to: &Path) {
        vscode_session::remove_dir_all_if_exists(to);
        if from.is_dir() {
            vscode_session::copy_dir_recursive(from, to).unwrap();
        }
    }

    /// 反查目标会话的整目录备份：`<备份根>/<utc_iso>/<workspaceHash>/<会话 id>-overwrite`。
    ///
    /// 按目录名反查而不是重新拼 `utc_iso()`，避免断言与备份时刻跨秒时取到别的目录。
    fn find_overwrite_backup(fixture: &Fixture, conv_id: &str) -> PathBuf {
        let root = fixture.paths().backup_root().join(VSCODE_STORE.backup_kind);
        let mut pending = vec![root];
        let mut found: Vec<PathBuf> = Vec::new();
        while let Some(dir) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default();
                if name == format!("{conv_id}{OVERWRITE_SUFFIX}") {
                    found.push(path);
                } else {
                    pending.push(path);
                }
            }
        }
        assert_eq!(found.len(), 1, "应恰好有一份整目录备份：{found:?}");
        found.pop().unwrap()
    }

    /// 把文件 mtime 设成指定时刻（Windows 上 `File::set_modified` 需要 `FILE_WRITE_ATTRIBUTES`，
    /// 只读句柄会得到 `os error 5`，故用可写句柄打开——同 `token_stats::pin_mtime`）。
    fn set_mtime(path: &Path, time: SystemTime) {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(time)
            .unwrap();
    }

    /// 造「进程恰好在写完新增消息文件、更新索引之前被 kill」的现场（design §6.1 步骤 2→3 之间）。
    ///
    /// 先正常同步一次（产出派生 id 的消息文件），再把会话索引与关联存储回滚到操作前、
    /// 保留已落盘的消息文件；返回这批「已落盘但未被索引引用」的文件名。
    fn interrupted_index_write_scene(fixture: &Fixture, target_conv: &str) -> BTreeSet<String> {
        let target_dir = fixture.dst_conv_dir(target_conv);
        let index_path = target_dir.join("index.json");
        let bak_path = target_dir.join(".index_bak.json");
        let before_index = std::fs::read(&index_path).unwrap();
        let before_bak = std::fs::read(&bak_path).ok();
        let store_backup = fixture.base.join("store-before-sync");
        copy_tree(&fixture.store, &store_backup);

        // 第一次同步正常完成：目标里留下两条派生 id 的消息文件，索引与基线都已更新。
        let preview =
            links_preview_at(&fixture.root, &fixture.paths(), SRC_UID, &target_acc()).unwrap();
        let group = &preview["groups"][0];
        assert_eq!(group["verdict"], "fastForward");
        let group_id = group["groupId"].as_str().unwrap().to_string();
        let token = group["previewToken"].as_str().unwrap().to_string();
        let first = sync_selected_at(
            &fixture.root,
            &fixture.paths(),
            SRC_UID,
            &target_acc(),
            &[selection(&group_id, &token, SyncMode::FastForward)],
        )
        .unwrap();
        assert_eq!(first["synced"].as_array().unwrap().len(), 1, "{first}");
        assert_eq!(records(&target_dir).len(), 4, "造数：第一次同步成功");
        let written = message_files(&target_dir);
        assert_eq!(written.len(), 4, "造数：两条新增消息文件已落盘");

        // 回滚索引与关联存储（消息文件保留）→ 「上次中途失败」的现场。
        std::fs::write(&index_path, &before_index).unwrap();
        match &before_bak {
            Some(bytes) => std::fs::write(&bak_path, bytes).unwrap(),
            None => {
                let _ = std::fs::remove_file(&bak_path);
            }
        }
        copy_tree(&store_backup, &fixture.store);

        let tracked: BTreeSet<String> = records(&target_dir)
            .iter()
            .map(|id| format!("{id}.json"))
            .collect();
        assert_eq!(tracked.len(), 2, "造数：索引回到操作前");
        let orphans: BTreeSet<String> = written.difference(&tracked).cloned().collect();
        assert_eq!(orphans.len(), 2, "造数：两条新增文件已落盘但未被索引引用");
        orphans
    }
