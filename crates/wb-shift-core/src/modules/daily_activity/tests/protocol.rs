// 五步流程的合同测试：全部走脚本化桩，不发任何真实请求。

use super::protocol::tests_support::fast_limits;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// ① 五步全成功 → 记「今日已打卡」、收尾调 `stop`（不是 `delete`）。
#[tokio::test]
async fn happy_path_records_state_and_calls_stop() {
    let dir = state::tests_support::TempDir::new("happy");
    let state_file = dir.join(state::STATE_FILE_NAME);
    let stub = Stub::happy();

    let result =
        run_daily_activity_with_limits(&[ai_account()], &stub, &state_file, &fast_limits()).await;

    let row = &result["accounts"][0];
    assert_eq!(row["accountId"], serde_json::json!("acc-1"));
    assert_eq!(row["ok"], serde_json::json!(true));
    assert_eq!(row["skipped"], serde_json::json!(false));
    assert_eq!(row["phase"], serde_json::json!("done"));
    assert_eq!(row["status"], serde_json::json!("completed"));
    assert_eq!(row["model"], serde_json::json!("hy3"));
    assert_eq!(row["modelApplied"], serde_json::json!(true));
    assert_eq!(row["cleanup"], serde_json::json!("stop"));
    assert_eq!(row["notice"], serde_json::Value::Null);

    // 五步的顺序：建会话 → 取沙箱 → 四条 ACP → 轮询 → stop。
    assert_eq!(
        stub.rpc_methods(),
        vec![
            "initialize".to_string(),
            "session/load".to_string(),
            "session/set_model".to_string(),
            "session/prompt".to_string()
        ]
    );
    assert!(stub.called_action("stop"));
    assert!(!stub.called_action("delete"));
    // 每条 ACP 请求都带 Acp-Connection-Id 与 Authorization。
    assert_eq!(
        stub.acp_connection_ids(),
        vec![Some("conn-1".to_string()); 4]
    );
    assert!(stub.acp_all_authorized());

    // 状态文件：记「今天 + 实际模型」，且不含 prompt / token / 沙箱 query。
    let saved = state::load(&state_file);
    assert!(saved.is_done_today("acc-1", &state::local_today()));
    assert_eq!(saved.accounts["acc-1"].model, "hy3");
    let raw = std::fs::read_to_string(&state_file).expect("状态文件应已写入");
    assert!(!raw.contains("tok-ACCESS-SENTINEL"), "{raw}");
    assert!(!raw.contains("tok-SANDBOX-SENTINEL"), "{raw}");
    assert!(!raw.contains("SENTINEL-QUERY"), "{raw}");
}

/// ② 某步失败 → 收尾调 `delete`，且**不记**「今日已打卡」。
#[tokio::test]
async fn failed_step_calls_delete_and_does_not_mark_done() {
    let dir = state::tests_support::TempDir::new("fail");
    let state_file = dir.join(state::STATE_FILE_NAME);
    // 取沙箱这一步恒 500：两个模型候选都会失败，整轮判失败。
    let stub = Stub::happy().fail_phase("session");

    let result =
        run_daily_activity_with_limits(&[ai_account()], &stub, &state_file, &fast_limits()).await;

    let row = &result["accounts"][0];
    assert_eq!(row["ok"], serde_json::json!(false));
    assert_eq!(row["phase"], serde_json::json!("session"));
    assert_eq!(row["cleanup"], serde_json::json!("delete"));
    assert_eq!(
        row["modelsTried"],
        serde_json::json!(["hy3", "deepseek-v4.1-flash"])
    );

    assert!(stub.called_action("delete"));
    assert!(!stub.called_action("stop"));

    // 状态文件里没有「已打卡」记录（文件可能压根没创建）。
    let saved = state::load(&state_file);
    assert!(!saved.is_done_today("acc-1", &state::local_today()));
    assert!(!saved.accounts.contains_key("acc-1"));

    // 错误文案不带 token / 沙箱地址 query。
    let error = row["error"].as_str().expect("失败必须有 error");
    assert!(!error.contains("tok-ACCESS-SENTINEL"), "{error}");
    assert!(!error.contains("tok-SANDBOX-SENTINEL"), "{error}");
    assert!(!error.contains("SENTINEL-QUERY"), "{error}");
    assert!(!error.contains("Bearer"), "{error}");
}

