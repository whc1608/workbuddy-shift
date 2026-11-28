//! 导入：解析/预览、合并结果与导入入口。

use super::*;

/// 凭据字段的第三方别名（按优先级）：BuddyRelay 之类工具导出的是 camelCase。
///
/// 只用于**补齐**规范键，绝不覆盖已有合法 `access_token` / `refresh_token`。
const ACCESS_TOKEN_ALIASES: [&str; 2] = ["accessToken", "token"];
const REFRESH_TOKEN_ALIASES: [&str; 1] = ["refreshToken"];

/// 第三方号池「加密池密文」的前缀：本机无法解密，既不预览成可用凭据，也不入库。
const FOREIGN_CIPHER_PREFIX: &str = "enc:v1:";

/// 归一化一条导入记录：补齐 `access_token` / `refresh_token`，其余字段原样保留。
///
/// - 规范键缺失、空串或不是凭据（普通对象 / 数字等）时，按 [`ACCESS_TOKEN_ALIASES`] /
///   [`REFRESH_TOKEN_ALIASES`] 的顺序回退；已有合法规范键时**不覆盖**。
/// - 只加键不改名、不裁剪：`$wbEncrypted` 信封、`auth_raw`、来源端自定义字段都必须原样
///   透传到账号库（信封依赖本机 keyblob 才能解密，改写即失效）。
fn normalize_import_record(item: &Value) -> Value {
    let Value::Object(source) = item else {
        return item.clone();
    };
    let mut record = source.clone();
    for (canonical, aliases) in [
        ("access_token", ACCESS_TOKEN_ALIASES.as_slice()),
        ("refresh_token", REFRESH_TOKEN_ALIASES.as_slice()),
    ] {
        if account::secret_value(item, canonical).is_some() {
            continue;
        }
        if let Some(value) = aliases
            .iter()
            .find_map(|alias| account::secret_value(item, alias))
        {
            record.insert(canonical.to_string(), value);
        }
    }
    Value::Object(record)
}

/// 该字段值是否为第三方加密池密文（`enc:v1:` 前缀，允许前导空白）。
///
/// 这类字符串既不是明文凭据、也不是 WorkBuddy 的 `$wbEncrypted` 信封：本机无法解密，
/// 导入后只会得到一条「看似有 token、实际不可用」的账号，因此单独识别并明确提示。
pub(super) fn is_foreign_ciphertext(value: &Value) -> bool {
    value
        .as_str()
        .map(str::trim_start)
        .is_some_and(|text| text.starts_with(FOREIGN_CIPHER_PREFIX))
}

/// 解析导出/导入文件文本：必须是 JSON 数组，或 `{ "accounts": [...] }` 包装对象。
///
/// 每项都会先经 [`normalize_import_record`] 归一化（第三方号池的 camelCase 凭据字段），
/// 预览与合并共用这份结果，保证「预览说有凭据」与「导入真的入库」口径一致。
/// 失败时返回带位置的明确错误文案（非法 JSON / 非数组 / 元素不是对象）。
pub fn parse_accounts_json(text: &str) -> Result<Vec<Value>, String> {
    if text.trim().is_empty() {
        return Err("文件内容为空".to_string());
    }
    let parsed: Value =
        serde_json::from_str(text).map_err(|e| format!("文件不是合法的 JSON：{e}"))?;
    let array = match &parsed {
        Value::Array(array) => array,
        // 第三方号池常见 `{ "accounts": [...] }` 包装：只认这一层，别的一律当非数组。
        Value::Object(map) => map
            .get("accounts")
            .and_then(Value::as_array)
            .ok_or_else(|| "文件内容应为 JSON 数组（账号列表）".to_string())?,
        _ => return Err("文件内容应为 JSON 数组（账号列表）".to_string()),
    };
    for (index, item) in array.iter().enumerate() {
        if !item.is_object() {
            return Err(format!("文件第 {} 项不是合法的账号对象", index + 1));
        }
    }
    Ok(array.iter().map(normalize_import_record).collect())
}

/// 生成导入文件的脱敏预览（含文件内索引，不含 token）。
///
/// 展示字段（uid/nickname/email）一律走 `account::display_value` 折叠，与账号
/// 列表接口同口径：WorkBuddy 5.6 加密信封等对象/数组不得漏进前端渲染路径，
/// 否则会被当作 React 子节点渲染导致整树卸载（issue #100）。
/// `encrypted` 标记 access_token 为加密信封：可导入，但仅切换可用。
/// `foreignEncrypted` 标记第三方号池的 `enc:v1:` 密文：本机无法解密，既不算有凭据、
/// 也不入库（前端据此展示「加密池密文」而不是「缺少 token」）。
pub fn preview_accounts(text: &str) -> Result<Value, String> {
    let array = parse_accounts_json(text)?;
    let items: Vec<Value> = array
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let foreign_encrypted = item.get("access_token").is_some_and(is_foreign_ciphertext);
            json!({
                "index": index,
                "uid": account::display_value(item, "uid"),
                "nickname": account::display_value(item, "nickname"),
                "email": account::display_value(item, "email"),
                "hasToken": !foreign_encrypted
                    && account::secret_value(item, "access_token").is_some(),
                "encrypted": account::is_envelope(item, "access_token"),
                "foreignEncrypted": foreign_encrypted,
            })
        })
        .collect();
    Ok(json!({ "accounts": items, "total": array.len() }))
}

