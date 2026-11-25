//! 请求体打包与响应体解包。
//!
//! 与 `autopcr/core/apiclient.py` 对齐：
//!
//! ```text
//! 请求：msgpack(use_bin_type=false) → AES-256-CBC(PKCS7) → 末尾追加 32 字节密钥 → 裸字节 POST
//! 响应：base64 文本 → 取末 32 字节作密钥 → AES-256-CBC 解密 → msgpack
//! ```
//!
//! `viewer_id` 字段单独处理：用同一把密钥加密后 base64，且同样在密文后追加密钥。

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use aes::Aes256;
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use generic_array::GenericArray;
use rand::Rng;
use rmpv::Value;

use crate::consts::AES_IV;
use crate::{Error, Result};

const KEY_LEN: usize = 32;

/// 每次请求随机生成的会话密钥：32 个十六进制字符。
pub struct Key([u8; KEY_LEN]);

impl Key {
    pub fn random() -> Self {
        const ALPHABET: &[u8; 16] = b"0123456789abcdef";
        let mut rng = rand::thread_rng();
        let mut bytes = [0u8; KEY_LEN];
        for slot in bytes.iter_mut() {
            *slot = ALPHABET[rng.gen_range(0..16)];
        }
        Key(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

fn cipher_encrypt(data: &[u8], key: &[u8]) -> Vec<u8> {
    // 预留一个块，保证 PKCS7 总能补满。
    let mut buffer = vec![0u8; data.len() + 16];
    buffer[..data.len()].copy_from_slice(data);

    let encrypted = cbc::Encryptor::<Aes256>::new(
        GenericArray::from_slice(key),
        GenericArray::from_slice(&AES_IV),
    )
    .encrypt_padded_mut::<Pkcs7>(&mut buffer, data.len())
    .expect("buffer 已预留足够的填充空间")
    .to_vec();

    let mut out = encrypted;
    out.extend_from_slice(key);
    out
}

/// 加密一个 msgpack 请求体。
pub fn encrypt_payload(payload: &Value, key: &Key) -> Vec<u8> {
    let mut packed = Vec::new();
    rmpv::encode::write_value(&mut packed, payload).expect("写入 Vec 不会失败");
    cipher_encrypt(&packed, key.as_bytes())
}

/// 加密 `viewer_id` 字段（同样是密文 + 密钥，再 base64）。
pub fn encrypt_viewer_id(viewer_id: u64, key: &Key) -> String {
    B64.encode(cipher_encrypt(
        viewer_id.to_string().as_bytes(),
        key.as_bytes(),
    ))
}

/// 解包一个响应体。
pub fn decrypt_response(body: &[u8]) -> Result<Value> {
    let compact: Vec<u8> = body
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    let raw = B64
        .decode(compact)
        .map_err(|e| Error::Decode(format!("响应体不是合法 base64: {e}")))?;

    if raw.len() < KEY_LEN + 16 {
        return Err(Error::Decode(format!("响应体长度异常: {} 字节", raw.len())));
    }
    let (cipher, key) = raw.split_at(raw.len() - KEY_LEN);

    let mut buffer = cipher.to_vec();
    let plain = cbc::Decryptor::<Aes256>::new(
        GenericArray::from_slice(key),
        GenericArray::from_slice(&AES_IV),
    )
    .decrypt_padded_mut::<Pkcs7>(&mut buffer)
    .map_err(|e| Error::Decode(format!("响应体解密失败（PKCS7 校验不通过）: {e}")))?;

    let mut cursor = plain;
    rmpv::decode::read_value(&mut cursor)
        .map_err(|e| Error::Decode(format!("响应体 msgpack 解析失败: {e}")))
}

/// 明文（非加密）响应的 JSON → `rmpv::Value`，便于统一处理。
pub fn json_to_rmpv(value: &serde_json::Value) -> Value {
    use serde_json::Value as J;
    match value {
        J::Null => Value::Nil,
        J::Bool(b) => Value::Boolean(*b),
        J::Number(n) => {
            if let Some(unsigned) = n.as_u64() {
                Value::from(unsigned)
            } else if let Some(signed) = n.as_i64() {
                Value::from(signed)
            } else {
                Value::from(n.as_f64().unwrap_or(0.0))
            }
        }
        J::String(s) => Value::from(s.as_str()),
        J::Array(items) => Value::Array(items.iter().map(json_to_rmpv).collect()),
        J::Object(map) => Value::Map(
            map.iter()
                .map(|(k, v)| (Value::from(k.as_str()), json_to_rmpv(v)))
                .collect(),
        ),
    }
}

/// 复刻 `autopcr/core/apiclient.py::_no_null_key`。
///
/// 服务端用」稀疏 map」（键为整数、可能缺项）表示列表，缺的位置键是 nil。
/// 判定规则：map 里存在 nil 键，且其余键全是整数 → 按整数键排序后取值组成数组；
/// 否则只丢弃 nil 键。
pub fn no_null_key(value: Value) -> Value {
    match value {
        Value::Map(entries) => {
            let has_nil_key = entries.iter().any(|(k, _)| k.is_nil());
            let only_int_keys = entries
                .iter()
                .all(|(k, _)| k.is_nil() || k.is_i64() || k.is_u64());

            if has_nil_key && only_int_keys {
                let mut pairs: Vec<(i64, Value)> = entries
                    .into_iter()
                    .filter_map(|(k, v)| k.as_i64().map(|index| (index, no_null_key(v))))
                    .collect();
                pairs.sort_by_key(|(index, _)| *index);
                Value::Array(pairs.into_iter().map(|(_, v)| v).collect())
            } else {
                Value::Map(
                    entries
                        .into_iter()
                        .filter(|(k, _)| !k.is_nil())
                        .map(|(k, v)| (k, no_null_key(v)))
                        .collect(),
                )
            }
        }
        Value::Array(items) => Value::Array(items.into_iter().map(no_null_key).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_32_hex_chars() {
        let key = Key::random();
        assert_eq!(key.as_bytes().len(), 32);
        assert!(key
            .as_bytes()
            .iter()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    }

    /// 加密 → base64 → 解包，验证 AES/PKCS7/msgpack 闭环。
    #[test]
    fn roundtrip_matches_wire_format() {
        let key = Key::random();
        let payload = Value::Map(vec![
            (Value::from("uid"), Value::from("123456789")),
            (Value::from("apptype"), Value::from(0)),
            (Value::from("campaign_data"), Value::from("")),
        ]);
        let cipher = encrypt_payload(&payload, &key);
        let wire = B64.encode(&cipher);

        let decoded = decrypt_response(wire.as_bytes()).expect("decrypt");
        assert_eq!(decoded, payload);
    }

    #[test]
    fn viewer_id_is_cipher_plus_key_in_base64() {
        let key = Key::random();
        let text = encrypt_viewer_id(123456, &key);
        let raw = B64.decode(text).expect("base64");
        assert_eq!(&raw[raw.len() - 32..], key.as_bytes());
        // 解密后应是十进制字符串
        let mut buffer = raw[..raw.len() - 32].to_vec();
        let plain = cbc::Decryptor::<Aes256>::new(
            GenericArray::from_slice(key.as_bytes()),
            GenericArray::from_slice(&AES_IV),
        )
        .decrypt_padded_mut::<Pkcs7>(&mut buffer)
        .expect("unpad");
        assert_eq!(plain, b"123456");
    }

    #[test]
    fn sparse_map_becomes_array() {
        let value = Value::Map(vec![
            (Value::from(0), Value::from("a")),
            (Value::Nil, Value::from("hole")),
            (Value::from(2), Value::from("c")),
        ]);
        let converted = no_null_key(value);
        assert_eq!(
            converted,
            Value::Array(vec![Value::from("a"), Value::from("c")])
        );
    }

    #[test]
    fn ordinary_map_keeps_keys_and_drops_nil() {
        let value = Value::Map(vec![
            (Value::from("sid"), Value::from("x")),
            (Value::from("missing"), Value::from("")),
            (Value::Nil, Value::from("dropped")),
        ]);
        let converted = no_null_key(value);
        let Value::Map(entries) = converted else {
            panic!("应为 map")
        };
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|(k, _)| k.as_str() == Some("sid")));
    }
}
