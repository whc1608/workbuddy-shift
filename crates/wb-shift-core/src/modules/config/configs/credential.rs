//! 凭据（R6-06 按需解密）设置：`<store_root>/credential.json`。
//!
//! ```json
//! {"decryptReadOnly":true,"decryptWrite":false,"firstPromptShown":false,
//!  "platformProbe":"unknown","platformProbeVersion":"<客户端版本>"}
//! ```
//!
//! 语义（跨轨道契约，见清单 0.1.11 第 4 条）：
//! - `decryptReadOnly`：只读解密（积分 / 官方用量）开关，默认**开启**；
//! - `decryptWrite`：写操作解密（签到 / 旅行 / 每日活跃 / 自动轮换）开关，默认**关闭**；
//! - **只读关闭时写开关一并失效**：读取时归一（把 `decryptWrite` 强制为 false），
//!   避免文件里留下「只读关、写开」这种语义矛盾的状态；
//! - `firstPromptShown`：首次说明弹窗是否已展示过（前端置位，后端只负责持久化）；
//! - `platformProbe` / `platformProbeVersion`：平台闸门的探测结论与版本（失败不反复重试）。
//!
//! 这份配置**不进设置包导出**（`config::export_settings_bundle` 的键白名单里没有它）：
//! 它描述的是本机客户端的解密能力，换机后必须重新探测。

use super::*;

/// 平台探测结论（与 `credential.json` 的 `platformProbe` 一一对应）。
///
/// 单独成枚举是为了让闸门判定是穷举匹配：新增取值时编译器会逼着改所有分支，
/// 而不是让一个 `str` 比较悄悄落进「默认放行」。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProbeState {
    /// 尚未探测（全新安装 / 换机 / 手改成非法值）。
    Unknown,
    /// 探测通过，可启用解密通道。
    Ok,
    /// 探测失败；只有客户端版本变化后才重新探测。
    Failed,
}

/// 出厂默认值。
pub fn default_credential_config() -> Value {
    json!({
        "decryptReadOnly": true,
        "decryptWrite": false,
        "firstPromptShown": false,
        "platformProbe": "unknown",
        "platformProbeVersion": "",
    })
}

