//! CodeBuddy IDE 数据源：会话索引扫描、请求去重与 IDE 聚合。

use super::*;

/// CodeBuddy IDE 的数据根（`<home>/…/CodeBuddyExtension/Data`）。
///
/// 与登录态同源（`variant::extension_data_dir_at`）：早期实现用 `dirs::data_local_dir()`
/// 自己拼路径，不认 `WB_SHIFT_HOME`，于是沙箱里 IDE 一栏的用量读的是真实用户数据。
pub(super) fn codebuddy_extension_data_dir_at(home: &Path) -> PathBuf {
    crate::modules::variant::extension_data_dir_at(home, HostOs::current())
}

pub(super) fn is_ide_conversation_index(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some("index.json")
        && path
            .parent()
            .and_then(|parent| parent.parent())
            .and_then(|parent| parent.parent())
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            == Some("history")
}

pub(super) fn ide_index_files(root: &Path, output: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        // 同 `files()`：目录判定优先用目录项自带的类型，省掉一次 stat。
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        let path = entry.path();
        if is_dir || path.is_dir() {
            let name = path.file_name().and_then(|name| name.to_str());
            // Message bodies contain chat content and must not be scanned.
            // Checkpoints and the shared Public bucket are unrelated to usage.
            if !matches!(
                name,
                Some("messages" | "check-point" | "backups" | "Public")
            ) {
                ide_index_files(&path, output);
            }
        } else if is_ide_conversation_index(&path) {
            output.push(path);
        }
    }
}

pub(super) fn decode_genie_workspace(name: &str) -> Option<String> {
    use base64::Engine;
    let engine = base64::engine::general_purpose::STANDARD;
    let try_decode = |value: &str| -> Option<String> {
        let padded = match value.len() % 4 {
            0 => value.to_string(),
            remainder => format!("{value}{}", "=".repeat(4 - remainder)),
        };
        let bytes = engine.decode(padded).ok()?;
        let text = String::from_utf8(bytes).ok()?;
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.contains('\0') {
            return None;
        }
        Some(trimmed.to_string())
    };
    try_decode(name)
        .or_else(|| try_decode(&name.replace('_', "/")))
        .or_else(|| try_decode(&name.replace('_', "+")))
}

pub(super) fn ide_project_by_session() -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Some(root) = crate::modules::vscode_cn_inject::codebuddy_cn_data_dir().map(|dir| {
        dir.join("User")
            .join("globalStorage")
            .join("tencent-cloud.coding-copilot")
            .join("genie-history")
    }) else {
        return map;
    };
    let Ok(entries) = std::fs::read_dir(root) else {
        return map;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let folder = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let project = decode_genie_workspace(folder)
            .as_deref()
            .map(Path::new)
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .map(str::trim)
            .filter(|name| !name.is_empty() && name.len() <= 120)
            .unwrap_or("未知项目")
            .to_string();
        let Ok(conversations) = std::fs::read_dir(path.join("conversations")) else {
            continue;
        };
        for conversation in conversations.flatten() {
            if let Some(id) = conversation.file_name().to_str() {
                if !id.is_empty() {
                    map.insert(id.to_string(), project.clone());
                }
            }
        }
    }
    map
}

pub(super) fn ide_workspace_meta(conv_index: &Path, conv_id: &str) -> (Option<String>, String) {
    let Some(ws_index) = conv_index
        .parent()
        .and_then(|parent| parent.parent())
        .map(|parent| parent.join("index.json"))
    else {
        return (None, "未知模型".to_string());
    };
    let Ok(text) = std::fs::read_to_string(ws_index) else {
        return (None, "未知模型".to_string());
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return (None, "未知模型".to_string());
    };
    let Some(conversations) = value.get("conversations").and_then(Value::as_array) else {
        return (None, "未知模型".to_string());
    };
    for conversation in conversations {
        if conversation.get("id").and_then(Value::as_str) != Some(conv_id) {
            continue;
        }
        let title = non_empty_text(conversation.get("name"))
            .or_else(|| non_empty_text(conversation.get("title")));
        let model = non_empty_text(conversation.get("selectedModelId"))
            .or_else(|| non_empty_text(conversation.get("modelId")))
            .or_else(|| non_empty_text(conversation.get("model")))
            .unwrap_or_else(|| "未知模型".to_string());
        return (title, model);
    }
    (None, "未知模型".to_string())
}

