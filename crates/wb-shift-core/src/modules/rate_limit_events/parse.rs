//! payload 解析与账号归因：把 hook payload 变成限额事件并确定归属账号。

use super::state::*;
use super::*;

/// transcript 首行上限：`session-meta` 很小；防止无换行的坏文件把数十 MB 读进内存。
pub(super) const TRANSCRIPT_HEAD_LIMIT: u64 = 64 * 1024;

// ---------------------------------------------------------------------------
// payload 解析
// ---------------------------------------------------------------------------

/// hook payload 里本模块用到的字段（其余字段与版本差异一律忽略）。
///
/// 历史事件行可能带已废弃的 `_hookTs`（旧脚本写入的时刻锚）：它不在结构里，
/// `serde` 默认忽略未知字段，因此这些行照常消费。
#[derive(Deserialize)]
pub(super) struct HookPayload {
    #[serde(default)]
    pub(super) transcript_path: Option<String>,
    #[serde(default)]
    pub(super) session_id: Option<String>,
    #[serde(default)]
    pub(super) model: Option<String>,
    #[serde(default)]
    pub(super) last_assistant_message: Option<String>,
}

/// 事件的客户端来源（由 `transcript_path` 前缀判定）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HookSource {
    /// CodeBuddy CLI（`~/.codebuddy/projects/…`）。
    Cli,
    /// WorkBuddy 客户端（`~/.workbuddy` / `~/.workbuddy-ai`）。
    WorkBuddy(WbVariant),
}

impl HookSource {
    /// 对外来源标签（与 `limits::ScanSource::label()` 同一套字符串）。
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Cli => "codebuddy-cli",
            Self::WorkBuddy(WbVariant::Cn) => "workbuddy-cn",
            Self::WorkBuddy(WbVariant::Ai) => "workbuddy-ai",
        }
    }
}

/// 一行 payload 里识别出的限额事件（来源判定需要环境，见 `source_of`）。
pub(super) struct QuotaEvent {
    pub(super) transcript_path: String,
    pub(super) session_id: Option<String>,
    pub(super) model: Option<String>,
    pub(super) reset_at: i64,
    /// 该 429 请求**自身**的时刻（由文案尾部 requestId 解码）。
    ///
    /// 归因判据用它而不是消费时刻：`Stop` 的 `last_assistant_message` 是「会话最后一条助手
    /// 消息」，会话恢复 / 压缩后可能是更早一轮（更早账号）的 429 文案。取不到 ⇒ `None`
    /// （调用方丢弃该事件，不回退到消费时刻）。
    pub(super) request_at: Option<i64>,
}

/// requestId 时刻的合理下界（2024-01-01T00:00:00Z，毫秒）。
///
/// UUIDv7 是 2024 年才有的格式：早于此的「时刻」说明取到的不是请求时刻，按解析失败处理。
pub(super) const REQUEST_TIME_FLOOR_MS: i64 = 1_704_067_200_000;

/// 从限额文案里取「该 429 请求自身的时刻」（毫秒）。
///
/// 文案尾部形如 `… 继续使用。 (01a0c3baf1dc7cebb43642413d3ad593/01a0c392-…-…)`：
/// requestId 是 32 位十六进制 UUIDv7，前 12 位十六进制 = 毫秒时刻（本机实测与 CLI 日志
/// `[ModelProvider] Sending request` 相差 ~80ms，见
/// `.trellis/tasks/09-21-fix-stale-429-attribution/research/2026-09-21-stale-429-replay-evidence.md`）。
///
/// 校验：32 位十六进制 + version nibble == `7` + variant == `0b10` + 时刻 ≥ 2024-01-01；
/// 任一不满足 → `None`（调用方丢弃该事件）。上界（`> now`）由 `attribute_cli` 判定。
pub(super) fn request_time_of(message: &str) -> Option<i64> {
    // 取「最后一个 `(` … 其后第一个 `/`」：文案里的括号只出现在尾部身份段，
    // 从最后一个左括号找起可避免吃到正文里的括号（`(` 与 `/` 都是 ASCII，索引必在字符边界）。
    let start = message.rfind('(')? + 1;
    let rest = &message[start..];
    let hex = &rest[..rest.find('/')?];
    if hex.len() != 32 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    // UUIDv7：version nibble（第 13 位十六进制）必须是 `7`。
    if hex.as_bytes()[12] != b'7' {
        return None;
    }
    // variant：第 17 位十六进制的高两位必须是 `0b10`。
    let variant = u8::from_str_radix(&hex[16..18], 16).ok()?;
    if variant >> 6 != 0b10 {
        return None;
    }
    let at = i64::from_str_radix(&hex[..12], 16).ok()?;
    (at >= REQUEST_TIME_FLOOR_MS).then_some(at)
}

