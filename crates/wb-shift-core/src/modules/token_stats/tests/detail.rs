#[test]
fn request_detail_window_keeps_only_the_newest_rows() {
    let row = |timestamp| RequestRow {
        timestamp,
        model: "fixture-model".to_string(),
        project: "fixture-project".to_string(),
        session_id: "fixture-session".to_string(),
        title: None,
        uid: "fixture-uid".to_string(),
        usage: Usage {
            input: 1,
            output: 1,
            read: 0,
            write: 0,
        },
        thinking: 0,
    };

    // 触达内存硬上限后只保留最新窗口，且按时间倒序。
    let mut collector = SourceCollector {
        collect_requests: true,
        ..SourceCollector::default()
    };
    for timestamp in 0..=(REQUEST_COLLECT_LIMIT as i64) {
        collector.push_request(row(timestamp));
    }
    assert_eq!(collector.requests.len(), REQUEST_WINDOW);
    assert_eq!(
        collector.requests[0].timestamp,
        REQUEST_COLLECT_LIMIT as i64
    );

    // 未触达硬上限时，输出窗口同样截断到最新 REQUEST_WINDOW 条。
    let mut bounded = SourceCollector {
        collect_requests: true,
        ..SourceCollector::default()
    };
    for timestamp in 0..(REQUEST_WINDOW as i64 + 100) {
        bounded.push_request(row(timestamp));
    }
    let value = bounded.into_value("fixture", 1);
    let rows = value["requests"].as_array().expect("request rows");
    assert_eq!(rows.len(), REQUEST_WINDOW);
    assert_eq!(rows[0]["timestamp"], REQUEST_WINDOW as i64 + 99);
    assert_eq!(rows[REQUEST_WINDOW - 1]["timestamp"], 100);

    // 关闭明细采集时不缓存任何行，也不输出 requests 键。
    let mut off = SourceCollector::default();
    off.push_request(row(1));
    assert!(off.into_value("fixture", 1).get("requests").is_none());
}

#[test]
fn request_detail_window_stays_the_global_newest_after_any_arrival_order() {
    let row = |timestamp| RequestRow {
        timestamp,
        model: "fixture-model".to_string(),
        project: "fixture-project".to_string(),
        session_id: "fixture-session".to_string(),
        title: None,
        uid: "fixture-uid".to_string(),
        usage: Usage {
            input: 1,
            output: 1,
            read: 0,
            write: 0,
        },
        thinking: 0,
    };

    // 文件按 mtime 顺序处理、行内时间戳可以乱序：中途裁剪过之后，后到的
    // 旧行不能占位，后到的新行仍须进入窗口 —— 结果恒等于「全局最新 500」。
    let mut collector = SourceCollector {
        collect_requests: true,
        ..SourceCollector::default()
    };
    for timestamp in 0..=(REQUEST_COLLECT_LIMIT as i64) {
        collector.push_request(row(timestamp));
    }
    collector.push_request(row(-1));
    for timestamp in 10_000..10_100 {
        collector.push_request(row(timestamp));
    }

    let value = collector.into_value("fixture", 1);
    let rows = value["requests"].as_array().expect("request rows");
    assert_eq!(rows.len(), REQUEST_WINDOW);
    assert_eq!(rows[0]["timestamp"], 10_099);
    assert_eq!(rows[99]["timestamp"], 10_000);
    // 裁剪后残留的旧行必须让位：第 101 条是最新一轮保留里的最新一行。
    assert_eq!(rows[100]["timestamp"], REQUEST_COLLECT_LIMIT as i64);
    // 全局最新 500 = 100 条新行 + 上一轮保留里最新的 400 条（4000..=3601）。
    assert_eq!(rows[REQUEST_WINDOW - 1]["timestamp"], 3_601);
    assert!(rows.iter().all(|row| row["timestamp"] != -1));
}

