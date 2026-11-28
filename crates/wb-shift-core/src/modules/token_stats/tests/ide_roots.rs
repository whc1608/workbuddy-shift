/// 切号复制会让同一会话散落在多个账号数据目录里：副本整份重放父历史（请求连时间戳
/// 一起逐字复制）后各自继续追加，会话 id 还会被重随机。统计必须按请求指纹取**并集**：
/// 父历史不重复累计，各副本复制后各自追加的请求也不丢。
#[test]
fn ide_source_merges_copied_session_history_as_a_union() {
    let now = crate::modules::config::now_ms();
    let root = std::env::temp_dir().join(format!(
        "workbuddy-shift-token-stats-ide-copy-{}-{now}",
        std::process::id()
    ));
    let conversation = |uid: &str, conv: &str| {
        root.join(uid)
            .join("VSCode")
            .join(uid)
            .join("history")
            .join("workspace-hash")
            .join(conv)
    };
    let original = conversation("uid-a", "conv-a");
    // 插件侧副本取新 id：目录名与源不同，但起点请求相同。
    let replay = conversation("uid-b", "conv-replay");
    // 另一个账号从同一份历史往下走：两份分叉后各自独有的请求都要保留。
    let fork = conversation("uid-c", "conv-fork");
    for dir in [&original, &replay, &fork] {
        fs::create_dir_all(dir).expect("create conversation dirs");
    }
    let request = |id: &str, input: u64| {
        json!({
            "id": id,
            "state": "complete",
            "startedAt": now,
            "usage": {
                "inputTokens": input,
                "outputTokens": 0,
                "cacheTokens": 0,
                "cachedWriteTokens": 0
            }
        })
    };
    fs::write(
        original.join("index.json"),
        json!({ "requests": [request("req-1", 100)] }).to_string(),
    )
    .expect("write original conversation");
    fs::write(
        replay.join("index.json"),
        json!({ "requests": [request("req-1", 100), request("req-2", 200)] }).to_string(),
    )
    .expect("write replayed conversation");
    fs::write(
        fork.join("index.json"),
        json!({ "requests": [request("req-1", 100), request("req-3", 400)] }).to_string(),
    )
    .expect("write forked conversation");

    let result = ide_source(
        root.clone(),
        "codebuddy-ide",
        None,
        &HashMap::new(),
        &mut Attribution::empty(),
    );

    assert_eq!(result["filesScanned"], 3);
    // 并集：100 + 200 + 400；重放的 req-1 只算一次，分叉两边独有的请求都在。
    assert_eq!(result["summary"]["records"], 3);
    assert_eq!(result["summary"]["input"], 700);
    assert_eq!(result["summary"]["total"], 700);
    // 同一会话跨三个账号：各账号各出一行（各自有用量），键不撞。
    // 重放的 req-1 归最早处理的那份（uid-a），req-2 归 uid-b，req-3 归 uid-c。
    let sessions = result["sessions"].as_array().expect("session groups");
    assert_eq!(sessions.len(), 3);
    let mut uids: Vec<&str> = sessions
        .iter()
        .map(|session| session["uid"].as_str().unwrap_or("?"))
        .collect();
    uids.sort_unstable();
    assert_eq!(uids, ["uid-a", "uid-b", "uid-c"]);
    let account_input = |uid: &str| {
        result["accounts"]
            .as_array()
            .expect("accounts")
            .iter()
            .find(|account| account["uid"] == uid)
            .map(|account| account["summary"]["input"].as_u64().unwrap_or(0))
            .unwrap_or(0)
    };
    assert_eq!(account_input("uid-a"), 100);
    assert_eq!(account_input("uid-b"), 200);
    assert_eq!(account_input("uid-c"), 400);

    fs::remove_dir_all(root).expect("remove fixture");
}

#[test]
fn decode_genie_workspace_recovers_unix_project_path() {
    assert_eq!(
        decode_genie_workspace("L1VzZXJzL2FwcGxlL0RvY3VtZW50cy9Qcm9qZWN0L215LWFnZW50").as_deref(),
        Some("/Users/apple/Documents/Project/my-agent")
    );
}

