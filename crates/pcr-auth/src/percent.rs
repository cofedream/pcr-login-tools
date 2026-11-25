//! `urllib.parse.quote` 兼容的百分号编码。
//!
//! B 站 SDK 的签名体里 `pwd` 字段用 Python 的 `quote()` 编码过一次，
//! 其默认 `safe='/'`：字母数字与 `_.-~/` 不转义，其余字节转成大写 `%XX`。
//! 用 `url::form_urlencoded` 或 `percent-encoding` 会得到不同结果（`/` 会被转义），
//! 签名就对不上了。

/// 等价于 `urllib.parse.quote(s)`。
pub fn quote(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::quote;

    #[test]
    fn matches_python_quote() {
        // 对照 python3 -c "import urllib.parse;print(urllib.parse.quote(s))"
        assert_eq!(quote("abcXYZ019-_.~/"), "abcXYZ019-_.~/");
        assert_eq!(quote("a+b/c="), "a%2Bb/c%3D");
        assert_eq!(quote("中"), "%E4%B8%AD");
        // Base64 密文里常见的三种特殊字符
        assert_eq!(quote("rxwA8J+GcVdqa3ql/"), "rxwA8J%2BGcVdqa3ql/");
    }
}
