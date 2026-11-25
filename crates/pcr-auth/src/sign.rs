//! B 站 SDK 的请求签名。
//!
//! 与 `autopcr/autopcr/sdk/bsgamesdk.py::setsign` 等价，包括两个必须照抄的怪癖：
//!
//! 1. `pwd` 字段在表单体里出现两次：一次 URL 编码、一次原始值；
//! 2. 签名串是「按 key 升序把值直接拼接」+ 固定盐，再 MD5。值之间没有分隔符，
//!    所以多一个空字段就会改变签名。
//!
//! 表单体的字段顺序取自模板 JSON 的原始顺序（`serde_json` 开启了
//! `preserve_order`），签名则按 key 排序，两者互不影响。

use md5::{Digest, Md5};
use serde_json::{Map, Value};

use crate::percent;

/// 签名盐，官方客户端内置常量。
pub const SALT: &str = "fe8aac4e02f845b8ad67c427d48bfaf1";

/// 解析设备指纹模板（保留字段顺序）。
pub fn parse_template(template: &str) -> Map<String, Value> {
    match serde_json::from_str::<Value>(template) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

/// Python `f"{value}"` 的等价物：字符串取原值、数字取十进制、`None` 取 `"None"`。
fn value_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "None".to_string(),
        Value::Bool(b) => {
            if *b {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        other => other.to_string(),
    }
}

/// 按模板字段顺序拼接表单体，并在末尾追加 `sign=`。
pub fn build_body(data: &Map<String, Value>) -> String {
    let mut body = String::with_capacity(2048);
    for (key, value) in data {
        if key == "pwd" {
            body.push_str("pwd=");
            body.push_str(&percent::quote(&value_text(value)));
            body.push('&');
        }
        body.push_str(key);
        body.push('=');
        body.push_str(&value_text(value));
        body.push('&');
    }
    body.push_str("sign=");
    body.push_str(&sign(data));
    body
}

/// 签名：按 key 升序拼接所有值 + 盐，MD5 小写十六进制。
pub fn sign(data: &Map<String, Value>) -> String {
    let mut keys: Vec<&String> = data.keys().collect();
    keys.sort();

    let mut joined = String::with_capacity(2048);
    for key in keys {
        if let Some(value) = data.get(key) {
            joined.push_str(&value_text(value));
        }
    }
    joined.push_str(SALT);

    let mut hasher = Md5::new();
    hasher.update(joined.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// 在模板上叠加字段（按需覆盖已有 key），补上时间戳，返回可直接提交的表单体。
pub fn signed_body(template: &str, overrides: &[(&str, Value)]) -> String {
    let mut data = parse_template(template);
    for (key, value) in overrides {
        data.insert((*key).to_string(), value.clone());
    }
    let now = now_secs();
    data.insert("timestamp".to_string(), Value::from(now));
    data.insert("client_timestamp".to_string(), Value::from(now));
    build_body(&data)
}

/// Unix 秒。签名体里的 `timestamp` / `client_timestamp` 都是秒级整数。
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn md5_hex(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts;

    #[test]
    fn body_starts_with_template_order_and_ends_with_sign() {
        let body = signed_body(consts::MODOL_RSA, &[]);
        assert!(body.starts_with("operators=5&merchant_id=1&isRoot=0&"));
        assert!(body.contains("&timestamp="));
        assert!(body.contains("&client_timestamp="));
        let (_, sign) = body.rsplit_once("sign=").expect("missing sign");
        assert_eq!(sign.len(), 32);
        assert_eq!(body.matches("&sign=").count(), 1);
    }

    #[test]
    fn pwd_is_emitted_twice_once_encoded() {
        let body = signed_body(consts::MODOL_LOGIN, &[("pwd", Value::from("a+b/c="))]);
        // quote 保持 '/' 不转义，转义 '+' 与 '='
        assert!(body.contains("pwd=a%2Bb/c%3D&"), "{body}");
        assert!(body.contains("pwd=a+b/c=&"), "{body}");
    }

    /// 签名 = md5(按 key 升序拼值 + 盐)。字段声明顺序不影响签名。
    #[test]
    fn sign_concatenates_values_in_key_order() {
        let data = parse_template(r#"{"b":"2","a":"1"}"#);
        assert_eq!(sign(&data), md5_hex(&format!("12{SALT}")));

        let mut reordered = parse_template(r#"{"a":"1","b":"2"}"#);
        assert_eq!(sign(&data), sign(&reordered));

        reordered.insert("c".to_string(), Value::from(3));
        assert_eq!(sign(&reordered), md5_hex(&format!("123{SALT}")));
    }

    /// 空值不影响签名串（只拼值），但会影响表单体——这正是「字段增减导致签名失配」的根源。
    #[test]
    fn empty_value_changes_body_but_not_signature() {
        let with_empty = parse_template(r#"{"a":"1","b":""}"#);
        let without = parse_template(r#"{"a":"1"}"#);

        assert_ne!(build_body(&with_empty), build_body(&without));
        assert_eq!(sign(&with_empty), sign(&without));
        assert_eq!(sign(&with_empty), md5_hex(&format!("1{SALT}")));
    }
}
