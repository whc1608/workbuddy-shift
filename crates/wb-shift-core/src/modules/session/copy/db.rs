//! 复制写库：数据库一致性备份、sessions 行读取 / 插入 / 归一与校验。
use super::*;
/// 备份 workbuddy.db（含 -wal/-shm），返回主库备份路径。
///
/// 任何一步失败都返回 Err——不能沿用「忽略 copy 错误后仍宣称备份成功」的旧行为，
/// 备份不可信时后续数据库写入必须先停下来（design §1）。
pub(in crate::modules::session) fn backup_workbuddy_db(
    paths: &SessionPaths,
    backup_root: &Path,
) -> Result<PathBuf, String> {
    let db = paths.workbuddy_db();
    if !db.is_file() {
        return Err("会话数据不存在，未复制".to_string());
    }
    std::fs::create_dir_all(backup_root).map_err(|error| format!("备份目录创建失败：{error}"))?;
    for suffix in ["", "-wal", "-shm"] {
        let src = PathBuf::from(format!("{}{}", db.to_string_lossy(), suffix));
        if !src.is_file() {
            continue;
        }
        let dest = backup_root.join(format!("workbuddy.db{suffix}"));
        std::fs::copy(&src, &dest).map_err(|error| format!("备份 {suffix} 失败：{error}"))?;
        let (src_len, dest_len) = (
            std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0),
            std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0),
        );
        if src_len != dest_len {
            return Err(format!("备份 {suffix} 校验失败：大小不一致，未复制"));
        }
    }
    Ok(backup_root.join("workbuddy.db"))
}

/// 数据库插入结果：`No*` 与 `SourceRowMissing` 都不允许被当成成功。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::modules::session) enum DbCopyOutcome {
    /// `aux_note` 非空表示目标库缺账本表/列，副本不含用量记录（不算失败，只说明原因）。
    /// `space_note` 非空表示目标账号同一目录命中多个空间、本次按目录新建了分组。
    Inserted {
        aux_note: Option<&'static str>,
        space_note: Option<&'static str>,
    },
    SourceRowMissing,
    NoSessionsTable,
    NoDb,
}

/// 会话级账本写入失败的错误前缀：前端据此给出「仍然复制（不带用量）」的显式忽略入口。
pub(in crate::modules::session) const AUX_WRITE_FAILED_PREFIX: &str = "会话用量记录保存失败：";

/// 目标客户端不支持会话账本时的诊断文案（旧版库缺表/缺列）。
const AUX_UNSUPPORTED_NOTE: &str = "目标客户端不支持会话用量记录，副本不含账本";

/// R6-08：默认口径（`copy_usage = false`）下随复制报告返回的说明。
///
/// 副本的「共消耗」会显示 0/偏少，必须让用户知道这是**设计如此**（历史消耗留在来源
/// 账号），否则会以为丢数据。
pub(in crate::modules::session) const AUX_USAGE_LEFT_IN_SOURCE_NOTE: &str =
    "历史消耗归来源账号，副本只记录自己的新增";

/// 该错误是否属于可以「显式忽略」的账本写入失败。
pub(in crate::modules::session) fn is_aux_write_failure(error: &str) -> bool {
    error.starts_with(AUX_WRITE_FAILED_PREFIX)
}

/// 源会话的账本（用量 + 中断记录），由 [`insert_session_copy`] 在同一事务里搬运。
#[derive(Debug, Default)]
struct SessionAux {
    usage: Option<SessionUsageRow>,
    interruptions: Vec<SessionInterruptionRow>,
}

#[derive(Debug)]
struct SessionUsageRow {
    used: i64,
    size: i64,
    credit_json: Option<String>,
}

#[derive(Debug)]
struct SessionInterruptionRow {
    request_id: String,
    reason_code: Option<String>,
    presentation: Option<String>,
}

fn aux_error(error: &rusqlite::Error) -> String {
    format!("{AUX_WRITE_FAILED_PREFIX}{error}")
}

