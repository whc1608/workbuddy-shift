    fn record(
        id: &str,
        uid: Option<&str>,
        nickname: &str,
        email: Option<&str>,
        token: bool,
    ) -> Value {
        json!({
            "id": id,
            "uid": uid,
            "nickname": nickname,
            "email": email,
            "access_token": if token { format!("token-{id}") } else { String::new() },
            "createdAt": 1,
        })
    }

    #[test]
    fn parse_rejects_empty_invalid_and_non_array() {
        assert!(parse_accounts_json("").is_err());
        assert!(parse_accounts_json("not json").is_err());
        assert!(parse_accounts_json(r#"{ "a": 1 }"#).is_err());
    }

    #[test]
    fn parse_rejects_non_object_element() {
        let err = parse_accounts_json(r#"[{ "uid": "u1" }, 42]"#).unwrap_err();
        assert!(err.contains("第 2 项"), "错误应带位置：{err}");
    }

    #[test]
    fn parse_accepts_object_array() {
        let parsed = parse_accounts_json(r#"[{ "uid": "u1" }, {}]"#).unwrap();
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn merge_overwrites_same_uid_preserving_imported_record() {
        let mut accounts = vec![record("local", Some("u1"), "旧名称", None, true)];
        let text = r#"[{ "id": "imported", "uid": "u1", "nickname": "新名称", "access_token": "tok-new" }]"#;
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();
        assert_eq!(result.imported, 1);
        assert_eq!(result.overwritten, 1);
        assert_eq!(result.skipped, 0);
        assert_eq!(accounts.len(), 1, "同 uid 应覆盖而不是新增");
        assert_eq!(accounts[0]["id"], "imported", "覆盖保留导入记录原样");
        assert_eq!(accounts[0]["nickname"], "新名称");
        assert_eq!(accounts[0]["access_token"], "tok-new");
    }

    #[test]
    fn merge_preserves_local_id_when_imported_record_has_none() {
        let mut accounts = vec![record("local-id", Some("u1"), "旧名称", None, true)];
        let text = r#"[{ "uid": "u1", "nickname": "新名称", "access_token": "tok" }]"#;
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();
        assert_eq!(result.imported, 1);
        assert_eq!(result.overwritten, 1);
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0]["id"], "local-id", "导入记录缺 id 时保留本地 id");
        assert_eq!(accounts[0]["nickname"], "新名称");
        assert_eq!(accounts[0]["access_token"], "tok");
    }

    /// R5-04：导入**旧备份**（不含 `mark`）不得冲掉本地标记。
    ///
    /// 覆盖分支会用导入记录整条替换本地记录，所以 `mark` 必须加进保留列表，
    /// 与 `note` / `displayField` 同一处理。
    #[test]
    fn merge_keeps_local_mark_when_backup_has_none() {
        let mut local = record("local", Some("u1"), "旧名称", None, true);
        local["note"] = json!("设计组-主号");
        local["mark"] = json!({
            "text": "即将限额",
            "color": "warning",
            "createdAt": 1_763_654_321_000_i64,
            "expiresAt": null,
        });
        let mut accounts = vec![local];

        let text = r#"[{ "id": "local", "uid": "u1", "nickname": "新名称", "access_token": "tok-new" }]"#;
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();

        assert_eq!(result.overwritten, 1);
        assert_eq!(accounts[0]["mark"]["text"], "即将限额", "旧备份缺 mark 时保留本地标记");
        assert_eq!(accounts[0]["mark"]["color"], "warning");
        assert_eq!(accounts[0]["note"], "设计组-主号", "note 同样保留");
        assert_eq!(accounts[0]["nickname"], "新名称", "官方字段仍以导入记录为准");
    }

    /// R5-04：备份自带 `mark` 时以备份为准（导入即恢复用户当时的展示配置）。
    #[test]
    fn merge_restores_mark_from_backup() {
        let mut local = record("local", Some("u1"), "旧名称", None, true);
        local["mark"] = json!({"text": "暂不使用", "color": "neutral", "createdAt": 1, "expiresAt": null});
        let mut accounts = vec![local];

        let text = r#"[{ "id": "local", "uid": "u1", "nickname": "新名称", "access_token": "tok-new",
            "mark": { "text": "积分将过期", "color": "danger", "createdAt": 2, "expiresAt": 1800000000000 } }]"#;
        merge_import_records(&mut accounts, text, &[0]).unwrap();

        assert_eq!(accounts[0]["mark"]["text"], "积分将过期");
        assert_eq!(accounts[0]["mark"]["color"], "danger");
        assert_eq!(accounts[0]["mark"]["expiresAt"], json!(1_800_000_000_000_i64));
    }

    /// issue #184「最近切换」：导入**旧备份**（不含 `lastUsedAt` / `lastUsedClient`）不得抹掉
    /// 本机记录。
    ///
    /// 覆盖分支用导入记录整条替换本地记录，所以这两个键必须在 `LOCAL_ACCOUNT_FIELDS` 里；
    /// 二者要**成对**保留——前端按「时间戳缺失 = 从未记录」判断，只留下来源端文案会让卡片
    /// 显示不出徽标，用户以为这次切换没生效。
    #[test]
    fn merge_keeps_local_last_used_pair_from_old_backup() {
        let mut local = record("local", Some("u1"), "旧名称", None, true);
        local["lastUsedAt"] = json!(1_763_654_321_000_i64);
        local["lastUsedClient"] = json!("vscode");
        let mut accounts = vec![local];

        let text =
            r#"[{ "id": "local", "uid": "u1", "nickname": "新名称", "access_token": "tok-new" }]"#;
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();

        assert_eq!(result.overwritten, 1);
        assert_eq!(
            accounts[0]["lastUsedAt"],
            json!(1_763_654_321_000_i64),
            "旧备份缺 lastUsedAt 时保留本机时间戳"
        );
        assert_eq!(
            accounts[0]["lastUsedClient"], "vscode",
            "旧备份缺 lastUsedClient 时保留本机来源端"
        );
        assert_eq!(accounts[0]["nickname"], "新名称", "官方字段仍以备份为准");
    }

    /// 备份自带「最近切换」记录时以备份为准（导入即恢复用户当时的记录，与 `mark` 同口径）。
    #[test]
    fn merge_restores_last_used_from_backup() {
        let mut local = record("local", Some("u1"), "旧名称", None, true);
        local["lastUsedAt"] = json!(1_763_654_321_000_i64);
        local["lastUsedClient"] = json!("vscode");
        let mut accounts = vec![local];

        let text = r#"[{ "id": "local", "uid": "u1", "nickname": "新名称", "access_token": "tok-new",
            "lastUsedAt": 1763700000000, "lastUsedClient": "jetbrains" }]"#;
        merge_import_records(&mut accounts, text, &[0]).unwrap();

        assert_eq!(accounts[0]["lastUsedAt"], json!(1_763_700_000_000_i64));
        assert_eq!(accounts[0]["lastUsedClient"], "jetbrains");
    }

    // -----------------------------------------------------------------------
    // 本地字段白名单不变量（`account::LOCAL_ACCOUNT_FIELDS`）
    //
    // 导入路径是第三处会覆盖整条记录的写回，必须与「刷新 Token」「扫码重登」保持同一份白名单。
    // 这里按白名单逐个字段跑：新增字段却没给样例值时会直接失败并报出字段名。
    // -----------------------------------------------------------------------

    fn local_field_sample(key: &str) -> Value {
        match key {
            "note" => json!("工作号"),
            "displayField" => json!("phone"),
            "mark" => json!({
                "text": "即将限额",
                "color": "warning",
                "createdAt": 1_763_654_321_000_i64,
                "expiresAt": null,
            }),
            // issue #184「最近切换」：官方资料与旧备份里都没有这两个键，必须整对保留，
            // 否则每导入一次旧备份，卡片上的「最近切换」徽标就会集体消失。
            "lastUsedAt" => json!(1_763_654_321_000_i64),
            "lastUsedClient" => json!("codebuddy-ide"),
            // R6-01：导入路径**不**继承本机来源（备份没有等价信息），见
            // `merge_marks_missing_credential_source_as_import`；这里保留样例值只为
            // 让「白名单新增字段必须补样例」这条不变量继续成立。
            "credentialSource" => json!("local"),
            other => panic!(
                "LOCAL_ACCOUNT_FIELDS 新增了 `{other}`，请在导入侧不变量测试里补样例值，\
                 并确认旧备份导入不会覆盖它"
            ),
        }
    }

    #[test]
    fn merge_keeps_every_local_account_field_from_old_backup() {
        for key in account::LOCAL_ACCOUNT_FIELDS {
            // `credentialSource` 是**有意**的例外：备份里没有等价信息（另一台机器的来源
            // 不可知），按 R6-01 的口径统一补 `import`，不做本机值继承。
            // 该字段的导入语义由下面两条专门用例钉住。
            if key == account::CREDENTIAL_SOURCE_FIELD {
                continue;
            }
            let mut local = record("local", Some("u1"), "旧名称", None, true);
            local[key] = local_field_sample(key);
            let mut accounts = vec![local];

            // 旧备份只有官方字段 + token。
            let text =
                r#"[{ "id": "local", "uid": "u1", "nickname": "新名称", "access_token": "tok-new" }]"#;
            merge_import_records(&mut accounts, text, &[0]).unwrap();

            assert_eq!(
                accounts[0][key],
                local_field_sample(key),
                "导入旧备份不得覆盖本地字段 `{key}`"
            );
            assert_eq!(accounts[0]["nickname"], "新名称", "官方字段仍以备份为准");
        }
    }

    /// R6-01：备份没有 `credentialSource`（旧备份 / 第三方号池）时，导入落 `import`。
    ///
    /// 保守档的理由写在 `account::CredentialSource`：判成 `oauth` 会让本工具用导入的旧
    /// refresh token 去刷，把另一台机器上的客户端顶掉；判成 `import` 最坏只是「明文过期后
    /// 不再自动刷新、提示重扫」，可恢复。
    #[test]
    fn merge_marks_missing_credential_source_as_import() {
        // 追加分支（跨机导入的新账号）。
        let mut accounts: Vec<Value> = vec![];
        let text = r#"[{ "id": "n1", "uid": "u9", "access_token": "tok" }]"#;
        merge_import_records(&mut accounts, text, &[0]).unwrap();
        assert_eq!(accounts[0]["credentialSource"], "import");

        // 覆盖分支（同 uid 覆盖本地记录）：同样落 `import`，不继承本机来源。
        let mut accounts = vec![record("local", Some("u1"), "旧名称", None, true)];
        accounts[0]["credentialSource"] = json!("oauth");
        let text = r#"[{ "id": "local", "uid": "u1", "access_token": "tok-new" }]"#;
        merge_import_records(&mut accounts, text, &[0]).unwrap();
        assert_eq!(
            accounts[0]["credentialSource"], "import",
            "备份没带来源时一律按 import（不猜）"
        );
    }

    /// R6-01：备份自带 `credentialSource` 时原样保留（导出/导入往返不改口径）。
    #[test]
    fn merge_keeps_credential_source_from_backup() {
        let mut accounts = vec![record("local", Some("u1"), "旧名称", None, true)];
        accounts[0]["credentialSource"] = json!("local");
        let text = r#"[{ "id": "local", "uid": "u1", "access_token": "tok-new",
            "credentialSource": "oauth" }]"#;
        merge_import_records(&mut accounts, text, &[0]).unwrap();
        assert_eq!(accounts[0]["credentialSource"], "oauth");
    }

    #[test]
    fn merge_appends_when_uid_missing_or_unmatched() {
        let mut accounts = vec![record("local", Some("u1"), "甲", None, true)];
        let text = r#"[
            { "id": "n1", "nickname": "无uid", "access_token": "t1" },
            { "id": "n2", "uid": "u-other", "nickname": "uid不匹配", "access_token": "t2" }
        ]"#;
        let result = merge_import_records(&mut accounts, text, &[0, 1]).unwrap();
        assert_eq!(result.imported, 2);
        assert_eq!(result.overwritten, 0);
        assert_eq!(accounts.len(), 3);
    }

    #[test]
    fn merge_skips_missing_token() {
        let mut accounts: Vec<Value> = vec![];
        let text = r#"[{ "id": "no-token", "uid": "u9", "nickname": "缺token" }]"#;
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();
        assert_eq!(result.imported, 0);
        assert_eq!(result.skipped, 1);
        assert!(accounts.is_empty(), "缺 token 的记录不得进入账号库");
    }

    /// 回归 issue #100：加密信封凭据按采集口径放行，且导入后原样保留（不落空串）。
    #[test]
    fn merge_imports_envelope_token_as_is() {
        let mut accounts: Vec<Value> = vec![];
        let text = r#"[{
            "id": "env-1",
            "uid": "u-env",
            "nickname": {"$wbEncrypted": 1, "envelope": "nick"},
            "access_token": {"$wbEncrypted": 1, "envelope": "a"},
            "refresh_token": {"$wbEncrypted": 1, "envelope": "r"}
        }]"#;
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();

        assert_eq!(result.imported, 1, "信封凭据应计入 imported");
        assert_eq!(result.skipped, 0);
        assert_eq!(accounts.len(), 1);
        assert_eq!(
            accounts[0]["access_token"],
            json!({"$wbEncrypted": 1, "envelope": "a"}),
            "信封凭据必须原样保留"
        );
        assert_eq!(accounts[0]["refresh_token"]["envelope"], "r");
    }

    /// 空串与普通对象（非信封）仍按无凭据处理：预览「缺少 token」、导入计 skipped。
    #[test]
    fn merge_still_skips_blank_and_plain_object_token() {
        let mut accounts: Vec<Value> = vec![];
        let text = r#"[
            { "id": "blank", "uid": "u-blank", "access_token": "   " },
            { "id": "object", "uid": "u-object", "access_token": { "token": "t" } }
        ]"#;
        let result = merge_import_records(&mut accounts, text, &[0, 1]).unwrap();

        assert_eq!(result.imported, 0);
        assert_eq!(result.skipped, 2);
        assert!(accounts.is_empty(), "空串 / 普通对象不得进入账号库");
    }

    #[test]
    fn merge_counts_out_of_range_index_as_skipped() {
        let mut accounts: Vec<Value> = vec![];
        let result =
            merge_import_records(&mut accounts, r#"[{ "access_token": "t" }]"#, &[5]).unwrap();
        assert_eq!(result.imported, 0);
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn merge_deduplicates_within_file() {
        let mut accounts: Vec<Value> = vec![];
        let text = r#"[
            { "id": "f1", "uid": "u9", "access_token": "t1" },
            { "id": "f2", "uid": "u9", "access_token": "t2" }
        ]"#;
        let result = merge_import_records(&mut accounts, text, &[0, 1]).unwrap();
        assert_eq!(result.imported, 2);
        assert_eq!(result.overwritten, 1);
        assert_eq!(accounts.len(), 1, "文件内重复 uid 也不得产生重复账号");
        assert_eq!(accounts[0]["id"], "f2");
    }

    /// 回归 issue #111：导入记录缺 id 时必须补一个，否则刷新回写会反复追加副本。
    #[test]
    fn merge_generates_id_for_imported_record_without_id() {
        let mut accounts: Vec<Value> = vec![];
        let text = r#"[{ "uid": "u-new", "nickname": "无id", "access_token": "t1" }]"#;
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();

        assert_eq!(result.imported, 1);
        assert_eq!(accounts.len(), 1);
        assert!(
            accounts[0]["id"]
                .as_str()
                .is_some_and(|id| !id.trim().is_empty()),
            "追加入库的记录必须带 id"
        );
    }

    /// 本地同 uid 记录也是无 id 的历史脏数据时，覆盖后同样要补 id。
    #[test]
    fn merge_generates_id_when_neither_imported_nor_local_record_has_one() {
        let mut accounts = vec![json!({
            "uid": "u1",
            "nickname": "旧名称",
            "access_token": "t-old",
        })];
        let text = r#"[{ "uid": "u1", "nickname": "新名称", "access_token": "t-new" }]"#;
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();

        assert_eq!(result.overwritten, 1);
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0]["nickname"], "新名称");
        assert!(
            accounts[0]["id"]
                .as_str()
                .is_some_and(|id| !id.trim().is_empty()),
            "本地也无 id 时覆盖后必须补 id"
        );
    }

    #[test]
    fn merge_repairs_empty_local_id_when_imported_record_has_no_id() {
        let mut accounts = vec![json!({
            "id": "  ",
            "uid": "u1",
            "access_token": "t-old",
        })];
        let text = r#"[{ "uid": "u1", "access_token": "t-new" }]"#;

        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();

        assert_eq!(result.overwritten, 1);
        assert_eq!(accounts.len(), 1);
        assert!(account::get_str(&accounts[0], "id").is_some());
        assert_eq!(accounts[0]["access_token"], "t-new");
    }

    #[test]
    fn export_requires_selection() {
        let accounts = vec![record("a1", Some("u1"), "甲", None, true)];
        assert!(select_export_records(&accounts, &[]).is_err());
        assert!(select_export_records(&accounts, &["missing".to_string()]).is_err());
    }

    #[test]
    fn export_returns_full_records_with_tokens() {
        let accounts = vec![
            record("a1", Some("u1"), "甲", Some("a@b.c"), true),
            record("a2", Some("u2"), "乙", None, true),
        ];
        let exported =
            select_export_records(&accounts, &["a1".to_string(), "missing".to_string()]).unwrap();
        assert_eq!(exported.len(), 1);
        assert_eq!(exported[0]["id"], "a1");
        assert_eq!(exported[0]["access_token"], "token-a1");
    }

    #[test]
    fn exported_records_roundtrip_import() {
        let accounts = vec![json!({
            "id": "a1",
            "uid": "u1",
            "nickname": "甲",
            "email": "a@b.c",
            "access_token": "tok-1",
            "refresh_token": "ref-1",
            "auth_raw": { "k": "v" },
            "createdAt": 1,
        })];
        let exported = select_export_records(&accounts, &["a1".to_string()]).unwrap();
        let text = serde_json::to_string(&exported).unwrap();
        let mut target: Vec<Value> = vec![];
        let result = merge_import_records(&mut target, &text, &[0]).unwrap();
        assert_eq!(result.imported, 1);
        assert_eq!(target.len(), 1);
        assert_eq!(
            target[0]["access_token"], "tok-1",
            "round-trip 保留 access_token"
        );
        assert_eq!(target[0]["refresh_token"], "ref-1");
        assert_eq!(target[0]["auth_raw"]["k"], "v");
    }

    #[test]
    fn preview_exposes_only_desensitized_fields() {
        let text = r#"[{
            "id": "a1",
            "uid": "u1",
            "nickname": "小明",
            "email": "x@y.z",
            "access_token": "SECRET"
        }]"#;
        let preview = preview_accounts(text).unwrap();
        assert_eq!(preview["total"], 1);
        assert_eq!(preview["accounts"][0]["index"], 0);
        assert_eq!(preview["accounts"][0]["uid"], "u1");
        assert_eq!(preview["accounts"][0]["nickname"], "小明");
        assert_eq!(preview["accounts"][0]["email"], "x@y.z");
        assert_eq!(preview["accounts"][0]["hasToken"], true);
        assert!(
            preview["accounts"][0].get("access_token").is_none(),
            "预览不得泄露 token"
        );
    }

    /// 回归 issue #100：预览的对象字段必须折叠为 null（防 React error #31 白屏），
    /// 信封凭据标记 encrypted，明文凭据不标记。
    #[test]
    fn preview_folds_object_fields_and_marks_envelope_token() {
        let text = r#"[
            {
                "id": "env-1",
                "uid": "u-env",
                "nickname": {"$wbEncrypted": 1, "envelope": "nick"},
                "email": {"$wbEncrypted": 1, "envelope": "mail"},
                "access_token": {"$wbEncrypted": 1, "envelope": "a"}
            },
            {
                "id": "plain-1",
                "uid": "u-plain",
                "nickname": "明文昵称",
                "email": "x@y.z",
                "access_token": "SECRET"
            }
        ]"#;
        let preview = preview_accounts(text).unwrap();

        assert_eq!(preview["total"], 2);
        let env = &preview["accounts"][0];
        assert_eq!(env["uid"], "u-env");
        assert!(
            env["nickname"].is_null(),
            "信封 nickname 必须折叠为 null，不得透传对象"
        );
        assert!(
            env["email"].is_null(),
            "信封 email 必须折叠为 null，不得透传对象"
        );
        assert_eq!(env["hasToken"], true, "信封凭据应视为有凭据");
        assert_eq!(env["encrypted"], true, "信封凭据应标记 encrypted");
        assert!(env.get("access_token").is_none(), "预览不得泄露 token");

        let plain = &preview["accounts"][1];
        assert_eq!(plain["nickname"], "明文昵称");
        assert_eq!(plain["email"], "x@y.z");
        assert_eq!(plain["hasToken"], true);
        assert_eq!(plain["encrypted"], false, "明文凭据不得误标 encrypted");
    }

    // -----------------------------------------------------------------------
    // issue #232：第三方号池（BuddyRelay 等）导出格式兼容
    //
    // 根因：对方用 camelCase `accessToken` / `refreshToken`，本工具只认 snake_case，
    // 于是整份号池在预览里全部显示「缺少 token」。归一化只**补键**，不裁剪、不改名，
    // 既有 `$wbEncrypted` 信封语义原样保留。
    // -----------------------------------------------------------------------

    /// camelCase 凭据归一化后可以入库，其它字段（含 `auth_raw`）原样保留。
    #[test]
    fn parse_normalizes_camel_case_credentials() {
        let text = r#"[{
            "id": "relay-1",
            "uid": "u-relay",
            "nickname": "号池账号",
            "accessToken": "tok-relay",
            "refreshToken": "ref-relay",
            "auth_raw": { "k": "v" }
        }]"#;
        let mut accounts: Vec<Value> = vec![];
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();

        assert_eq!(result.imported, 1, "camelCase 凭据必须能入库");
        assert_eq!(result.skipped, 0);
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0]["access_token"], "tok-relay");
        assert_eq!(accounts[0]["refresh_token"], "ref-relay");
        assert_eq!(
            accounts[0]["auth_raw"]["k"], "v",
            "归一化只补键：其它字段不得被裁剪"
        );
        assert_eq!(accounts[0]["nickname"], "号池账号");
    }

    /// `token` 是第二优先级的别名：没有 `accessToken` 时也认。
    #[test]
    fn parse_falls_back_to_token_alias() {
        let parsed = parse_accounts_json(r#"[{ "uid": "u1", "token": "tok-short" }]"#)
            .expect("应能解析");
        assert_eq!(parsed[0]["access_token"], "tok-short");
    }

    /// 规范键优先：同时存在时以 `access_token` 为准；规范键为空串时才回退别名。
    #[test]
    fn parse_prefers_canonical_token_over_alias() {
        let parsed = parse_accounts_json(
            r#"[
                { "uid": "u1", "access_token": "tok-canonical", "accessToken": "tok-alias" },
                { "uid": "u2", "access_token": "   ", "accessToken": "tok-fallback" }
            ]"#,
        )
        .expect("应能解析");

        assert_eq!(
            parsed[0]["access_token"], "tok-canonical",
            "已有合法规范键时不得被别名覆盖"
        );
        assert_eq!(
            parsed[1]["access_token"], "tok-fallback",
            "规范键为空串时应回退别名"
        );
    }

    /// 别名指向 `$wbEncrypted` 信封时按信封原样搬过来（不拆、不改写）。
    #[test]
    fn parse_keeps_envelope_when_carried_by_alias() {
        let text = r#"[{
            "uid": "u-env-alias",
            "accessToken": { "$wbEncrypted": 1, "envelope": "a" },
            "refreshToken": { "$wbEncrypted": 1, "envelope": "r" }
        }]"#;
        let parsed = parse_accounts_json(text).expect("应能解析");
        assert_eq!(
            parsed[0]["access_token"],
            json!({"$wbEncrypted": 1, "envelope": "a"}),
            "信封语义必须原样保留"
        );
        assert_eq!(parsed[0]["refresh_token"]["envelope"], "r");
    }

    /// `{ "accounts": [...] }` 包装可解析，且预览索引与数组下标一致。
    #[test]
    fn parse_accepts_accounts_wrapper_and_keeps_preview_indexes() {
        let text = r#"{
            "version": 1,
            "accounts": [
                { "uid": "u1", "nickname": "甲", "accessToken": "t1" },
                { "uid": "u2", "nickname": "乙" }
            ]
        }"#;
        let parsed = parse_accounts_json(text).expect("包装对象应能解析");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0]["access_token"], "t1");

        let preview = preview_accounts(text).unwrap();
        assert_eq!(preview["total"], 2);
        assert_eq!(preview["accounts"][0]["index"], 0);
        assert_eq!(preview["accounts"][0]["nickname"], "甲");
        assert_eq!(preview["accounts"][1]["index"], 1);
        assert_eq!(preview["accounts"][1]["hasToken"], false);
    }

    /// 包装对象的 `accounts` 不是数组时仍按「非数组」拒绝（文案不变）。
    #[test]
    fn parse_rejects_wrapper_without_account_array() {
        let err = parse_accounts_json(r#"{ "accounts": 1 }"#).unwrap_err();
        assert!(
            err.contains("JSON 数组"),
            "包装格式错误应仍报数组文案：{err}"
        );
        assert!(
            parse_accounts_json(r#"{ "other": [] }"#).is_err(),
            "不认识的顶层对象不得被当成账号列表"
        );
    }

    /// `enc:v1:` 密文：预览标记 `foreignEncrypted`、`hasToken=false`，导入计 skipped 且不落库。
    #[test]
    fn foreign_ciphertext_is_marked_in_preview_and_skipped_on_merge() {
        let text = r#"[
            {
                "uid": "u-pool",
                "nickname": "加密池账号",
                "access_token": "enc:v1:AAAA",
                "encrypted_access_token": "enc:v1:AAAA"
            },
            {
                "uid": "u-plain",
                "nickname": "明文账号",
                "access_token": "SECRET"
            }
        ]"#;
        let preview = preview_accounts(text).unwrap();
        let pool = &preview["accounts"][0];
        assert_eq!(pool["hasToken"], false, "密文不得算作有凭据");
        assert_eq!(pool["foreignEncrypted"], true, "应标记第三方密文");
        assert_eq!(pool["encrypted"], false, "`enc:v1:` 不是 WorkBuddy 信封");
        assert_eq!(
            preview["accounts"][1]["foreignEncrypted"], false,
            "明文凭据不得误标第三方密文"
        );

        let mut accounts: Vec<Value> = vec![];
        let result = merge_import_records(&mut accounts, text, &[0, 1]).unwrap();
        assert_eq!(result.imported, 1, "只有明文账号可入库");
        assert_eq!(result.skipped, 1, "密文账号计 skipped");
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0]["uid"], "u-plain", "密文账号不得进入账号库");
    }

    /// 密文判定允许前导空白（对方导出时可能带换行/空格），且只认字符串。
    #[test]
    fn foreign_ciphertext_detection_tolerates_leading_whitespace() {
        assert!(is_foreign_ciphertext(&json!("enc:v1:abc")));
        assert!(is_foreign_ciphertext(&json!("  \n enc:v1:abc")));
        assert!(!is_foreign_ciphertext(&json!("enc:v2:abc")));
        assert!(!is_foreign_ciphertext(&json!("SECRET")));
        assert!(!is_foreign_ciphertext(&json!({ "$wbEncrypted": 1 })));
        assert!(!is_foreign_ciphertext(&Value::Null));
    }

    // -----------------------------------------------------------------------
    // issue #232：QA 独立复核（真实导出形态 + 边界）
    //
    // 与上面「最小样例」互补：这里用一份**贴近 BuddyRelay 真实导出**的多账号文件
    // （`{accounts:[...]}` 包装 + camelCase 凭据 + `domain` / `expiresAt` / `authRaw`），
    // 确认预览口径、导入计数与字段保真度三者一致 —— 归一化只允许「补键」。
    // -----------------------------------------------------------------------

    /// 真实形态导出：预览全部有凭据 → 全部可入库 → 来源端字段一个都不少。
    #[test]
    fn relay_shaped_export_previews_and_imports_without_field_loss() {
        let text = r#"{
            "version": 2,
            "exportedAt": 1893456000000,
            "accounts": [
                {
                    "id": "relay-cn-1", "uid": "uid-cn-1", "nickname": "国内号",
                    "email": "cn1@example.com", "domain": "workbuddy.cn",
                    "accessToken": "at-cn-1", "refreshToken": "rt-cn-1",
                    "expiresAt": 1893456000000,
                    "authRaw": { "accessToken": "at-cn-1", "domain": "workbuddy.cn" }
                },
                {
                    "id": "relay-ai-1", "uid": "uid-ai-1", "nickname": "国际号",
                    "email": "ai1@example.com", "domain": "www.workbuddy.ai",
                    "accessToken": "at-ai-1", "refreshToken": "rt-ai-1",
                    "expiresAt": 1893456000000,
                    "authRaw": { "accessToken": "at-ai-1", "domain": "www.workbuddy.ai" }
                },
                {
                    "id": "relay-cn-2", "uid": "uid-cn-2", "nickname": "国内号 2",
                    "domain": "workbuddy.cn", "accessToken": "at-cn-2", "expiresAt": 1
                }
            ]
        }"#;

        let preview = preview_accounts(text).expect("真实形态导出必须能预览");
        assert_eq!(preview["total"], 3);
        for index in 0..3 {
            assert_eq!(
                preview["accounts"][index]["hasToken"], true,
                "第 {index} 项应识别出凭据（否则又会退化成整份号池「缺少 token」）"
            );
            assert_eq!(preview["accounts"][index]["foreignEncrypted"], false);
            assert_eq!(preview["accounts"][index]["encrypted"], false);
        }

        let mut accounts: Vec<Value> = vec![];
        let result = merge_import_records(&mut accounts, text, &[0, 1, 2]).expect("应能导入");
        assert_eq!(result.imported, 3, "预览说有凭据的数必须与真正入库的数一致");
        assert_eq!(result.skipped, 0);
        assert_eq!(result.overwritten, 0, "空账号库里没有可覆盖的记录");

        let ai = &accounts[1];
        assert_eq!(ai["access_token"], "at-ai-1");
        assert_eq!(ai["refresh_token"], "rt-ai-1");
        // 「只补键、不改名、不裁剪」：`domain` 决定档位、`authRaw` 是切号写回的原始凭据、
        // `expiresAt` 决定明文 Token 是否仍算有效，少一个就切不过去或判成过期。
        assert_eq!(ai["domain"], "www.workbuddy.ai");
        assert_eq!(ai["expiresAt"], json!(1893456000000i64));
        assert_eq!(ai["authRaw"]["accessToken"], "at-ai-1");
        assert_eq!(ai["authRaw"]["domain"], "www.workbuddy.ai");
        assert_eq!(ai["email"], "ai1@example.com");
        assert_eq!(ai["id"], "relay-ai-1", "来源自带的 id 必须保留");
        assert_eq!(
            account::variant_of(ai).as_str(),
            "ai",
            "`domain` 必须能继续决定档位（归一化不能改写它）"
        );
    }

    /// 第三方号池的 `domain` 是导入时**唯一**的档位线索：CodeBuddy 系国际版域
    /// （`www.codebuddy.ai` / apex `codebuddy.ai`）必须判成国际版，相似域不得误判。
    ///
    /// 这是从导入入口（`parse_accounts_json` → 合并 → `variant_of`）走的端到端口径，
    /// 而不是只测 `variant.rs` 的私有判定：导入路径不会给记录补 `variant` 字段，
    /// 因此档位 100% 由 `domain` 兜底决定 —— 判错就写进国内桶、切号打到错误的客户端。
    #[test]
    fn imported_pool_accounts_take_their_variant_from_the_domain() {
        let text = r#"[
            { "uid": "u-cb-ai", "domain": "www.codebuddy.ai", "accessToken": "t1" },
            { "uid": "u-cb-ai-apex", "domain": "codebuddy.ai", "accessToken": "t2" },
            { "uid": "u-wb-ai", "domain": "workbuddy.ai", "accessToken": "t3" },
            { "uid": "u-cb-cn", "domain": "www.codebuddy.cn", "accessToken": "t4" },
            { "uid": "u-cb-lookalike", "domain": "codebuddy.ai.cn", "accessToken": "t5" },
            { "uid": "u-cb-prefix", "domain": "xcodebuddy.ai", "accessToken": "t6" },
            { "uid": "u-cb-evil", "domain": "www.codebuddy.ai.evil.com", "accessToken": "t7" }
        ]"#;
        let mut accounts: Vec<Value> = vec![];
        merge_import_records(&mut accounts, text, &[0, 1, 2, 3, 4, 5, 6]).unwrap();

        for (index, expected) in [
            (0, "ai"),         // www.codebuddy.ai
            (1, "ai"),         // apex
            (2, "ai"),         // apex（WorkBuddy 国际版）
            (3, "cn"),         // 国内版产品域
            (4, "cn"),         // 标签边界外的相似域
            (5, "cn"),         // 前缀粘连，不是子域
            (6, "cn"),         // 后缀注入
        ] {
            assert_eq!(
                account::variant_of(&accounts[index]).as_str(),
                expected,
                "`{}` 的档位判定不对（uid={}）",
                accounts[index]["domain"],
                accounts[index]["uid"]
            );
        }
    }

    /// 规范键存在但「不是凭据」（null / 数字 / 普通对象）时仍要回退别名。
    ///
    /// 真实号池里 `access_token` 常被写成 `null` 占位；只判「存在」而不判「是不是凭据」
    /// 会把这类记录判成「有规范键」从而放弃别名，等于又回到 #232 的原始症状。
    #[test]
    fn alias_fallback_covers_null_number_and_plain_object() {
        let parsed = parse_accounts_json(
            r#"[
                { "uid": "u-null", "access_token": null, "accessToken": "at-null" },
                { "uid": "u-num", "access_token": 42, "accessToken": "at-num" },
                { "uid": "u-obj", "access_token": { "k": "v" }, "token": "at-obj" },
                { "uid": "u-arr", "access_token": [], "refreshToken": "only-refresh" }
            ]"#,
        )
        .expect("应能解析");

        assert_eq!(parsed[0]["access_token"], "at-null");
        assert_eq!(parsed[1]["access_token"], "at-num");
        assert_eq!(parsed[2]["access_token"], "at-obj", "`token` 是第二优先级别名");
        assert_eq!(
            parsed[3]["access_token"],
            json!([]),
            "数组不是凭据：归一化只补键，既有值原样保留（合并时会因无凭据被跳过）"
        );
        assert_eq!(parsed[3]["refresh_token"], "only-refresh");
        assert!(
            account::secret_value(&parsed[3], "access_token").is_none(),
            "`[]` 不得被当成有效凭据"
        );
        assert_eq!(
            account::secret_value(&parsed[3], "refresh_token"),
            Some(json!("only-refresh")),
            "只有 refreshToken 时保留它，但不能凭空造出 access_token"
        );
    }

    /// 别名搬来的密文同样要命中 `enc:v1:` 判定（贵号池导出就是 camelCase）。
    ///
    /// 这条钉住「先归一化、后判定」的顺序：先判密文再归一化会漏掉 `accessToken` 形态。
    #[test]
    fn foreign_ciphertext_carried_by_camel_alias_is_still_detected() {
        let text = r#"[
            { "uid": "u-alias-cipher", "accessToken": "  enc:v1:ZZZ" },
            { "uid": "u-alias-plain", "accessToken": "PLAIN" }
        ]"#;

        let preview = preview_accounts(text).unwrap();
        assert_eq!(preview["accounts"][0]["hasToken"], false);
        assert_eq!(preview["accounts"][0]["foreignEncrypted"], true);
        assert_eq!(preview["accounts"][1]["hasToken"], true);
        assert_eq!(preview["accounts"][1]["foreignEncrypted"], false);

        let mut accounts: Vec<Value> = vec![];
        let result = merge_import_records(&mut accounts, text, &[0, 1]).unwrap();
        assert_eq!(result.skipped, 1);
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0]["uid"], "u-alias-plain");
    }

    /// `enc:v2:` 是对方后来的新格式：本机同样解不开，但**不是**本次登记的 `enc:v1:` 口径，
    /// 现在按普通明文处理（至少不会让用户以为文件坏了）。这条钉住「前缀精确匹配」。
    #[test]
    fn enc_v2_prefix_is_not_treated_as_foreign_ciphertext() {
        let text = r#"[{ "uid": "u-v2", "accessToken": "enc:v2:AAA" }]"#;
        let parsed = parse_accounts_json(text).unwrap();
        assert_eq!(parsed[0]["access_token"], "enc:v2:AAA", "原样保留，不改写");

        let preview = preview_accounts(text).unwrap();
        assert_eq!(
            preview["accounts"][0]["foreignEncrypted"], false,
            "只认登记过的 `enc:v1:` 前缀"
        );
        assert_eq!(preview["accounts"][0]["hasToken"], true);
    }

    /// 顶层不是数组也不是包装对象（字符串 / 数字 / 布尔 / null）→ 明确拒绝，且文案说清要求。
    #[test]
    fn parse_rejects_top_level_scalars() {
        for text in [r#""accounts""#, "42", "3.5", "true", "null"] {
            let err = parse_accounts_json(text).unwrap_err();
            assert!(
                err.contains("JSON 数组"),
                "顶层标量 {text} 应报数组文案，实际：{err}"
            );
        }
    }

    /// 包装对象里 `accounts` 为字符串 / 对象 / null 时同样拒绝（只认数组这一层）；
    /// `accounts: []` 是合法输入（空列表 ≠ 格式错误，前端据此报「共 0 个账号」）。
    #[test]
    fn parse_rejects_wrapper_with_non_array_accounts() {
        for text in [
            r#"{ "accounts": "x" }"#,
            r#"{ "accounts": { "0": {} } }"#,
            r#"{ "accounts": null }"#,
        ] {
            assert!(
                parse_accounts_json(text).is_err(),
                "非数组 accounts 必须拒绝：{text}"
            );
        }
        assert_eq!(
            parse_accounts_json(r#"{ "accounts": [] }"#)
                .expect("`accounts: []` 是合法输入")
                .len(),
            0
        );
    }

    /// 既有备份（snake_case + `$wbEncrypted` 信封）不得被归一化改动：
    /// 信封原样保留、已有规范键不被别名覆盖、`refresh_token` 不丢。
    #[test]
    fn normalize_leaves_native_backup_records_untouched() {
        let text = r#"[{
            "id": "native-1",
            "uid": "u-native",
            "access_token": { "$wbEncrypted": [1, 2, 3], "envelope": "abc" },
            "refresh_token": "rt-native",
            "mark": "red",
            "note": "本地备注",
            "accessToken": "should-not-override"
        }]"#;
        let parsed = parse_accounts_json(text).unwrap();
        assert_eq!(
            parsed[0]["access_token"],
            json!({ "$wbEncrypted": [1, 2, 3], "envelope": "abc" }),
            "信封依赖本机 keyblob，改写即失效"
        );
        assert_eq!(parsed[0]["refresh_token"], "rt-native");
        assert_eq!(parsed[0]["mark"], "red");
        assert_eq!(parsed[0]["note"], "本地备注");
        assert_eq!(
            parsed[0]["accessToken"], "should-not-override",
            "别名键本身也要原样保留（只加键，不改名/不删键）"
        );

        let preview = preview_accounts(text).unwrap();
        assert_eq!(preview["accounts"][0]["encrypted"], true, "信封仍标 encrypted");
        assert_eq!(preview["accounts"][0]["hasToken"], true);
        assert_eq!(preview["accounts"][0]["foreignEncrypted"], false);

        let mut accounts = vec![json!({
            "id": "native-1", "uid": "u-native", "access_token": { "$wbEncrypted": 1 },
            "mark": "blue",
        })];
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();
        assert_eq!(result.overwritten, 1, "同 uid 备份应覆盖");
        assert_eq!(accounts[0]["mark"], "red", "备份自带展示字段时以备份为准");
        assert_eq!(accounts[0]["refresh_token"], "rt-native");
    }

    /// 索引越界与重复索引的口径：越界计 skipped、重复索引不去重（同一记录会被合并两次）。
    ///
    /// 重复索引不会产生重复账号（同 uid 第二次走覆盖），计数口径必须与之一致。
    #[test]
    fn merge_counts_out_of_range_and_duplicate_indexes() {
        let text = r#"[{ "id": "a", "uid": "u1", "access_token": "t1" }]"#;
        let mut accounts: Vec<Value> = vec![];

        let result = merge_import_records(&mut accounts, text, &[0, 0, 5]).unwrap();
        assert_eq!(result.imported, 2, "索引 0 被合并两次（第二次覆盖）");
        assert_eq!(result.overwritten, 1);
        assert_eq!(result.skipped, 1, "越界索引计 skipped");
        assert_eq!(accounts.len(), 1, "同 uid 不产生副本");
    }

    /// 密文记录**不得覆盖**本地同 uid 的可用账号（最坏情况：导入一份别人的加密池备份，
    /// 把本机还能用的账号冲成一条不可用凭据）。凭据判定必须在 uid 匹配之前。
    #[test]
    fn foreign_ciphertext_never_overwrites_a_local_account_with_the_same_uid() {
        let mut accounts = vec![json!({
            "id": "local-1",
            "uid": "u-shared",
            "nickname": "本地可用",
            "access_token": "GOOD-TOKEN",
            "refresh_token": "GOOD-REFRESH",
            "mark": "blue",
        })];

        let text = r#"[{
            "uid": "u-shared",
            "nickname": "别人的加密池",
            "accessToken": "enc:v1:AAAA",
            "refreshToken": "enc:v1:BBBB"
        }]"#;
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();

        assert_eq!(result.skipped, 1, "密文记录整条跳过");
        assert_eq!(result.imported, 0);
        assert_eq!(result.overwritten, 0, "不得计为覆盖");
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0]["access_token"], "GOOD-TOKEN", "本地凭据必须原样保留");
        assert_eq!(accounts[0]["refresh_token"], "GOOD-REFRESH");
        assert_eq!(accounts[0]["nickname"], "本地可用");
        assert_eq!(accounts[0]["mark"], "blue");
    }

    #[test]
    fn export_file_name_rejects_traversal_and_non_json() {
        assert!(validate_export_file_name("../../etc/passwd.json").is_err());
        assert!(validate_export_file_name("a/b.json").is_err());
        assert!(validate_export_file_name("a\\b.json").is_err());
        assert!(validate_export_file_name("out.txt").is_err());
        assert!(validate_export_file_name("").is_err());
        assert!(validate_export_file_name("workbuddy-shift-accounts-2026-08-21.json").is_ok());
    }

    #[test]
    fn export_path_validation() {
        assert!(
            validate_export_path("relative/out.json").is_err(),
            "必须绝对路径"
        );
        // 绝对路径的形态按平台不同（Windows 需盘符或 UNC 前缀），fixture 必须取自
        // 当前平台：写死 `/tmp/...` 会在 Windows 上被判为相对路径而误报。
        let dir = std::env::temp_dir();
        let txt = dir.join("out.txt").to_string_lossy().to_string();
        let upper = dir.join("out.JSON").to_string_lossy().to_string();
        let json = dir.join("out.json").to_string_lossy().to_string();
        assert!(validate_export_path(&txt).is_err(), "必须 .json");
        assert!(validate_export_path(&upper).is_ok(), "扩展名不区分大小写");
        assert!(validate_export_path(&json).is_ok());
    }

    #[test]
    fn export_accounts_to_file_writes_json_with_tokens() {
        let dir = std::env::temp_dir();
        let file_name = format!(
            "workbuddy-shift-accounts-test-{}.json",
            uuid::Uuid::new_v4().simple()
        );
        let records = vec![record("a1", Some("u1"), "甲", None, true)];
        let path = write_records_to_file(&dir, &records, &file_name).unwrap();
        let written: Vec<Value> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0]["access_token"], "token-a1", "导出文件保留 token");
    }

    #[test]
    fn write_records_to_file_rejects_bad_name() {
        let dir = std::env::temp_dir();
        let records = vec![record("a1", Some("u1"), "甲", None, true)];
        assert!(write_records_to_file(&dir, &records, "../escape.json").is_err());
        assert!(write_records_to_file(&dir, &records, "no-ext").is_err());
    }

    /// 导入备份：记录未携带本地展示字段（备注 / 显示选择）时保留本地值。
    #[test]
    fn merge_preserves_local_display_fields_when_absent() {
        let mut local = record("local", Some("u1"), "旧名称", None, true);
        local["note"] = json!("本地备注");
        local["displayField"] = json!("note");
        let mut accounts = vec![local];

        let text = r#"[{ "uid": "u1", "nickname": "新名称", "access_token": "tok-new" }]"#;
        merge_import_records(&mut accounts, text, &[0]).unwrap();
        assert_eq!(accounts[0]["note"], "本地备注");
        assert_eq!(accounts[0]["displayField"], "note");
        assert_eq!(accounts[0]["nickname"], "新名称");
    }

    /// 导入备份：记录自带展示字段时以导入为准（导入即恢复当时的展示配置）。
    #[test]
    fn merge_uses_imported_display_fields_when_present() {
        let mut local = record("local", Some("u1"), "旧名称", None, true);
        local["note"] = json!("本地备注");
        local["displayField"] = json!("note");
        let mut accounts = vec![local];

        let text = r#"[{ "uid": "u1", "nickname": "新名称", "access_token": "tok-new",
            "note": "备份备注", "displayField": "phone" }]"#;
        merge_import_records(&mut accounts, text, &[0]).unwrap();
        assert_eq!(accounts[0]["note"], "备份备注");
        assert_eq!(accounts[0]["displayField"], "phone");
    }