#[test]
fn get_statistics_returns_six_isolated_sources() {
    // memo 是进程级单格全局状态，这里也要串行（见 `memo_test_guard` 的注释）。
    let _guard = memo_test_guard();
    let value = get_statistics(None);
    let sources = value["sources"].as_array().expect("sources");
    let names: Vec<_> = sources
        .iter()
        .map(|source| source["source"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        names,
        [
            "workbuddy",
            "workbuddy-ai",
            "codebuddy-cli",
            "vscode-ext",
            "jetbrains",
            "codebuddy-ide"
        ]
    );
    // 国际版与国内版是两个独立 source，不合并；三个插件来源也各自独立。
    assert_ne!(sources[0]["source"], sources[1]["source"]);
    assert_ne!(sources[3]["source"], sources[4]["source"]);
}

/// 国际版数据根缺少 jsonl 根时返回空集，不报错。
#[test]
fn ai_source_is_empty_and_error_free_when_roots_are_missing() {
    let value = source_from_roots(
        &[],
        "workbuddy-ai",
        None,
        false,
        &HashMap::new(),
        &mut Attribution::empty(),
        Some(namespace_for_variant(WbVariant::Ai)),
    );
    assert_eq!(value["source"], "workbuddy-ai");
    assert_eq!(value["filesScanned"], 0);
    assert_eq!(value["summary"]["input"], 0);
    assert_eq!(value["parseErrors"], 0);
    assert_eq!(value["sessions"].as_array().map(Vec::len), Some(0));
    assert_eq!(value["projects"].as_array().map(Vec::len), Some(0));
}

/// 多根共享同一个指纹集：国际版两个根之间重放的同一请求只计一次，
/// 同时第二个根自身的新记录仍要计入（证明第二个根确实被扫了）。
#[test]
fn multi_root_source_deduplicates_across_roots() {
    let now = crate::modules::config::now_ms();
    let base = std::env::temp_dir().join(format!(
        "workbuddy-shift-token-stats-multiroot-{}-{now}",
        std::process::id()
    ));
    let projects = base.join("projects");
    let sessions = base.join("sessions");
    fs::create_dir_all(&projects).expect("create projects root");
    fs::create_dir_all(&sessions).expect("create sessions root");
    let replayed = json!({
        "timestamp": now,
        "cwd": "/fixture/example-project",
        "message": { "usage": { "input_tokens": 10, "output_tokens": 3 } }
    });
    fs::write(
        projects.join("session-original.jsonl"),
        format!("{}\n", replayed),
    )
    .expect("write original fixture");
    // 第二个根里的会话重放了同一个请求，并另有一条自己的新请求。
    fs::write(
        sessions.join("session-root-two.jsonl"),
        format!(
            "{}\n{}\n",
            replayed,
            json!({
                "timestamp": now + 1_000,
                "message": { "usage": { "input_tokens": 7, "output_tokens": 2 } }
            })
        ),
    )
    .expect("write second-root fixture");
    // 与 source_deduplicates_copied_session_history 同理：毫秒级 mtime 并列时
    // 顺序退化为 readdir，重放记录可能先被第二个根认领。pin 住 mtime 让
    // 原始会话先处理，断言才稳定。
    pin_mtime(
        projects.join("session-original.jsonl"),
        std::time::SystemTime::now() - std::time::Duration::from_secs(60),
    );
    pin_mtime(
        sessions.join("session-root-two.jsonl"),
        std::time::SystemTime::now(),
    );

    let result = source_from_roots(
        &[projects.clone(), sessions.clone()],
        "workbuddy-ai",
        None,
        false,
        &HashMap::new(),
        &mut Attribution::empty(),
        Some(namespace_for_variant(WbVariant::Ai)),
    );
    // 两个根都被扫到；重放记录计一次，第二个根的新记录另计一次。
    assert_eq!(result["filesScanned"], 2);
    assert_eq!(result["summary"]["records"], 2);
    assert_eq!(result["summary"]["input"], 17);
    assert_eq!(result["summary"]["output"], 5);
    fs::remove_dir_all(base).expect("remove fixture");
}

/// 候选根按档位不同：国内版单根，国际版双根（无 projects/ 时只扫 sessions/）。
#[test]
fn jsonl_root_candidates_differ_by_variant() {
    let root = PathBuf::from("/tmp/wb-root");
    assert_eq!(
        jsonl_root_candidates(WbVariant::Cn, &root),
        vec![PathBuf::from("/tmp/wb-root/projects")]
    );
    assert_eq!(
        jsonl_root_candidates(WbVariant::Ai, &root),
        vec![
            PathBuf::from("/tmp/wb-root/projects"),
            PathBuf::from("/tmp/wb-root/sessions")
        ]
    );
}

// -----------------------------------------------------------------------
// 沙箱：统计根必须跟着注入的家目录走，不得回落到 dirs::home_dir()
// -----------------------------------------------------------------------

/// 一个绝不可能等于真实家目录的探针目录（`temp_dir()` 可能就在家目录下，
/// 所以靠 uuid 后缀保证唯一；只拼路径、不落盘）。
fn stats_probe_home() -> PathBuf {
    std::env::temp_dir().join(format!("wb-token-stats-{}", uuid::Uuid::new_v4()))
}

/// 注入临时家目录 → 所有候选根都落在它下面，且不出现真实家目录派生的路径。
///
/// 防回归的核心断言：把 `stats_roots_at` 里任何一处改回 `dirs::home_dir()`，
/// 结果会指向真实家目录，这条立刻红——否则沙箱里点开「Token 统计」看到的
/// 就是真实用户数据。
#[test]
fn stats_roots_stay_under_injected_home() {
    let probe = stats_probe_home();
    let roots = stats_roots_at(&probe);

    let real = crate::modules::config::real_home_dir();
    let poisoned = [
        real.join(".codebuddy").join("projects"),
        real.join(".workbuddy").join("projects"),
        real.join(".workbuddy-ai").join("projects"),
        real.join(".workbuddy-ai").join("sessions"),
        codebuddy_extension_data_dir_at(&real),
    ];

    for root in roots.all() {
        assert!(
            root.starts_with(&probe),
            "统计根必须落在注入的家目录下，实际得到 {}",
            root.display()
        );
        assert!(
            !poisoned.contains(&root),
            "统计根回落到真实家目录了：{}",
            root.display()
        );
    }
}

/// CLI 与国内版 / 国际版的 projects 根一个都不能少（漏一个就少一个来源的统计）。
#[test]
fn stats_roots_include_cli_and_cn_project_roots() {
    let probe = stats_probe_home();
    let roots = stats_roots_at(&probe);

    assert_eq!(
        roots.codebuddy_cli,
        probe.join(".codebuddy").join("projects")
    );
    assert_eq!(
        roots.workbuddy,
        vec![probe.join(".workbuddy").join("projects")]
    );
    assert_eq!(
        roots.workbuddy_ai,
        vec![
            probe.join(".workbuddy-ai").join("projects"),
            probe.join(".workbuddy-ai").join("sessions"),
        ]
    );
    // IDE 根与登录态同源：跟着注入的家目录走，且仍是 `…/CodeBuddyExtension/Data`。
    assert_eq!(roots.codebuddy_ide, codebuddy_extension_data_dir_at(&probe));
    assert!(roots.codebuddy_ide.starts_with(&probe));
    assert!(roots
        .codebuddy_ide
        .ends_with(Path::new("CodeBuddyExtension").join("Data")));
}

/// 存在性过滤只看注入的家目录：空目录 → 空源（不报错、不去扫真实目录）；
/// 建出国内版 `projects/` 后只返回那一个根，且仍然落在注入目录下。
#[test]
fn stats_roots_filter_keeps_only_existing_dirs_under_injected_home() {
    let probe = stats_probe_home();
    let roots = stats_roots_at(&probe);
    assert!(existing_roots(&roots.workbuddy).is_empty());
    assert!(existing_roots(&roots.workbuddy_ai).is_empty());

    let cn_projects = probe.join(".workbuddy").join("projects");
    fs::create_dir_all(&cn_projects).expect("create cn projects fixture");
    assert_eq!(existing_roots(&roots.workbuddy), vec![cn_projects.clone()]);
    assert!(existing_roots(&roots.workbuddy_ai).is_empty());
    assert!(cn_projects.starts_with(&probe));

    fs::remove_dir_all(&probe).ok();
}

// -----------------------------------------------------------------------
// 单文件解析缓存：命中 / 失效 / 新文件纳入
// -----------------------------------------------------------------------

/// 缓存测试专用夹具：独立临时目录 + 一条可参数化的用量记录。
struct CacheFixture {
    root: PathBuf,
    file: PathBuf,
}

impl CacheFixture {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "workbuddy-shift-token-stats-cache-{label}-{}-{}",
            std::process::id(),
            crate::modules::config::now_ms()
        ));
        let project = root.join("fixture-project");
        fs::create_dir_all(&project).expect("create cache fixture dirs");
        let file = project.join("session.jsonl");
        Self { root, file }
    }

    /// 写入 `records` 条用量记录（`input` 从 10 开始递增）。
    fn write(&self, records: u64) {
        let now = crate::modules::config::now_ms();
        let body: String = (0..records)
            .map(|index| {
                json!({
                    "timestamp": now + index as i64,
                    "cwd": "/fixture/fixture-project",
                    "providerData": { "model": "fixture-model" },
                    "message": { "usage": {
                        "input_tokens": 10 + index,
                        "output_tokens": 1
                    }}
                })
                .to_string()
                    + "\n"
            })
            .collect();
        fs::write(&self.file, body).expect("write cache fixture");
    }

    fn statistics(&self) -> Value {
        source(self.root.clone(), "fixture", None, false)
    }
}