/// ③ 当天重复调用 → 直接跳过，**一个请求都不发**。
#[tokio::test]
async fn second_run_same_day_skips_without_any_request() {
    let dir = state::tests_support::TempDir::new("skip");
    let state_file = dir.join(state::STATE_FILE_NAME);
    let mut saved = ActivityState::default();
    saved.mark_done("acc-1", &state::local_today(), "hy3", 1);
    state::save(&state_file, &saved).expect("预置状态");

    let stub = Stub::happy();
    let result =
        run_daily_activity_with_limits(&[ai_account()], &stub, &state_file, &fast_limits()).await;

    let row = &result["accounts"][0];
    assert_eq!(row["ok"], serde_json::json!(true));
    assert_eq!(row["skipped"], serde_json::json!(true));
    assert_eq!(row["phase"], serde_json::json!("skipped"));
    assert!(stub.is_empty(), "当天已完成时不得发任何请求");
}

/// ④ DoH 只在「系统解析失败 / 命中 fake-ip」时启用；正常解析一次都不查。
#[tokio::test]
async fn doh_is_used_only_for_failed_or_fake_ip_resolution() {
    let host = "sbx-1.sandbox.cloudstudio.club";
    let real: std::net::IpAddr = "203.0.113.7".parse().expect("合法 IP");
    let fake: std::net::IpAddr = "198.18.0.5".parse().expect("合法 IP");

    let stub = Stub::happy();
    // 正常解析：不查 DoH，也不固定地址。
    let pinned = dns::resolve_override(&stub, host, Ok(vec![real])).await;
    assert_eq!(pinned, None);
    assert_eq!(stub.doh_queries(), 0);

    // fake-ip：查 DoH，取到真实 A 记录。
    let pinned = dns::resolve_override(&stub, host, Ok(vec![fake])).await;
    assert_eq!(pinned, Some(real));
    assert_eq!(stub.doh_queries(), 1);

    // 解析失败：同样走 DoH。
    let pinned = dns::resolve_override(&stub, host, Err("dns timeout".to_string())).await;
    assert_eq!(pinned, Some(real));
    assert_eq!(stub.doh_queries(), 2);

    // 非沙箱域名：解析失败也不查 DoH（兜底只对沙箱域名生效）。
    let pinned =
        dns::resolve_override(&stub, "www.workbuddy.ai", Err("dns timeout".to_string())).await;
    assert_eq!(pinned, None);
    assert_eq!(stub.doh_queries(), 2);
}

/// ⑤ `hy3` 不可用 → 顺延到 `deepseek-v4.1-flash`，并在结果里标注实际模型。
#[tokio::test]
async fn hy3_failure_falls_back_to_deepseek_model() {
    let dir = state::tests_support::TempDir::new("fallback");
    let state_file = dir.join(state::STATE_FILE_NAME);
    let stub = Stub::happy().fail_create_for("hy3");

    let result =
        run_daily_activity_with_limits(&[ai_account()], &stub, &state_file, &fast_limits()).await;

    let row = &result["accounts"][0];
    assert_eq!(row["ok"], serde_json::json!(true));
    assert_eq!(row["model"], serde_json::json!("deepseek-v4.1-flash"));
    assert_eq!(
        row["modelsTried"],
        serde_json::json!(["hy3", "deepseek-v4.1-flash"])
    );
    // hy3 建会话失败后按上限重试了一次，才顺延到 deepseek。
    assert_eq!(
        stub.created_models(),
        vec![
            "hy3".to_string(),
            "hy3".to_string(),
            "deepseek-v4.1-flash".to_string()
        ]
    );
    // 成功那条会话仍然只收尾一次（stop）。
    assert!(stub.called_action("stop"));

    let saved = state::load(&state_file);
    assert_eq!(saved.accounts["acc-1"].model, "deepseek-v4.1-flash");
}

/// `set_model` 被拒不算致命：本轮照跑，但结果必须如实标 `modelApplied=false`。
#[tokio::test]
async fn rejected_set_model_is_not_fatal_but_flagged() {
    let dir = state::tests_support::TempDir::new("set-model");
    let state_file = dir.join(state::STATE_FILE_NAME);
    let stub = Stub::happy().reject_set_model();

    let result =
        run_daily_activity_with_limits(&[ai_account()], &stub, &state_file, &fast_limits()).await;

    let row = &result["accounts"][0];
    assert_eq!(row["ok"], serde_json::json!(true));
    assert_eq!(row["modelApplied"], serde_json::json!(false));
    assert_eq!(row["model"], serde_json::json!("hy3"));
}

