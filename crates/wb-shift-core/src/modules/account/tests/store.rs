    /// 回归 issue #94：信封凭据要能被识别并给出可读错误，明文/缺字段不误报。
    #[test]
    fn envelope_token_error_only_fires_on_envelope_access_token() {
        let envelope = json!({
            "id": "a1",
            "access_token": {"$wbEncrypted": true, "envelope": "…"},
            "refresh_token": {"$wbEncrypted": true, "envelope": "…"},
        });
        let err = envelope_token_error(&envelope).expect("信封 access_token 应返回错误");
        assert!(err.contains("信封"), "错误文案应可读：{err}");
        assert!(err.contains("OAuth"), "应给出扫码重新添加的指引：{err}");

        let plain = json!({"id": "a2", "access_token": "SECRET", "refresh_token": "R"});
        assert!(envelope_token_error(&plain).is_none(), "明文凭据不应报错");

        let legacy = json!({"id": "a3"});
        assert!(
            envelope_token_error(&legacy).is_none(),
            "缺 access_token 的历史账号不在此拦截（保持既有行为）"
        );
    }

    #[test]
    fn account_meta_strips_tokens() {
        let acc = json!({
            "id": "a1",
            "uid": "u1",
            "email": "x@y.z",
            "nickname": "小明",
            "enterpriseName": "某公司",
            "access_token": "SECRET_ACCESS",
            "refresh_token": "SECRET_REFRESH",
            "expiresAt": 123456,
            "needs_relogin": true,
            "needs_relogin_reason": "刷新失败",
        });
        let meta = account_meta(&acc);
        assert_eq!(meta["id"], "a1");
        assert_eq!(meta["needsRelogin"], true);
        assert_eq!(meta["needsReloginReason"], "刷新失败");
        assert!(meta.get("access_token").is_none(), "不得泄露 token");
        assert!(meta.get("refresh_token").is_none(), "不得泄露 token");
    }

    #[test]
    fn account_meta_carries_variant_for_filtering() {
        // 无字段的历史账号按国内版解释，前端仍能按 variant 过滤。
        assert_eq!(account_meta(&json!({"id": "a1"}))["variant"], "cn");
        assert_eq!(
            account_meta(&json!({"id": "a2", "variant": "ai"}))["variant"],
            "ai"
        );
        // 域名为空的账号同样落回国内版（domain 兜底见 variant 单测）。
        assert_eq!(
            account_meta(&json!({"id": "a3", "domain": "", "variant": ""}))["variant"],
            "cn"
        );
    }

    /// 回归 issue #167：企业版判定矩阵（enterpriseId / type 兜底 / 普通账号不命中）。
    #[test]
    fn enterprise_account_detection_matrix() {
        assert!(is_enterprise_account(&json!({"enterpriseId": "e-1"})));
        assert!(is_enterprise_account(&json!({"enterprise_id": "e-2"})));
        assert_eq!(
            enterprise_id_of(&json!({"enterpriseId": "  e-3  "})),
            Some("e-3".to_string()),
            "企业 ID 应 trim"
        );
        // 老备份可能只带 type 而缺 ID：仍判定为企业版（取数前补拉 ID）。
        assert!(is_enterprise_account(&json!({"type": "ultimate"})));
        assert!(is_enterprise_account(&json!({"type": "exclusive"})));
        // 普通个人版账号不命中。
        assert!(!is_enterprise_account(&json!({"type": "personal"})));
        assert!(!is_enterprise_account(&json!({"type": ""})));
        assert!(!is_enterprise_account(&json!({})));
        // 空串企业 ID 不算命中。
        assert!(!is_enterprise_account(&json!({"enterpriseId": "  "})));
        assert_eq!(enterprise_id_of(&json!({"enterprise_id": ""})), None);
    }

    /// account_meta 下发企业标记，供前端展示「企业版」徽标。
    #[test]
    fn account_meta_exposes_enterprise_flag() {
        assert_eq!(
            account_meta(&json!({"id": "a1", "enterpriseId": "e-1"}))["isEnterprise"],
            true
        );
        assert_eq!(
            account_meta(&json!({"id": "a2", "type": "ultimate"}))["isEnterprise"],
            true
        );
        assert_eq!(
            account_meta(&json!({"id": "a3", "type": "personal"}))["isEnterprise"],
            false
        );
        assert_eq!(account_meta(&json!({"id": "a4"}))["isEnterprise"], false);
    }

    /// R5-07：前后端同一套「备注优先」顺序（昵称优先于邮箱）。
    #[test]
    fn account_display_name_priority() {
        assert_eq!(
            account_display_name(&json!({"email": "a@b.c", "nickname": "n"})),
            "n"
        );
        assert_eq!(
            account_display_name(&json!({"email": "a@b.c", "nickname": "n", "note": "备注"})),
            "备注"
        );
        assert_eq!(
            account_display_name(&json!({"nickname": "n", "uid": "u"})),
            "n"
        );
        assert_eq!(account_display_name(&json!({"uid": "u"})), "u");
        assert_eq!(account_display_name(&json!({"id": "i"})), "i");
        assert_eq!(account_display_name(&json!({})), "unknown");
    }

    #[test]
    fn get_str_trims_and_filters_empty() {
        assert_eq!(get_str(&json!({"k": "  v  "}), "k"), Some("v".to_string()));
        assert_eq!(get_str(&json!({"k": "  "}), "k"), None);
        assert_eq!(get_str(&json!({"k": 123}), "k"), None);
    }

    fn account(id: &str, uid: Option<&str>, nickname: &str, email: Option<&str>) -> Value {
        json!({
            "id": id,
            "uid": uid,
            "nickname": nickname,
            "email": email,
            "access_token": format!("token-{id}"),
            "createdAt": 1,
        })
    }

    // -----------------------------------------------------------------------
    // 本地字段白名单不变量（`LOCAL_ACCOUNT_FIELDS`）
    //
    // 三处写回路径（刷新 Token / 扫码重登 / 导入备份）都必须保留白名单里的字段。
    // 这里按白名单**逐个字段**跑一遍，新增字段而忘了给样例值时用例会直接失败并报出字段名，
    // 避免再出现「加了字段但漏了一处保留列表 → 刷新后用户数据被覆盖」这类 bug。
    // -----------------------------------------------------------------------

    /// 每个本地字段给一份非默认样例值；白名单新增字段时必须在这里补一条。
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
            // issue #184「最近切换」：官方资料里没有这两个键，必须整对保留。
            "lastUsedAt" => json!(1_763_654_321_000_i64),
            "lastUsedClient" => json!("vscode"),
            // R6-01：凭据来源只由三个写入点（扫码 / 本机采集 / 导入）写，刷新的 `updated`
            // 对象天然不带它；不继承就会被清空、账号退回「判不出」。
            "credentialSource" => json!("oauth"),
            other => panic!(
                "LOCAL_ACCOUNT_FIELDS 新增了 `{other}`，请在本不变量测试里补一条样例值，\
                 并确认三处写回路径都会保留它"
            ),
        }
    }

    #[test]
    fn refresh_upsert_keeps_every_local_account_field() {
        for key in LOCAL_ACCOUNT_FIELDS {
            let mut existing = account("a-1", Some("uid-1"), "小明", None);
            existing[key] = local_field_sample(key);
            let mut accounts = vec![existing];

            // 官方刷新对象天然只带 token / 官方字段。
            upsert_account_in(
                &mut accounts,
                &json!({"id": "a-1", "uid": "uid-1", "access_token": "refreshed"}),
            );

            assert_eq!(
                accounts[0][key],
                local_field_sample(key),
                "刷新 Token 不得覆盖本地字段 `{key}`"
            );
            assert_eq!(accounts[0]["access_token"], "refreshed", "官方字段仍应更新");
        }
    }

    #[test]
    fn collected_upsert_keeps_every_local_account_field() {
        for key in LOCAL_ACCOUNT_FIELDS {
            let mut existing = account("a-1", Some("uid-1"), "小明", None);
            existing[key] = local_field_sample(key);
            let mut accounts = vec![existing];

            upsert_collected_account(
                &mut accounts,
                json!({"uid": "uid-1", "nickname": "小明（官方更新）", "access_token": "collected"}),
            );

            assert_eq!(
                accounts[0][key],
                local_field_sample(key),
                "扫码重登 / 导入本机登录态不得覆盖本地字段 `{key}`"
            );
            assert_eq!(accounts[0]["nickname"], "小明（官方更新）", "官方字段仍应更新");
        }
    }

    #[test]
    fn same_nickname_with_different_uids_is_retained() {
        let mut accounts = vec![account("old", Some("uid-1"), "同名", Some("同名"))];
        let saved =
            upsert_collected_account(&mut accounts, account("new", Some("uid-2"), "同名", None));

        assert_eq!(accounts.len(), 2);
        assert_eq!(saved["id"], "new");
    }

    #[test]
    fn same_uid_refresh_preserves_local_id_and_removes_duplicates() {
        let mut accounts = vec![
            account("stable", Some("uid-1"), "旧名称", Some("old@example.com")),
            account("duplicate", Some("uid-1"), "重复记录", None),
        ];
        let saved = upsert_collected_account(
            &mut accounts,
            account("generated", Some("uid-1"), "新名称", None),
        );

        assert_eq!(accounts.len(), 1);
        assert_eq!(saved["id"], "stable");
        assert_eq!(saved["nickname"], "新名称");
        assert_eq!(saved["access_token"], "token-generated");
    }

    #[test]
    fn different_uids_with_same_real_email_are_retained() {
        let mut accounts = vec![account(
            "old",
            Some("uid-1"),
            "账号一",
            Some("shared@example.com"),
        )];
        upsert_collected_account(
            &mut accounts,
            account("new", Some("uid-2"), "账号二", Some("shared@example.com")),
        );

        assert_eq!(accounts.len(), 2);
    }

    #[test]
    fn real_email_is_fallback_only_when_collected_uid_is_missing() {
        let mut accounts = vec![account("stable", None, "旧名称", Some("user@example.com"))];
        let saved = upsert_collected_account(
            &mut accounts,
            account("generated", None, "新名称", Some("USER@example.com")),
        );

        assert_eq!(accounts.len(), 1);
        assert_eq!(saved["id"], "stable");
        assert_eq!(saved["nickname"], "新名称");
    }

    #[test]
    fn legacy_synthetic_email_does_not_merge_accounts() {
        let mut accounts = vec![account("old", None, "同名", Some("同名"))];
        upsert_collected_account(&mut accounts, account("new", None, "同名", Some("同名")));

        assert_eq!(accounts.len(), 2);
    }

    #[test]
    fn variant_of_defaults_to_cn_and_reads_domain_fallback() {
        assert_eq!(variant_of(&json!({"uid": "u-1"})), WbVariant::Cn);
        assert_eq!(
            variant_of(&json!({"uid": "u-1", "variant": "ai"})),
            WbVariant::Ai
        );
        assert_eq!(
            variant_of(&json!({"uid": "u-1", "domain": "www.workbuddy.ai"})),
            WbVariant::Ai
        );
    }

    #[test]
    fn upsert_keeps_variant_of_recollected_account() {
        let mut accounts = vec![json!({
            "id": "a-1",
            "uid": "uid-1",
            "variant": "ai",
            "access_token": "old-token",
        })];
        // 不含档位字段的再次采集（如刷新）不得丢掉已入库档位
        let saved =
            upsert_collected_account(&mut accounts, account("a-1", Some("uid-1"), "新名称", None));

        assert_eq!(accounts.len(), 1);
        assert_eq!(saved["variant"], "ai");
        assert_eq!(accounts[0]["variant"], "ai");
    }

    #[test]
    fn envelope_reimport_keeps_unexpired_plain_oauth_token() {
        let envelope = json!({"$wbEncrypted": 1, "envelope": "enc"});
        let mut accounts = vec![json!({
            "id": "a-1",
            "uid": "uid-1",
            "nickname": "明文昵称",
            "access_token": "plain-token",
            "refresh_token": "plain-refresh",
            "expiresAt": crate::modules::config::now_ms() + 86_400_000_i64,
        })];
        // UI 自动 importLocal 会拿本机加密态重采集同一 uid：
        // 不得让信封覆盖仍未过期的明文 token（否则签到/积分失效）。
        let collected = json!({
            "uid": "uid-1",
            "nickname": envelope,
            "access_token": {"$wbEncrypted": 1, "envelope": "a"},
            "refresh_token": {"$wbEncrypted": 1, "envelope": "r"},
        });
        let saved = upsert_collected_account(&mut accounts, collected);

        assert_eq!(accounts.len(), 1);
        assert_eq!(saved["access_token"], "plain-token");
        assert_eq!(saved["refresh_token"], "plain-refresh");
        assert_eq!(saved["nickname"], "明文昵称");
    }

    #[test]
    fn protected_envelope_reimport_repairs_missing_local_id_without_changing_tokens() {
        let mut accounts = vec![json!({
            "uid": "uid-1",
            "access_token": "plain-token",
            "refresh_token": "plain-refresh",
            "expiresAt": crate::modules::config::now_ms() + 86_400_000_i64,
        })];
        let collected = json!({
            "uid": "uid-1",
            "access_token": {"$wbEncrypted": 1, "envelope": "a"},
            "refresh_token": {"$wbEncrypted": 1, "envelope": "r"},
        });

        let saved = upsert_collected_account(&mut accounts, collected);

        assert!(get_str(&saved, "id").is_some());
        assert_eq!(saved["access_token"], "plain-token");
        assert_eq!(saved["refresh_token"], "plain-refresh");
        assert_eq!(accounts[0]["id"], saved["id"]);
    }

    #[test]
    fn envelope_reimport_takes_over_after_plain_token_expired() {
        let mut accounts = vec![json!({
            "id": "a-1",
            "uid": "uid-1",
            "access_token": "stale-plain",
            "expiresAt": crate::modules::config::now_ms() - 1_000_i64,
        })];
        let collected = json!({
            "uid": "uid-1",
            "access_token": {"$wbEncrypted": 1, "envelope": "a"},
        });
        let saved = upsert_collected_account(&mut accounts, collected);

        assert_eq!(accounts.len(), 1);
        // 明文已过期：信封接管（切换仍可用，由 WorkBuddy 自解）。
        assert!(saved.get("access_token").and_then(|v| v.as_str()).is_none());
    }

    #[test]
    fn fresh_plain_oauth_token_replaces_envelope_record() {
        let mut accounts = vec![json!({
            "id": "a-1",
            "uid": "uid-1",
            "access_token": {"$wbEncrypted": 1, "envelope": "old"},
        })];
        // 重新扫码得到新明文：应正常替换。
        let collected = json!({
            "uid": "uid-1",
            "access_token": "fresh-plain",
            "expiresAt": crate::modules::config::now_ms() + 86_400_000_i64,
        });
        let saved = upsert_collected_account(&mut accounts, collected);

        assert_eq!(accounts.len(), 1);
        assert_eq!(saved["access_token"], "fresh-plain");
    }

    #[test]
    fn upsert_accepts_caller_supplied_variant_for_new_account() {
        let mut accounts = vec![];
        let saved = upsert_collected_account(
            &mut accounts,
            json!({"id": "a-ai", "uid": "uid-ai", "variant": "ai", "access_token": "t"}),
        );

        assert_eq!(saved["variant"], "ai");
        assert_eq!(variant_of(&accounts[0]), WbVariant::Ai);
    }

    #[test]
    fn upsert_account_keeps_existing_variant_when_updated_lacks_it() {
        let mut accounts =
            vec![json!({"id": "a-1", "uid": "uid-1", "variant": "ai", "access_token": "old"})];

        let mut refreshed = accounts[0].clone();
        refreshed["access_token"] = json!("new");
        refreshed.as_object_mut().unwrap().remove("variant");
        upsert_account_in(&mut accounts, &refreshed);

        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0]["access_token"], "new");
        assert_eq!(accounts[0]["variant"], "ai", "覆盖写入不得丢档位");

        // 追加新账号时按调用方给定的档位入库
        upsert_account_in(
            &mut accounts,
            &json!({"id": "a-2", "uid": "uid-2", "variant": "cn", "access_token": "t2"}),
        );
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[1]["variant"], "cn");
    }

    /// 回归 issue #111：无 id 记录刷新回写时按 uid 收敛，不得追加副本。
    #[test]
    fn upsert_without_id_falls_back_to_uid_and_keeps_store_size() {
        let mut accounts = vec![json!({
            "id": "local-id",
            "uid": "uid-1",
            "access_token": "old",
        })];
        let mut refreshed = accounts[0].clone();
        refreshed.as_object_mut().unwrap().remove("id");
        refreshed["access_token"] = json!("new");

        upsert_account_in(&mut accounts, &refreshed);

        assert_eq!(accounts.len(), 1, "缺 id 刷新不得追加副本");
        assert_eq!(accounts[0]["id"], "local-id", "按 uid 命中时保留库中 id");
        assert_eq!(accounts[0]["access_token"], "new");
    }

    #[test]
    fn uid_fallback_keeps_local_id_when_updated_has_a_different_id() {
        let mut accounts = vec![json!({
            "id": "local-id",
            "uid": "uid-1",
            "access_token": "old",
        })];

        upsert_account_in(
            &mut accounts,
            &json!({
                "id": "foreign-id",
                "uid": "uid-1",
                "access_token": "new",
            }),
        );

        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0]["id"], "local-id");
        assert_eq!(accounts[0]["access_token"], "new");
    }

    #[test]
    fn uid_fallback_repairs_empty_local_id() {
        let mut accounts = vec![json!({
            "id": "  ",
            "uid": "uid-1",
            "access_token": "old",
        })];

        upsert_account_in(
            &mut accounts,
            &json!({
                "id": "",
                "uid": "uid-1",
                "access_token": "new",
            }),
        );

        assert_eq!(accounts.len(), 1);
        assert!(get_str(&accounts[0], "id").is_some());
        assert_eq!(accounts[0]["access_token"], "new");
    }
