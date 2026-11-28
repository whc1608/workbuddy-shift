//! `$wbEncrypted` 信封解析、AAD 构造与 AES-256-GCM 解密。
//!
//! **全部是纯函数**（不碰文件、不碰进程、不碰全局状态）：固定 key / nonce / tag /
//! ciphertext 就能完整覆盖解密链路，单测因此可以逐字节断言 AAD 并在不接触真实客户端
//! 的前提下验证「篡改 tag 必须失败且不 panic」。
//!
//! 格式与实测结论见 `wb-shift-envelope-decrypt-feasibility.md`：
//!
//! ```text
//! 信封：{"$wbEncrypted": 1, "envelope": "<base64>"}
//! 解出的载荷：{"suite":1,"keyId":"<16 hex>","nonce":"<b64 12B>",
//!              "authTag":"<b64 16B>","ciphertext":"<b64>"}
//! 算法：AES-256-GCM（12 字节 nonce、16 字节 tag）
//! ```

use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce, Tag};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::Value;
use zeroize::Zeroizing;

use super::AtRestError;

/// 唯一的 suite（AES-256-GCM）。其他取值一律按「无法解密」处理，不做猜测降级。
pub(super) const SUPPORTED_SUITE: u32 = 1;
/// AES-GCM 的 nonce / tag 长度是**格式常量**，长度不对就是信封损坏，不必尝试解密。
pub(super) const NONCE_LEN: usize = 12;
pub(super) const TAG_LEN: usize = 16;

/// 解析后的信封载荷（密文与 nonce 不是秘密，key 与明文才是）。
pub(super) struct Envelope {
    pub(super) suite: u32,
    pub(super) key_id: String,
    nonce: Vec<u8>,
    auth_tag: Vec<u8>,
    ciphertext: Vec<u8>,
}

impl Envelope {
    /// 信封声明的 keyId。与派生 key 的 keyId 不一致 = 不是同一把密钥（换机 / 客户端重建）。
    pub(super) fn key_id(&self) -> &str {
        &self.key_id
    }
}

/// 解析 `{$wbEncrypted:1, envelope:"<base64>"}`。
///
/// 失败类别刻意收得很细（`Malformed`）：base64 损坏、缺字段、`suite != 1`、nonce/tag
/// 长度不对都属于「信封形态不认识」，调用方只会回退、不会重试。
pub(super) fn parse(value: &Value) -> Result<Envelope, AtRestError> {
    let object = value.as_object().ok_or(AtRestError::Malformed)?;
    // `is_envelope` 的判定口径是「有 $wbEncrypted 键」；这里再确认一次，避免把别的
    // 普通对象当成信封去解。
    if !object.contains_key("$wbEncrypted") {
        return Err(AtRestError::Malformed);
    }
    let blob = object
        .get("envelope")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or(AtRestError::Malformed)?;
    let raw = BASE64.decode(blob).map_err(|_| AtRestError::Malformed)?;
    let payload: Value = serde_json::from_slice(&raw).map_err(|_| AtRestError::Malformed)?;
    let suite = payload
        .get("suite")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(AtRestError::Malformed)?;
    if suite != SUPPORTED_SUITE {
        return Err(AtRestError::Malformed);
    }
    let key_id = payload
        .get("keyId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or(AtRestError::Malformed)?
        .to_string();
    let nonce = decode_field(&payload, "nonce")?;
    let auth_tag = decode_field(&payload, "authTag")?;
    let ciphertext = decode_field(&payload, "ciphertext")?;
    if nonce.len() != NONCE_LEN || auth_tag.len() != TAG_LEN || ciphertext.is_empty() {
        return Err(AtRestError::Malformed);
    }
    Ok(Envelope {
        suite,
        key_id,
        nonce,
        auth_tag,
        ciphertext,
    })
}

fn decode_field(payload: &Value, key: &str) -> Result<Vec<u8>, AtRestError> {
    let text = payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or(AtRestError::Malformed)?;
    BASE64.decode(text).map_err(|_| AtRestError::Malformed)
}

/// 构造字段级 AAD（`framing = "field"`）。
///
/// 布局（与客户端 `buildAuthenticatedContextAad` 一致，**逐字节**重要）：
///
/// ```text
/// "WB-AAD\0"          7 字节 ASCII
/// 0x01                版本
/// u32(5) "WBEV1"      域分隔（field）
/// u32(6) "sym-v1"     scheme
/// u32(suite)
/// u32(len) keyId      UTF-8 字节
/// 0x02                framing：file=1，field=2
/// 0x00                optional sequence 不存在
/// 0x00                final 不存在
/// ```
///
/// 其中 `u32(n)` 一律是 4 字节大端。
pub(super) fn field_aad(suite: u32, key_id: &str) -> Vec<u8> {
    let mut aad: Vec<u8> = Vec::with_capacity(64);
    aad.extend_from_slice(b"WB-AAD\0");
    aad.push(0x01);
    push_len_prefixed(&mut aad, b"WBEV1");
    push_len_prefixed(&mut aad, b"sym-v1");
    aad.extend_from_slice(&suite.to_be_bytes());
    push_len_prefixed(&mut aad, key_id.as_bytes());
    aad.push(0x02);
    aad.push(0x00);
    aad.push(0x00);
    aad
}

fn push_len_prefixed(out: &mut Vec<u8>, bytes: &[u8]) {
    // 长度前缀是 u32 大端；这里的分段都是短常量，长度不会溢出 u32。
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// 用派生 key 解开信封，返回明文（`Zeroizing` 持有，Drop 时清零）。
///
/// keyId 不匹配**先于**解密返回 `KeyMismatch`：换机 / 客户端重建密钥时能给出准确类别，
/// 也避免拿错钥匙去解（GCM 失败会把真因掩盖成「密文损坏」）。
pub(super) fn open(
    envelope: &Envelope,
    key: &[u8; 32],
    key_id: &str,
) -> Result<Zeroizing<String>, AtRestError> {
    if envelope.key_id != key_id {
        return Err(AtRestError::KeyMismatch);
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| AtRestError::KeyMismatch)?;
    let nonce = Nonce::from_slice(&envelope.nonce);
    let tag = Tag::from_slice(&envelope.auth_tag);
    let aad = field_aad(envelope.suite, &envelope.key_id);
    // 就地解密：密文副本与明文都放进 `Zeroizing`，失败路径（tag 校验不过）也不会把
    // 半解出的字节留在可被后续分配复用的内存里。
    let mut buffer = Zeroizing::new(envelope.ciphertext.clone());
    cipher
        .decrypt_in_place_detached(nonce, &aad, buffer.as_mut_slice(), tag)
        .map_err(|_| AtRestError::GcmFailed)?;
    let text = std::str::from_utf8(buffer.as_slice()).map_err(|_| AtRestError::GcmFailed)?;
    Ok(Zeroizing::new(text.to_string()))
}
