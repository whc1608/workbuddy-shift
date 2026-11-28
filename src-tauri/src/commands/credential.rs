//! 凭据（R6-06 按需解密）设置：只读 / 写开关与首次说明标记。
//!
//! 唯一 Tauri IPC 边界的一部分；只做参数解析与错误字符串化，业务逻辑在 wb-shift-core。
//!
//! # 边界纪律（安全红线的最后一道）
//!
//! 这**两个**命令就是解密能力的全部外在接口：签名里没有 token、没有路径、没有
//! 「decrypt」动词，只读写三个布尔位。任何「帮我解一下这个信封」的需求都必须走
//! `wb_shift_core::modules::at_rest` 的内部调用，不得新增命令。

use serde_json::Value;

/// 读取凭据设置（含 `defaults` 出厂值，设置页「恢复默认」直接用它）。
///
/// 回包只有开关与探测结论，不含任何令牌、密钥或文件路径。
#[tauri::command]
pub fn get_credential_config() -> Value {
    wb_shift_core::modules::config::load_credential_config_with_defaults()
}

/// 保存凭据设置：**只接受** `decryptReadOnly` / `decryptWrite` / `firstPromptShown`。
///
/// 平台探测结论（`platformProbe` / `platformProbeVersion`）由闸门自己写，提交里带了
/// 也会被忽略（见 `config::save_credential_switches`）；只读关闭时写开关一并失效。
#[tauri::command(rename_all = "camelCase")]
pub fn save_credential_config(config: Value) -> Result<Value, String> {
    wb_shift_core::modules::config::save_credential_switches(&config)
        .map_err(|error| error.to_string())
}
