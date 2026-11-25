//! 游戏服务器协议层。
//!
//! 请求体是 `msgpack → AES-256-CBC → 追加 32 字节密钥`，响应体是
//! `base64 → 去掉末 32 字节密钥 → AES 解密 → msgpack`。两者方向不同，不要写反。

pub mod codec;
pub mod headers;
pub mod session;

pub use headers::Headers;
pub use session::{LoginInfo, Session};

use rmpv::Value;

/// 取 map 字段。
pub(crate) fn get<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    let Value::Map(entries) = value else {
        return None;
    };
    entries
        .iter()
        .find(|(k, _)| k.as_str() == Some(key))
        .map(|(_, v)| v)
}

pub(crate) fn get_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    get(value, key).and_then(|v| v.as_str())
}

pub(crate) fn get_u64(value: &Value, key: &str) -> Option<u64> {
    get(value, key).and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
}

pub(crate) fn get_i64(value: &Value, key: &str) -> Option<i64> {
    get(value, key).and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
}
