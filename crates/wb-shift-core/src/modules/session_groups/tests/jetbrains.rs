    // -----------------------------------------------------------------------
    // 第 4 个客户端：JetBrains IDE 插件（IDEA / PyCharm 共用同一份数据目录）
    // -----------------------------------------------------------------------
    //
    // 真实布局：`<uid>/JetBrains/<projectKey>/history/<bucket-32hex>/<convId>/`——客户端段之后
    // 那一级是**项目 key**（项目路径的 base64url），不是账号 uid；`<bucket>` 是**安装级常量**
    // （本机所有项目 / 账号共用一个值）。没有打开项目时项目段退化成 `<uid>`（无项目桶）。
    // 这里钉住五件事：
    //
    // 1. 命名空间隔离：主表 / 目录 / 锁名独立，不与其它三个客户端串台；
    // 2. 会话发现：项目桶与无项目桶都能列出来，行里带 `projectKey`（`workspaceHash` 仍是 bucket）；
    // 3. 复制一律新 id（用户决策），且落到**目标账号该项目下已存在的 bucket**；
    // 4. 目标账号没有这个项目时整次拒绝，且绝不凭空创建目录；
    // 5. 同步计划：基线后新增消息 → `fastForward` + 可执行凭据（不写坏目标）。

    const JB_WS: &str = "0123456789abcdef0123456789abcdef";
    const JB_SRC_UID: &str = "uid-jb-src";
    const JB_DST_UID: &str = "uid-jb-dst";
    const JB_CONV: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const JB_MSG_1: &str = "11111111111111111111111111111111";
    const JB_MSG_2: &str = "22222222222222222222222222222222";
    /// 项目 key = `d:/Python基础练习` 的 base64url（无 padding），取自真机目录名。
    const JB_PROJECT_KEY: &str = "ZDovUHl0aG9u5Z+656GA57uD5Lmg";
    /// 目标账号侧该项目的会话桶（安装级常量，真机实测值）。
    const JB_TARGET_BUCKET: &str = "f6b91d610bcb56eaf9e6a28b52c30c2f";

    fn jb_paths(store: &TempStore) -> SessionPaths {
        SessionPaths::for_jetbrains_at(store.paths.store_root.clone())
    }

    /// 某账号某项目的会话历史根：`<uid>/JetBrains/<projectKey>/history`。
    fn jb_project_history(root: &Path, uid: &str, project_key: &str) -> PathBuf {
        vscode_session::scope_history_root_in(JETBRAINS_STORE, root, uid, project_key)
    }

    /// 在指定项目桶下写一个工作区索引（`conversations` 为 `(会话 id, 标题)`）。
    fn jb_write_project_index(
        root: &Path,
        uid: &str,
        project_key: &str,
        workspace: &str,
        items: &[(&str, &str)],
    ) {
        let ws_dir = jb_project_history(root, uid, project_key).join(workspace);
        std::fs::create_dir_all(&ws_dir).unwrap();
        let conversations: Vec<Value> = items
            .iter()
            .map(|(id, name)| json!({ "id": id, "type": "craft", "name": name }))
            .collect();
        std::fs::write(
            ws_dir.join("index.json"),
            json!({ "conversations": conversations }).to_string(),
        )
        .unwrap();
    }

    /// 在指定项目桶下造一条完整会话（`index.json` + `messages/<id>.json`）。
    fn jb_write_project_conversation(
        root: &Path,
        uid: &str,
        project_key: &str,
        workspace: &str,
        conversation: &str,
        messages: &[(&str, &str)],
    ) {
        let conv_dir = jb_project_history(root, uid, project_key)
            .join(workspace)
            .join(conversation);
        std::fs::create_dir_all(conv_dir.join("messages")).unwrap();
        let ids: Vec<Value> = messages.iter().map(|(id, _)| json!({ "id": id })).collect();
        std::fs::write(
            conv_dir.join("index.json"),
            json!({ "messages": ids, "requests": [] }).to_string(),
        )
        .unwrap();
        for (id, text) in messages {
            std::fs::write(
                conv_dir.join("messages").join(format!("{id}.json")),
                json!({ "id": id, "role": "user", "text": text }).to_string(),
            )
            .unwrap();
        }
    }

    /// 写一个工作区索引（`conversations` 为 `(会话 id, 标题)`）。
    fn jb_write_workspace_index(root: &Path, uid: &str, workspace: &str, items: &[(&str, &str)]) {
        let ws_dir = vscode_session::history_root_in(JETBRAINS_STORE, root, uid).join(workspace);
        std::fs::create_dir_all(&ws_dir).unwrap();
        let conversations: Vec<Value> = items
            .iter()
            .map(|(id, name)| json!({ "id": id, "type": "craft", "name": name }))
            .collect();
        std::fs::write(
            ws_dir.join("index.json"),
            json!({ "conversations": conversations }).to_string(),
        )
        .unwrap();
    }

    /// 造一条完整会话：会话 `index.json` + `messages/<id>.json`（内容摘要的硬前提）。
    fn jb_write_conversation(
        root: &Path,
        uid: &str,
        workspace: &str,
        conversation: &str,
        messages: &[(&str, &str)],
    ) {
        let conv_dir = vscode_session::history_root_in(JETBRAINS_STORE, root, uid)
            .join(workspace)
            .join(conversation);
        std::fs::create_dir_all(conv_dir.join("messages")).unwrap();
        let ids: Vec<Value> = messages
            .iter()
            .map(|(id, _)| json!({ "id": id }))
            .collect();
        std::fs::write(
            conv_dir.join("index.json"),
            json!({ "messages": ids, "requests": [] }).to_string(),
        )
        .unwrap();
        for (id, text) in messages {
            std::fs::write(
                conv_dir.join("messages").join(format!("{id}.json")),
                json!({ "id": id, "role": "user", "text": text }).to_string(),
            )
            .unwrap();
        }
    }

    /// 命名空间隔离：四个客户端的文件 / 目录 / 锁名两两不同。
    #[test]
    fn jetbrains_namespace_is_isolated_from_the_other_three_clients() {
        let store = TempStore::new("jb-namespace");
        let root = store.paths.store_root.clone();
        let paths = jb_paths(&store);
        assert!(paths
            .session_links_file()
            .to_string_lossy()
            .ends_with("jetbrains_session_links.json"));
        assert!(paths
            .session_links_dir()
            .to_string_lossy()
            .ends_with("jetbrains-session-links"));
        assert!(paths
            .client_ops_lock_file()
            .to_string_lossy()
            .ends_with("session-client-jetbrains.lock"));
        assert!(paths
            .link_store_lock_file()
            .to_string_lossy()
            .ends_with("jetbrains-session-links.lock"));

        let others = [
            SessionPaths::for_vscode_ext_at(root.clone()).session_links_file(),
            SessionPaths::for_codebuddy_ide_at(root.clone()).session_links_file(),
            SessionPaths {
                store_root: root.clone(),
                data_root: PathBuf::new(),
                auth_file: PathBuf::new(),
                link_namespace: crate::modules::session::LinkNamespace::WorkBuddy,
            }
            .session_links_file(),
        ];
        for other in others {
            assert_ne!(paths.session_links_file(), other, "主表文件名不得与他人共用");
        }
    }

    /// 客户端枚举与档位校验（JetBrains 与 VS Code 插件同组：不接受 cn / ai 档位）。
    #[test]
    fn jetbrains_client_enum_matches_the_plugin_contract() {
        assert_eq!(SessionClient::parse("jetbrains").unwrap(), SessionClient::Jetbrains);
        assert_eq!(SessionClient::Jetbrains.as_str(), "jetbrains");
        assert!(SessionClient::Jetbrains.validate_scope(None).is_ok());
        assert!(SessionClient::Jetbrains
            .validate_scope(Some(WbVariant::Ai))
            .is_err());
        assert!(SessionClient::Jetbrains.is_editor_plugin());
        assert!(!SessionClient::CodebuddyIde.is_editor_plugin());
        assert_eq!(SessionClient::Jetbrains.store_spec(), Some(JETBRAINS_STORE));
    }

    /// 会话发现：只读 `JetBrains` 段，同 uid 的 `VSCode` 段不串台；无数据返回空列表。
    #[test]
    fn jetbrains_sessions_are_discovered_and_isolated_from_the_vscode_tree() {
        let store = TempStore::new("jb-list");
        let root = store.index_root();
        jb_write_workspace_index(&root, JB_SRC_UID, JB_WS, &[(JB_CONV, "JetBrains 会话")]);
        // 同一个 uid 在 VS Code 段的另一条会话：绝不能出现在 JetBrains 结果里。
        store.write_workspace_index(VSCODE_STORE, JB_SRC_UID, JB_WS, &[(JB_MSG_1, "VS Code 会话")]);

        let listed = vscode_session::list_sessions_in_store(JETBRAINS_STORE, &root, JB_SRC_UID);
        let sessions = listed["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 1, "{listed}");
        assert_eq!(sessions[0]["id"], json!(JB_CONV));
        assert_eq!(sessions[0]["title"], json!("JetBrains 会话"));
        assert_eq!(sessions[0]["workspaceHash"], json!(JB_WS));
        assert_eq!(listed["dataRoot"], json!(root.to_string_lossy()));

        // 没有任何历史目录 → 空列表（而不是报错）：列表与「数据目录没找到」是两回事。
        let empty = vscode_session::list_sessions_in_store(JETBRAINS_STORE, &root, JB_DST_UID);
        assert!(empty["sessions"].as_array().unwrap().is_empty());
    }

    /// 项目桶与无项目桶都能被列出：两条同 bucket、不同 projectKey 的会话靠 `projectKey` 区分。
    #[test]
    fn jetbrains_project_buckets_are_listed_with_their_project_key() {
        let store = TempStore::new("jb-project-list");
        let root = store.index_root();
        jb_write_project_index(
            &root,
            JB_SRC_UID,
            JB_PROJECT_KEY,
            JB_WS,
            &[(JB_CONV, "项目里的会话")],
        );
        jb_write_workspace_index(&root, JB_SRC_UID, JB_WS, &[(JB_MSG_1, "无项目的会话")]);

        let listed = vscode_session::list_sessions_in_store(JETBRAINS_STORE, &root, JB_SRC_UID);
        let sessions = listed["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 2, "{listed}");
        let project_row = sessions
            .iter()
            .find(|row| row["id"] == json!(JB_CONV))
            .expect("项目桶里的会话必须被列出");
        assert_eq!(project_row["projectKey"], json!(JB_PROJECT_KEY));
        assert_eq!(
            project_row["projectPath"],
            json!("D:/Python基础练习"),
            "项目 key 必须能反解成项目路径（盘符统一大写）"
        );
        // `workspaceHash` 保持原义 = bucket（安装级常量），前端既有字段不用改。
        assert_eq!(project_row["workspaceHash"], json!(JB_WS));
        let legacy_row = sessions
            .iter()
            .find(|row| row["id"] == json!(JB_MSG_1))
            .expect("无项目桶里的会话必须被列出");
        // 无项目桶的项目段退化成 uid：它不是一个能解码出路径的项目 key。
        assert_eq!(legacy_row["projectKey"], json!(JB_SRC_UID));
        assert_eq!(legacy_row["projectPath"], json!(""));
    }

    /// 复制落到**目标账号该项目下已存在的 bucket**（bucket 是安装级常量，不能沿用源值）。
    #[test]
    fn jetbrains_copy_writes_into_the_target_project_bucket() {
        let store = TempStore::new("jb-project-copy");
        let paths = jb_paths(&store);
        let root = store.index_root();
        jb_write_project_index(
            &root,
            JB_SRC_UID,
            JB_PROJECT_KEY,
            JB_WS,
            &[(JB_CONV, "项目会话")],
        );
        jb_write_project_conversation(
            &root,
            JB_SRC_UID,
            JB_PROJECT_KEY,
            JB_WS,
            JB_CONV,
            &[(JB_MSG_1, "第一条")],
        );
        // 目标账号同一项目（项目 key 由项目路径决定，跨账号同值），bucket 用本机实际值。
        jb_write_project_index(&root, JB_DST_UID, JB_PROJECT_KEY, JB_TARGET_BUCKET, &[]);

        let items = [CopyItem {
            workspace_hash: JB_WS.to_string(),
            conversation_id: JB_CONV.to_string(),
            project_key: Some(JB_PROJECT_KEY.to_string()),
            store: None,
        }];
        let (report, link_errors) = plugin_copy_and_register(
            JETBRAINS_STORE,
            &root,
            &paths,
            WbVariant::Cn,
            JB_SRC_UID,
            JB_DST_UID,
            &items,
        )
        .unwrap();
        assert!(link_errors.is_empty(), "{link_errors:?}");
        assert_eq!(
            report["copied"][0]["projectKey"],
            json!(JB_PROJECT_KEY),
            "报告必须回传项目 key：{report}"
        );
        let new_id = report["copied"][0]["newId"].as_str().unwrap();

        let project_history = jb_project_history(&root, JB_DST_UID, JB_PROJECT_KEY);
        let target_bucket = project_history.join(JB_TARGET_BUCKET);
        assert!(
            target_bucket.join(new_id).is_dir(),
            "副本必须写进目标该项目下已存在的 bucket"
        );
        assert!(
            !project_history.join(JB_WS).exists(),
            "不得凭空创建源侧 bucket 目录"
        );
        assert!(
            !vscode_session::history_root_in(JETBRAINS_STORE, &root, JB_DST_UID)
                .join(JB_WS)
                .exists(),
            "绝不能写进「无项目」桶"
        );
        let index: Value = serde_json::from_str(
            &std::fs::read_to_string(target_bucket.join("index.json")).unwrap(),
        )
        .unwrap();
        assert!(
            index["conversations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["id"] == json!(new_id)),
            "副本必须登记进目标 bucket 的工作区索引：{index}"
        );
    }

    /// 目标账号在这个项目下没有会话：整次拒绝、指名项目、且不创建任何目录。
    #[test]
    fn jetbrains_copy_refuses_when_the_target_has_no_session_in_that_project() {
        let store = TempStore::new("jb-project-missing");
        let paths = jb_paths(&store);
        let root = store.index_root();
        jb_write_project_index(
            &root,
            JB_SRC_UID,
            JB_PROJECT_KEY,
            JB_WS,
            &[(JB_CONV, "项目会话")],
        );
        jb_write_project_conversation(
            &root,
            JB_SRC_UID,
            JB_PROJECT_KEY,
            JB_WS,
            JB_CONV,
            &[(JB_MSG_1, "第一条")],
        );

        let items = [CopyItem {
            workspace_hash: JB_WS.to_string(),
            conversation_id: JB_CONV.to_string(),
            project_key: Some(JB_PROJECT_KEY.to_string()),
            store: None,
        }];
        let error = plugin_copy_and_register(
            JETBRAINS_STORE,
            &root,
            &paths,
            WbVariant::Cn,
            JB_SRC_UID,
            JB_DST_UID,
            &items,
        )
        .expect_err("目标账号没有该项目时必须拒绝");
        assert!(error.contains("目标账号在这个项目下还没有会话"), "{error}");
        assert!(error.contains("先在 PyCharm 里打开该项目"), "{error}");
        assert!(
            error.contains("D:/Python基础练习"),
            "必须指出是哪个项目：{error}"
        );
        // 不凭空创建：项目目录与 bucket 目录都不得出现。
        assert!(!jb_project_history(&root, JB_DST_UID, JB_PROJECT_KEY).exists());
        assert!(
            !vscode_session::history_root_in(JETBRAINS_STORE, &root, JB_DST_UID)
                .join(JB_WS)
                .exists()
        );
    }

    /// 复制（一律新 id）+ 登记：源目录只读、目标索引合并保留既有会话、关联表建在同一命名空间。
    #[test]
    fn jetbrains_copy_uses_a_new_id_and_registers_in_its_own_namespace() {
        let store = TempStore::new("jb-copy");
        let paths = jb_paths(&store);
        let root = store.index_root();
        jb_write_workspace_index(&root, JB_SRC_UID, JB_WS, &[(JB_CONV, "要复制的会话")]);
        jb_write_conversation(&root, JB_SRC_UID, JB_WS, JB_CONV, &[(JB_MSG_1, "第一条")]);
        // 目标账号该工作区已有一条无关会话：复制后必须原样保留。
        jb_write_workspace_index(&root, JB_DST_UID, JB_WS, &[(JB_MSG_2, "目标已有会话")]);

        let items = [CopyItem {
            workspace_hash: JB_WS.to_string(),
            conversation_id: JB_CONV.to_string(),
            project_key: None,
            store: None,
        }];
        let (report, link_errors) = plugin_copy_and_register(
            JETBRAINS_STORE,
            &root,
            &paths,
            WbVariant::Cn,
            JB_SRC_UID,
            JB_DST_UID,
            &items,
        )
        .unwrap();
        assert!(link_errors.is_empty(), "{link_errors:?}");

        let copied = report["copied"].as_array().unwrap();
        assert_eq!(copied.len(), 1, "{report}");
        let new_id = copied[0]["newId"].as_str().unwrap();
        assert_ne!(new_id, JB_CONV, "插件侧复制必须取新 id");
        assert_eq!(new_id.len(), 32);

        // 目标工作区索引：既有会话 + 新副本（标题沿用源标题）。
        let target_ws = vscode_session::history_root_in(JETBRAINS_STORE, &root, JB_DST_UID).join(JB_WS);
        let index: Value =
            serde_json::from_str(&std::fs::read_to_string(target_ws.join("index.json")).unwrap())
                .unwrap();
        let ids: Vec<&str> = index["conversations"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["id"].as_str())
            .collect();
        assert!(ids.contains(&JB_MSG_2), "既有会话不得被覆盖：{ids:?}");
        assert!(ids.contains(&new_id), "新副本必须登记进工作区索引：{ids:?}");

        // 源会话目录依然存在（只读，不搬移）。
        assert!(vscode_session::history_root_in(JETBRAINS_STORE, &root, JB_SRC_UID)
            .join(JB_WS)
            .join(JB_CONV)
            .is_dir());

        // 关联表落在 JetBrains 自己的文件里（不与 VS Code 插件串台）。
        let link_store = store.store_at(&paths);
        assert_eq!(link_store.groups.len(), 1, "复制后应建立 1 个关联组");
        let group = &link_store.groups[0];
        assert_eq!(group.members.len(), 2);
        assert!(paths.session_links_file().exists());
        assert!(!SessionPaths::for_vscode_ext_at(store.paths.store_root.clone())
            .session_links_file()
            .exists());
    }

    /// 同步计划：基线后源端新增消息 → `fastForward` 且带可执行凭据。
    #[test]
    fn jetbrains_sync_plan_is_generated_after_a_new_source_message() {
        let store = TempStore::new("jb-sync-plan");
        let paths = jb_paths(&store);
        let root = store.index_root();
        jb_write_workspace_index(&root, JB_SRC_UID, JB_WS, &[(JB_CONV, "同步计划")]);
        jb_write_conversation(&root, JB_SRC_UID, JB_WS, JB_CONV, &[(JB_MSG_1, "第一条")]);
        // 目标账号在这个项目下已有会话（bucket 是安装级常量，只能沿用不能新建）。
        jb_write_workspace_index(&root, JB_DST_UID, JB_WS, &[]);

        let items = [CopyItem {
            workspace_hash: JB_WS.to_string(),
            conversation_id: JB_CONV.to_string(),
            project_key: None,
            store: None,
        }];
        let (report, link_errors) = plugin_copy_and_register(
            JETBRAINS_STORE,
            &root,
            &paths,
            WbVariant::Cn,
            JB_SRC_UID,
            JB_DST_UID,
            &items,
        )
        .unwrap();
        assert!(link_errors.is_empty(), "{link_errors:?}");
        let target_conv = report["copied"][0]["newId"].as_str().unwrap().to_string();

        let target_account = json!({ "id": "acc-dst", "uid": JB_DST_UID, "variant": "cn" });

        // 复制刚完成：两侧一致 → identical（无待同步内容）。
        let same = vscode_session_sync::links_preview_in(
            JETBRAINS_STORE,
            &root,
            &paths,
            JB_SRC_UID,
            &target_account,
        )
        .unwrap();
        let group_preview = &same["groups"][0];
        assert_eq!(group_preview["verdict"], json!("identical"), "{same}");

        // 源端追加一条消息 → 计划变成 fastForward 并给出凭据。
        jb_write_conversation(
            &root,
            JB_SRC_UID,
            JB_WS,
            JB_CONV,
            &[(JB_MSG_1, "第一条"), (JB_MSG_2, "第二条")],
        );
        let ahead = vscode_session_sync::links_preview_in(
            JETBRAINS_STORE,
            &root,
            &paths,
            JB_SRC_UID,
            &target_account,
        )
        .unwrap();
        let item = &ahead["groups"][0];
        assert_eq!(item["verdict"], json!("fastForward"), "{ahead}");
        assert!(
            item["previewToken"].as_str().is_some_and(|t| !t.is_empty()),
            "必须返回可执行凭据：{ahead}"
        );
        // 计划里指名的目标会话就是刚才复制出来的那份（新 id）。
        assert_eq!(item["target"]["sessionId"], json!(target_conv));
    }

    /// 关闭决策（纯函数）：未运行不关不拉；运行中 + 未授权 → 报错；运行中 + 授权 → 关闭。
    #[test]
    fn jetbrains_restart_decision_blocks_manual_mode_while_running() {
        let hint = JETBRAINS_SYNC_RUNNING_HINT;
        assert!(!plan_ide_restart(false, false, hint).unwrap());
        assert!(!plan_ide_restart(false, true, hint).unwrap());
        assert!(plan_ide_restart(true, true, hint).unwrap());
        assert_eq!(
            plan_ide_restart(true, false, hint).unwrap_err(),
            JETBRAINS_SYNC_RUNNING_HINT
        );
        // 文案必须指名 JetBrains（不能复述 VS Code），否则用户会去关错客户端。
        assert!(JETBRAINS_SYNC_RUNNING_HINT.contains("JetBrains"));
        assert!(!JETBRAINS_SYNC_RUNNING_HINT.contains("VS Code"));
        assert!(jetbrains_restart_precheck(&[]).get("required").is_some());
    }

    // -----------------------------------------------------------------------
    // 存活与清理（10.6 第 6 条验收口）
    // -----------------------------------------------------------------------
    //
    // 口径必须与 VS Code 插件一致：索引条目命中 **或** 会话目录还在 → Alive；
    // 两边都没有 → Gone（可清理）；索引读不了 → 展示但拒绝清理，并说清是哪个账号。

    /// 索引命中 / 目录证据 → 都不清理；两者皆无 → 清理该组。
    #[test]
    fn jetbrains_pruning_keeps_alive_groups_and_drops_dead_ones() {
        let store = TempStore::new("jb-prune");
        let paths = jb_paths(&store);
        let root = store.index_root();

        // 索引里能查到 representative 会话（`session-<uid>` 由 stored_member 生成）。
        store.write_workspace_index(
            JETBRAINS_STORE,
            "uid-jb-index",
            JB_WS,
            &[("session-uid-jb-index", "索引命中")],
        );
        // 索引里没有条目，但会话目录还在（半截写入现场）→ 也算活着。
        store.write_workspace_index(JETBRAINS_STORE, "uid-jb-dir", JB_WS, &[]);
        store.create_session_dir(JETBRAINS_STORE, "uid-jb-dir", JB_WS, "session-uid-jb-dir");
        // 索引可读、条目与目录都没有 → 可信消失。
        store.write_workspace_index(JETBRAINS_STORE, "uid-jb-gone", JB_WS, &[]);

        store.seed_at(
            &paths,
            vec![
                group(
                    "g-index",
                    WbVariant::Cn,
                    vec![stored_member("m-index", "uid-jb-index", MemberState::Active)],
                    Vec::new(),
                ),
                group(
                    "g-dir",
                    WbVariant::Cn,
                    vec![stored_member("m-dir", "uid-jb-dir", MemberState::Active)],
                    Vec::new(),
                ),
                group(
                    "g-gone",
                    WbVariant::Cn,
                    vec![stored_member("m-gone", "uid-jb-gone", MemberState::Active)],
                    Vec::new(),
                ),
            ],
        );

        let report = prune_at_paths(
            &store,
            &paths,
            SessionClient::Jetbrains,
            None,
            false,
            Some(root),
        )
        .unwrap();
        assert_eq!(report["groupIds"], json!(["g-gone"]));

        let ids: Vec<String> = store
            .store_at(&paths)
            .groups
            .iter()
            .map(|item| item.id.clone())
            .collect();
        assert_eq!(ids, vec!["g-index", "g-dir"], "索引命中与目录证据都不得清理");
    }

    /// 索引损坏（读不到条目）→ 拒绝清理，且原因里必须出现 JetBrains 与账号 uid。
    #[test]
    fn jetbrains_pruning_refuses_a_corrupt_index_with_client_scoped_reason() {
        let store = TempStore::new("jb-prune-broken");
        let paths = jb_paths(&store);
        let root = store.index_root();
        store.write_broken_workspace_index(JETBRAINS_STORE, "uid-jb-broken", JB_WS);
        store.seed_at(
            &paths,
            vec![group(
                "g-broken",
                WbVariant::Cn,
                vec![stored_member("m-broken", "uid-jb-broken", MemberState::Active)],
                Vec::new(),
            )],
        );

        let error = prune_at_paths(
            &store,
            &paths,
            SessionClient::Jetbrains,
            None,
            false,
            Some(root),
        )
        .expect_err("索引损坏时必须拒绝清理");
        assert!(error.contains("JetBrains 插件"), "必须指出是哪个客户端：{error}");
        assert!(error.contains("uid-jb-broken"), "必须指出是哪个账号：{error}");
        assert!(error.contains("无法判定"), "{error}");
        assert_eq!(
            store.store_at(&paths).groups.len(),
            1,
            "拒绝清理时不得改动关联表"
        );
    }

    /// 数据根解析不到 → 同样拒绝，文案要求用户先去打开 IDEA / PyCharm。
    #[test]
    fn jetbrains_pruning_without_data_root_asks_user_to_open_the_ide() {
        let store = TempStore::new("jb-prune-no-root");
        let paths = jb_paths(&store);
        store.seed_at(
            &paths,
            vec![group(
                "g-x",
                WbVariant::Cn,
                vec![stored_member("m-x", "uid-x", MemberState::Active)],
                Vec::new(),
            )],
        );

        let error = prune_at_paths(&store, &paths, SessionClient::Jetbrains, None, false, None)
            .expect_err("没有数据根时必须拒绝清理");
        assert!(error.contains("IDEA / PyCharm"), "{error}");
        assert!(!error.contains("VS Code"), "不能把用户指到 VS Code：{error}");
    }

    // -----------------------------------------------------------------------
    // 同步执行（10.6 第 4 条验收口）
    // -----------------------------------------------------------------------

    /// 复制 → 源端新增消息 → 按计划执行：目标会话消息补齐 2 条，正文与来源一致。
    #[test]
    fn jetbrains_sync_writes_the_new_messages_into_the_target_conversation() {
        let store = TempStore::new("jb-sync-exec");
        let paths = jb_paths(&store);
        let root = store.index_root();
        jb_write_workspace_index(&root, JB_SRC_UID, JB_WS, &[(JB_CONV, "执行同步")]);
        jb_write_conversation(&root, JB_SRC_UID, JB_WS, JB_CONV, &[(JB_MSG_1, "第一条")]);
        // 目标账号在这个项目下已有会话（bucket 是安装级常量，只能沿用不能新建）。
        jb_write_workspace_index(&root, JB_DST_UID, JB_WS, &[]);

        let items = [CopyItem {
            workspace_hash: JB_WS.to_string(),
            conversation_id: JB_CONV.to_string(),
            project_key: None,
            store: None,
        }];
        let (report, link_errors) = plugin_copy_and_register(
            JETBRAINS_STORE,
            &root,
            &paths,
            WbVariant::Cn,
            JB_SRC_UID,
            JB_DST_UID,
            &items,
        )
        .unwrap();
        assert!(link_errors.is_empty(), "{link_errors:?}");
        let target_conv = report["copied"][0]["newId"].as_str().unwrap().to_string();

        // 源端新增一条消息 → 预览给出可执行凭据。
        jb_write_conversation(
            &root,
            JB_SRC_UID,
            JB_WS,
            JB_CONV,
            &[(JB_MSG_1, "第一条"), (JB_MSG_2, "第二条")],
        );
        let target_account = json!({ "id": "acc-dst", "uid": JB_DST_UID, "variant": "cn" });
        let preview = vscode_session_sync::links_preview_in(
            JETBRAINS_STORE,
            &root,
            &paths,
            JB_SRC_UID,
            &target_account,
        )
        .unwrap();
        let item = &preview["groups"][0];
        let group_id = item["groupId"].as_str().unwrap().to_string();
        let token = item["previewToken"].as_str().unwrap().to_string();

        let executed = vscode_session_sync::sync_selected_in(
            JETBRAINS_STORE,
            &root,
            &paths,
            JB_SRC_UID,
            &target_account,
            &[SyncSelection {
                group_id,
                preview_token: token,
                mode: SyncMode::FastForward,
            }],
        )
        .unwrap();
        assert_eq!(executed["synced"].as_array().unwrap().len(), 1, "{executed}");

        // 目标会话正文：消息文件补齐到 2 条，且内容与来源一致。
        // 注意插件侧复制会为新副本重新派生消息 id，所以按目标索引里的 id 去读文件。
        let target_dir =
            vscode_session::history_root_in(JETBRAINS_STORE, &root, JB_DST_UID)
                .join(JB_WS)
                .join(&target_conv);
        let mut files: Vec<String> = std::fs::read_dir(target_dir.join("messages"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert_eq!(files.len(), 2, "目标会话应补齐两条消息：{files:?}");

        let target_index: Value =
            serde_json::from_str(&std::fs::read_to_string(target_dir.join("index.json")).unwrap())
                .unwrap();
        let ids: Vec<String> = target_index["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|message| message["id"].as_str().map(str::to_string))
            .collect();
        assert_eq!(ids.len(), 2, "索引里应有 2 条消息：{target_index}");
        let mut texts: Vec<String> = ids
            .iter()
            .map(|id| {
                std::fs::read_to_string(target_dir.join("messages").join(format!("{id}.json")))
                    .unwrap_or_else(|error| panic!("{id} 的消息文件读不到：{error}"))
            })
            .collect();
        texts.sort();
        assert!(
            texts.iter().any(|text| text.contains("第一条"))
                && texts.iter().any(|text| text.contains("第二条")),
            "目标会话必须包含源端两条消息：{texts:?}"
        );

        // 再预览一次：已一致，不再提供可执行凭据（同步是幂等的）。
        let settled = vscode_session_sync::links_preview_in(
            JETBRAINS_STORE,
            &root,
            &paths,
            JB_SRC_UID,
            &target_account,
        )
        .unwrap();
        assert_eq!(settled["groups"][0]["verdict"], json!("identical"), "{settled}");
    }

    /// 索引里登记了消息但 `messages/<id>.json` 缺失 → 明确报错，不静默丢内容。
    #[test]
    fn jetbrains_sync_reports_a_missing_message_file() {
        let store = TempStore::new("jb-sync-missing-file");
        let paths = jb_paths(&store);
        let root = store.index_root();
        jb_write_workspace_index(&root, JB_SRC_UID, JB_WS, &[(JB_CONV, "缺文件")]);
        jb_write_conversation(&root, JB_SRC_UID, JB_WS, JB_CONV, &[(JB_MSG_1, "第一条")]);
        // 目标账号在这个项目下已有会话（bucket 是安装级常量，只能沿用不能新建）。
        jb_write_workspace_index(&root, JB_DST_UID, JB_WS, &[]);

        let items = [CopyItem {
            workspace_hash: JB_WS.to_string(),
            conversation_id: JB_CONV.to_string(),
            project_key: None,
            store: None,
        }];
        plugin_copy_and_register(
            JETBRAINS_STORE,
            &root,
            &paths,
            WbVariant::Cn,
            JB_SRC_UID,
            JB_DST_UID,
            &items,
        )
        .unwrap();

        // 源端索引新增一条消息，但正文文件没落盘（半截写入现场）。
        let src_dir = vscode_session::history_root_in(JETBRAINS_STORE, &root, JB_SRC_UID)
            .join(JB_WS)
            .join(JB_CONV);
        std::fs::write(
            src_dir.join("index.json"),
            json!({
                "messages": [{"id": JB_MSG_1}, {"id": JB_MSG_2}],
                "requests": []
            })
            .to_string(),
        )
        .unwrap();

        let target_account = json!({ "id": "acc-dst", "uid": JB_DST_UID, "variant": "cn" });
        let preview = vscode_session_sync::links_preview_in(
            JETBRAINS_STORE,
            &root,
            &paths,
            JB_SRC_UID,
            &target_account,
        )
        .unwrap();
        let item = &preview["groups"][0];
        // 内容不可摘要 → 不可勾选、不发凭据（不能拿坏数据去写目标）。
        assert_eq!(item["availableModes"], json!([]), "{preview}");
        assert!(
            item.get("previewToken").is_none() || item["previewToken"].is_null(),
            "内容不可用时不得发凭据：{preview}"
        );
        let reason = item["reason"].as_str().unwrap_or_default();
        assert!(
            reason.contains("不存在") || reason.contains("不完整") || reason.contains("无法"),
            "必须给出可读原因：{preview}"
        );
    }

