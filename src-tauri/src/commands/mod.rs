//! Tauri commands：前端调用的薄包装，与 `wb-shift-server` 的 HTTP 路由同源。
//!
//! 这里是唯一的 Tauri IPC 边界：只做参数解析与错误字符串化，业务逻辑全部落在
//! `wb_shift_core::modules`，桌面端与 webui 因此共用同一套实现。
//!
//! 按业务域拆分为子模块，但命令名与参数保持逐字不变；`lib.rs` 仍通过
//! `commands::<command>` 注册（子模块的 `pub use` 保证路径不变）。

use serde::Serialize;
use serde_json::Value;

mod accounts;
mod automation;
mod checkin;
mod clients;
mod credential;
mod daily_activity;
mod diagnostics;
mod groups;
mod rate_limits;
mod sessions;
mod settings_sync;
mod stats;
mod storage;
mod switch;
mod system;
mod update;
mod webview;

pub use accounts::*;
pub use automation::*;
pub use checkin::*;
pub use clients::*;
pub use credential::*;
pub use daily_activity::*;
pub use diagnostics::*;
pub use groups::*;
pub use rate_limits::*;
pub use sessions::*;
pub use settings_sync::*;
pub use stats::*;
pub use storage::*;
pub use switch::*;
pub(crate) use system::relaunch_app_inner;
pub use system::*;
pub use update::*;
pub use webview::*;

/// 序列化字段必须与前端 `AppStatus`（src/lib/types/account.ts）及 webui
/// `GET /api/status` 的 camelCase 契约一致；缺省 serde 会输出 auth_file / app_path，
/// 前端读 status.authFile 会拿到 undefined。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStatus {
    running: bool,
    auth_file: String,
    current: Option<Value>,
    app_path: String,
    version: String,
    variant: String,
}

#[cfg(test)]
mod app_status_tests {
    use super::AppStatus;

    /// 回归：桌面 AppStatus 必须与前端 `AppStatus`（src/lib/types/account.ts）以及
    /// webui `GET /api/status` 的 camelCase 字段契约一致。历史上只 derive(Serialize)
    /// 会输出 `auth_file` / `app_path`，前端读 `status.authFile` 拿到 undefined，
    /// 设置页的认证文件路径与更新卡片会静默丢字段。
    #[test]
    fn app_status_serializes_camel_case_like_webui() {
        let status = AppStatus {
            running: true,
            auth_file: "/tmp/workbuddy-desktop.info".to_string(),
            current: Some(serde_json::json!({ "uid": "u1" })),
            app_path: "/Applications/WorkBuddy.app".to_string(),
            version: "0.1.0".to_string(),
            variant: "cn".to_string(),
        };

        let json = serde_json::to_value(&status).expect("AppStatus 必须可序列化");

        assert_eq!(json["authFile"], "/tmp/workbuddy-desktop.info");
        assert_eq!(json["appPath"], "/Applications/WorkBuddy.app");
        assert_eq!(json["running"], true);
        assert_eq!(json["variant"], "cn");
        assert!(
            json.get("auth_file").is_none(),
            "不得输出 snake_case auth_file"
        );
        assert!(
            json.get("app_path").is_none(),
            "不得输出 snake_case app_path"
        );
    }
}
