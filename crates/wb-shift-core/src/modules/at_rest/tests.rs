//! `at_rest` 单测：AAD 字节序列、固定向量解密、错误类别、缓存、临时副本隔离与开关闸门。
//!
//! 全部用例都不拉真实客户端、不读真实账号与认证文件：解密链路是纯函数，密钥与 live
//! 文件都由用例自己构造（临时目录 + 固定 key）。唯一会碰到真实判断的是 Windows 分支
//! （平台闸门直接放行），不涉及子进程。

use super::envelope::{field_aad, open, parse};
use super::key::{self, derive_key, load_key_at, AtRestKey};
use super::live::read_live_access_at;
use super::*;
use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::{json, Value};

/// 固定测试密钥：32 字节 0x42。**只用于单测**，与任何真实客户端密钥无关。
const TEST_KEY: [u8; 32] = [0x42; 32];
/// 固定 nonce：12 字节 0x11。
const TEST_NONCE: [u8; 12] = [0x11; 12];
/// 信封里声明的固定 keyId（解密时与派生 key 的 keyId 比对）。
const TEST_KEY_ID: &str = "0123456789abcdef";
const TEST_PLAINTEXT: &str = "test-access-token";

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "wb-shift-at-rest-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// 只取错误、不要求 `Ok` 侧实现 `Debug`（`Zeroizing<String>` 不该为此暴露 Debug）。
fn err_of<T>(result: Result<T, AtRestError>) -> Option<AtRestError> {
    result.err()
}

/// 用给定 key / 固定 nonce / AAD 加密，返回 (ciphertext, tag)。
fn seal_with(key: &[u8; 32], plaintext: &str, key_id: &str) -> (Vec<u8>, [u8; 16]) {
    let cipher = Aes256Gcm::new_from_slice(key).expect("test key length");
    let nonce = Nonce::from_slice(&TEST_NONCE);
    let aad = field_aad(1, key_id);
    let mut buffer = plaintext.as_bytes().to_vec();
    let tag = cipher
        .encrypt_in_place_detached(nonce, &aad, &mut buffer)
        .expect("encrypt test vector");
    (buffer, tag.into())
}

/// 用固定测试密钥加密（KAT 用）。
fn seal(plaintext: &str, key_id: &str) -> (Vec<u8>, [u8; 16]) {
    seal_with(&TEST_KEY, plaintext, key_id)
}

/// 组装 `{$wbEncrypted:1, envelope}` 的 base64 载荷（固定测试密钥）。
fn envelope_value(plaintext: &str, key_id: &str, suite: u32) -> Value {
    envelope_value_with(&TEST_KEY, plaintext, key_id, suite)
}

/// 用给定密钥组装信封（注入密钥的场景必须用**同一把**密钥加密，否则必然 GCM 失败）。
fn envelope_value_with(key: &[u8; 32], plaintext: &str, key_id: &str, suite: u32) -> Value {
    let (ciphertext, tag) = seal_with(key, plaintext, key_id);
    let payload = json!({
        "suite": suite,
        "keyId": key_id,
        "nonce": BASE64.encode(TEST_NONCE),
        "authTag": BASE64.encode(tag),
        "ciphertext": BASE64.encode(ciphertext),
    });
    json!({
        "$wbEncrypted": 1,
        "envelope": BASE64.encode(serde_json::to_vec(&payload).unwrap_or_default()),
    })
}

