// ---------------------------------------------------------------------------
// 同步失败的文案必须自解释（issue #3：同一个「同步失败」三轮都定位不到原因）
// ---------------------------------------------------------------------------
//
// 背景：`plan_sync_selection` 的两条失败路径原本各丢一次真实原因——
//   1. 「该组判定为 X，不允许以 Y 模式同步」丢掉了 `decision.reason`；
//   2. 「双方内容不可验证，检查结果已失效」丢掉了是哪一侧、为什么不可验证。
// 结果：正文末行半截、成员行缺失、预览过期这些完全不同的故障，用户看到的都是同一句
// 没有信息量的话。下面几条用例守住「拒绝/跳过文案里必须带上真实原因」。

/// 用**当前内容**造一份服务端预览凭据：`verdict` 由调用方指定。
///
/// 执行阶段的 `verify_preview` 会把凭据与实时绑定逐字段比对，所以凭据必须用与
/// `plan_sync_selection` 完全相同的一路输入装配——这样用例才能走到判定门禁本身，
/// 而不是被「检查结果已失效」提前拦下（那条提示不在本次验证范围内）。
fn live_token(env: &Env, group_id: &str, verdict: SyncVerdict) -> String {
    let paths = env.paths();
    let store = env.store();
    let group = store
        .groups
        .iter()
        .find(|group| group.id == group_id)
        .expect("组必须存在");
    let source_member = session_link::active_member_for(group, "uid-a").expect("来源成员有效");
    let target_member = session_link::active_member_for(group, "uid-b").expect("目标成员有效");
    let source_content = member_content_state(&paths, &source_member.session_id);
    let target_content = member_content_state(&paths, &target_member.session_id);
    let baseline = session_link::load_pair_baseline(
        &paths,
        group,
        &source_member.member_id,
        &target_member.member_id,
    );
    let binding = live_preview_binding(
        group,
        source_member,
        target_member,
        &source_content,
        &target_content,
        &baseline,
        verdict,
        &preview_archive_state(
            &paths,
            &source_member.session_id,
            &paths,
            &target_member.session_id,
        ),
    );
    session_link::save_preview_token(&paths, binding).expect("凭据应可保存")
}

/// 在临时环境上直接跑一次「执行前的计划校验」（与生产入口同一段逻辑）。
fn plan_outcome(env: &Env, group_id: &str, token: &str, mode: SyncMode) -> SyncItemOutcome {
    let paths = env.paths();
    let store = env.store();
    let pending: Vec<Operation> = Vec::new();
    let context = SyncContext {
        paths: &paths,
        variant: WbVariant::Cn,
        source_paths: &paths,
        source_variant: WbVariant::Cn,
        source_uid: "uid-a",
        source_account_id: None,
        target_uid: "uid-b",
        target_account_id: None,
        pending: &pending,
    };
    plan_sync_selection(
        &context,
        Some(&store),
        &SyncSelection {
            group_id: group_id.to_string(),
            preview_token: token.to_string(),
            mode,
        },
    )
}

/// 取出拒绝文案；其它形态直接失败（用例只验证拒绝路径）。
fn rejected_message(outcome: SyncItemOutcome) -> String {
    match outcome {
        SyncItemOutcome::Rejected { message } => message,
        SyncItemOutcome::Skipped { message, .. } => panic!("应为拒绝，实际是跳过：{message}"),
        SyncItemOutcome::Validated { .. } => panic!("应为拒绝，实际通过了校验"),
    }
}

/// 一条合法记录 + 一行半截 JSON（无结尾换行）：进程被杀留下的真实形态。
fn truncated_tail_body(cid: &str) -> String {
    format!(
        "{}\n{{\"c\":\"半截\"",
        json!({"type": "user", "sessionId": cid, "text": "你好"})
    )
}

/// 来源正文末行半截 → 拒绝文案必须说清「哪一侧、坏在第几行」。
#[test]
fn rejected_sync_message_explains_unverifiable_source_tail() {
    let env = ready_env("plan-reason-source-tail");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    let group_id = env.store().groups[0].id.clone();
    let target_body_before = std::fs::read_to_string(env.body_path(&target_id)).unwrap();

    // 复制之后再弄坏来源：目标副本仍是健康的，故障只剩「来源读不出来」这一件事。
    std::fs::write(env.body_path("sess-1"), truncated_tail_body("sess-1")).unwrap();
    // 前提：判定确实落到「不可验证」（否则这条用例证明不了文案分支）。
    let state = member_content_state(&env.paths(), "sess-1");
    assert!(
        matches!(state, ContentState::Unavailable(_)),
        "来源正文应判为不可验证：{state:?}"
    );

    let token = live_token(&env, &group_id, SyncVerdict::Unknown);
    let message = rejected_message(plan_outcome(&env, &group_id, &token, SyncMode::FastForward));
    assert!(
        message.contains("当前账号的内容无法确认"),
        "拒绝文案必须指出是哪一侧、为什么，实际：{message}"
    );
    assert!(
        message.contains("第 2 行的格式无法识别"),
        "拒绝文案必须指出坏在文件第几行，实际：{message}"
    );
    // 拒绝不写入：目标副本保持原样。
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        target_body_before
    );
}