pub(super) fn ide_request_usage(request: &Value) -> Option<Usage> {
    let object = request.get("usage")?.as_object()?;
    field(object, &["inputTokens", "input_tokens", "prompt_tokens"])?;
    Some(Usage {
        input: field(object, &["inputTokens", "input_tokens", "prompt_tokens"]).unwrap_or(0),
        output: field(
            object,
            &["outputTokens", "output_tokens", "completion_tokens"],
        )
        .unwrap_or(0),
        read: field(
            object,
            &[
                "cacheTokens",
                "cacheReadInputTokens",
                "cache_read_input_tokens",
            ],
        )
        .unwrap_or(0),
        write: positive_field(
            object,
            &[
                "cachedWriteTokens",
                "cacheWriteInputTokens",
                "cache_write_input_tokens",
                "cache_creation_input_tokens",
            ],
        )
        .unwrap_or(0),
    })
}

pub(super) fn ide_request_timestamp(request: &Value) -> Option<i64> {
    timestamp(request).or_else(|| {
        request.get("startedAt").and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_u64().and_then(|n| i64::try_from(n).ok()))
                .or_else(|| value.as_str()?.trim().parse::<i64>().ok())
        })
    })
}

/// 单条请求的指纹：优先「时间戳 + 用量」，缺时间戳时退回请求 id。
///
/// 副本之间是逐字重放——时间戳与用量都不变——所以这个指纹可以直接当并集去重的键；
/// 两者都没有就返回 `None`（不进并集，按原样计数，实测不存在这种记录）。
pub(super) fn ide_request_fingerprint(
    request: &Value,
    usage: &Usage,
    timestamp: Option<i64>,
) -> Option<String> {
    Some(match timestamp {
        Some(timestamp) => format!(
            "{timestamp}:{}:{}:{}:{}",
            usage.input, usage.output, usage.read, usage.write
        ),
        None => non_empty_text(request.get("id"))?.to_string(),
    })
}

/// 会话 id：会话目录名。副本会被重随机 id，所以它**不是**会话身份（见 [`ide_file_weight`]）。
pub(super) fn ide_session_id(path: &Path) -> String {
    path.parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("未知会话")
        .to_string()
}

/// 一份会话 index 的轻量指标：**会话指纹**（首个可计入请求的指纹，同一会话的所有副本
/// 共享它）、可计入的请求条数、token 总量。
///
/// 切号复制会让同一会话在多个账号数据目录下各留一份：副本整份重放父历史，之后各自继续
/// 追加——所以各份长短不一、内容并不完全相同，会话 id 也可能被重随机（插件侧
/// `CopyIdPolicy::AlwaysNew`）。会话身份因此不能按目录名判，只能按「同一个起点请求」判。
/// 指标里的条数/总量用于挑出最完整的那份，作为该会话的标题、模型与项目归属。
pub(super) fn ide_file_weight(path: &Path) -> Option<(String, usize, u64)> {
    let text = std::fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<Value>(&text).ok()?;
    let requests = value.get("requests")?.as_array()?;
    let mut key: Option<String> = None;
    let mut count = 0usize;
    let mut total = 0u64;
    for request in requests {
        let Some(usage) = ide_request_usage(request) else {
            continue;
        };
        if key.is_none() {
            key = ide_request_fingerprint(request, &usage, ide_request_timestamp(request));
        }
        count += 1;
        total = total.saturating_add(usage_total(usage));
    }
    Some((key?, count, total))
}