/// 按开关裁剪地读账本：`want_usage = false` 时**根本不查** `session_usage`
/// （R6-08 的「不搬历史消耗」从数据源头保证，而不是读了再丢）。
///
/// 三种「没有数据可搬」的情况返回空账本而不是失败：表不存在、缺关键列、该会话无行
/// （旧库/未产生用量）。表存在但查询报错才算失败——那种情况静默跳过会让副本悄悄少数据。
fn read_session_aux_filtered(
    conn: &Connection,
    sid: &str,
    want_usage: bool,
    want_interruptions: bool,
) -> Result<SessionAux, String> {
    let mut aux = SessionAux::default();
    if want_usage
        && table_exists(conn, "session_usage")
        && column_exists(conn, "session_usage", "used")
        && column_exists(conn, "session_usage", "size")
        && column_exists(conn, "session_usage", "credit_json")
    {
        match conn.query_row(
            "SELECT used, size, credit_json FROM session_usage WHERE session_id = ?1",
            [sid],
            |row| {
                Ok(SessionUsageRow {
                    used: row.get::<_, Option<i64>>(0)?.unwrap_or_default(),
                    size: row.get::<_, Option<i64>>(1)?.unwrap_or_default(),
                    credit_json: row.get(2)?,
                })
            },
        ) {
            Ok(usage) => aux.usage = Some(usage),
            Err(rusqlite::Error::QueryReturnedNoRows) => {}
            Err(error) => return Err(aux_error(&error)),
        }
    }
    if want_interruptions
        && table_exists(conn, "session_turn_interruptions")
        && column_exists(conn, "session_turn_interruptions", "request_id")
    {
        let has_reason = column_exists(conn, "session_turn_interruptions", "reason_code");
        let has_presentation = column_exists(conn, "session_turn_interruptions", "presentation");
        let reason_col = if has_reason { "reason_code" } else { "NULL" };
        let presentation_col = if has_presentation {
            "presentation"
        } else {
            "NULL"
        };
        let sql = format!(
            "SELECT request_id, {reason_col}, {presentation_col} \
             FROM session_turn_interruptions WHERE session_id = ?1 ORDER BY request_id"
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| aux_error(&e))?;
        let rows = stmt
            .query_map([sid], |row| {
                Ok(SessionInterruptionRow {
                    request_id: row.get(0)?,
                    reason_code: row.get(1)?,
                    presentation: row.get(2)?,
                })
            })
            .map_err(|e| aux_error(&e))?;
        for row in rows {
            aux.interruptions.push(row.map_err(|e| aux_error(&e))?);
        }
    }
    Ok(aux)
}

/// `copy_usage = false` 时副本写的零值行内容（`used = 0`, `size = 0`, `credit_json = '{}'`）。
///
/// R6-08 的取舍：**写零值行而不是不写行**。客户端会话详情的「共消耗」直接读
/// `session_usage`，历史版本对「副本没有这一行」的行为未经验证（可能显示异常/NaN）；
/// 零值行是只增不改的占位——`ON CONFLICT DO NOTHING` 保证它**绝不会**覆盖客户端
/// 复制后自己写下的真实用量。这是「绝不写来源数值」的延伸。
const WRITE_ZERO_USAGE_ROW: bool = true;

/// 零值行的 `credit_json`：空对象（不是 `NULL`，避免客户端按「无字段」解析失败）。
const ZERO_USAGE_CREDIT_JSON: &str = "{}";

