//! HTTP handler 按业务域拆分；router 通过本模块的 re-export 统一引用。

pub(crate) mod accounts;
pub(crate) mod automation;
pub(crate) mod checkin;
pub(crate) mod clients;
pub(crate) mod credential;
pub(crate) mod daily_activity;
pub(crate) mod diagnostics;
pub(crate) mod groups;
pub(crate) mod notifications;
pub(crate) mod rate_limits;
pub(crate) mod sessions;
pub(crate) mod settings_sync;
pub(crate) mod stats;
pub(crate) mod storage;
pub(crate) mod update;

pub(crate) use accounts::*;
pub(crate) use automation::*;
pub(crate) use checkin::*;
pub(crate) use clients::*;
pub(crate) use credential::*;
pub(crate) use daily_activity::*;
pub(crate) use diagnostics::*;
pub(crate) use groups::*;
pub(crate) use notifications::*;
pub(crate) use rate_limits::*;
pub(crate) use sessions::*;
pub(crate) use settings_sync::*;
pub(crate) use stats::*;
pub(crate) use storage::*;
pub(crate) use update::*;
