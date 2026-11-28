//! 诊断包的行为测试：脱敏不泄漏、分段齐全、日志聚合、失败降级、并行探测。
//!
//! 全部用临时目录与手写事实，**不读真实数据目录**：只有
//! [`probe_store_writable`](super::report::probe_store_writable) 这类生产专用探测
//! 不在测试路径上。

use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::facts::{self, AccessState, AccountFact, ReloginState, TokenKind};
use super::report::{
    build_report, hook_state_from_status, probe_clients, save_report, ClientProbe, HookState,
    HookTarget, HostForm, ProbeSpec, ProbeStatus, ReportFacts, ReportInput, ReportSources,
    DEFAULT_LOG_LINES, MAX_REPORT_BYTES,
};
use super::{account_bucket, format_timestamp_with_offset, redact_text};

/// 临时目录 + `error.log` 路径。
fn temp_log(name: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "workbuddy-shift-diagnostics-{name}-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("创建临时目录");
    let path = dir.join("error.log");
    (dir, path)
}

fn write_log(path: &Path, lines: &[serde_json::Value]) {
    let text = lines
        .iter()
        .map(|line| serde_json::to_string(line).expect("日志行可序列化"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(path, format!("{text}\n")).expect("写入临时日志");
}

fn input() -> ReportInput {
    ReportInput {
        host_form: HostForm::Desktop,
        locale: "zh-CN".to_string(),
        account_count: 2,
        clients: vec![
            ClientProbe::new("WorkBuddy", true, Some("5.11.3".to_string())),
            ClientProbe {
                name: "JetBrains 插件".to_string(),
                installed: false,
                version: None,
                running: None,
                probe_status: ProbeStatus::Timeout,
                elapsed_ms: Some(800),
            },
        ],
    }
}

/// 手动构造两条账号事实：一个明文、一个信封。
fn sample_account_facts(now: i64) -> Vec<AccountFact> {
    let accounts = vec![
        json!({
            "access_token": "PLAINTEXT-SECRET-TOKEN-abcdef",
            "refresh_token": "REFRESH-SECRET-abcdef",
            "expiresAt": now + 5 * 3_600_000i64,
            "credentialSource": "oauth",
            "variant": "cn",
            "email": "leak@example.com",
            "uid": "uid-123",
            "nickname": "私有昵称",
        }),
        json!({
            "access_token": {"$wbEncrypted": "ENVELOPE-BODY"},
            "variant": "cn",
            "credentialSource": "local",
        }),
    ];
    facts::account_facts(&accounts, now)
}

fn base_sources(log: PathBuf, facts: ReportFacts) -> ReportSources {
    ReportSources {
        error_log: log,
        error_log_limit: DEFAULT_LOG_LINES,
        hook: Some(HookState {
            installed: true,
            registered_targets: 1,
            total_targets: 2,
            script_exists: true,
            targets: vec![
                HookTarget {
                    label: "Claude CLI".to_string(),
                    exists: true,
                    installed: true,
                },
                HookTarget {
                    label: "CodeBuddy".to_string(),
                    exists: true,
                    installed: false,
                },
            ],
        }),
        store_writable: Some(true),
        generated_at_ms: 1_700_000_000_123,
        utc_offset_seconds: 8 * 3600,
        facts,
    }
}

#[test]
fn format_timestamp_uses_injected_offset() {
    // 2026-11-27 16:53:36 UTC = 2026-11-28 00:53:36 +08:00
    let ms = 1_795_798_416_000i64;
    assert_eq!(
        format_timestamp_with_offset(ms, 8 * 3600),
        "2026-11-28 00:53:36 +08:00"
    );
    // UTC 偏移 0。
    assert_eq!(
        format_timestamp_with_offset(ms, 0),
        "2026-11-27 16:53:36 +00:00"
    );
    // 负偏移。
    assert_eq!(
        format_timestamp_with_offset(ms, -5 * 3600),
        "2026-11-27 11:53:36 -05:00"
    );
}

/// 报告快照：分段齐全、账号只有 #1/#2、全文不含 token / 路径 / 邮箱样例子串。
#[test]
fn report_snapshot_has_sections_and_no_secrets() {
    let (dir, log) = temp_log("snapshot");
    write_log(
        &log,
        &[
            json!({"ts": 1_700_000_000_000i64, "kind": "backend", "message": "读取 /Users/alex/x.json 失败", "detail": "Authorization: Bearer sk-secret-123"}),
            json!({"ts": 1_700_000_000_100i64, "kind": "backend", "message": "切换失败", "detail": ""}),
            json!({"ts": 1_700_000_000_200i64, "kind": "backend", "message": "联系 leak@example.com", "detail": ""}),
        ],
    );
    let now = 1_700_000_000_123i64;
    let facts = ReportFacts {
        accounts: sample_account_facts(now),
        account_store: Some(facts::AccountStoreFact {
            total: 2,
            size_bytes: Some(1024),
            backups: 1,
        }),
        workbuddy: vec![facts::WorkBuddyFacts {
            variant: "国内版".to_string(),
            installed: true,
            running: true,
            version: Some("5.11.3".to_string()),
            auth_present: true,
            auth_parseable: true,
            auth_bytes: Some(2048),
            account_count: Some(1),
            token_kind: "信封".to_string(),
        }],
        ..Default::default()
    };
    let report = build_report(&input(), &base_sources(log, facts));

    for needle in [
        "[时间]",
        "[环境]",
        "[WorkBuddy]",
        "[账号]",
        "[凭据健康]",
        "[刷新与保活]",
        "[客户端]",
        "[限额 hook]",
        "[会话关联]",
        "[统计与自动化]",
        "[设置摘要]",
        "[凭据事件]",
        "[最近错误日志]",
        "[诊断生成]",
        "生成时间: 2023-11-15 06:13:20 +08:00",
        "#1 国内版",
        "#2 国内版",
        "token=明文",
        "token=信封",
        "探测超时（>800ms）",
        "已注册配置: 1/2 处",
        "5.11.3",
    ] {
        assert!(report.contains(needle), "报告缺少 {needle}：\n{report}");
    }
    // 头部含本地偏移。
    assert!(report.contains("+08:00"), "头部应含本地偏移：\n{report}");
    // 账号只以 #1/#2 出现，不出现身份。
    for secret in [
        "PLAINTEXT-SECRET-TOKEN-abcdef",
        "REFRESH-SECRET-abcdef",
        "ENVELOPE-BODY",
        "leak@example.com",
        "uid-123",
        "私有昵称",
        "/Users/alex/x.json",
        "sk-secret-123",
    ] {
        assert!(
            !report.contains(secret),
            "诊断包泄漏了 {secret}：\n{report}"
        );
    }
    // 正文里不应出现 "账号 #3" 之类。
    assert!(!report.contains("#3"));

    std::fs::remove_dir_all(&dir).expect("清理临时目录");
}

/// 分段顺序固定（便于 diff，也是与本项目 issue 模板对齐的契约）。
#[test]
fn sections_are_in_fixed_order() {
    const EXPECTED: [&str; 14] = [
        "[时间]",
        "[环境]",
        "[WorkBuddy]",
        "[账号]",
        "[客户端]",
        "[限额 hook]",
        "[会话关联]",
        "[统计与自动化]",
        "[设置摘要]",
        "[凭据事件]",
        "[凭据健康]",
        "[刷新与保活]",
        "[最近错误日志]",
        "[诊断生成]",
    ];
    let (dir, log) = temp_log("order");
    write_log(&log, &[]);
    let report = build_report(&input(), &base_sources(log, ReportFacts::default()));
    let mut cursor = 0usize;
    for section in EXPECTED {
        let found = report[cursor..].find(section).unwrap_or_else(|| {
            panic!("分段顺序错误：找不到 {section}（从 {cursor} 起）\n{report}")
        });
        cursor += found + section.len();
    }
    std::fs::remove_dir_all(&dir).expect("清理临时目录");
}

/// 日志按 (kind,message) 聚合：重复项计一次并带 count。
#[test]
fn error_log_is_aggregated_with_count() {
    let (dir, log) = temp_log("aggregate");
    write_log(
        &log,
        &[
            json!({"ts": 1_700_000_000_000i64, "kind": "backend", "message": "更新超时", "detail": "a"}),
            json!({"ts": 1_700_000_000_100i64, "kind": "backend", "message": "更新超时", "detail": "b"}),
            json!({"ts": 1_700_000_000_200i64, "kind": "backend", "message": "更新超时", "detail": "c"}),
            json!({"ts": 1_700_000_000_300i64, "kind": "frontend_crash", "message": "渲染崩溃", "detail": ""}),
        ],
    );
    let report = build_report(&input(), &base_sources(log, ReportFacts::default()));
    assert!(
        report.contains("更新超时") && report.contains("×3"),
        "重复日志应聚合计数：\n{report}"
    );
    // 另一条不同 (kind,message) 不计数。
    assert!(report.contains("渲染崩溃"), "\n{report}");
    std::fs::remove_dir_all(&dir).expect("清理临时目录");
}

/// 非 JSON 日志行（旧版本直接写纯文本）仍要出现在报告里，且被脱敏；
/// 不允许出现「kind=backend、message 为空」的空壳行。
#[test]
fn raw_log_lines_stay_visible_and_masked() {
    let (dir, log) = temp_log("raw");
    std::fs::write(
        &log,
        "\"panic at C:/Users/alex/app.log\"\nnot json at /home/alex/x\n",
    )
    .expect("写入临时日志");
    let report = build_report(&input(), &base_sources(log, ReportFacts::default()));
    assert!(report.contains("[raw]"), "原生行应单独成组：\n{report}");
    assert!(report.contains("panic at"), "\n{report}");
    assert!(
        !report.contains("C:/Users/alex"),
        "路径应被脱敏：\n{report}"
    );
    assert!(!report.contains("/home/alex"), "路径应被脱敏：\n{report}");
    // 不应出现空的 backend 行（旧实现的 bug）。
    assert!(
        !report
            .lines()
            .any(|line| line.trim() == "[backend]  时间未知"),
        "出现了空壳日志行：\n{report}"
    );
    std::fs::remove_dir_all(&dir).expect("清理临时目录");
}

/// 日志窗口固定在最近 [`DEFAULT_LOG_LINES`] 条（更早的丢弃）。
#[test]
fn error_log_window_keeps_last_fifty_lines() {
    let (dir, log) = temp_log("window");
    let lines: Vec<serde_json::Value> = (0..60)
        .map(|i| {
            json!({
                "ts": 1_700_000_000_000i64 + i * 1_000,
                "kind": "backend",
                "message": format!("事件-{i:02}"),
                "detail": "",
            })
        })
        .collect();
    write_log(&log, &lines);
    let report = build_report(&input(), &base_sources(log, ReportFacts::default()));
    // 最近 50 条 = 事件-10 .. 事件-59；更早的 0..9 必须被丢掉。
    assert!(report.contains("事件-59"), "\n{report}");
    assert!(report.contains("事件-10"), "\n{report}");
    assert!(
        !report.contains("事件-09"),
        "超出窗口的日志不应出现：\n{report}"
    );
    assert!(
        report.contains("回看: 最近 50 条"),
        "应写明回看条数：\n{report}"
    );
    std::fs::remove_dir_all(&dir).expect("清理临时目录");
}

/// 报告超过 4 MiB 上限时省略日志段并注明，绝不返回超限内容。
#[test]
fn oversized_report_drops_log_section_with_note() {
    let (dir, log) = temp_log("oversize");
    let huge = "x".repeat(MAX_REPORT_BYTES + 1024);
    write_log(
        &log,
        &[
            json!({"ts": 1_700_000_000_000i64, "kind": "backend", "message": "巨型日志", "detail": huge}),
        ],
    );
    let report = build_report(&input(), &base_sources(log, ReportFacts::default()));
    assert!(
        report.len() <= MAX_REPORT_BYTES,
        "报告 {} 字节超过上限 {}",
        report.len(),
        MAX_REPORT_BYTES
    );
    assert!(
        report.contains("本段整体省略"),
        "应注明日志段被截断：\n{}",
        &report[..report.len().min(2000)]
    );
    assert!(!report.contains("巨型日志"), "超限日志不应出现");
    std::fs::remove_dir_all(&dir).expect("清理临时目录");
}

/// 读不到日志 / 读不到 hook / 未探测目录时也要出完整报告，只写降级说明。
#[test]
fn report_survives_missing_inputs() {
    let report = build_report(
        &ReportInput::new(HostForm::WebUi, "en-US", 0),
        &ReportSources {
            error_log: std::env::temp_dir().join("workbuddy-shift-does-not-exist.log"),
            error_log_limit: DEFAULT_LOG_LINES,
            hook: None,
            store_writable: None,
            generated_at_ms: 0,
            utc_offset_seconds: 0,
            facts: ReportFacts::default(),
        },
    );
    assert!(report.contains("（没有日志文件，或读不出来）"));
    assert!(report.contains("状态: 读取失败"));
    assert!(report.contains("未探测"));
    assert!(report.contains("（未提供探测结果）"));
    assert!(report.contains("宿主标识: webui"));
    assert!(report.contains("账号数量: 0 个"));
    assert!(report.contains("credential: 未配置"));
    assert!(report.contains("会话关联") && report.contains("无记录"));
}

/// hook 状态投影：只保留标签 / 布尔 / 计数。
#[test]
fn hook_state_projection_counts_targets() {
    let status = json!({
        "installed": true,
        "scriptExists": true,
        "targets": [
            {"label": "A", "path": "/secret/a", "exists": true, "installed": true},
            {"label": "B", "path": "/secret/b", "exists": false, "installed": false},
        ]
    });
    let hook = hook_state_from_status(&status).expect("some");
    assert!(hook.installed);
    assert!(hook.script_exists);
    assert_eq!(hook.total_targets, 2);
    assert_eq!(hook.registered_targets, 1);
    assert_eq!(hook.targets[0].label, "A");
}

/// 并行探测：快 / 慢（超时）/ panic 三个假 probe，断言超时项、耗时、顺序稳定、panic 不传播。
#[test]
fn probe_clients_parallel_timeout_and_panic() {
    let specs = vec![
        ProbeSpec::new("fast", || json!({"installed": true, "version": "1.0.0"})),
        ProbeSpec::new("slow", || {
            std::thread::sleep(Duration::from_millis(300));
            json!({"installed": true})
        }),
        ProbeSpec::new("boom", || panic!("probe exploded")),
    ];
    let probes = probe_clients(specs, 80);
    assert_eq!(probes.len(), 3);
    // 顺序稳定。
    assert_eq!(probes[0].name, "fast");
    assert_eq!(probes[1].name, "slow");
    assert_eq!(probes[2].name, "boom");
    // 快的正常、慢的超时、panic 记为 error。
    assert_eq!(probes[0].probe_status, ProbeStatus::Ok);
    assert!(probes[0].installed);
    assert_eq!(probes[1].probe_status, ProbeStatus::Timeout);
    assert_eq!(probes[2].probe_status, ProbeStatus::Error);
    // 耗时字段存在；超时项耗时等于阈值。
    assert!(probes[0].elapsed_ms.is_some());
    assert_eq!(probes[1].elapsed_ms, Some(80));
}

/// 账号数量分桶（保留旧入口）。
#[test]
fn account_bucket_covers_boundaries() {
    assert_eq!(account_bucket(0), "0");
    assert_eq!(account_bucket(1), "1-5");
    assert_eq!(account_bucket(5), "1-5");
    assert_eq!(account_bucket(6), "6-10");
    assert_eq!(account_bucket(11), "11-20");
    assert_eq!(account_bucket(21), "21-50");
    assert_eq!(account_bucket(51), "50+");
}

/// 落盘：扩展名白名单 / 空内容 / 父目录必须存在 / 超限，全部返回可读错误。
#[test]
fn save_report_validates_path_and_size() {
    let (dir, _) = temp_log("save");
    let target = dir.join("diagnostics.txt");

    let saved = save_report(&target.to_string_lossy(), "诊断内容").expect("正常保存");
    assert_eq!(saved["ok"], json!(true));
    assert_eq!(saved["bytes"], json!("诊断内容".len()));
    assert_eq!(std::fs::read_to_string(&target).expect("读回"), "诊断内容");

    assert!(save_report(&dir.join("d.md").to_string_lossy(), "x").is_ok());
    assert!(save_report("", "x").is_err());
    assert!(save_report(&dir.join("d.csv").to_string_lossy(), "x")
        .expect_err("csv 不在白名单")
        .contains(".txt"));
    assert!(save_report(&target.to_string_lossy(), "").is_err());
    assert!(save_report(&dir.join("missing").join("d.txt").to_string_lossy(), "x").is_err());
    let huge = "x".repeat(MAX_REPORT_BYTES + 1);
    assert!(save_report(&dir.join("big.txt").to_string_lossy(), &huge).is_err());

    std::fs::remove_dir_all(&dir).expect("清理临时目录");
}

/// 脱敏入口对整份报告文本同样成立。
#[test]
fn redact_text_is_exposed_for_report_level_masking() {
    let masked = redact_text("路径 /Users/alex/x 与 alex@example.com");
    assert!(!masked.contains("/Users/alex"));
    assert!(!masked.contains("alex@example.com"));
}

/// 凭据健康事实里的枚举标签（不依赖真实账号）。
#[test]
fn account_fact_state_labels() {
    let fact = AccountFact {
        index: 1,
        variant: "国内版".to_string(),
        source: "oauth".to_string(),
        source_inferred: false,
        token_kind: TokenKind::Plain,
        access: AccessState::Valid(5 * 3_600_000),
        refresh: TokenKind::Plain,
        refresh_expired: false,
        relogin: ReloginState { needed: false },
        enterprise: false,
        has_note: false,
        age_ms: None,
        last_event: None,
    };
    assert_eq!(fact.access.label(), "有效(剩余 5h)");
    assert_eq!(fact.relogin.label(), "否");
    assert_eq!(fact.token_kind.label(), "明文");
    assert_eq!(fact.token_kind.refresh_label(), "有");
    assert_eq!(TokenKind::Envelope.refresh_label(), "信封");
    assert_eq!(TokenKind::Missing.refresh_label(), "缺失");
}

/// `[凭据健康]` 每行字段齐全（含 refresh 是否过期），且不含身份信息。
#[test]
fn credential_health_line_lists_states_without_identity() {
    let now = 1_700_000_000_000i64;
    let accounts = vec![json!({
        "access_token": "ACCESS-SECRET",
        "refresh_token": "REFRESH-SECRET",
        "expiresAt": now + 5 * 3_600_000i64,
        "refreshTokenExpiresAt": now - 3_600_000i64,
        "credentialSource": "oauth",
        "variant": "cn",
        "uid": "uid-9",
        "email": "who@example.com",
        "nickname": "昵称",
    })];
    let mut account_facts = facts::account_facts(&accounts, now);
    facts::attach_account_events(
        &mut account_facts,
        &[facts::CredentialEvent {
            ts: now - 2 * 3_600_000,
            account: 1,
            variant: "cn".to_string(),
            action: "refreshed".to_string(),
            reason: "ok".to_string(),
            count: 1,
        }],
        now,
    );
    let (dir, log) = temp_log("cred-health");
    std::fs::write(&log, "").expect("空日志");
    let report_facts = ReportFacts {
        accounts: account_facts,
        ..Default::default()
    };
    let report = build_report(&input(), &base_sources(log, report_facts));
    let line = report
        .lines()
        .find(|line| line.trim_start().starts_with("#1 "))
        .unwrap_or_else(|| panic!("缺少 #1 行：\n{report}"));
    for needle in [
        "#1 国内版",
        "来源=oauth",
        "token=明文",
        "access=有效(剩余 5h)",
        "refresh=有(已过期)",
        "需重登=否",
        "最近刷新=refreshed(2h 前)",
    ] {
        assert!(line.contains(needle), "凭据健康行缺少 {needle}：{line}");
    }
    for secret in [
        "ACCESS-SECRET",
        "REFRESH-SECRET",
        "uid-9",
        "who@example.com",
        "昵称",
    ] {
        assert!(
            !report.contains(secret),
            "诊断包泄漏了 {secret}：\n{report}"
        );
    }
    std::fs::remove_dir_all(&dir).expect("清理临时目录");
}

/// 推断来源用中文后缀标注：无 `credentialSource` 但含 `auth` + `account` → `local（推断）`；
/// 显式 `credentialSource: "local"` 不带后缀（有据可查的就不该说成推断）。
#[test]
fn inferred_source_is_marked_with_chinese_suffix() {
    let now = 1_700_000_000_000i64;
    let accounts = vec![
        json!({
            "access_token": "TOKEN-A",
            "auth": {"accessToken": "live-a"},
            "account": {"id": "account-a"},
        }),
        json!({
            "access_token": "TOKEN-B",
            "credentialSource": "local",
        }),
    ];
    let (dir, log) = temp_log("source-inferred");
    std::fs::write(&log, "").expect("空日志");
    let report_facts = ReportFacts {
        accounts: facts::account_facts(&accounts, now),
        ..Default::default()
    };
    let report = build_report(&input(), &base_sources(log, report_facts));
    let line_of = |index: &str| {
        report
            .lines()
            .find(|line| line.trim_start().starts_with(index))
            .unwrap_or_else(|| panic!("缺少 {index} 行：\n{report}"))
            .to_string()
    };
    let inferred = line_of("#1 ");
    assert!(
        inferred.contains("来源=local（推断）"),
        "推断来源应带中文后缀：{inferred}"
    );
    let explicit = line_of("#2 ");
    assert!(
        explicit.contains("来源=local "),
        "显式来源应原样输出：{explicit}"
    );
    assert!(
        !explicit.contains("（推断）"),
        "显式来源不得被标成推断：{explicit}"
    );
    std::fs::remove_dir_all(&dir).expect("清理临时目录");
}

/// 节流合并后的行带 `×N`，`outcome_counts` 按 `count` 求和（5 次失败不能只算 1 次）。
#[test]
fn throttled_event_shows_count_and_outcomes_sum_by_count() {
    let now = 1_700_000_000_000i64;
    let (dir, log) = temp_log("event-count");
    std::fs::write(&log, "").expect("空日志");
    let event = facts::CredentialEvent {
        ts: now,
        account: 3,
        variant: "cn".to_string(),
        action: "decrypt_failed".to_string(),
        reason: "decrypt_binding_missing".to_string(),
        count: 5,
    };
    let report_facts = ReportFacts {
        credential_events: vec![event.clone()],
        ..Default::default()
    };
    let report = build_report(&input(), &base_sources(log, report_facts));
    assert!(
        report.contains("decrypt_failed/decrypt_binding_missing ×5"),
        "合并事件应带 ×5：\n{report}"
    );

    // 空文件没有事件时不应冒出 `×1`。
    assert!(!report.contains("×1"), "count=1 不写后缀：\n{report}");

    // 事件文件里的 `count` 会被解析出来；缺省按 1。
    let events_path = dir.join("credential-events.jsonl");
    std::fs::write(
        &events_path,
        concat!(
            r#"{"ts":1700000000100,"account":3,"variant":"cn","action":"decrypt_failed","reason":"decrypt_binding_missing","count":5}"#,
            "\n",
            r#"{"ts":1700000000200,"account":4,"variant":"cn","action":"refreshed","reason":"ok"}"#,
            "\n"
        ),
    )
    .expect("写入事件文件");
    let parsed = facts::read_credential_events(&events_path, 10);
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].count, 5, "应解析出 count");
    assert_eq!(parsed[1].count, 1, "旧文件没有 count 按 1");

    // outcome 求和用 count，不按行数。
    let summed = facts::outcome_counts(&[
        facts::CredentialEvent {
            action: "refreshed".to_string(),
            ..event.clone()
        },
        facts::CredentialEvent {
            action: "refreshed".to_string(),
            count: 0,
            ..event
        },
    ]);
    let refreshed = summed
        .iter()
        .find(|(name, _)| *name == "refreshed")
        .map(|(_, count)| *count);
    assert_eq!(refreshed, Some(6), "5 + 归一后的 1（count=0 → 1）");

    std::fs::remove_dir_all(&dir).expect("清理临时目录");
}