/// 只读解密开关：默认 **开启**；读不到配置时也按开启处理。
pub fn decrypt_read_enabled(cfg: &Value) -> bool {
    cfg.get("decryptReadOnly")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// 写操作解密开关：默认 **关闭**，且**只读关闭时一并失效**（单向包含）。
///
/// 这是「只读关闭 → 写一并失效」的唯一实现点：不要在别处再判一次。
pub fn decrypt_write_enabled(cfg: &Value) -> bool {
    decrypt_read_enabled(cfg)
        && cfg
            .get("decryptWrite")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

/// 探测结论 + 探测时的客户端版本（版本为空 → `None`，便于与 `client_version` 直接比较）。
pub fn platform_probe(cfg: &Value) -> (ProbeState, Option<String>) {
    let state = match cfg.get("platformProbe").and_then(Value::as_str) {
        Some("ok") => ProbeState::Ok,
        Some("failed") => ProbeState::Failed,
        _ => ProbeState::Unknown,
    };
    let version = cfg
        .get("platformProbeVersion")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    (state, version)
}

/// 写入探测结论（只动这两个字段，其它字段原样保留）。
pub fn set_platform_probe(cfg: &mut Value, state: ProbeState, version: Option<&str>) {
    if !cfg.is_object() {
        *cfg = default_credential_config();
    }
    let text = match state {
        ProbeState::Unknown => "unknown",
        ProbeState::Ok => "ok",
        ProbeState::Failed => "failed",
    };
    cfg["platformProbe"] = json!(text);
    cfg["platformProbeVersion"] = json!(version.unwrap_or("").trim());
}

/// 读取凭据配置（缺失 / 损坏 / 手改非法值一律归一）。
pub fn load_credential_config() -> Value {
    load_credential_config_at(&credential_config_file())
}

/// 设置页回包：配置 + 出厂默认值（`defaults`，纯增量字段，与其它配置同构）。
pub fn load_credential_config_with_defaults() -> Value {
    with_defaults(load_credential_config(), default_credential_config())
}

/// 可注入路径的读取实现（单测用临时文件）。
pub(crate) fn load_credential_config_at(path: &Path) -> Value {
    let input = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    resolve_credential_config(input.as_ref())
}

/// 归一：已知字段逐个校验，未知字段丢弃，只读关闭时写开关一并失效。
pub(crate) fn resolve_credential_config(input: Option<&Value>) -> Value {
    let mut cfg = default_credential_config();
    let Some(map) = input.and_then(Value::as_object) else {
        return cfg;
    };
    for key in ["decryptReadOnly", "decryptWrite", "firstPromptShown"] {
        if let Some(value) = map.get(key).and_then(Value::as_bool) {
            cfg[key] = json!(value);
        }
    }
    // 探测状态只接受已知取值：手改文件把 platformProbe 写成 ok 会绕过平台闸门，
    // 这里一律按 unknown 处理（unknown = 下次使用时重新探测）。
    let probe = match map.get("platformProbe").and_then(Value::as_str) {
        Some("ok") => "ok",
        Some("failed") => "failed",
        _ => "unknown",
    };
    cfg["platformProbe"] = json!(probe);
    cfg["platformProbeVersion"] = json!(map
        .get("platformProbeVersion")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim());
    // 只读关闭 ⇒ 写开关一并失效（归一在读取侧做，保存侧不用重复判断）。
    if cfg.get("decryptReadOnly").and_then(Value::as_bool) != Some(true) {
        cfg["decryptWrite"] = json!(false);
    }
    cfg
}

/// 设置页允许提交的字段：**只有这三个布尔位**。
///
/// 平台探测结论（`platformProbe` / `platformProbeVersion`）由闸门自己写，
/// 绝不接受外部提交 —— 否则前端（或任何能调命令的进程）提交一句
/// `platformProbe:"ok"` 就能绕过平台闸门。
pub const CREDENTIAL_SWITCH_KEYS: [&str; 3] =
    ["decryptReadOnly", "decryptWrite", "firstPromptShown"];

/// 把提交里的三个开关位合进现有配置（磁盘现状为底），返回归一后的配置。
///
/// **纯函数**：不读盘、不写盘，单测可以直接断言「提交 platformProbe 会被忽略」。
pub(crate) fn apply_credential_switches(cfg: Value, patch: &Value) -> Value {
    let mut merged = cfg;
    if let Some(map) = patch.as_object() {
        for key in CREDENTIAL_SWITCH_KEYS {
            if let Some(value) = map.get(key).and_then(Value::as_bool) {
                merged[key] = json!(value);
            }
        }
    }
    // 只读关闭 ⇒ 写一并失效：统一交给归一函数（唯一实现点）。
    resolve_credential_config(Some(&merged))
}

/// 保存设置页提交的开关（只写白名单字段 + 原子替换），返回回读值（含 `defaults`）。
///
/// 桌面端与 WebUI 共用这一个实现：两个宿主都只做参数拆封，不各自拼配置。
pub fn save_credential_switches(patch: &Value) -> std::io::Result<Value> {
    let merged = apply_credential_switches(load_credential_config(), patch);
    save_credential_config(&merged)?;
    Ok(load_credential_config_with_defaults())
}

/// 保存凭据配置（只保留已知字段；写入走原子替换）。
pub fn save_credential_config(cfg: &Value) -> std::io::Result<()> {
    save_credential_config_at(&credential_config_file(), cfg)
}

/// 可注入路径的保存实现（`at_rest` 的平台探测写 `platformProbe` 时用；单测用临时文件）。
///
/// 只写白名单字段 + 原子替换：手改文件里的未知键不会因为一次保存被带回来。
pub(crate) fn save_credential_config_at(path: &Path, cfg: &Value) -> std::io::Result<()> {
    let merged = resolve_credential_config(Some(cfg));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(&merged).unwrap_or_default();
    atomic_write(path, &content)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wb-shift-credential-config-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir.join("credential.json")
    }

    /// 缺文件 = 全新安装：只读开、写关、未弹过说明、未探测。
    #[test]
    fn missing_file_returns_documented_defaults() {
        let path = temp_path("missing");
        let cfg = load_credential_config_at(&path);
        assert_eq!(cfg["decryptReadOnly"], json!(true));
        assert_eq!(cfg["decryptWrite"], json!(false));
        assert_eq!(cfg["firstPromptShown"], json!(false));
        assert_eq!(cfg["platformProbe"], json!("unknown"));
        assert_eq!(cfg["platformProbeVersion"], json!(""));
        let _ = std::fs::remove_file(&path);
    }

    /// 只读关闭时写开关一并失效（读取侧归一），且未知字段被丢弃。
    #[test]
    fn read_only_off_forces_write_off() {
        let cfg = resolve_credential_config(Some(&json!({
            "decryptReadOnly": false,
            "decryptWrite": true,
            "firstPromptShown": true,
            "platformProbe": "ok",
            "platformProbeVersion": "5.11.3",
            "unknownKey": 1,
        })));
        assert_eq!(cfg["decryptReadOnly"], json!(false));
        assert_eq!(cfg["decryptWrite"], json!(false), "写开关必须一并失效");
        assert_eq!(cfg["firstPromptShown"], json!(true));
        assert_eq!(cfg["platformProbe"], json!("ok"));
        assert_eq!(cfg["platformProbeVersion"], json!("5.11.3"));
        assert!(cfg.get("unknownKey").is_none(), "未知字段不得落盘");
    }

    /// 手改出来的非法探测状态按 unknown 处理（否则等于给平台闸门开后门）。
    #[test]
    fn invalid_probe_status_falls_back_to_unknown() {
        let cfg = resolve_credential_config(Some(&json!({
            "platformProbe": "pass",
            "platformProbeVersion": "  5.11.3  ",
        })));
        assert_eq!(cfg["platformProbe"], json!("unknown"));
        assert_eq!(cfg["platformProbeVersion"], json!("5.11.3"));
    }

    /// 保存后可原样读回（含只读关闭带走写开关的归一结果）。
    #[test]
    fn save_then_load_roundtrip() {
        let path = temp_path("roundtrip");
        let merged = resolve_credential_config(Some(&json!({
            "decryptReadOnly": true,
            "decryptWrite": true,
        })));
        std::fs::write(&path, serde_json::to_string(&merged).unwrap_or_default())
            .expect("write credential config");
        let loaded = load_credential_config_at(&path);
        assert_eq!(loaded["decryptReadOnly"], json!(true));
        assert_eq!(loaded["decryptWrite"], json!(true));
        let _ = std::fs::remove_file(&path);
    }

    /// 平台探测结论**不接受**外部提交：提交里写 `platformProbe:"ok"` 一律被忽略。
    #[test]
    fn submitted_platform_probe_is_ignored() {
        let disk = json!({
            "decryptReadOnly": true,
            "decryptWrite": false,
            "firstPromptShown": false,
            "platformProbe": "failed",
            "platformProbeVersion": "5.11.3",
        });
        let merged = apply_credential_switches(
            disk,
            &json!({
                "decryptReadOnly": true,
                "decryptWrite": true,
                "platformProbe": "ok",
                "platformProbeVersion": "9.9.9",
            }),
        );
        assert_eq!(merged["decryptWrite"], json!(true));
        assert_eq!(
            merged["platformProbe"],
            json!("failed"),
            "探测结论只能由闸门写"
        );
        assert_eq!(merged["platformProbeVersion"], json!("5.11.3"));
    }

    /// 只读关闭时，同一次提交里的写开关也必须被归一掉（不留矛盾状态）。
    #[test]
    fn submitted_read_only_off_forces_write_off() {
        let disk = json!({"decryptReadOnly": true, "decryptWrite": true});
        let merged = apply_credential_switches(
            disk,
            &json!({"decryptReadOnly": false, "decryptWrite": true}),
        );
        assert_eq!(merged["decryptReadOnly"], json!(false));
        assert_eq!(merged["decryptWrite"], json!(false));
    }

    /// 非布尔值一律忽略（前端传字符串 / null 不会把开关改成奇怪状态）。
    #[test]
    fn submitted_non_boolean_values_are_ignored() {
        let disk = json!({"decryptReadOnly": true, "decryptWrite": false});
        let merged = apply_credential_switches(
            disk,
            &json!({"decryptReadOnly": "false", "decryptWrite": null}),
        );
        assert_eq!(merged["decryptReadOnly"], json!(true));
        assert_eq!(merged["decryptWrite"], json!(false));
    }
}