/// 单条导入记录的合并动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MergeOutcome {
    /// 追加为新账号。
    Appended,
    /// 覆盖同 uid 的本地账号（保留导入记录原样）。
    Overwritten,
    /// 缺少可导入凭据（缺失 / 空串 / 普通对象），跳过。
    Skipped,
}

/// 导入记录补齐凭据来源（R6-01）。
///
/// 口径（见 `执行默认`）：**导出/导入保留该字段**——备份自带 `credentialSource` 时原样保留
/// （同一台机器导出再导入，账号的刷新策略不该被改写）。备份**没有**该字段（旧备份、第三方
/// 号池导出）时按 [`account::CredentialSource::Import`] 保守处理：备份无法区分来源，而
/// 误判成 `oauth` 会让本工具用导入的旧 refresh token 去刷，把另一台机器上的客户端顶掉。
///
/// 必须在 `inherit_local_fields` **之前**调用：一旦这里补上了键，本地值的继承就不会生效——
/// 这正是「备份里的 token 已经替换了本地 token，来源也随之变成 import」的正确语义。
fn apply_import_credential_source(record: &mut Value) {
    if account::explicit_credential_source(record).is_some() {
        return;
    }
    account::set_credential_source(record, account::CredentialSource::Import);
}

/// 纯函数：把一条导入记录合并进账号列表。
///
/// 按 uid 去重：同 uid 覆盖（保留导入记录原样）；uid 缺失或无法匹配则追加。
/// 凭据判定与采集口径一致（`account::secret_value`）：非空明文串或
/// `$wbEncrypted` 加密信封均可入库，缺失 / 空串 / 普通对象跳过；
/// 第三方 `enc:v1:` 密文同样跳过（本机解不开，写进账号库等于留下一条不可用凭据）。
pub(super) fn merge_import_record(accounts: &mut Vec<Value>, item: &Value) -> MergeOutcome {
    let Some(token) = account::secret_value(item, "access_token") else {
        return MergeOutcome::Skipped;
    };
    if is_foreign_ciphertext(&token) {
        return MergeOutcome::Skipped;
    }
    if let Some(uid) = account::get_str(item, "uid").as_deref() {
        if let Some(existing) = accounts
            .iter_mut()
            .find(|a| account::get_str(a, "uid").as_deref() == Some(uid))
        {
            let mut replaced = item.clone();
            // 凭据来源：备份带了就保留，没带就按 `import`（见 `apply_import_credential_source`）。
            apply_import_credential_source(&mut replaced);
            // 本地展示字段（备注 / 显示选择 / 标记）：备份未携带时保留本地值；
            // 备份自带则以备份为准（导入即恢复用户当时的展示配置）。
            // 白名单见 `account::LOCAL_ACCOUNT_FIELDS`——导入**旧备份**（不含 mark）时
            // 漏掉 `mark` 会把本地标记整条覆盖掉（R5-04）。
            account::inherit_local_fields(existing, &mut replaced);
            // 账号库不允许出现无 id 记录（删除按 id、列表 key、导出选择都依赖
            // id）：优先保留本地 id；本地也是无 id 的历史脏数据时补一个。
            if account::get_str(&replaced, "id").is_none() {
                replaced["id"] = account::get_str(existing, "id")
                    .map(Value::String)
                    .unwrap_or_else(|| Value::String(uuid::Uuid::new_v4().to_string()));
            }
            *existing = replaced;
            return MergeOutcome::Overwritten;
        }
    }
    // 追加同样必须带 id：缺 id 的记录在刷新回写时无法被按 id 覆盖，会被
    // 反复追加副本（issue #111）。
    let mut appended = item.clone();
    apply_import_credential_source(&mut appended);
    if account::get_str(&appended, "id").is_none() {
        appended["id"] = Value::String(uuid::Uuid::new_v4().to_string());
    }
    accounts.push(appended);
    MergeOutcome::Appended
}

/// 纯函数：解析文件文本并按选中索引把记录合并进账号列表，返回计数。
///
/// 不做文件读写，便于单测。索引越界视为跳过。
pub fn merge_import_records(
    accounts: &mut Vec<Value>,
    text: &str,
    indexes: &[usize],
) -> Result<ImportResult, String> {
    let array = parse_accounts_json(text)?;
    let mut result = ImportResult::default();
    for &index in indexes {
        match array.get(index) {
            None => result.skipped += 1,
            Some(item) => match merge_import_record(accounts, item) {
                MergeOutcome::Appended => result.imported += 1,
                MergeOutcome::Overwritten => {
                    result.imported += 1;
                    result.overwritten += 1;
                }
                MergeOutcome::Skipped => result.skipped += 1,
            },
        }
    }
    Ok(result)
}

/// 导入：读账号库 → 合并 → 写回，返回计数。
pub fn import_accounts(text: &str, indexes: &[usize]) -> Result<ImportResult, String> {
    let mut accounts = account::load_accounts();
    let result = merge_import_records(&mut accounts, text, indexes)?;
    account::save_accounts(&accounts).map_err(|e| format!("保存账号库失败：{e}"))?;
    Ok(result)
}