/// 一行 payload → 限额事件；非限额行、解析失败、缺恢复时刻统一返回 None（静默忽略）。
pub(super) fn parse_quota_line(line: &str) -> Option<QuotaEvent> {
    let payload: HookPayload = serde_json::from_str(line.trim()).ok()?;
    // 限额文案在 `last_assistant_message` 里；`FinalStop` 等没有该字段的事件行直接忽略。
    let message = payload.last_assistant_message.as_deref()?;
    if !limits::QUOTA_MARKERS
        .iter()
        .any(|marker| message.contains(marker))
    {
        return None;
    }
    // 取不到官方恢复时刻就不是可入账的限额事件（不猜时间）。
    let reset_at = limits::parse_reset_at(message)?;
    // 模型直接采用**本次 payload** 的字段：它是 429 当轮客户端侧选定的模型，
    // 与失败请求同轮（不读 transcript 全文猜模型 —— 延迟消费时那段文本已经属于
    // 之后的轮次，会给出别的模型，2026-09-19 实证假 chip）。
    let model = payload
        .model
        .filter(|model| !limits::MODEL_UNKNOWN.contains(&model.as_str()))
        .filter(|model| !model.trim().is_empty());
    Some(QuotaEvent {
        transcript_path: payload.transcript_path?,
        session_id: payload.session_id,
        model,
        reset_at,
        // 请求时刻只从**文案本身**取：payload 的其它字段（如 `generation_id`）是「轮」的 id，
        // 会话恢复后指向新轮，用它判陈旧会漏判（见 research 文档的取舍）。
        request_at: request_time_of(message),
    })
}

/// 由 `transcript_path` 前缀判定来源；不匹配（IDE 路径、空路径、未知客户端）返回 None。
pub(super) fn source_of(
    transcript_path: &str,
    roots: &[(PathBuf, HookSource)],
) -> Option<HookSource> {
    let path = Path::new(transcript_path);
    roots
        .iter()
        .find(|(root, _)| path.starts_with(root))
        .map(|(_, source)| *source)
}

// ---------------------------------------------------------------------------
// 归因
// ---------------------------------------------------------------------------

/// 归因所需的路径与账号库（显式传入，单测不得触碰真实用户数据）。
pub(super) struct IngestContext<'a> {
    pub(super) events: PathBuf,
    pub(super) state_path: PathBuf,
    pub(super) accounts: Vec<Value>,
    /// transcript 路径前缀 → 来源。
    pub(super) source_roots: Vec<(PathBuf, HookSource)>,
    /// CLI 当前生效账号的 `activeAccountId` 状态文件。
    pub(super) cli_state_path: PathBuf,
    /// CLI 运行中会话注册表目录（`~/.codebuddy/sessions`）：`sessionId → 进程启动时刻`。
    ///
    /// 路径由 `codebuddy_cli::sessions_dir()` 给出（唯一拼接点），这里只做注入以便单测。
    pub(super) cli_sessions_dir: PathBuf,
    /// 两档位的客户端登录态文件。
    pub(super) auth_file_paths: Vec<(WbVariant, PathBuf)>,
    /// 兜底：会话 id → 账号 id（默认实现查 `sessions` 表）。
    pub(super) session_lookup: &'a dyn Fn(WbVariant, &str) -> Option<String>,
}