impl Drop for CacheFixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

/// 指纹不变 → 命中缓存（不再解析），响应完全一致。
#[test]
fn file_cache_reuses_parse_result_when_fingerprint_is_unchanged() {
    let fixture = CacheFixture::new("hit");
    fixture.write(3);

    let first = fixture.statistics();
    assert_eq!(parse_count(&fixture.file), 1, "首次必须真正解析一次");

    let second = fixture.statistics();
    assert_eq!(
        parse_count(&fixture.file),
        1,
        "同一份文件第二次不该再解析（缓存未命中）"
    );
    assert_eq!(first, second, "命中缓存的响应必须与首次逐字一致");
    assert_eq!(second["summary"]["records"], 3);
}

/// mtime 变化 → 指纹失效 → 重解析，且新内容立刻生效。
///
/// 也包括「改小 mtime」：只要和缓存里的指纹不同就必须重解析，不能因为
/// 「看起来更旧」就沿用缓存的旧结果。
#[test]
fn file_cache_invalidates_when_mtime_changes() {
    let fixture = CacheFixture::new("mtime");
    fixture.write(2);
    let first = fixture.statistics();
    assert_eq!(first["summary"]["records"], 2);

    // 内容与 mtime 一起变（写新内容后把 mtime 推到过去）。
    fixture.write(4);
    pin_mtime(
        fixture.file.clone(),
        std::time::SystemTime::now() - std::time::Duration::from_secs(30),
    );
    let second = fixture.statistics();
    assert_eq!(parse_count(&fixture.file), 2, "mtime 变化必须重解析");
    assert_eq!(second["summary"]["records"], 4);

    // 再取一次又应命中。
    let third = fixture.statistics();
    assert_eq!(parse_count(&fixture.file), 2);
    assert_eq!(third, second);
}