#[test]
fn request_detail_deduplicates_copied_session_history() {
    let now = crate::modules::config::now_ms();
    let root = std::env::temp_dir().join(format!(
        "workbuddy-shift-token-stats-detail-fork-{}-{now}",
        std::process::id()
    ));
    let project = root.join("fixture-project");
    fs::create_dir_all(&project).expect("create fixture dirs");
    let record = json!({
        "timestamp": now,
        "message": { "usage": { "input_tokens": 10, "output_tokens": 3 } }
    });
    fs::write(
        project.join("session-original.jsonl"),
        format!("{}\n", record),
    )
    .expect("write original fixture");
    fs::write(
        project.join("session-forked.jsonl"),
        format!(
            "{}\n{}\n",
            record,
            json!({
                "timestamp": now + 1_000,
                "message": { "usage": { "input_tokens": 7, "output_tokens": 2 } }
            })
        ),
    )
    .expect("write forked fixture");
    // 与聚合去重用例相同：固定 mtime，保证原始会话先于副本被处理。
    pin_mtime(
        project.join("session-original.jsonl"),
        std::time::SystemTime::now() - std::time::Duration::from_secs(60),
    );
    pin_mtime(
        project.join("session-forked.jsonl"),
        std::time::SystemTime::now(),
    );

    let result = source(root.clone(), "fixture", None, true);
    assert_eq!(result["summary"]["records"], 2);
    let rows = result["requests"].as_array().expect("request rows");
    // 复制会话重放的记录沿用同一指纹，明细里只出现一次。
    assert_eq!(rows.len(), 2);
    let inputs: Vec<u64> = rows
        .iter()
        .filter_map(|row| row["input"].as_u64())
        .collect();
    assert_eq!(inputs, [7, 10]);
    assert_eq!(rows[0]["sessionId"], "session-forked");
    assert_eq!(rows[1]["sessionId"], "session-original");

    fs::remove_dir_all(root).expect("remove fixture");
}

