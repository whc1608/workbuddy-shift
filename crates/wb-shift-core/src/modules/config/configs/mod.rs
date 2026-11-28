use super::paths::auto_rotate_config_file;
use super::paths::auto_rotate_logs_file;
use super::paths::backup_dir;
use super::paths::checkin_config_file;
use super::paths::checkin_logs_file;
use super::paths::credential_config_file;
use super::paths::rate_limit_config_file;
use super::paths::store_dir;
use super::paths::travel_cache_file;
use super::paths::travel_config_file;
use super::store::atomic_write;
use super::store::norm_ts;
use super::store::now_ms;
use super::CHECKIN_LOG_KEEP_DAYS;
use super::CHECKIN_LOG_MAX_RECORDS;
use crate::modules::locks;
use chrono::{Local, TimeZone};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static CHECKIN_LOG_WRITE_LOCK: Mutex<()> = Mutex::new(());

static TRAVEL_CACHE_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Serialize travel-cache read-modify-write across depart and claim cycles.
pub fn with_travel_cache_lock<T>(f: impl FnOnce() -> T) -> T {
    // 纯串行化守卫（`Mutex<()>`）：中毒不携带数据，取回后继续写入。
    let _guard = locks::recover(&TRAVEL_CACHE_WRITE_LOCK);
    f()
}

/// 给配置对象附上出厂默认值（`defaults`，见 [`checkin::load_checkin_config_with_defaults`]）。
///
/// 纯增量字段：不删也不改任何现有字段，旧前端读到的其它键一个字节都不变。设置页的
/// 「恢复默认」直接用这份值，前端因此不需要硬编码任何默认值。
pub(crate) fn with_defaults(mut cfg: Value, defaults: Value) -> Value {
    if let Value::Object(map) = &mut cfg {
        map.insert("defaults".to_string(), defaults);
    }
    cfg
}

mod automation;
mod bundle;
mod checkin;
mod credential;
mod travel;

pub use automation::{
    add_rotate_log, auto_rotate_notify_file, default_auto_rotate_config, default_rate_limit_config,
    load_auto_rotate_config, load_auto_rotate_config_at, load_auto_rotate_config_with_defaults,
    load_rate_limit_config, load_rate_limit_config_at, load_rate_limit_config_with_defaults,
    load_rotate_logs, save_auto_rotate_config, save_rate_limit_config_at, save_rotate_logs,
    set_rate_limit_hook_opt_out_at, try_consume_rotate_notify, ROTATE_LOG_MAX_RECORDS,
    ROTATE_NOTIFY_DAILY_LIMIT,
};
pub use credential::{
    decrypt_read_enabled, decrypt_write_enabled, default_credential_config, load_credential_config,
    load_credential_config_with_defaults, platform_probe, save_credential_config,
    save_credential_switches, set_platform_probe, ProbeState, CREDENTIAL_SWITCH_KEYS,
};
// 可注入 store 根的读写：`at_rest` 的平台探测写 `platformProbe` 时用，单测也用它隔离
// `credential.json`。路径只在 `credential` 里定义一次。
pub use bundle::{
    export_settings_bundle, import_settings_bundle, SETTINGS_BUNDLE_KEYS, SETTINGS_BUNDLE_VERSION,
};
pub use checkin::{
    add_checkin_log, compact_checkin_logs, default_checkin_config, load_checkin_config,
    load_checkin_config_with_defaults, load_checkin_logs, parse_clock, save_checkin_config,
};
pub(crate) use credential::{load_credential_config_at, save_credential_config_at};
pub use travel::{
    default_travel_config, load_travel_cache, load_travel_config, load_travel_config_with_defaults,
    save_travel_cache, save_travel_config,
};

#[allow(unused_imports)]
pub(crate) use automation::{
    local_date, rotate_notify_count_at, try_consume_rotate_notify_at, ROTATE_NOTIFY_FILE_NAME,
    ROTATE_NOTIFY_LOCK,
};
#[allow(unused_imports)]
pub(crate) use checkin::{
    apply_checkin_config, checkin_config_with_enabled, checkin_log_local_date,
    compact_checkin_logs_at, legacy_checkin_identity, legacy_default_checkin_config,
    load_checkin_config_at, merge_checkin_config, normalize_checkin_logs, resolve_checkin_config,
    save_checkin_logs_unlocked,
};
#[allow(unused_imports)]
pub(crate) use travel::{
    legacy_default_travel_config, load_travel_config_at, resolve_travel_config,
    travel_config_with_enabled,
};