/// 目标正文损坏 → 拒绝文案同样要指出是「目标账号」这一侧。
///
/// 说明（与任务书措辞的差异）：用户报告里的「目标侧坏掉」在当前判定规则下**不是** `skipped`。
/// `decide_sync` 判为 unknown 后，模式门禁（`verdict.allows`）先命中，因此结果是 `Rejected`；
/// 且 unknown 的组本就不下发预览凭据，真实用户也走不到这里。这个用例验证的是本次真正要守住
/// 的东西：目标侧的真实原因必须出现在用户可见文案里。
#[test]
fn rejected_sync_message_explains_unverifiable_target_tail() {
    let env = ready_env("plan-reason-target-tail");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    let group_id = env.store().groups[0].id.clone();
    let source_body_before = std::fs::read_to_string(env.body_path("sess-1")).unwrap();

    std::fs::write(env.body_path(&target_id), truncated_tail_body(&target_id)).unwrap();
    let state = member_content_state(&env.paths(), &target_id);
    assert!(
        matches!(state, ContentState::Unavailable(_)),
        "目标正文应判为不可验证：{state:?}"
    );

    let token = live_token(&env, &group_id, SyncVerdict::Unknown);
    let message = rejected_message(plan_outcome(&env, &group_id, &token, SyncMode::FastForward));
    assert!(
        message.contains("目标账号的内容无法确认"),
        "拒绝文案必须指出是目标侧、为什么，实际：{message}"
    );
    assert!(
        message.contains("第 2 行的格式无法识别"),
        "拒绝文案必须带着解析器给出的行号，实际：{message}"
    );
    // 来源永不被写。
    assert_eq!(
        std::fs::read_to_string(env.body_path("sess-1")).unwrap(),
        source_body_before
    );
}

/// 真分歧配快进模式 → 拒绝文案**以**既有前缀开头，原因追加在后（前缀契约）。
///
/// 前缀 `该组判定为 X，不允许以 Y 模式同步` 是前端提示与既有用例的匹配锚点，
/// 逐字改动会让提示与测试一起失效；真实原因只能追加在它后面。
#[test]
fn rejected_sync_message_keeps_prefix_and_appends_reason() {
    let env = ready_env("plan-reason-diverge-prefix");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    // 两边各自追加不同内容：条数相同、内容不同 = 真分歧。
    append_records(&env.body_path("sess-1"), "sess-1", 10, 2);
    append_records(&env.body_path(&target_id), &target_id, 20, 2);
    let target_body_before = std::fs::read_to_string(env.body_path(&target_id)).unwrap();

    // 走真实预览 → 真实执行入口，验证的是用户实际会看到的那条 error 文案。
    let group = preview(&env, "uid-b")["groups"][0].clone();
    assert_eq!(group["verdict"], "diverge", "{group}");
    let group_id = group["groupId"].as_str().unwrap().to_string();
    let token = group["previewToken"].as_str().unwrap().to_string();

    let report = sync(
        &env,
        "uid-b",
        &[selection(&group_id, &token, SyncMode::FastForward)],
    );
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
    // 拒绝不写入。
    assert_eq!(
        std::fs::read_to_string(env.body_path(&target_id)).unwrap(),
        target_body_before
    );
}