impl IngestContext<'_> {
    /// 真实运行环境（读真实账号库与真实状态文件）。
    pub(super) fn real() -> IngestContext<'static> {
        let mut source_roots = vec![(home_dir().join(limits::CLI_DATA_DIR), HookSource::Cli)];
        source_roots.extend(
            WbVariant::ALL
                .into_iter()
                .map(|variant| (variant.data_root(), HookSource::WorkBuddy(variant))),
        );
        IngestContext {
            events: rate_limit_hook::events_path(),
            state_path: state_path(),
            accounts: account::load_accounts(),
            source_roots,
            cli_state_path: home_dir()
                .join(limits::CLI_ROTATE_DIR)
                .join(limits::CLI_STATE_FILE),
            cli_sessions_dir: codebuddy_cli::sessions_dir(),
            auth_file_paths: WbVariant::ALL
                .into_iter()
                .map(|variant| (variant, variant.auth_file_path()))
                .collect(),
            session_lookup: &session_account_lookup,
        }
    }
}

/// 默认的会话兜底查询：与日志扫描共用 `limits::account_by_session`。
pub(super) fn session_account_lookup(variant: WbVariant, session_id: &str) -> Option<String> {
    let sessions = std::collections::BTreeSet::from([session_id.to_string()]);
    limits::account_by_session(variant, &sessions)
        .get(session_id)
        .cloned()
}

/// 账号库里按 uid 找账号 id（uid 在库内唯一，不按档位过滤）。
pub(super) fn account_id_by_uid(accounts: &[Value], uid: &str) -> Option<String> {
    accounts
        .iter()
        .find(|acc| account::get_str(acc, "uid").as_deref() == Some(uid))
        .and_then(|acc| account::get_str(acc, "id"))
}

/// 账号库里按 id 找账号（确认状态文件里的 id 仍然有效）。
pub(super) fn account_exists(accounts: &[Value], account_id: &str) -> bool {
    accounts
        .iter()
        .any(|acc| account::get_str(acc, "id").as_deref() == Some(account_id))
}

/// CLI 轮换状态文件里的当前生效账号（账号库 id）。
pub(super) fn cli_active_account_id(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let root: Value = serde_json::from_str(&text).ok()?;
    root.get("activeAccountId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// 客户端登录态文件里的 uid（`account.uid`，兼容根节点 `uid`）。
pub(super) fn auth_file_uid(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let root: Value = serde_json::from_str(&text).ok()?;
    root.get("account")
        .and_then(|account| account.get("uid"))
        .and_then(Value::as_str)
        .or_else(|| root.get("uid").and_then(Value::as_str))
        .map(str::trim)
        .filter(|uid| !uid.is_empty())
        .map(str::to_string)
}

/// 事件所属会话「当前进程」的启动时刻（= 该进程取 key 的时刻）。
///
/// 读 `~/.codebuddy/sessions/*.json`，取 `sessionId` 匹配且 `startedAt ≤ at` 的**最大**
/// `startedAt`：跨进程接管后旧注册表文件可能残留，这个判据保证取「`at` 时刻最后一个持有者」。
/// 调用方传该 429 请求的时刻（不是消费时刻，见 `attribute_cli`）。
/// 目录缺失 / 无匹配 / 文件坏 / `session_id` 为空 → None（调用方回落 transcript 首行）。
pub(super) fn session_snapshot_time(dir: &Path, session_id: Option<&str>, at: i64) -> Option<i64> {
    let session_id = session_id.map(str::trim).filter(|id| !id.is_empty())?;
    let mut snapshot: Option<i64> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(root) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if root.get("sessionId").and_then(Value::as_str) != Some(session_id) {
            continue;
        }
        let Some(started_at) = root.get("startedAt").and_then(Value::as_i64) else {
            continue;
        };
        if started_at > at {
            continue;
        }
        snapshot = Some(snapshot.map_or(started_at, |current| current.max(started_at)));
    }
    snapshot
}

/// transcript 首行（`session-meta`）的会话创建时刻；只读第一行（文件可达数十 MB）。
///
/// 会话可跨进程复用，所以这是**兜底**：只用于注册表没有该会话时的近似。
pub(super) fn session_created_at(transcript_path: &str) -> Option<i64> {
    let file = std::fs::File::open(transcript_path).ok()?;
    let mut line = String::new();
    std::io::BufReader::new(file.take(TRANSCRIPT_HEAD_LIMIT))
        .read_line(&mut line)
        .ok()?;
    let root: Value = serde_json::from_str(&line).ok()?;
    root.get("timestamp").and_then(Value::as_i64)
}

/// CLI 状态文件快照：当前生效账号 id + 文件 mtime（毫秒）。
///
/// mtime 不依赖文件内 `updatedAt` 字段（缺该字段的旧文件也要能用）。
/// 文件缺失 / 非 JSON / 无 `activeAccountId` → None（该事件丢弃）。
pub(super) fn cli_state_snapshot(path: &Path) -> Option<(String, Option<i64>)> {
    let current_id = cli_active_account_id(path)?;
    let mtime = std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64);
    Some((current_id, mtime))
}