/// 会话进入 `failed` → 立即判失败并 `delete`（不必等到超时）。
#[tokio::test]
async fn failed_session_status_deletes_without_waiting_for_timeout() {
    let dir = state::tests_support::TempDir::new("failed-status");
    let state_file = dir.join(state::STATE_FILE_NAME);
    let stub = Stub::happy().with_statuses(&["working", "failed"]);

    let result =
        run_daily_activity_with_limits(&[ai_account()], &stub, &state_file, &fast_limits()).await;

    let row = &result["accounts"][0];
    assert_eq!(row["ok"], serde_json::json!(false));
    assert_eq!(row["phase"], serde_json::json!("poll"));
    assert_eq!(row["cleanup"], serde_json::json!("delete"));
    assert!(stub.called_action("delete"));
}

/// 国内版账号不发任何请求（国际版没有签到，国内版没有每日活跃）。
#[tokio::test]
async fn cn_account_is_unsupported_without_requests() {
    let dir = state::tests_support::TempDir::new("cn");
    let state_file = dir.join(state::STATE_FILE_NAME);
    let stub = Stub::happy();
    let account = serde_json::json!({
        "id": "cn-1",
        "variant": "cn",
        "domain": "www.codebuddy.cn",
        "access_token": "tok-CN"
    });

    let result =
        run_daily_activity_with_limits(&[account], &stub, &state_file, &fast_limits()).await;

    let row = &result["accounts"][0];
    assert_eq!(row["ok"], serde_json::json!(false));
    assert_eq!(row["phase"], serde_json::json!("unsupported"));
    assert!(stub.is_empty());
}

/// 加密信封态 / 缺凭据的账号：不发请求，给出可读原因。
#[tokio::test]
async fn missing_plain_token_is_unsupported_without_requests() {
    let dir = state::tests_support::TempDir::new("no-token");
    let state_file = dir.join(state::STATE_FILE_NAME);
    let stub = Stub::happy();
    let account = serde_json::json!({"id": "acc-2", "variant": "ai", "access_token": ""});

    let result =
        run_daily_activity_with_limits(&[account], &stub, &state_file, &fast_limits()).await;

    let row = &result["accounts"][0];
    assert_eq!(row["ok"], serde_json::json!(false));
    assert_eq!(row["phase"], serde_json::json!("unsupported"));
    assert!(row["error"]
        .as_str()
        .unwrap_or_default()
        .contains("access_token"));
    assert!(stub.is_empty());
}

/// 脱敏：带 query 的地址在文案里被截到 `?` 之前。
#[test]
fn scrub_removes_url_query() {
    let text = "SSE 连接失败: error sending request for url \
                (https://sbx-1.sandbox.cloudstudio.club/acp?token=SENTINEL-QUERY)";
    let cleaned = scrub(text);
    assert!(!cleaned.contains("SENTINEL-QUERY"), "{cleaned}");
    assert!(
        cleaned.contains("https://sbx-1.sandbox.cloudstudio.club/acp"),
        "{cleaned}"
    );
    // 普通文本原样保留（不误伤 model id 之类）。
    assert_eq!(scrub("model hy3 not available"), "model hy3 not available");
    assert_eq!(
        redact_url("https://www.workbuddy.ai/app?a=1#x"),
        "https://www.workbuddy.ai/app"
    );
}

/// 活跃名额满（429 / 10105「maximum number of active」）→ 列会话、停最旧、重试**一次**。
///
/// 这是本模块唯一没有用例覆盖的新分支（作者自认的空白）：不腾位就永远建不出会话，
/// 而「腾位」本身会改上游状态，必须逐条断言顺序（列表 → stop → 再建一次）。
#[tokio::test]
async fn active_limit_prunes_the_oldest_session_and_retries_once() {
    let dir = state::tests_support::TempDir::new("active-limit");
    let state_file = dir.join(state::STATE_FILE_NAME);
    let stub = Stub::happy().with_active_limit();

    let result =
        run_daily_activity_with_limits(&[ai_account()], &stub, &state_file, &fast_limits()).await;

    let row = &result["accounts"][0];
    assert_eq!(row["ok"], serde_json::json!(true), "{row}");
    assert_eq!(row["model"], serde_json::json!("hy3"));
    assert_eq!(row["cleanup"], serde_json::json!("stop"));

    // 建会话被拒一次 → 列了一次会话 → 停掉最旧那条 → 只重试一次就成功。
    assert_eq!(
        stub.created_models(),
        vec!["hy3".to_string(), "hy3".to_string()],
        "名额满只允许重试一次（第二次是腾位后的重试）"
    );
    assert_eq!(stub.list_requests(), 1, "必须列一次会话找最旧的");
    // 两次 stop：一次腾位（conv-old），一次本轮的正常收尾（conv-1）。
    assert_eq!(stub.console_calls(ConsoleRoute::Stop), 2, "腾位 + 正常收尾各一次");
    assert!(!stub.called_action("delete"), "成功路径不得删除会话");

    // 腾位成功后本轮照常记状态。
    assert!(state::load(&state_file).is_done_today("acc-1", &state::local_today()));
}