/// 在写事务内搬运账本；目标库缺表/缺列时跳过并返回 `false`（不算失败）。
///
/// 本函数只写两张账本表：不碰 `automations` / `automation_runs` / `workspaces` /
/// `cloud_sync_ledger`，也不改写 `credit_json` 的键与数值（原样搬运，不重算不取整）。
///
/// R6-08：`copy_usage = false`（默认）时**绝不写来源数值**——历史消耗留在来源账号，
/// 副本只累计自己的新增（「谁用的记在谁头上」）。
fn write_session_aux(
    tx: &rusqlite::Transaction<'_>,
    sid: &str,
    aux: &SessionAux,
    now: i64,
    copy_usage: bool,
) -> Result<bool, String> {
    let mut supported = true;
    let usage_usable = table_exists(tx, "session_usage")
        && column_exists(tx, "session_usage", "used")
        && column_exists(tx, "session_usage", "size")
        && column_exists(tx, "session_usage", "updated_at")
        && column_exists(tx, "session_usage", "credit_json");
    if copy_usage {
        if let Some(usage) = &aux.usage {
            if usage_usable {
                tx.execute(
                    "INSERT INTO session_usage (session_id, used, size, updated_at, credit_json) \
                     VALUES (?1, ?2, ?3, ?4, ?5) \
                     ON CONFLICT(session_id) DO UPDATE SET \
                       used = excluded.used, size = excluded.size, \
                       updated_at = excluded.updated_at, credit_json = excluded.credit_json",
                    rusqlite::params![sid, usage.used, usage.size, now, usage.credit_json],
                )
                .map_err(|e| aux_error(&e))?;
            } else {
                supported = false;
            }
        }
    } else if WRITE_ZERO_USAGE_ROW {
        // 「谁用的记在谁头上」：只给客户端一个「行存在」的占位，值恒为零。
        if usage_usable {
            tx.execute(
                "INSERT INTO session_usage (session_id, used, size, updated_at, credit_json) \
                 VALUES (?1, 0, 0, ?2, ?3) \
                 ON CONFLICT(session_id) DO NOTHING",
                rusqlite::params![sid, now, ZERO_USAGE_CREDIT_JSON],
            )
            .map_err(|e| aux_error(&e))?;
        } else {
            supported = false;
        }
    }
    if !aux.interruptions.is_empty() {
        let usable = table_exists(tx, "session_turn_interruptions")
            && column_exists(tx, "session_turn_interruptions", "request_id")
            && column_exists(tx, "session_turn_interruptions", "reason_code")
            && column_exists(tx, "session_turn_interruptions", "presentation");
        if usable {
            // 复合主键 (session_id, request_id)：重放同一操作必须幂等，不能产生重复行。
            for row in &aux.interruptions {
                tx.execute(
                    "INSERT OR REPLACE INTO session_turn_interruptions \
                     (session_id, request_id, reason_code, presentation) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![sid, row.request_id, row.reason_code, row.presentation],
                )
                .map_err(|e| aux_error(&e))?;
            }
        } else {
            supported = false;
        }
    }
    Ok(supported)
}

/// 会话行归属（未删除时）。
///
/// `deleted_at` 列按存在性拼 SQL：历史库没有软删除标记时套 `deleted_at IS NULL` 会让
/// prepare 失败 → 所有行都被判成「行缺失」，与存活索引（缺列按全部存活读）的结论分叉。
/// 与 `session::sync::preview::session_row_info` 必须同口径。
/// 读源会话的专家绑定字段（`expert_id` + `expert_runtime_identity`，只读）。
///
/// 只用于「目标账号能不能继续以该专家身份对话」的**展示判定**（R5-13）；两列都缺失时返回
/// `(None, None)`，判定侧据此给 `unknown`（不提示）。列不存在（老库）同样是 None。
pub(in crate::modules::session) fn read_expert_ref(
    paths: &SessionPaths,
    cid: &str,
) -> (Option<String>, Option<String>) {
    let Some(conn) = open_db(&paths.workbuddy_db(), true) else {
        return (None, None);
    };
    if !column_exists(&conn, "sessions", "expert_id") {
        return (None, None);
    }
    let has_identity = column_exists(&conn, "sessions", "expert_runtime_identity");
    let sql = if has_identity {
        "SELECT expert_id, expert_runtime_identity FROM sessions WHERE id = ?1"
    } else {
        "SELECT expert_id, NULL FROM sessions WHERE id = ?1"
    };
    conn.query_row(sql, [cid], |row| {
        Ok((
            row.get::<_, Option<String>>(0).unwrap_or(None),
            row.get::<_, Option<String>>(1).unwrap_or(None),
        ))
    })
    .unwrap_or((None, None))
}