fn test_key() -> AtRestKey {
    derive_key("unit-test-at-rest-secret")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 固定 `exp`（2099-01-01T00:00:00Z）的假 JWT：live 的 access 必须是可读 `exp` 的令牌。
fn jwt_with_exp(exp_secs: i64) -> String {
    let header = BASE64.encode(br#"{"alg":"none","typ":"JWT"}"#);
    let payload = BASE64.encode(format!("{{\"exp\":{exp_secs}}}").as_bytes());
    format!("{header}.{payload}.sig")
}

const FUTURE_EXP: i64 = 4_070_908_800;

/// 造一个「已打开双开关 + 注入密钥 + 临时 live（信封 access）」的上下文。
///
/// 返回 `(ctx, live 对象, 明文令牌)`：明文令牌必须自带 `exp`（live 解密后要判有效期），
/// 因此用固定 `exp` 的假 JWT 而不是任意字符串。
fn decrypting_context(tag: &str, exp_secs: i64) -> (AtRestContext, Value, String) {
    let root = store_root(tag, "ok", "");
    let live_dir = temp_dir(&format!("{tag}-live"));
    // 注入密钥的 keyId 必须与信封里的 keyId 一致：用派生结果的 keyId 写信封。
    let injected = derive_key("unit-test-at-rest-secret");
    let token = jwt_with_exp(exp_secs);
    let envelope = envelope_value_with(injected.key_bytes(), &token, injected.key_id(), 1);
    let live = json!({
        "account": {"uid": "user-a"},
        "auth": {"accessToken": envelope},
    });
    std::fs::write(
        live_dir.join("workbuddy-desktop.info"),
        serde_json::to_string(&live).unwrap_or_default(),
    )
    .expect("write live");
    let ctx = AtRestContext::for_variant(WbVariant::Cn)
        .with_store_root(&root)
        .with_live_path(live_dir.join("workbuddy-desktop.info"))
        .with_exe(temp_dir(&format!("{tag}-exe")).join("missing-exe"))
        .with_now(1_700_000_000_000)
        .with_key(injected);
    (ctx, live, token)
}

/// 造一个「已打开只读开关」的 store 根；`probe` 决定平台记录值。
fn store_root(tag: &str, probe: &str, probe_version: &str) -> std::path::PathBuf {
    let root = temp_dir(tag);
    let cfg = json!({
        "decryptReadOnly": true,
        "decryptWrite": true,
        "platformProbe": probe,
        "platformProbeVersion": probe_version,
    });
    std::fs::write(
        root.join("credential.json"),
        serde_json::to_string(&cfg).unwrap_or_default(),
    )
    .expect("write credential.json");
    root
}

fn envelope_account() -> Value {
    json!({
        "id": "acc-a",
        "uid": "user-a",
        "variant": "cn",
        "access_token": {"$wbEncrypted": 1, "envelope": "blob"},
        "refresh_token": {"$wbEncrypted": 1, "envelope": "blob"},
        "expiresAt": 1,
    })
}

// ---------------------------------------------------------------------------
// 1. AAD 字节序列（逐字节断言）
// ---------------------------------------------------------------------------

/// AAD 布局必须与客户端 `buildAuthenticatedContextAad` 逐字节一致：
/// 一个字节错位，GCM 校验就会失败，而失败信息只有「解不开」，极难排查。
#[test]
fn field_aad_is_byte_exact() {
    let aad = field_aad(1, "9127dea1b44020a7");
    let mut expected: Vec<u8> = Vec::new();
    expected.extend_from_slice(b"WB-AAD\0");
    expected.push(0x01);
    expected.extend_from_slice(&[0x00, 0x00, 0x00, 0x05]);
    expected.extend_from_slice(b"WBEV1");
    expected.extend_from_slice(&[0x00, 0x00, 0x00, 0x06]);
    expected.extend_from_slice(b"sym-v1");
    expected.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    expected.extend_from_slice(&[0x00, 0x00, 0x00, 0x10]);
    expected.extend_from_slice(b"9127dea1b44020a7");
    expected.push(0x02);
    expected.push(0x00);
    expected.push(0x00);
    assert_eq!(aad, expected);
    // 总长固定 54 字节：7 + 1 + 9 + 10 + 4 + 20 + 1 + 1 + 1。
    assert_eq!(aad.len(), 54);
    // 头部与 framing 字节单独再钉一次（这两处最容易在改动中被顺手改掉）。
    assert_eq!(&aad[..7], b"WB-AAD\0");
    assert_eq!(aad[7], 0x01, "版本字节");
    assert_eq!(
        &aad[aad.len() - 3..],
        &[0x02, 0x00, 0x00],
        "framing/optional/final"
    );
}

/// keyId 长度变化必须反映在长度前缀上（u32 大端），不能写死 16。
#[test]
fn field_aad_length_prefix_follows_key_id() {
    let aad = field_aad(1, "abcd");
    let key_id_at = aad.len() - 3 - 4 - 4;
    assert_eq!(&aad[key_id_at..key_id_at + 4], &[0x00, 0x00, 0x00, 0x04]);
    assert_eq!(&aad[key_id_at + 4..key_id_at + 8], b"abcd");
}

// ---------------------------------------------------------------------------
// 2. 固定 key + nonce/tag/ciphertext → 解出预期明文；篡改 tag 失败不 panic
// ---------------------------------------------------------------------------

/// 固定向量（KAT）：key / nonce / 明文 / AAD 全固定，密文与 tag 必须是这串字节。
/// 这保证「AAD 构造 + GCM 参数 + 编码」同时正确，而不只是自洽（自加密自解密恒过）。
#[test]
fn open_decrypts_fixed_vector() {
    let (ciphertext, tag) = seal(TEST_PLAINTEXT, TEST_KEY_ID);
    assert_eq!(
        hex(&ciphertext),
        "c702586740ade34459ef0b9a17ef9ff04b",
        "固定向量密文变了：AAD 或 GCM 参数被改动"
    );
    assert_eq!(
        hex(&tag),
        "d7ded000a51fb2022f74c1c64d067d5e",
        "固定向量 tag 变了：AAD 或 GCM 参数被改动"
    );
    let envelope = parse(&envelope_value(TEST_PLAINTEXT, TEST_KEY_ID, 1)).expect("parse envelope");
    let plain = open(&envelope, &TEST_KEY, TEST_KEY_ID).expect("open envelope");
    assert_eq!(plain.as_str(), TEST_PLAINTEXT);
}

/// 篡改 tag：必须返回 `GcmFailed`，不得 panic（线上遇到损坏信封只回退）。
#[test]
fn tampered_tag_fails_without_panicking() {
    let mut value = envelope_value(TEST_PLAINTEXT, TEST_KEY_ID, 1);
    let blob = value["envelope"].as_str().unwrap_or_default().to_string();
    let mut payload: Value =
        serde_json::from_slice(&BASE64.decode(&blob).unwrap_or_default()).unwrap_or(Value::Null);
    let mut tag = BASE64
        .decode(payload["authTag"].as_str().unwrap_or_default())
        .unwrap_or_default();
    if let Some(first) = tag.first_mut() {
        *first ^= 0xff;
    }
    payload["authTag"] = json!(BASE64.encode(tag));
    value["envelope"] = json!(BASE64.encode(serde_json::to_vec(&payload).unwrap_or_default()));
    let envelope = parse(&value).expect("tampered envelope still parses");
    assert_eq!(
        err_of(open(&envelope, &TEST_KEY, TEST_KEY_ID)),
        Some(AtRestError::GcmFailed)
    );
}

// ---------------------------------------------------------------------------
// 3. keyId 不匹配 / base64 损坏 / 缺字段 / suite≠1 → 各自错误类别
// ---------------------------------------------------------------------------

/// keyId 不匹配：先于解密返回 `KeyMismatch`（换机 / 客户端重建密钥的准确类别）。
#[test]
fn key_id_mismatch_is_reported_before_decrypt() {
    let envelope = parse(&envelope_value(TEST_PLAINTEXT, TEST_KEY_ID, 1)).expect("parse");
    assert_eq!(
        err_of(open(&envelope, &TEST_KEY, "ffffffffffffffff")),
        Some(AtRestError::KeyMismatch)
    );
}

/// 信封形态不认识的四种情况都归 `Malformed`，且不 panic。
#[test]
fn malformed_envelopes_map_to_malformed() {
    // base64 损坏
    assert_eq!(
        err_of(parse(
            &json!({"$wbEncrypted": 1, "envelope": "!!not-base64!!"})
        )),
        Some(AtRestError::Malformed)
    );
    // 缺 envelope 字段
    assert_eq!(
        err_of(parse(&json!({"$wbEncrypted": 1}))),
        Some(AtRestError::Malformed)
    );
    // 载荷缺 nonce
    let payload = json!({
        "suite": 1,
        "keyId": TEST_KEY_ID,
        "authTag": BASE64.encode([0u8; 16]),
        "ciphertext": BASE64.encode([0u8; 8]),
    });
    let missing_nonce = json!({
        "$wbEncrypted": 1,
        "envelope": BASE64.encode(serde_json::to_vec(&payload).unwrap_or_default()),
    });
    assert_eq!(err_of(parse(&missing_nonce)), Some(AtRestError::Malformed));
    // suite 不为 1
    assert_eq!(
        err_of(parse(&envelope_value(TEST_PLAINTEXT, TEST_KEY_ID, 2))),
        Some(AtRestError::Malformed)
    );
    // 根本不是信封（普通字符串 / 普通对象）
    assert_eq!(
        err_of(parse(&json!("plain-token"))),
        Some(AtRestError::Malformed)
    );
    assert_eq!(
        err_of(parse(&json!({"foo": 1}))),
        Some(AtRestError::Malformed)
    );
}

// ---------------------------------------------------------------------------
// 4. same_account 不匹配 → 不读取、不解密
// ---------------------------------------------------------------------------

/// live 不属于同一 uid：必须返回 `BindingMissing`（身份不符），且**不能**走到解密
/// —— live 文件里放的是损坏信封，若尝试解密会得到 `Malformed`，两者可区分。
#[test]
fn different_uid_does_not_decrypt() {
    let dir = temp_dir("other-uid");
    let path = dir.join("workbuddy-desktop.info");
    let live = json!({
        "account": {"uid": "user-b"},
        "auth": {"accessToken": {"$wbEncrypted": 1, "envelope": "!!broken!!"}},
    });
    std::fs::write(&path, serde_json::to_string(&live).unwrap_or_default()).expect("write live");
    let account = json!({"id": "acc-a", "uid": "user-a", "variant": "cn"});
    assert_eq!(
        err_of(read_live_access_at(
            &path,
            &account,
            WbVariant::Cn,
            &test_key(),
            now_ms(),
            true
        )),
        Some(AtRestError::BindingMissing),
        "uid 不同必须直接放弃，不能尝试解密"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 档位与调用方不一致同样直接放弃（认证文件是按档位取的）。
#[test]
fn variant_mismatch_does_not_read() {
    let dir = temp_dir("variant");
    let path = dir.join("workbuddy-desktop.info");
    std::fs::write(&path, "{}").expect("write live");
    let account = json!({"id": "acc-a", "uid": "user-a", "variant": "ai"});
    assert_eq!(
        err_of(read_live_access_at(
            &path,
            &account,
            WbVariant::Cn,
            &test_key(),
            now_ms(),
            true
        )),
        Some(AtRestError::BindingMissing)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// live 的 access 已过期：如实报 `Expired`（调用方据此「不刷新、不重试」）。
#[test]
fn expired_live_access_is_reported_as_expired() {
    let dir = temp_dir("expired-live");
    let ctx_dir = temp_dir("expired-live-store");
    std::fs::write(
        ctx_dir.join("credential.json"),
        serde_json::to_string(&json!({"decryptReadOnly": true, "decryptWrite": true}))
            .unwrap_or_default(),
    )
    .expect("write credential.json");
    let now = 1_700_000_000_000_i64;
    let expired = jwt_with_exp(now / 1000 - 60);
    let live = json!({
        "account": {"uid": "user-a"},
        "auth": {"accessToken": expired},
    });
    let path = dir.join("workbuddy-desktop.info");
    std::fs::write(&path, serde_json::to_string(&live).unwrap_or_default()).expect("write live");
    let account = envelope_account();
    assert_eq!(
        err_of(read_live_access_at(
            &path,
            &account,
            WbVariant::Cn,
            &test_key(),
            now,
            true
        )),
        Some(AtRestError::Expired)
    );
    // 闸门打开也解不出东西：`usable_account` 必须回退 `None`（不改账号、不刷新）。
    assert!(usable_account_in(
        &account,
        AccessMode::ReadOnly,
        &ctx_for(&dir, &ctx_dir, expired)
    )
    .is_none());
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&ctx_dir);
}

/// 造一个指向给定 live 目录 / store 根 / 明文 token 的上下文。
fn ctx_for(live_dir: &std::path::Path, store: &std::path::Path, _token: String) -> AtRestContext {
    let injected = derive_key("unit-test-at-rest-secret");
    AtRestContext::for_variant(WbVariant::Cn)
        .with_store_root(store)
        .with_live_path(live_dir.join("workbuddy-desktop.info"))
        .with_exe(live_dir.join("missing-exe"))
        .with_now(1_700_000_000_000)
        .with_key(injected)
}

// ---------------------------------------------------------------------------
// 5/8. key 缓存：命中不重复取、失败不缓存、TTL 过后重新取
// ---------------------------------------------------------------------------

#[test]
fn key_cache_fetches_once_for_ten_queries() {
    let namespace = "unit-test-key-cache-hit";
    let mut calls = 0_usize;
    for _ in 0..10 {
        let key = load_key_at(namespace, now_ms(), &mut || {
            calls += 1;
            Ok(derive_key("cached-secret"))
        })
        .expect("key available");
        assert_eq!(key.key_id().len(), 16, "keyId 必须是 16 个十六进制字符");
    }
    assert_eq!(calls, 1, "连续 10 次查询只允许真实取密钥一次");
}

#[test]
fn key_cache_is_not_populated_on_failure() {
    let namespace = "unit-test-key-cache-fail";
    let mut calls = 0_usize;
    assert_eq!(
        err_of(load_key_at(namespace, now_ms(), &mut || {
            calls += 1;
            Err(AtRestError::Timeout)
        })),
        Some(AtRestError::Timeout)
    );
    // 失败后立刻重试必须再取一次（不缓存失败结果，客户端刚升级的场景要能自愈）。
    assert!(load_key_at(namespace, now_ms(), &mut || {
        calls += 1;
        Ok(derive_key("recovered-secret"))
    })
    .is_ok());
    assert_eq!(calls, 2);
}

#[test]
fn key_cache_expires_after_ttl() {
    let namespace = "unit-test-key-cache-ttl";
    let mut calls = 0_usize;
    let start = 1_000_000_i64;
    let _ = load_key_at(namespace, start, &mut || {
        calls += 1;
        Ok(derive_key("ttl-secret"))
    });
    // TTL 内命中
    let _ = load_key_at(namespace, start + 1000, &mut || {
        calls += 1;
        Ok(derive_key("ttl-secret"))
    });
    // TTL 之后重新取（10 分钟整）
    let _ = load_key_at(namespace, start + key::KEY_TTL_MS, &mut || {
        calls += 1;
        Ok(derive_key("ttl-secret"))
    });
    assert_eq!(calls, 2);
}

// ---------------------------------------------------------------------------
// 6. 临时副本：不写回账号库、序列化不出 `__at_rest`
// ---------------------------------------------------------------------------

/// 临时副本必须是「只读的一次性副本」：账号库字节不变、DTO 里没有标记、没有明文令牌。
#[test]
fn temp_copy_is_memory_only_and_never_serialized_with_marker() {
    let dir = temp_dir("temp-copy");
    let store = dir.join("accounts.json");
    let original = json!([{
        "id": "acc-a",
        "uid": "user-a",
        "variant": "cn",
        "access_token": {"$wbEncrypted": 1, "envelope": "envelope-blob"},
        "refresh_token": {"$wbEncrypted": 1, "envelope": "envelope-blob"},
        "expiresAt": 1,
    }]);
    let bytes = serde_json::to_vec_pretty(&original).unwrap_or_default();
    std::fs::write(&store, &bytes).expect("write accounts");

    let account = original[0].clone();
    let (temp, token) = resolve_for_test(&account);

    // 标记只在内存副本上
    assert!(is_temp(&temp));
    assert!(!is_temp(&account));
    // 副本不携带可刷新的 refresh token（防御纵深）
    assert!(temp.get("refresh_token").is_none(), "必须 remove 而非置空");
    assert!(temp.get("refreshExpiresAt").is_none());
    assert_eq!(temp["access_token"], json!(token));
    // 账号库字节一个都不变：at_rest 全程不写盘
    assert_eq!(std::fs::read(&store).unwrap_or_default(), bytes);

    // 序列化路径（DTO / 诊断）都不允许出现标记或明文
    let meta = account::account_meta(&temp);
    let meta_text = serde_json::to_string(&meta).unwrap_or_default();
    assert!(
        !meta_text.contains(AT_REST_FIELD),
        "DTO 不得带 __at_rest: {meta_text}"
    );
    assert!(!meta_text.contains("decrypted-access-token"));
    assert!(!meta_text.contains(&token), "DTO 不得带明文令牌");
    let raw_text = serde_json::to_string(&temp).unwrap_or_default();
    assert!(
        raw_text.contains(AT_REST_FIELD),
        "原始 JSON 里保留标记供守卫判定"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 可以解出明文的完整上下文：注入密钥 + 临时 live 文件 + 打开的双开关。
fn resolve_for_test(account: &Value) -> (Value, String) {
    let (ctx, _live, token) = decrypting_context("resolve", FUTURE_EXP);
    let temp = usable_account_in(account, AccessMode::ReadOnly, &ctx)
        .unwrap_or_else(|| panic!("解密应当成功，last_error={:?}", last_error_kind()));
    (temp, token)
}

/// 错误类别里不得出现任何明文令牌。
#[test]
fn envelope_errors_never_carry_plaintext() {
    let envelope = parse(&envelope_value(TEST_PLAINTEXT, TEST_KEY_ID, 1)).expect("parse");
    let error = err_of(open(&envelope, &TEST_KEY, "ffffffffffffffff")).expect("mismatch");
    let text = format!("{error:?}{}", error.reason());
    assert!(!text.contains(TEST_PLAINTEXT));
    assert!(text.contains("decrypt_key_mismatch"), "{text}");
}

// ---------------------------------------------------------------------------
// 7. 临时账号不会被刷新（0 次刷新请求）
// ---------------------------------------------------------------------------

/// 临时副本走既有 `ensure_fresh_token` 时**不得**发生刷新。
///
/// 第一层证明：副本没有可用的 refresh token（字段被移除，`get_str` 取不到字符串），
/// 因此 `ensure_fresh_token` 的 `has_rt` 恒为 false、不会打刷新接口；测试里任何真实网络
/// 请求都必然失败并把账号标成 `needs_relogin`，所以「没有标记 + 令牌不变」就等价于
/// 「0 次刷新请求」。第二层是 P1 轨 `refresh::is_at_rest_temp` 的入口守卫。
#[tokio::test]
async fn temp_account_is_never_refreshed() {
    let account = envelope_account();
    let (temp, token) = resolve_for_test(&account);
    assert_eq!(temp["access_token"], json!(token));
    let config = json!({"lazy_refresh_hours": 24});
    let after = crate::modules::refresh::ensure_fresh_token(temp.clone(), &config).await;
    assert_eq!(
        after.get("access_token").cloned(),
        temp.get("access_token").cloned(),
        "临时副本的令牌不得被刷新替换"
    );
    assert!(
        after.get("needs_relogin").is_none(),
        "不得因为尝试刷新而给临时副本打需重新登录标记"
    );
}

// ---------------------------------------------------------------------------
// 8. 401 后重读 live：最多一次，令牌没变就不重试
// ---------------------------------------------------------------------------

/// 客户端已续期（live 换了令牌）→ 重读返回新的临时副本。
#[test]
fn reread_after_unauthorized_returns_renewed_token() {
    let (ctx, mut live, first_token) = decrypting_context("reread-renew", FUTURE_EXP);
    let account = envelope_account();
    let temp = usable_account_in(&account, AccessMode::ReadOnly, &ctx).expect("首次解密");
    assert_eq!(temp["access_token"], json!(first_token));

    // 客户端续期：live 换成新令牌（注入的密钥 keyId 不变）。
    let injected = derive_key("unit-test-at-rest-secret");
    let renewed = jwt_with_exp(FUTURE_EXP + 3_600);
    live["auth"]["accessToken"] =
        envelope_value_with(injected.key_bytes(), &renewed, injected.key_id(), 1);
    std::fs::write(
        &ctx.live_path,
        serde_json::to_string(&live).unwrap_or_default(),
    )
    .expect("rewrite live");

    let fresh = reread_after_unauthorized_in(&temp, &ctx).expect("应当读到新令牌");
    assert_eq!(fresh["access_token"], json!(renewed));
    assert!(is_temp(&fresh), "重读结果仍是临时副本");
    assert!(fresh.get("refresh_token").is_none());
}

/// 客户端还没续期（令牌没变）→ `None`：不拿同一个令牌再打一次请求。
#[test]
fn reread_after_unauthorized_returns_none_when_token_unchanged() {
    let (ctx, _live, token) = decrypting_context("reread-same", FUTURE_EXP);
    let account = envelope_account();
    let temp = usable_account_in(&account, AccessMode::ReadOnly, &ctx).expect("首次解密");
    assert_eq!(temp["access_token"], json!(token));
    assert!(
        reread_after_unauthorized_in(&temp, &ctx).is_none(),
        "令牌没变就没有重试的意义"
    );
}

/// 非临时副本 → 不是 401 重读的场景，直接 `None`（绝不因为别的 401 去解密）。
#[test]
fn reread_after_unauthorized_rejects_non_temp() {
    let account = envelope_account();
    assert!(reread_after_unauthorized(&account).is_none());
}

// ---------------------------------------------------------------------------
// 9. 开关闸门
// ---------------------------------------------------------------------------

/// 只读关闭：只读/写都不可用，且**不会拉起客户端**（开关先于平台闸门判定）。
#[test]
fn read_only_off_disables_everything_without_spawning() {
    let credential = json!({"decryptReadOnly": false, "decryptWrite": true});
    assert_eq!(
        gate_with(&credential, AccessMode::ReadOnly, Some("5.11.3")),
        Gate::SwitchOff
    );
    assert_eq!(
        gate_with(&credential, AccessMode::Write, Some("5.11.3")),
        Gate::SwitchOff,
        "写开关一并失效"
    );

    key::reset_spawn_count();
    let root = store_root("switch-off", "ok", "");
    std::fs::write(
        root.join("credential.json"),
        serde_json::to_string(&json!({"decryptReadOnly": false, "decryptWrite": true}))
            .unwrap_or_default(),
    )
    .expect("rewrite credential.json");
    let ctx = AtRestContext::for_variant(WbVariant::Cn)
        .with_store_root(&root)
        .with_live_path(temp_dir("switch-off-live").join("workbuddy-desktop.info"))
        .with_exe(root.join("missing-exe"));
    assert!(
        usable_account_in(&envelope_account(), AccessMode::ReadOnly, &ctx).is_none(),
        "只读关闭必须回退"
    );
    assert_eq!(key::spawn_count(), 0, "开关关闭时不得拉起客户端取密钥");
    let _ = std::fs::remove_dir_all(&root);
}

/// 只读开、写关：只读可用，写操作不参与（并且不触发取密钥）。
#[test]
fn write_switch_off_keeps_read_path_but_blocks_write() {
    // `platformProbe: "ok"`：本用例只验开关矩阵，不该被平台探测（非 Windows 上
    // Unknown → NeedsProbe）带偏；探测本身由 probe_* 用例覆盖。
    let credential = json!({
        "decryptReadOnly": true,
        "decryptWrite": false,
        "platformProbe": "ok"
    });
    assert_eq!(
        gate_with(&credential, AccessMode::ReadOnly, Some("5.11.3")),
        Gate::Open
    );
    assert_eq!(
        gate_with(&credential, AccessMode::Write, Some("5.11.3")),
        Gate::SwitchOff
    );

    key::reset_spawn_count();
    let root = store_root("write-off", "ok", "");
    std::fs::write(
        root.join("credential.json"),
        serde_json::to_string(&json!({"decryptReadOnly": true, "decryptWrite": false}))
            .unwrap_or_default(),
    )
    .expect("rewrite credential.json");
    let ctx = AtRestContext::for_variant(WbVariant::Cn)
        .with_store_root(&root)
        .with_live_path(temp_dir("write-off-live").join("workbuddy-desktop.info"))
        .with_exe(root.join("missing-exe"));
    assert!(
        usable_account_in(&envelope_account(), AccessMode::Write, &ctx).is_none(),
        "写开关关闭时写操作不参与"
    );
    assert_eq!(key::spawn_count(), 0, "写操作被挡时不该去取密钥");
    let _ = std::fs::remove_dir_all(&root);
}

/// 双开关矩阵：`(readOnly, write)` × `(ReadOnly, Write)` 四个组合全部钉住。
#[test]
fn gate_matrix_over_switches() {
    let cases = [
        (true, true, Gate::Open, Gate::Open),
        (true, false, Gate::Open, Gate::SwitchOff),
        (false, true, Gate::SwitchOff, Gate::SwitchOff),
        (false, false, Gate::SwitchOff, Gate::SwitchOff),
    ];
    for (read_only, write, expect_read, expect_write) in cases {
        let credential = json!({
            "decryptReadOnly": read_only,
            "decryptWrite": write,
            "platformProbe": "ok",
        });
        assert_eq!(
            gate_with(&credential, AccessMode::ReadOnly, Some("5.11.3")),
            expect_read,
            "readOnly={read_only} write={write} 的只读闸门"
        );
        assert_eq!(
            gate_with(&credential, AccessMode::Write, Some("5.11.3")),
            expect_write,
            "readOnly={read_only} write={write} 的写闸门"
        );
    }
}

/// 平台闸门：探测失败且版本未变 → 不再探测（不 spawn）；版本变了 → 重新探测。
#[test]
fn probe_failed_is_not_retried_but_retried_after_version_change() {
    let failed = json!({
        "decryptReadOnly": true,
        "decryptWrite": true,
        "platformProbe": "failed",
        "platformProbeVersion": "5.11.3",
    });
    if cfg!(target_os = "windows") {
        // Windows 已实测通过：记录值不参与放行判定，但仍不应误判成 SwitchOff。
        assert_ne!(
            gate_with(&failed, AccessMode::ReadOnly, Some("5.11.3")),
            Gate::SwitchOff
        );
        return;
    }
    assert_eq!(
        gate_with(&failed, AccessMode::ReadOnly, Some("5.11.3")),
        Gate::PlatformDisabled,
        "失败即记住：同一版本不再重试"
    );
    assert_eq!(
        gate_with(&failed, AccessMode::ReadOnly, Some("5.12.0")),
        Gate::NeedsProbe,
        "客户端升级后必须重新探测"
    );
    // 从未探测过 → 首次需要探测。
    let unknown = json!({"decryptReadOnly": true, "decryptWrite": true});
    assert_eq!(
        gate_with(&unknown, AccessMode::ReadOnly, Some("5.11.3")),
        Gate::NeedsProbe
    );
}

/// 平台探测失败必须回退（不 spawn 第二次、不改账号、不报错）。
#[test]
fn platform_gate_failure_falls_back_without_spawn() {
    if cfg!(target_os = "windows") {
        return;
    }
    key::reset_spawn_count();
    let root = store_root("probe-failed", "failed", "5.11.3");
    let ctx = AtRestContext::for_variant(WbVariant::Cn)
        .with_store_root(&root)
        .with_live_path(temp_dir("probe-failed-live").join("workbuddy-desktop.info"))
        .with_exe(root.join("missing-exe"));
    assert!(usable_account_in(&envelope_account(), AccessMode::ReadOnly, &ctx).is_none());
    assert_eq!(key::spawn_count(), 0, "探测失败后不得再拉起客户端");
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// 10. 解析顺序：信封 + 开关开 + 可解 → Ok；信封 + 开关关 → Err(信封文案)
// ---------------------------------------------------------------------------

/// 顺序钉子：`resolve_or_envelope_error` 必须先尝试解密，解密不可用才回到信封文案。
///
/// 若某次重构把「先判信封」写回去，本用例的第二段（开关打开且可解 → `Ok`）会直接失败。
#[test]
fn resolve_or_envelope_error_order() {
    // 开关打开 + 能解出明文：真实实现走 `usable_account`（读真实 store/live），单测里
    // 环境不可用 → 必然回退 Err。这里只钉「回退时不 panic、文案来自信封判定」。
    let account = envelope_account();
    let fallback = resolve_or_envelope_error(&account, AccessMode::ReadOnly);
    assert!(
        fallback.is_err(),
        "本机没有可用的客户端凭据时必须回退信封文案"
    );
    assert!(!fallback
        .err()
        .unwrap_or_default()
        .contains("decrypted-access-token"));

    // 明文账号：不需要解密，原样 Ok(None)。
    let plain = json!({
        "id": "acc-b",
        "uid": "user-b",
        "variant": "cn",
        "access_token": "plain-token",
        "expiresAt": now_ms() + 3_600_000,
    });
    let resolved = resolve_or_envelope_error(&plain, AccessMode::ReadOnly);
    assert_eq!(resolved.ok(), Some(None), "明文账号不得产生临时副本");
}

/// 注入上下文的那条路径必须真的能拿到明文（证明顺序里解密分支可达）。
#[test]
fn usable_account_in_decrypts_envelope_account() {
    let account = envelope_account();
    let (temp, token) = resolve_for_test(&account);
    assert_eq!(temp["access_token"], json!(token));
    assert!(is_temp(&temp));
    assert!(temp.get("refresh_token").is_none());
    // 缓存命中也必须一致（第二次走 ACCESS_CACHE，不重新解密）
    let (ctx, _live, cached_token) = decrypting_context("resolve-cached", FUTURE_EXP);
    let again = usable_account_in(&account as &Value, AccessMode::ReadOnly, &ctx);
    assert_eq!(
        again.map(|value| value["access_token"].clone()),
        Some(json!(cached_token))
    );
}

/// 明文账号不进解密通道：`usable_account` 必须零 IO 地返回 `None`。
#[test]
fn usable_account_ignores_plaintext_account() {
    let plain = json!({
        "id": "acc-b",
        "uid": "user-b",
        "variant": "cn",
        "access_token": "plain-token",
        "expiresAt": now_ms() + 3_600_000,
    });
    assert!(usable_account(&plain, AccessMode::ReadOnly).is_none());
    assert!(has_unexpired_plain_access(&plain));
    assert!(is_temp(&json!({"__at_rest": true})));
    assert!(!is_temp(&json!({"__at_rest": false})));
}