#[test]
fn request_detail_skips_records_without_timestamp() {
    let now = crate::modules::config::now_ms();
    let root = std::env::temp_dir().join(format!(
        "workbuddy-shift-token-stats-detail-undated-{}-{now}",
        std::process::id()
    ));
    let project = root.join("fixture-project");
    fs::create_dir_all(&project).expect("create fixture dirs");
    fs::write(
        project.join("session.jsonl"),
        format!(
            "{}\n{}\n",
            json!({
                "timestamp": now,
                "message": { "usage": { "input_tokens": 10, "output_tokens": 3 } }
            }),
            // 没有 timestamp 的记录仍然计入聚合，但无法排序，不进明细。
            json!({ "message": { "usage": { "input_tokens": 90, "output_tokens": 9 } } }),
        ),
    )
    .expect("write fixture");

    let result = source(root.clone(), "fixture", None, true);
    assert_eq!(result["summary"]["records"], 2);
    assert_eq!(result["summary"]["input"], 100);
    let rows = result["requests"].as_array().expect("request rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["input"], 10);

    fs::remove_dir_all(root).expect("remove fixture");
}

#[test]
fn request_detail_backfills_file_title_after_usage() {
    let now = crate::modules::config::now_ms();
    let root = std::env::temp_dir().join(format!(
        "workbuddy-shift-token-stats-detail-title-{}-{now}",
        std::process::id()
    ));
    let project = root.join("fixture-project");
    fs::create_dir_all(&project).expect("create fixture dirs");
    let record = |timestamp| {
        json!({
            "timestamp": timestamp,
            "message": { "usage": { "input_tokens": 10, "output_tokens": 3 } }
        })
    };
    fs::write(
        project.join("session.jsonl"),
        format!(
            "{}\n{}\n{}\n",
            json!({ "type": "summary", "summary": "摘要回退标题" }),
            record(now - 1_000),
            // aiTitle 出现在 usage 之后：文件读完后必须回填到所有明细行。
            json!({ "type": "ai-title", "aiTitle": "最新 AI 标题" }),
        ),
    )
    .expect("write fixture");

    let result = source(root.clone(), "fixture", None, true);
    let rows = result["requests"].as_array().expect("request rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["title"], "最新 AI 标题");

    fs::remove_dir_all(root).expect("remove fixture");
}

#[test]
fn request_detail_thinking_prefers_completion_thinking_tokens() {
    let now = crate::modules::config::now_ms();
    let root = std::env::temp_dir().join(format!(
        "workbuddy-shift-token-stats-detail-thinking-priority-{}-{now}",
        std::process::id()
    ));
    let project = root.join("fixture-project");
    fs::create_dir_all(&project).expect("create fixture dirs");
    fs::write(
        project.join("session.jsonl"),
        format!(
            "{}\n",
            json!({
                "timestamp": now,
                "message": { "usage": { "input_tokens": 10, "output_tokens": 40 } },
                // 两个来源同时存在时取扁平的 completion_thinking_tokens。
                "providerData": { "rawUsage": {
                    "completion_thinking_tokens": 7,
                    "completion_tokens_details": { "reasoning_tokens": 3 }
                }}
            })
        ),
    )
    .expect("write fixture");

    let result = source(root.clone(), "fixture", None, true);
    let rows = result["requests"].as_array().expect("request rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["thinking"], 7);
    assert_eq!(rows[0]["output"], 40);
    // 思考过程只是 output 的拆分，不改变合计口径。
    assert_eq!(rows[0]["total"], result["summary"]["total"]);

    fs::remove_dir_all(root).expect("remove fixture");
}

#[test]
fn request_detail_thinking_falls_back_to_reasoning_tokens_and_defaults_to_zero() {
    let now = crate::modules::config::now_ms();
    let root = std::env::temp_dir().join(format!(
        "workbuddy-shift-token-stats-detail-thinking-fallback-{}-{now}",
        std::process::id()
    ));
    let project = root.join("fixture-project");
    fs::create_dir_all(&project).expect("create fixture dirs");
    fs::write(
        project.join("session.jsonl"),
        format!(
            "{}\n{}\n",
            json!({
                "timestamp": now,
                "message": { "usage": { "input_tokens": 11, "output_tokens": 21 } },
                // rawUsage 只有思考计数字段时取 OpenAI 形状的 reasoning_tokens。
                "providerData": { "rawUsage": {
                    "completion_tokens_details": { "reasoning_tokens": 5 }
                }}
            }),
            json!({
                "timestamp": now - 1_000,
                "message": { "usage": { "input_tokens": 10, "output_tokens": 20 } },
                // rawUsage 缺少思考计数 -> 0；`reasoning` 是思考正文，
                // 只用来验证它不会被当成计数读取（隐私红线）。
                "providerData": {
                    "reasoning": "思考正文不得进入明细",
                    "rawUsage": { "prompt_cache_write_tokens": 1 }
                }
            })
        ),
    )
    .expect("write fixture");

    let result = source(root.clone(), "fixture", None, true);
    let rows = result["requests"].as_array().expect("request rows");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["thinking"], 5);
    assert_eq!(rows[1]["thinking"], 0);
    assert_eq!(rows[1]["cacheWrite"], 1);

    fs::remove_dir_all(root).expect("remove fixture");
}

#[test]
fn request_detail_thinking_over_output_saturates_reply_to_zero() {
    // 实测存在 thinking > output 的异常记录：后端如实返回两个原始计数，
    // 前端的 `output.saturating_sub(thinking)` 必须得到 0 而不是负数。
    let now = crate::modules::config::now_ms();
    let root = std::env::temp_dir().join(format!(
        "workbuddy-shift-token-stats-detail-thinking-overflow-{}-{now}",
        std::process::id()
    ));
    let project = root.join("fixture-project");
    fs::create_dir_all(&project).expect("create fixture dirs");
    fs::write(
        project.join("session.jsonl"),
        format!(
            "{}\n",
            json!({
                "timestamp": now,
                "message": { "usage": { "input_tokens": 10, "output_tokens": 2 } },
                "providerData": { "rawUsage": { "completion_thinking_tokens": 9 } }
            })
        ),
    )
    .expect("write fixture");

    let result = source(root.clone(), "fixture", None, true);
    let rows = result["requests"].as_array().expect("request rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["output"], 2);
    assert_eq!(rows[0]["thinking"], 9);
    let reply = rows[0]["output"]
        .as_u64()
        .unwrap_or(0)
        .saturating_sub(rows[0]["thinking"].as_u64().unwrap_or(0));
    assert_eq!(reply, 0);

    fs::remove_dir_all(root).expect("remove fixture");
}

#[test]
fn ide_source_reads_request_usage_and_skips_message_bodies() {
    let now = crate::modules::config::now_ms();
    let root = std::env::temp_dir().join(format!(
        "workbuddy-shift-token-stats-ide-{}-{now}",
        std::process::id()
    ));
    let history = root
        .join("uid")
        .join("CodeBuddyIDE")
        .join("uid")
        .join("history")
        .join("workspace-hash");
    let conv_a = history.join("conv-a");
    let conv_b = history.join("conv-b");
    let messages = conv_a.join("messages");
    fs::create_dir_all(&messages).expect("create ide fixture dirs");
    fs::create_dir_all(&conv_b).expect("create second conversation");

    fs::write(
        history.join("index.json"),
        json!({
            "conversations": [
                {
                    "id": "conv-a",
                    "name": "IDE 会话标题",
                    "selectedModelId": "deepseek-v4-flash"
                },
                {
                    "id": "conv-b",
                    "name": "范围外会话",
                    "selectedModelId": "hy4-preview"
                }
            ]
        })
        .to_string(),
    )
    .expect("write workspace index");
    fs::write(
        conv_a.join("index.json"),
        json!({
            "messages": [{ "id": "m1", "role": "assistant", "isComplete": true }],
            "requests": [{
                "id": "req-1",
                "state": "complete",
                "startedAt": now,
                "usage": {
                    "inputTokens": 100,
                    "outputTokens": 20,
                    "cacheTokens": 40,
                    "cachedWriteTokens": 5
                }
            }]
        })
        .to_string(),
    )
    .expect("write conversation index");
    fs::write(
        conv_b.join("index.json"),
        json!({
            "requests": [{
                "id": "req-old",
                "state": "complete",
                "startedAt": now - 100_000,
                "usage": {
                    "inputTokens": 999,
                    "outputTokens": 9,
                    "cacheTokens": 1,
                    "cachedWriteTokens": 0
                }
            }]
        })
        .to_string(),
    )
    .expect("write out-of-range conversation");
    fs::write(
        messages.join("ignored.json"),
        json!({
            "role": "assistant",
            "usage": { "inputTokens": 10_000, "outputTokens": 10_000 }
        })
        .to_string(),
    )
    .expect("write ignored message body");

    let mut projects = HashMap::new();
    projects.insert("conv-a".to_string(), "example-project".to_string());
    let result = ide_source(
        root.clone(),
        "codebuddy-ide",
        Some(now - 50_000),
        &projects,
        &mut Attribution::empty(),
    );

    assert_eq!(result["source"], "codebuddy-ide");
    assert_eq!(result["filesScanned"], 2);
    assert_eq!(result["summary"]["records"], 1);
    assert_eq!(result["summary"]["input"], 100);
    assert_eq!(result["summary"]["output"], 20);
    assert_eq!(result["summary"]["cacheRead"], 40);
    assert_eq!(result["summary"]["cacheWrite"], 5);
    assert_eq!(result["summary"]["total"], 125);
    assert_eq!(result["summary"]["uncachedInput"], 60);
    assert_eq!(result["models"][0]["key"], "deepseek-v4-flash");
    assert_eq!(result["projects"][0]["key"], "example-project");
    assert_eq!(result["sessions"][0]["sessionId"], "conv-a");
    assert_eq!(result["sessions"][0]["title"], "IDE 会话标题");
    assert_eq!(result["sessions"].as_array().map(Vec::len), Some(1));
    // IDE 来源不是明细来源，响应形状保持不变。
    assert!(result.get("requests").is_none());
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    assert_eq!(result["dailyByModel"]["deepseek-v4-flash"][0]["key"], today);

    fs::remove_dir_all(root).expect("remove fixture");
}