pub(in crate::modules::session) fn session_row_owner(
    paths: &SessionPaths,
    cid: &str,
) -> Option<String> {
    let db = paths.workbuddy_db();
    let conn = open_db(&db, true)?;
    let sql = if column_exists(&conn, "sessions", "deleted_at") {
        "SELECT user_id FROM sessions WHERE id = ?1 AND deleted_at IS NULL"
    } else {
        "SELECT user_id FROM sessions WHERE id = ?1"
    };
    conn.query_row(sql, [cid], |row| row.get::<_, Option<String>>(0))
        .ok()
        .flatten()
}

/// 复制前的行级校验：Claw 工作区绑定当前账号渠道，不允许复制。
///
/// 必须在**任何写库动作之前**执行（包括为目标账号新建 space）：否则拒绝复制时
/// 目标账号已经多出一条 0 会话的孤儿 space（QA-E1 实测到的残留）。
pub(in crate::modules::session) fn ensure_row_copyable(
    source_row: &[(String, rusqlite::types::Value)],
) -> Result<(), String> {
    let Some((_, value)) = source_row.iter().find(|(name, _)| name == "cwd") else {
        return Ok(());
    };
    if let rusqlite::types::Value::Text(path) = value {
        if is_claw_workspace(path) {
            return Err("Claw 工作区绑定当前账号渠道，不支持复制".to_string());
        }
    }
    Ok(())
}