/// 会话文件归属的账号 uid：IDE 数据根下第一段目录名。
///
/// 实测布局是 `Data/<uid>/VSCode/<uid>/history/<md5(工作区)>/<会话>/index.json`，
/// 第一段就是切号后的账号目录；root 级文件（例如公共索引）取不到第一段 → 未归属。
/// 路径 uid 与账号库 `AccountMeta.uid` 理论上同源；对不上时前端按「uid 后 6 位」兜底，
/// 不强行猜。
pub(super) fn ide_uid(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .ok()
        .and_then(|relative| relative.components().next())
        .and_then(|component| component.as_os_str().to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or_default()
        .to_string()
}

/// 一次会话副本归并的共用参数（模型/项目/时间窗由会话元信息决定，与账号无关）。
pub(super) struct IdeRequestContext<'a> {
    pub(super) cutoff: Option<i64>,
    pub(super) model_name: &'a str,
    pub(super) project: &'a str,
    /// 会话 id：归属索引键的一部分（`ide:vscode:<sessionId>` 等）。
    pub(super) session_id: &'a str,
    /// 归属查询：`(索引, 命名空间)`；`None` 表示该来源不做归属覆盖（回退文件 uid）。
    pub(super) origin: Option<(&'a OriginIndex, &'static str)>,
    /// 这一份副本文件自身所属账号（记录命中归属段时会被覆盖）。
    pub(super) file_uid: &'a str,
}

/// 把一个会话（`requests` 是它的某一份副本）里可计入的请求并入统计。
///
/// `seen` 按请求指纹去重、跨副本共享（R6-07 起升级为**三个 IDE 来源共用一份**）：副本
/// 重放父历史时带的是原始时间戳，所以并集既不会把历史日期重复抬高，也不会丢掉各副本
/// 复制后各自追加的请求。指纹缺失的记录一律计入。
///
/// 归属按**记录**走（R6-07）：先用 `(namespace, sessionId, ts)` 查归属段，命中即用段里的
/// 账号；没有条目/记录晚于所有段/无时间戳 → 回退这一份副本所在账号（文件 uid）。于是
/// 「重放的历史归来源账号、副本自己的新增归副本账号」不再依赖文件 mtime 顺序。
pub(super) fn collect_ide_requests(
    collector: &mut SourceCollector,
    requests: &[Value],
    context: &IdeRequestContext<'_>,
    seen: &mut HashSet<String>,
    totals_by_uid: &mut HashMap<String, Totals>,
) {
    for request in requests {
        let ts = ide_request_timestamp(request);
        if context
            .cutoff
            .is_some_and(|minimum| ts.is_none_or(|value| value < minimum))
        {
            continue;
        }
        let Some(usage) = ide_request_usage(request) else {
            continue;
        };
        if let Some(fingerprint) = ide_request_fingerprint(request, &usage, ts) {
            // 同一请求已在另一份副本 / 另一个 IDE 来源里计过。
            if !seen.insert(fingerprint) {
                continue;
            }
        }
        let attributed = context
            .origin
            .and_then(|(index, namespace)| index.attribute(namespace, context.session_id, ts));
        let uid = attributed.as_deref().unwrap_or(context.file_uid);
        totals_by_uid.entry(uid.to_string()).or_default().add(usage);
        collector.add(usage, context.model_name, context.project, ts, uid);
    }
}