/// CLI 归因：按「该 429 请求自身的时刻」取会话进程快照，判断这把 key 属于谁。
///
/// 判据必须落在**请求时刻**而不是消费时刻：`Stop` 的 `last_assistant_message` 是「会话最后一条
/// 助手消息」，会话恢复 / 压缩后会连同**上一轮（可能是上一个账号）**的 429 文案一起被重放，
/// 而触发 hook 的却是接管会话的新进程；用消费时刻取快照就会把旧账号的 429 记到新账号
/// （2026-09-21 本机实证，见
/// `.trellis/tasks/09-21-fix-stale-429-attribution/research/2026-09-21-stale-429-replay-evidence.md`）。
///
/// 不变式（INV）：活着的 CLI 进程持有的 key == `state.json` 的 `activeAccountId`。
/// 由两条写路径共同维持——手动切换先关进程再写 state；自动轮换只在无存活会话时切。
/// 于是归因只需要一个陈旧守卫（`snap_at` = 请求时刻该会话的进程启动时刻）：
///
/// - `snap_at ≥ state.mtime`：进程在最后一次切换之后启动 ⇒ 当前账号就是它的 key；
/// - `snap_at < state.mtime`：进程在切换之前启动且（按 INV）本该已经被关掉 ⇒
///   INV 被破坏（第三方改写 state / 关闭失败 / 时钟异常）⇒ **丢弃 + 告警**，
///   绝不猜"也许是切换前那个账号"；
/// - `request_at` 取不到 / 晚于当前时刻 / `snap_at` 或 `mtime` 取不到 ⇒ 丢弃
///   （宁可少显示，不可显示错账号）。
pub(super) fn attribute_cli(event: &QuotaEvent, ctx: &IngestContext, now: i64) -> Option<String> {
    // 证据不足即丢弃：不回退到「按消费时刻归因」——那正是陈旧文案错归的来源。
    let Some(request_at) = event.request_at else {
        eprintln!("[cli-attribution] 丢弃限额事件：文案里取不到 requestId，无法确定 429 请求时刻");
        return None;
    };
    if request_at > now {
        eprintln!(
            "[cli-attribution] 丢弃限额事件：429 请求时刻 {request_at} 晚于当前时刻 {now}（时钟异常）"
        );
        return None;
    }
    // 快照查询上界 = 请求时刻：只认「该请求发出时已在运行的进程」，不会被后来接管会话的
    // 新进程顶替。配合下面的 `snap_at ≥ mtime` 判据，自动蕴含「请求发出于最后一次切换之后」。
    //
    // 快照取不到时不再直接丢弃：新版 CLI 的 `~/.codebuddy/sessions/*.json` 只在交互式会话
    // 存活期间写，长时间运行的会话结束后文件可能已被清理，而会话库里的归属才是持久事实。
    let snap_at = session_snapshot_time(
        &ctx.cli_sessions_dir,
        event.session_id.as_deref(),
        request_at,
    )
    .or_else(|| session_created_at(&event.transcript_path));
    let Some(snap_at) = snap_at else {
        eprintln!(
            "[cli-attribution] 进程快照与 transcript 首行都取不到，改用会话库归属兜底"
        );
        return cli_session_owner(event, ctx);
    };
    if snap_at > now {
        // 时钟异常（transcript 首行时间在未来）时不猜。
        return None;
    }
    let Some((current_id, mtime)) = cli_state_snapshot(&ctx.cli_state_path) else {
        // 从没用本工具切过 CLI 账号时该文件不存在（2026-11-28 本机实证）：此时两条旧判据
        // 全部失效，走会话库兜底，而不是把 429 整条丢掉。
        eprintln!(
            "[cli-attribution] 读不到 CLI 轮换状态文件，改用会话库归属兜底（事件不会被丢弃）"
        );
        return cli_session_owner(event, ctx);
    };
    let Some(mtime) = mtime else {
        eprintln!(
            "[cli-attribution] 丢弃限额事件：无法读取 state.json 的写入时刻，无法判断归因是否成立"
        );
        return cli_session_owner(event, ctx);
    };
    if snap_at >= mtime {
        return account_exists(&ctx.accounts, &current_id).then_some(current_id);
    }
    eprintln!(
        "[cli-attribution] 丢弃限额事件：会话进程启动于 {snap_at}，早于 state.json 的写入时刻 {mtime}；\
         该 429 请求（{request_at}）发出时活进程持有的 key 与当前账号不一致（不变式被破坏）"
    );
    None
}