/// 在 workbuddy.db 中把源会话行复制为新 id（动态列，覆盖 id/user_id/时间戳）。
///
/// 跨档时源行与目标行分属两个库：源行从 `source_paths` 读取（跨档用只读连接），
/// 目标行按**目标表列序取交集**写入——目标存在而源没有的列不写，由库默认值补齐，
/// 不写 NULL 破坏约束（design §2.3；旧版客户端列少时靠这条兼容）。
/// 与旧实现不同：db/表/源行缺失都显式返回，不再静默 Ok。
///
/// 项目归属列不做「原样搬运」：[`map_target_space_id`] 负责把 `space_id` 换成目标
/// 账号自己的 space（同账号体系跨账号复制时源 id 在目标账号下不存在），
/// `project_id` 一律写 NULL（跨账号的外键引用一定悬空）。
///
/// 时间列的口径：`updated_at` 写本次（副本是刚产生的），`last_activity_at` 取
/// max(源值, `body_latest_at`) 并被本次时间夹住（见下方分支注释）。
///
/// `body_latest_at` 必须由调用方从**本次实际写入的那份正文**算出（`CopyBody::text`），
/// 不在这里回读目标文件：目标文件此时可能已被客户端改动，回读会引入 TOCTOU；
/// `None` 表示拿不到（本次没有内存副本、正文没有时间戳），退回沿用源值。
///
/// 参数过多的豁免（`clippy::too_many_arguments`）：这些都是「一次复制写库」的显式输入
/// （两侧档位、两个身份、两个 session id、账本开关、正文时间），打包成结构体只会让
/// 18 处调用点各写一份临时字面量、把「哪个字段是目标侧」藏起来。影响面仅限本 crate；
/// 与 `exec/backup.rs::create_sync_backup` 同口径。后续若再加输入，改为 `CopyRowInput`
/// 结构体一次收口（届时三处生产调用点一起改）。
#[allow(clippy::too_many_arguments)]
pub(in crate::modules::session) fn insert_session_copy(
    source_paths: &SessionPaths,
    target_paths: &SessionPaths,
    new_cid: &str,
    cid: &str,
    source_uid: &str,
    target_uid: &str,
    ignore_aux: bool,
    copy_usage: bool,
    copy_interruptions: bool,
    body_latest_at: Option<i64>,
) -> Result<DbCopyOutcome, String> {
    let dst_db = target_paths.workbuddy_db();
    if !dst_db.is_file() {
        return Ok(DbCopyOutcome::NoDb);
    }
    let src_db = source_paths.workbuddy_db();
    let same_db = src_db == dst_db;
    let (source_row, source_aux) = if src_db.is_file() {
        // 同档沿用读写连接（与改造前一致）；跨档对源档只读，不触碰源档 WAL 恢复。
        let Some(conn) = open_db(&src_db, !same_db) else {
            return Err("会话数据无法打开".to_string());
        };
        if !table_exists(&conn, "sessions") {
            return Ok(DbCopyOutcome::NoSessionsTable);
        }
        let row = read_source_session_row(&conn, cid, source_uid)?;
        // 账本与 sessions 行同源读取：行缺失时不必再读，忽略模式下降级为不搬。
        // R6-08：`copy_usage = false` 时**根本不查** `session_usage`（从源头保证不搬）。
        let aux = if ignore_aux || row.is_none() {
            SessionAux::default()
        } else {
            read_session_aux_filtered(&conn, cid, copy_usage, copy_interruptions)?
        };
        (row, aux)
    } else {
        (None, SessionAux::default())
    };
    let Some(source_row) = source_row else {
        return Ok(DbCopyOutcome::SourceRowMissing);
    };

    let Some(mut conn) = open_db(&dst_db, false) else {
        return Err("会话数据无法打开".to_string());
    };
    if !table_exists(&conn, "sessions") {
        return Ok(DbCopyOutcome::NoSessionsTable);
    }
    // 写事务的提交必须可靠持久：在本次实际写连接上确认 synchronous ≥ FULL。
    session_backup::ensure_full_synchronous(&conn)?;
    let target_cols = session_table_columns(&conn)?;

    // 行级校验必须在**任何写库动作之前**：否则「复制被拒」时目标账号已经多出一条
    // 0 会话的孤儿 space 行（QA-E1）。循环里不再重复校验。
    ensure_row_copyable(&source_row)?;

    // 「建 space（父行）→ 插会话（子行）」是一次父子写入：放进同一事务，任一失败
    // 整体回滚，绝不留下本次新建的孤儿 space。synchronous 已在上面的连接上确认。
    // 用 IMMEDIATE 而不是默认的 DEFERRED：本事务先 SELECT 再写，直接取写锁可避免
    // 升级锁时与并发写撞成 SQLITE_BUSY/快照失效（客户端仍可能开着别的写连接）。
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| format!("会话记录保存失败：{error}"))?;

    // 项目归属先算：源行的 space_id 属于源账号，必须换成目标账号自己的 space id
    // （见 [`map_target_space_id`]）。目标表没有该列时不查不建，保持旧行为。
    let (mapped_space_id, space_note) = if column_exists(&tx, "sessions", "space_id") {
        // 空间匹配按会话 cwd 走「路径优先 → 同名兜底 → 新建」（R5-20）。
        let cwd = row_text(&source_row, "cwd").unwrap_or_default();
        map_target_space_id(
            &tx,
            source_paths,
            target_paths,
            same_db,
            &source_row,
            target_uid,
            &cwd,
        )?
    } else {
        (None, None)
    };

    let mut cols: Vec<String> = Vec::with_capacity(target_cols.len());
    let mut vals: Vec<rusqlite::types::Value> = Vec::with_capacity(target_cols.len());
    // 会话行与账本用同一个时间戳：两表时序必须一致，否则客户端可能按「用量更新更晚」
    // 把副本判成另一个状态。
    let copy_now = now_ms();
    for col in &target_cols {
        let Some((_, value)) = source_row.iter().find(|(name, _)| name == col) else {
            continue;
        };
        let value = value.clone();
        let value = match col.as_str() {
            "id" => rusqlite::types::Value::Text(new_cid.to_string()),
            "user_id" => rusqlite::types::Value::Text(target_uid.to_string()),
            // 映射命中写目标 space；映射不出来（源为空 / 源 space 查不到）写 NULL。
            "space_id" => match mapped_space_id.as_ref() {
                Some(space_id) => rusqlite::types::Value::Text(space_id.clone()),
                None => rusqlite::types::Value::Null,
            },
            // 防御性处理：project_id 在跨账号复制后一定指向不存在的项目。
            "project_id" => rusqlite::types::Value::Null,
            // 创建时间保留源行原值：批量复制上百条历史会话后，若全部写成本次切号时刻，
            // 目标账号里所有副本都挤在同一分钟，按时间排序与统计全部失真。源值为 NULL
            // 时没有可保留的语义（旧库可能没写过），回退成本次时间戳，不写 NULL。
            "created_at" => match value {
                rusqlite::types::Value::Null => rusqlite::types::Value::Integer(copy_now),
                original => original,
            },
            // 更新时间仍写本次：这一列驱动客户端「最近更新」排序，副本沿用源的旧值会
            // 沉到列表底部，用户切号后看不到刚复制过来的会话。
            "updated_at" => rusqlite::types::Value::Integer(copy_now),
            // 活跃时间不能沿用源行：客户端侧栏的「活跃时间」读的就是这一列，而源行的值
            // 可能远早于正文最后一条消息（正文一直在更新、这一列没人维护），副本会
            // 永远显示成旧时间。取 max(源值, 本次写入正文的最新时间) 且用 copy_now
            // 夹住——源机时钟超前时不能让副本的活跃时间跑到未来。
            //
            // 本分支只在源行**确实有**这一列时才进入（列交集之外不存在这个 key），
            // 所以老库不会凭空多写一个源行没有的列。源值为 NULL、非正数（客户端不可能
            // 写出的坏值，0 会显示成 1970）或正文取不到时间时退回「沿用源值 / NULL」，
            // 不报错。正数口径与 `record_timestamp_of` 一致：两条写路径都只认正数。
            "last_activity_at" => {
                let from_row = match &value {
                    rusqlite::types::Value::Integer(number) if *number > 0 => Some(*number),
                    _ => None,
                };
                match advance_activity_at(from_row, body_latest_at) {
                    Some(timestamp) => rusqlite::types::Value::Integer(timestamp.min(copy_now)),
                    None => rusqlite::types::Value::Null,
                }
            }
            "deleted_at" => rusqlite::types::Value::Null,
            // 行状态与正文口径必须一致：正文末条已归一到 completed（见
            // [`normalize_copy_tail`]），行还写着 working/active 客户端照样显示思考中。
            // 目标表没有 status 列时这个分支不会执行（列交集里没有它），老库自然跳过。
            "status" => normalize_copy_row_status(value),
            _ => value,
        };
        cols.push(col.clone());
        vals.push(value);
    }

    let placeholders = cols.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    let colnames = cols.join(", ");
    // 用 INSERT 而不是 INSERT OR REPLACE：新 UUID 撞库时宁可失败，也不能悄悄覆盖既有会话。
    let sql = format!("INSERT INTO sessions ({colnames}) VALUES ({placeholders})");
    let params: Vec<&rusqlite::types::Value> = vals.iter().collect();
    tx.execute(&sql, rusqlite::params_from_iter(params))
        .map_err(|e| format!("会话记录保存失败：{e}"))?;
    // 账本与 sessions 行同事务：要么全有、要么全无。失败时事务回滚 → 目标库不留
    // sessions 行，操作停在 BodyWritten 可安全重试（正文已在上一阶段落盘）。
    let aux_note = if ignore_aux {
        // 显式忽略是**不可逆**选择：该会话永久不带账本，不做后台补写。
        None
    } else {
        // `copy_usage = false` 时 `write_session_aux` 只写零值行（`ON CONFLICT DO NOTHING`），
        // 绝不写来源数值；返回值仍是「目标库的账本表是否可用」。
        let supported = write_session_aux(&tx, new_cid, &source_aux, copy_now, copy_usage)?;
        if copy_usage {
            (!supported).then_some(AUX_UNSUPPORTED_NOTE)
        } else {
            // R6-08：不是失败，只是口径变化——必须告诉用户「共消耗」为什么是 0/偏少。
            Some(AUX_USAGE_LEFT_IN_SOURCE_NOTE)
        }
    };
    tx.commit().map_err(|e| format!("会话记录保存失败：{e}"))?;
    Ok(DbCopyOutcome::Inserted {
        aux_note,
        space_note,
    })
}