/// 只在「某个客户端子目录」下收集会话索引（第四轮 06：Token 统计按客户端拆 tab）。
///
/// 真实布局是 `Data/<uid>/<client>/<uid>/history/...`，`client` 固定为
/// `VSCode` / `JetBrains` / `CodeBuddyIDE` / `WorkBuddy`。早期实现整棵 `Data/**` 递归，
/// 把三个客户端混在一个 tab 里（JetBrains 被当成 IDE 的一部分），也无法排除
/// `WorkBuddy/` 那份由客户端内置扩展写的空壳。
pub(super) fn ide_client_index_files(root: &Path, client: &str, output: &mut Vec<PathBuf>) {
    let Ok(uids) = std::fs::read_dir(root) else {
        return;
    };
    for uid in uids.flatten() {
        let client_dir = uid.path().join(client);
        if client_dir.is_dir() {
            ide_index_files(&client_dir, output);
        }
    }
}

/// 全量版本：扫描整棵 `Data/**`（仅测试与旧调用路径使用；生产已按客户端拆分）。
#[cfg(test)]
pub(super) fn ide_source(
    root: PathBuf,
    name: &str,
    cutoff: Option<i64>,
    project_by_session: &HashMap<String, String>,
    attribution: &mut Attribution,
) -> Value {
    let mut paths = Vec::new();
    ide_index_files(&root, &mut paths);
    ide_source_from_paths(
        root,
        name,
        paths,
        cutoff,
        project_by_session,
        attribution,
        None,
    )
}

/// 按客户端目录收集后聚合：解析、去重与账号归因逻辑与全量版完全一致。
pub(super) fn ide_client_source(
    root: PathBuf,
    client: &str,
    name: &str,
    cutoff: Option<i64>,
    project_by_session: &HashMap<String, String>,
    attribution: &mut Attribution,
) -> Value {
    let mut paths = Vec::new();
    ide_client_index_files(&root, client, &mut paths);
    // 命名空间严格用写入侧同一份映射（`VSCode → ide:vscode` 等），未知客户端不覆盖。
    let namespace = Attribution::ide_namespace(client);
    ide_source_from_paths(
        root,
        name,
        paths,
        cutoff,
        project_by_session,
        attribution,
        namespace,
    )
}