/// 「双方内容不可验证」的兜底文案：只写确实不可验证的那一侧，两侧都坏才写两侧。
///
/// 这个分支在当前判定规则下是防御性兜底（`decide_sync` 只在两侧都 Ready 时给出非 unknown
/// 判定，`verdict.allows` 因此先命中），执行路径走不到；文案仍按契约拼好，一旦判定规则
/// 演进到「允许带一侧缺失写入」，用户看到的必须是能直接定位的那句。
#[test]
fn unverifiable_sides_names_only_the_broken_side() {
    let env = ready_env("plan-reason-sides");
    let ready = member_content_state(&env.paths(), "sess-1");
    assert!(matches!(ready, ContentState::Ready(_)), "前提：来源可验证");
    let missing = ContentState::Missing;
    let unavailable = ContentState::Unavailable(
        "第 2 行的格式无法识别（第 9 列，EOF while parsing an object；该行 14 字节；文件末行），内容可能不完整"
            .to_string(),
    );

    // 只有来源不可验证：只写来源，不把「另一侧没问题」也写进拒绝理由。
    let only_source = session_link::unverifiable_sides(&unavailable, &ready).expect("来源不可验证应有说明");
    assert!(
        only_source.contains("来源：当前账号的内容无法确认"),
        "{only_source}"
    );
    assert!(!only_source.contains("目标："), "{only_source}");

    // 只有目标不可验证：只写目标。
    let only_target = session_link::unverifiable_sides(&ready, &missing).expect("目标不可验证应有说明");
    assert!(
        only_target.contains("目标：目标账号的内容不存在，无法确认"),
        "{only_target}"
    );
    assert!(!only_target.contains("来源："), "{only_target}");

    // 两侧都不可验证：两侧都写，来源在前。
    let both = session_link::unverifiable_sides(&unavailable, &missing).expect("两侧都不可验证应有说明");
    assert!(both.starts_with("来源：当前账号的内容无法确认"), "{both}");
    assert!(
        both.contains("；目标：目标账号的内容不存在，无法确认"),
        "{both}"
    );

    // 两侧都可验证：返回 None，调用方据此不追加任何内容状态说明。
    assert!(session_link::unverifiable_sides(&ready, &ready).is_none());
}

/// 取出跳过文案与原因码；其它形态直接失败（用例只验证跳过路径）。
fn skipped_parts(outcome: SyncItemOutcome) -> (&'static str, String) {
    match outcome {
        SyncItemOutcome::Skipped {
            reason, message, ..
        } => (reason, message),
        SyncItemOutcome::Rejected { message } => panic!("应为跳过，实际是被拒绝：{message}"),
        SyncItemOutcome::Validated { .. } => panic!("应为跳过，实际通过了校验"),
    }
}

/// issue #3 的真实路径：**预览时正文完好、之后正文损坏**。
///
/// `verify_preview` 排在模式门禁之前，所以这条路径根本走不到 `decision.reason`；
/// 必须由这里把当前内容状态补上，否则用户永远只看到「内容已变化」，没有「第几行」。
#[test]
fn stale_preview_explains_corrupted_body_after_preview() {
    let env = ready_env("plan-reason-stale-broken");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    let group_id = env.store().groups[0].id.clone();

    // 凭据按健康内容签发：此时来源是目标的严格超集 → fastForward。
    let token = live_token(&env, &group_id, SyncVerdict::FastForward);
    // 签发之后才弄坏来源：末行半截、无结尾换行。
    std::fs::write(env.body_path("sess-1"), truncated_tail_body("sess-1")).unwrap();

    let (reason, message) =
        skipped_parts(plan_outcome(&env, &group_id, &token, SyncMode::FastForward));
    assert_eq!(reason, REASON_PREVIEW_STALE, "原因码不得变化");
    assert!(
        message.contains("检查结果已失效："),
        "前缀必须逐字保留（前端与既有测试按它匹配）：{message}"
    );
    assert!(
        message.contains("当前账号的内容无法确认"),
        "跳过文案必须补上「是哪一侧、为什么」：{message}"
    );
    assert!(
        message.contains("第 2 行的格式无法识别"),
        "跳过文案必须带着解析器给出的行号：{message}"
    );
    assert!(
        message.contains("当前内容状态："),
        "括号里的内容状态说明不可省略：{message}"
    );
    // 跳过不写入：目标副本保持原样。
    assert!(env.body_path(&target_id).exists());
}

/// 反例：两侧都只是「又长了」、两侧都可验证 → 不得追加内容状态说明（保持原文案）。
#[test]
fn stale_preview_omits_content_state_when_both_sides_readable() {
    let env = ready_env("plan-reason-stale-grow");
    let report = copy(&env, "uid-b", &["sess-1"]);
    let target_id = env.first_copy_id(&report);
    let group_id = env.store().groups[0].id.clone();

    let token = live_token(&env, &group_id, SyncVerdict::FastForward);
    // 预览之后两边各追加一条（内容不同）：两侧仍可验证，只是「内容变了」。
    append_records(&env.body_path("sess-1"), "sess-1", 10, 1);
    append_records(&env.body_path(&target_id), &target_id, 20, 1);

    let (reason, message) =
        skipped_parts(plan_outcome(&env, &group_id, &token, SyncMode::FastForward));
    assert_eq!(reason, REASON_PREVIEW_STALE);
    assert!(message.contains("检查结果已失效："), "{message}");
    assert!(
        !message.contains("当前内容状态"),
        "两侧都可验证时不要多说，保持原文案逐字不变：{message}"
    );
    assert!(
        !message.contains("无法确认"),
        "两侧都可验证时不得出现「无法确认」：{message}"
    );
}