/// 复制时归一目标行的 `sessions.status`（与正文口径对齐）。
///
/// 判定口径是 [`SETTLED_ROW_STATUSES`] 取反：
/// - 终态（`completed` / `error` / `terminated`）原样保留——错误信息不能被抹掉；
/// - 归档态 `archived` 也原样保留——它是用户显式做过的整理动作，复制一个已归档会话
///   不该把它变成未归档（注意它不在 [`ARCHIVE_TARGET_TERMINAL_STATUSES`] 里，那个常量
///   说的是归档动作的**目标**状态，不是保留集合）；
/// - 其余（`working` / `active` / `Pending` / 未知值）说明客户端可能还把它当「运行中」，
///   而副本里没有任何东西在跑，一律写 `completed`。
///
/// 用「保留集合取反」而不是另立「运行中白名单」：白名单会漏掉 `Pending` 这类合法默认值
/// 和未来新增状态。非文本值（含 NULL）原样返回。
pub(in crate::modules::session) fn normalize_copy_row_status(
    value: rusqlite::types::Value,
) -> rusqlite::types::Value {
    let rusqlite::types::Value::Text(text) = &value else {
        return value;
    };
    let status = text.trim();
    let is_preserved = SETTLED_ROW_STATUSES
        .iter()
        .any(|preserved| status.eq_ignore_ascii_case(preserved));
    if is_preserved {
        return value;
    }
    rusqlite::types::Value::Text("completed".to_string())
}