/// 最终兜底：用客户端自己记的会话归属（`~/.codebuddy/codebuddy.db` 的
/// `conversations.user_id`）反查账号库。
///
/// 为什么这条链是可信的：归属由 CLI/IDE 自己写，且与「谁在持 key」是同一事实 —— 会话属于
/// 哪个账号，它发出的 429 就记在哪个账号头上。前两条链（日志内鉴权 uid、轮换状态文件）
/// 都依赖本工具介入过才存在的数据，这条不依赖。
///
/// 仍然守「宁可少显示，不可错归」：会话 id 缺失、库里查不到、或 uid 不在账号库里 → `None`。
fn cli_session_owner(event: &QuotaEvent, ctx: &IngestContext) -> Option<String> {
    let session_id = event
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())?;
    let owner_uid = crate::modules::codebuddy_ide_session::conversation_account_uids(&[
        session_id.to_string(),
    ])
    .remove(session_id)?;
    let account_id = account_id_by_uid(&ctx.accounts, &owner_uid)?;
    eprintln!(
        "[cli-attribution] 会话 {session_id} 归属 uid {owner_uid} → 账号 {account_id}（会话库兜底）"
    );
    Some(account_id)
}

/// 归因：payload 不含账号 uid，只能靠客户端侧「谁在持 key」。
///
/// - CLI：该 429 请求发出时该会话的进程启动时刻不早于 `state.json` 写入时刻 → 当前账号
///   （见 `attribute_cli`）；
/// - WorkBuddy：① 登录态文件 uid → 账号库；② 会话 id 查 `sessions` 表兜底；
/// - 都拿不到 → 丢弃（宁可少显示，不可显示错账号）。
pub(super) fn attribute(
    event: &QuotaEvent,
    source: HookSource,
    ctx: &IngestContext,
    now: i64,
) -> Option<String> {
    match source {
        HookSource::Cli => attribute_cli(event, ctx, now),
        HookSource::WorkBuddy(variant) => {
            let uid = ctx
                .auth_file_paths
                .iter()
                .find(|(candidate, _)| *candidate == variant)
                .and_then(|(_, path)| auth_file_uid(path));
            if let Some(account_id) = uid.and_then(|uid| account_id_by_uid(&ctx.accounts, &uid)) {
                return Some(account_id);
            }
            (ctx.session_lookup)(variant, event.session_id.as_deref()?)
        }
    }
}