/// 大小变化（只追加、mtime 精度不足时也要失效）：新记录必须被纳入。
#[test]
fn file_cache_picks_up_appended_records() {
    let fixture = CacheFixture::new("append");
    fixture.write(1);
    assert_eq!(fixture.statistics()["summary"]["records"], 1);

    fixture.write(2);
    let grown = fixture.statistics();
    assert_eq!(parse_count(&fixture.file), 2);
    assert_eq!(grown["summary"]["records"], 2);
    assert_eq!(grown["summary"]["input"], 21);
}

/// 新出现的文件必须被纳入统计（缓存只作用于已经解析过的路径）。
#[test]
fn file_cache_includes_newly_created_files() {
    let fixture = CacheFixture::new("newfile");
    fixture.write(1);
    assert_eq!(fixture.statistics()["filesScanned"], 1);

    // 另一条独立记录（不同时间戳），证明新文件真的被读了。
    let extra = fixture.root.join("fixture-project").join("session-2.jsonl");
    fs::write(
        &extra,
        json!({
            "timestamp": crate::modules::config::now_ms() + 5_000,
            "cwd": "/fixture/fixture-project",
            "message": { "usage": { "input_tokens": 41, "output_tokens": 1 } }
        })
        .to_string()
            + "\n",
    )
    .expect("write extra fixture");

    let both = fixture.statistics();
    assert_eq!(both["filesScanned"], 2);
    assert_eq!(parse_count(&extra), 1);
    assert_eq!(both["summary"]["records"], 2);
    assert_eq!(both["summary"]["input"], 51);
}