fn ide_source_from_paths(
    root: PathBuf,
    name: &str,
    paths: Vec<PathBuf>,
    cutoff: Option<i64>,
    project_by_session: &HashMap<String, String>,
    attribution: &mut Attribution,
    namespace: Option<&'static str>,
) -> Value {
    // 复用 `Target`：mtime 只取一次，既用于「副本按落地顺序处理」，也用于窗口裁剪。
    let mut targets: Vec<Target> = paths
        .into_iter()
        .map(|path| Target::new(root.clone(), path))
        .collect();
    // 组内必须按 mtime 从旧到新处理：重放的历史请求归最早出现的那份文件（原账号），
    // 副本复制之后新增的请求才归副本账号。同一 mtime 用路径兜底保证顺序稳定。
    targets.sort_by(|left, right| {
        left.modified_ms
            .unwrap_or(i64::MAX)
            .cmp(&right.modified_ms.unwrap_or(i64::MAX))
            .then_with(|| left.path.cmp(&right.path))
    });
    let uids: Vec<String> = targets
        .iter()
        .map(|target| ide_uid(&root, &target.path))
        .collect();

    // 同一会话（按起点请求判身份）因切号复制可能散落在多个账号数据目录里。各份之间按
    // 请求指纹取**并集**：父历史只算一次（副本会把历史日期回溯性抬高），副本复制后各自
    // 追加的请求也都保留（只取「最长一份」会漏掉别的副本独有的请求）。会话标题、模型与
    // 项目归属取最完整的那份。
    let weights: Vec<Option<(String, usize, u64)>> = targets
        .iter()
        .map(|target| ide_file_weight(&target.path))
        .collect();
    let mut groups: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut lead: HashMap<&str, (usize, usize, u64)> = HashMap::new();
    for (index, weight) in weights.iter().enumerate() {
        let Some((fingerprint, count, total)) = weight else {
            continue;
        };
        groups.entry(fingerprint.as_str()).or_default().push(index);
        let replace = lead
            .get(fingerprint.as_str())
            .is_none_or(|current| (*count, *total) > (current.1, current.2));
        if replace {
            lead.insert(fingerprint.as_str(), (index, *count, *total));
        }
    }

    let mut collector = SourceCollector::default();

    // 归属索引的只读视图 + 「三个 IDE 来源共用」的指纹集合 + 存活键：三者是 `Attribution`
    // 的不同字段，拆开借用后互不干扰。
    let Attribution {
        origin,
        alive_keys,
        ide_seen,
        ..
    } = attribution;
    // 存活键：这一轮在磁盘上见到的会话（与窗口无关，仅用于统计结束后的 GC）。
    if let Some(namespace) = namespace {
        for target in &targets {
            let session_id = ide_session_id(&target.path);
            if !session_id.is_empty() {
                alive_keys.insert(key_of(namespace, &session_id));
            }
        }
    }

    for (index, _) in targets.iter().enumerate() {
        let fingerprint = weights[index]
            .as_ref()
            .map(|(fingerprint, _, _)| fingerprint.as_str());
        // 同一会话只在它的第一份副本处处理一次，并把其余副本一起并入。读不出会话指纹
        // （读取/解析失败）的文件照旧单独走一遍，让解析错误被记一笔。
        if let Some(fingerprint) = fingerprint {
            if groups.get(fingerprint).and_then(|members| members.first()) != Some(&index) {
                continue;
            }
        }
        // 标题/模型/项目取最完整的那份：副本 id 被重随机过，元信息不一定都齐全。
        let lead_index = fingerprint
            .and_then(|fingerprint| lead.get(fingerprint).map(|current| current.0))
            .unwrap_or(index);
        let lead_session_id = ide_session_id(&targets[lead_index].path);
        let (title, model_name) = ide_workspace_meta(&targets[lead_index].path, &lead_session_id);
        let project = project_by_session
            .get(&lead_session_id)
            .cloned()
            .unwrap_or_else(|| "未知项目".to_string());
        let members: &[usize] = match fingerprint {
            Some(fingerprint) => groups
                .get(fingerprint)
                .map(Vec::as_slice)
                .unwrap_or_default(),
            None => std::slice::from_ref(&index),
        };
        // 每个账号一份会话合计：同一会话跨账号时各出一行（各自有用量），键加序号避免撞键。
        let context = IdeRequestContext {
            cutoff,
            model_name: &model_name,
            project: &project,
            session_id: &lead_session_id,
            origin: namespace.map(|namespace| (&*origin, namespace)),
            file_uid: "",
        };
        let mut totals_by_uid: HashMap<String, Totals> = HashMap::new();
        for member in members {
            let Ok(text) = std::fs::read_to_string(&targets[*member].path) else {
                collector.note_parse_error();
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(&text) else {
                collector.note_parse_error();
                continue;
            };
            let Some(requests) = value.get("requests").and_then(Value::as_array) else {
                continue;
            };
            // 每一份副本都带自己的**会话 id**（副本会被重随机 id）与自己的账号目录：
            // 归属段按成员自己的 id 查，记录时间戳决定它是「继承来的历史」还是「自己新增的」。
            let member_session_id = ide_session_id(&targets[*member].path);
            let context = IdeRequestContext {
                session_id: &member_session_id,
                file_uid: &uids[*member],
                ..context
            };
            collect_ide_requests(
                &mut collector,
                requests,
                &context,
                ide_seen,
                &mut totals_by_uid,
            );
        }

        let mut ordered: Vec<(String, Totals)> = totals_by_uid.into_iter().collect();
        // 固定顺序（uid 升序）让同一份数据的响应可重复对比。
        ordered.sort_by(|left, right| left.0.cmp(&right.0));
        for (uid, totals) in ordered {
            collector.push_session(
                lead_session_id.clone(),
                title.clone(),
                project.clone(),
                uid,
                totals,
            );
        }
    }

    collector.into_value(name, targets.len())
}