/// 读取一行会话（列名 + 值，按源表列序），供跨库复制按目标列取交集。
pub(in crate::modules::session) fn read_source_session_row(
    conn: &Connection,
    cid: &str,
    uid: &str,
) -> Result<Option<Vec<(String, rusqlite::types::Value)>>, String> {
    let mut stmt = conn
        .prepare("SELECT * FROM sessions WHERE id = ?1 AND user_id = ?2")
        .map_err(|e| e.to_string())?;
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mut rows = stmt
        .query(rusqlite::params![cid, uid])
        .map_err(|e| e.to_string())?;
    let Some(row) = rows.next().map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let values = cols
        .into_iter()
        .enumerate()
        .map(|(i, col)| {
            let value = row
                .get::<_, rusqlite::types::Value>(i)
                .unwrap_or(rusqlite::types::Value::Null);
            (col, value)
        })
        .collect();
    Ok(Some(values))
}

/// 目标 sessions 表的列名（按表自身列序）。
pub(in crate::modules::session) fn session_table_columns(
    conn: &Connection,
) -> Result<Vec<String>, String> {
    let stmt = conn
        .prepare("SELECT * FROM sessions LIMIT 0")
        .map_err(|e| e.to_string())?;
    Ok(stmt.column_names().iter().map(|s| s.to_string()).collect())
}

/// 写后校验：目标行必须存在、归属目标账号且未删除。
pub(in crate::modules::session) fn verify_session_row(
    paths: &SessionPaths,
    new_cid: &str,
    target_uid: &str,
) -> Result<(), String> {
    match session_row_owner(paths, new_cid) {
        Some(owner) if owner == target_uid => Ok(()),
        Some(owner) => Err(format!(
            "会话记录归属校验失败：期望 {target_uid}，实际 {owner}"
        )),
        None => Err("会话记录保存后不可见，未按成功处理".to_string()),
    }
}