/// 名额满且腾不出位置 → 明确失败，且**不再换模型重试**（换模型也建不出来）。
#[tokio::test]
async fn active_limit_without_old_sessions_fails_fast() {
    let dir = state::tests_support::TempDir::new("active-limit-hard");
    let state_file = dir.join(state::STATE_FILE_NAME);
    // 列表里没有活跃会话：`prune_oldest` 腾不出名额。
    let stub = Stub::happy().with_active_limit().with_listed(&[]);

    let result =
        run_daily_activity_with_limits(&[ai_account()], &stub, &state_file, &fast_limits()).await;

    let row = &result["accounts"][0];
    assert_eq!(row["ok"], serde_json::json!(false), "{row}");
    assert_eq!(row["phase"], serde_json::json!("create"));
    assert_eq!(
        row["modelsTried"],
        serde_json::json!(["hy3"]),
        "名额满属于 blocked：换模型也不可能建出会话"
    );
    let error = row["error"].as_str().unwrap_or_default();
    assert!(error.contains("上限") || error.contains("活跃"), "{error}");
    assert!(!stub.called_action("stop"), "没有旧会话可停");
}

/// 致命步骤收到 `200 + {"error":…}` 必须**立刻**判失败，不能挂到轮询超时（120s）。
///
/// 用 `turn_timeout` 做哨兵：桩里把上限设成 2 秒，若实现改成「忽略 JSON-RPC 错误继续轮询」，
/// 这条用例会明显超过 1 秒并且 `phase` 变成 `poll`。
#[tokio::test]
async fn jsonrpc_error_on_fatal_step_fails_without_waiting_for_poll_timeout() {
    let dir = state::tests_support::TempDir::new("rpc-error");
    let state_file = dir.join(state::STATE_FILE_NAME);
    let stub = Stub::happy().reject_initialize();
    let limits = Limits {
        turn_timeout: Duration::from_secs(2),
        ..fast_limits()
    };

    let started = Instant::now();
    let result =
        run_daily_activity_with_limits(&[ai_account()], &stub, &state_file, &limits).await;
    let elapsed = started.elapsed();

    let row = &result["accounts"][0];
    assert_eq!(row["ok"], serde_json::json!(false), "{row}");
    assert_eq!(row["phase"], serde_json::json!("turn"), "{row}");
    assert_eq!(row["cleanup"], serde_json::json!("delete"), "失败必须删除会话");
    let error = row["error"].as_str().unwrap_or_default();
    assert!(error.contains("initialize"), "{error}");
    assert!(error.contains("-32602"), "错误里要带 JSON-RPC code：{error}");
    // 上界 10s：要排除的是「白等 120s 轮询超时」，不是精确耗时。R7 在本机并行跑
    // 全量用例时实测 8.14s（同时有桌面端编译/取证任务），静默单跑 3.29s；5s 上界在
    // 双核 CI 上同样是负载敏感断言，故放宽到 10s——仍远小于 turn_timeout=2s 的
    // 「继续轮询」路径最终会走到的时间量级，断言意图不变。
    assert!(
        elapsed < Duration::from_secs(10),
        "必须快速失败（实测 {elapsed:?}）：JSON-RPC 错误不得白等轮询超时"
    );
    // 没有进入第 4 步：一次状态查询都不该发。
    assert_eq!(
        stub.console_calls(ConsoleRoute::Status),
        0,
        "ACP 失败后不得继续轮询会话状态"
    );
    assert!(state::load(&state_file).accounts.is_empty(), "失败不得记今日已完成");
}

/// `EventStream` 的 `Drop` 必须真的 abort 后台读流任务（否则每轮都留一个 detached 任务）。
#[tokio::test]
async fn event_stream_drop_aborts_the_keepalive_task() {
    let counter = Arc::new(AtomicUsize::new(0));
    let ticked = Arc::clone(&counter);
    let keepalive = tokio::spawn(async move {
        loop {
            ticked.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
        }
    });

    let stream = EventStream::new("conn-abort", Some(keepalive));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        counter.load(Ordering::SeqCst) > 0,
        "后台任务应当真的在跑（否则这条用例证明不了 abort）"
    );

    drop(stream);
    // 让 abort 生效（abort 在下一个 await 点取消任务）。
    tokio::time::sleep(Duration::from_millis(50)).await;
    let frozen = counter.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        counter.load(Ordering::SeqCst),
        frozen,
        "Drop 之后后台任务必须已停止（不能留下 detached 任务）"
    );
}
