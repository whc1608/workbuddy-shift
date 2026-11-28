//! CodeBuddy IDE（桌面客户端）会话复制：切换账号时把勾选会话复制到目标账号。
//!
//! 国内版（`CodeBuddy CN.app`）与国际版（`CodeBuddy.app`）**共用同一套会话存储**
//! （`<数据根>/<uid>/CodeBuddyIDE/<uid>/history/<工作区目录名>/`）、同一份复制内核、索引合并
//! 与关联登记，也共用同一个关联命名空间（组内 `variant` 区分 cn / ai）。两者的差异只有三处，
//! 收敛在 [`IdeFlavor`]：来源 uid 从哪个 IDE 的登录 secret 解析、运行态如何判定、关闭/启动
//! 哪个客户端。本模块因此只负责：
//!
//! - 数据根与源账号 uid 的解析（源 uid = 当前 IDE 登录 secret 里的 uid，回退本地状态文件）；
//! - 「沿用源会话 id、冲突才重随机」的复制策略（[`vscode_session::CopyIdPolicy`]）；
//! - 切换编排水位：**校验（关闭前只挡确定失败） → 关闭 IDE → 再读源 uid → 复制 → 登记关联 → 同步 → 注入凭证 → 重启**。
//!
//! 写入前提：IDE 必须已关闭（复制与同步都会被运行中的客户端覆盖）。`restart = true` 且当时
//! 在运行时由本模块负责关闭与重开；`restart = false` 时 IDE 正在运行则直接拒绝，不做「假装写入」。

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::modules::account::{self, get_str};
use crate::modules::codebuddy_cn_ide;
use crate::modules::codebuddy_ide;
use crate::modules::codebuddy_ide_session_sync;
use crate::modules::config::{backup_dir, utc_iso};
use crate::modules::session::{SessionPaths, SyncSelection};
use crate::modules::vscode_session::{self, CopyItem, CODEBUDDY_IDE_STORE, IDE_COPY};

/// 会话复制 / 同步的 IDE 档位。
///
/// 两个 IDE 写同一棵会话树，复制内核与编排完全共用；档位只决定「来源 uid 解析」「运行态
/// 判定」与「关闭 / 启动」三个动作落在哪个客户端上（文案与存储均不区分）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IdeFlavor {
    /// 国内版 `CodeBuddy CN.app`。
    Cn,
    /// 国际版 `CodeBuddy.app`。
    Intl,
}

impl IdeFlavor {
    /// 日志用的客户端名（沿用各自模块既有措辞）。
    fn label(self) -> &'static str {
        match self {
            Self::Cn => "CodeBuddy CN",
            Self::Intl => "CodeBuddy",
        }
    }

    /// 当前 IDE 登录账号 uid（来源 uid 的权威读取）；非法 uid 视为未登录。
    pub(crate) fn active_uid(self) -> Option<String> {
        let uid = match self {
            Self::Cn => codebuddy_cn_ide::active_cn_ide_uid(),
            Self::Intl => codebuddy_ide::active_intl_ide_uid(),
        };
        uid.filter(|uid| vscode_session::is_safe_uid(uid))
    }

    /// IDE 是否正在运行。
    pub(crate) fn is_running(self) -> bool {
        match self {
            Self::Cn => codebuddy_cn_ide::is_codebuddy_cn_running(),
            Self::Intl => codebuddy_ide::is_codebuddy_ide_running(),
        }
    }

    /// 目标校验：账号存在 + 档位匹配 + `access_token` 非空 + 用户数据目录存在。
    fn validate_switch_target(self, account_id: &str) -> Result<(Value, PathBuf), String> {
        match self {
            Self::Cn => codebuddy_cn_ide::validate_switch_target(account_id),
            Self::Intl => codebuddy_ide::validate_switch_target(account_id),
        }
    }

    /// 空参数时的直通切换（行为与现网一致）。
    fn switch_account(self, account_id: &str, restart: bool) -> Result<Value, String> {
        match self {
            Self::Cn => codebuddy_cn_ide::switch_account(account_id, restart),
            Self::Intl => codebuddy_ide::switch_account(account_id, restart),
        }
    }

    fn close(self, timeout_secs: i64) -> Result<(), String> {
        match self {
            Self::Cn => codebuddy_cn_ide::close_codebuddy_cn(timeout_secs),
            Self::Intl => codebuddy_ide::close_codebuddy_ide(timeout_secs),
        }
    }

    fn launch(self) -> Result<(), String> {
        match self {
            Self::Cn => codebuddy_cn_ide::launch_codebuddy_cn(),
            Self::Intl => codebuddy_ide::launch_codebuddy_ide(),
        }
    }

    /// 注入凭证并收尾（记录当前账号 → 按需重启）；关闭动作由编排负责。
    fn inject_and_finish(
        self,
        account_id: &str,
        acc: &Value,
        data_dir: &Path,
        restart: bool,
    ) -> Result<Value, String> {
        match self {
            Self::Cn => {
                codebuddy_cn_ide::inject_session_and_finish(account_id, acc, data_dir, restart)
            }
            Self::Intl => {
                codebuddy_ide::inject_session_and_finish(account_id, acc, data_dir, restart)
            }
        }
    }
}

mod conversations;
mod copy;
mod list;
mod paths;
mod switch;

pub(crate) use copy::copy_codebuddy_ide_sessions_in;
pub use list::{
    list_codebuddy_ide_sessions, list_current_codebuddy_ide_sessions,
    list_current_intl_ide_sessions,
};
pub use paths::ide_data_root;
pub use switch::{switch_codebuddy_cn_ide_with_copy, switch_codebuddy_intl_ide_with_copy};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::vscode_session::{history_root_in, read_json};
    use std::collections::BTreeSet;
    use std::path::Path;

    use super::paths::*;
    include!("tests/switch.rs");
    include!("tests/copy.rs");
    include!("tests/conversations.rs");
}