/// 缓存的两道内存闸门：条目数与记录条数到顶后不再收新文件，
/// 但已有条目仍可覆盖（覆盖不会让总量继续增长）。
#[test]
fn file_cache_stops_growing_at_its_caps() {
    let parsed_with = |records: usize| {
        Arc::new(ParsedFile {
            records: vec![
                ParsedRecord {
                    timestamp: None,
                    model: Arc::from("fixture-model"),
                    project: None,
                    usage: Usage::default(),
                    thinking: 0,
                };
                records
            ],
            title: None,
            parse_errors: 0,
        })
    };
    let three = parsed_with(3);
    let fingerprint = Fingerprint {
        modified: None,
        size: 0,
    };

    // 记录条数闸门：已满时不再接收新文件。
    let mut cache = FileCache {
        records: CACHE_MAX_RECORDS,
        ..FileCache::default()
    };
    assert!(!cache.store(Path::new("a.jsonl"), fingerprint, &three));

    // 条目数闸门。
    let empty = parsed_with(0);
    let mut cache = FileCache::default();
    for index in 0..CACHE_MAX_FILES {
        let path = PathBuf::from(format!("{index}.jsonl"));
        assert!(cache.store(&path, fingerprint, &empty));
    }
    assert!(!cache.store(Path::new("overflow.jsonl"), fingerprint, &empty));
    // 已经缓存的路径仍可覆盖。
    assert!(cache.store(Path::new("0.jsonl"), fingerprint, &empty));

    // 记录条数按文件累计，覆盖同名文件时不能重复累加。
    let mut cache = FileCache::default();
    assert!(cache.store(Path::new("big.jsonl"), fingerprint, &three));
    assert_eq!(cache.records, 3);
    assert!(cache.store(Path::new("big.jsonl"), fingerprint, &three));
    assert_eq!(cache.records, 3, "覆盖同名条目不应重复累加");
}

// -----------------------------------------------------------------------
// 时间窗裁剪：整份落在窗口之前的文件不读盘
// -----------------------------------------------------------------------

// -----------------------------------------------------------------------
// 第四轮 06：按客户端目录拆 source，`WorkBuddy/` 空壳不参与统计
// -----------------------------------------------------------------------

#[test]
fn ide_client_source_only_counts_its_own_client_directory() {
    let now = crate::modules::config::now_ms();
    let root = std::env::temp_dir().join(format!(
        "workbuddy-shift-token-stats-client-split-{}-{now}",
        std::process::id()
    ));
    // 真实布局：Data/<uid>/<client>/<uid>/history/<bucket>/<conv>/index.json
    let conversation = |client: &str, conv: &str| {
        root.join("uid-a")
            .join(client)
            .join("uid-a")
            .join("history")
            .join("f6b91d61")
            .join(conv)
    };
    let write = |client: &str, conv: &str, input: u64| {
        let dir = conversation(client, conv);
        fs::create_dir_all(&dir).expect("create conversation dir");
        fs::write(
            dir.join("index.json"),
            json!({
                "requests": [{
                    "id": format!("req-{conv}"),
                    "state": "complete",
                    "startedAt": now - 1_000,
                    "usage": {
                        "inputTokens": input,
                        "outputTokens": 0,
                        "cacheTokens": 0,
                        "cachedWriteTokens": 0
                    }
                }]
            })
            .to_string(),
        )
        .expect("write conversation");
    };
    write("VSCode", "conv-vscode", 100);
    write("JetBrains", "conv-jetbrains", 200);
    write("CodeBuddyIDE", "conv-ide", 300);
    write("WorkBuddy", "conv-shell", 400);

    let empty = HashMap::new();
    let vscode = ide_client_source(
        root.clone(),
        "VSCode",
        "vscode-ext",
        None,
        &empty,
        &mut Attribution::empty(),
    );
    let jetbrains = ide_client_source(
        root.clone(),
        "JetBrains",
        "jetbrains",
        None,
        &empty,
        &mut Attribution::empty(),
    );
    let ide = ide_client_source(
        root.clone(),
        "CodeBuddyIDE",
        "codebuddy-ide",
        None,
        &empty,
        &mut Attribution::empty(),
    );

    assert_eq!(vscode["source"], "vscode-ext");
    assert_eq!(vscode["summary"]["input"], 100);
    assert_eq!(vscode["filesScanned"], 1);

    assert_eq!(jetbrains["source"], "jetbrains");
    assert_eq!(jetbrains["summary"]["input"], 200);
    assert_eq!(jetbrains["filesScanned"], 1);

    assert_eq!(ide["source"], "codebuddy-ide");
    assert_eq!(ide["summary"]["input"], 300);

    // `WorkBuddy/` 空壳（400）不出现在任何一个客户端 tab。
    for source in [&vscode, &jetbrains, &ide] {
        assert_ne!(source["summary"]["input"], 400);
    }

    std::fs::remove_dir_all(&root).ok();
}
