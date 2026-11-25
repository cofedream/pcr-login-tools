//! 游戏服务器请求头。
//!
//! 除了固定指纹，还有几项随账号与会话变化：
//!
//! | 头 | 来源 |
//! |---|---|
//! | `DEVICE-ID` | `md5("{uid}:{access_key}:{index}")`，遇到 203 时递增 index 重试 |
//! | `RES-KEY` | 渠道固定值，随后被维护公告响应的 `res_key` 覆盖 |
//! | `MANIFEST-VER` | 维护公告响应的 `required_manifest_ver` |
//! | `APP-VER` | `store_url` 里解析出的客户端版本 |
//! | `SID` / `REQUEST-ID` | 每个响应 `data_headers` 里带回来的会话标识 |

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::{consts, Channel, Platform};

#[derive(Debug, Clone)]
pub struct Headers(Vec<(String, String)>);

impl Headers {
    pub fn new(channel: Channel, platform: Platform, device_id: &str, app_ver: &str) -> Self {
        let base = match platform {
            Platform::Android => consts::ANDROID_HEADERS,
            Platform::Ios => consts::IOS_HEADERS,
        };

        let mut headers = Headers(
            base.iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        );
        headers.set("APP-VER", app_ver);
        headers.set("DEVICE-ID", device_id);
        headers.set("RES-KEY", channel.default_res_key());
        headers.set("PLATFORM", platform.header_value());
        headers.set("PLATFORM-ID", channel.platform_id(platform));
        headers.set("CHANNEL-ID", "1");
        headers
    }

    pub fn set(&mut self, key: &str, value: &str) {
        if let Some(entry) = self
            .0
            .iter_mut()
            .find(|(existing, _)| existing.eq_ignore_ascii_case(key))
        {
            entry.1 = value.to_string();
            return;
        }
        self.0.push((key.to_string(), value.to_string()));
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(existing, _)| existing.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str())
    }

    pub fn remove(&mut self, key: &str) {
        self.0
            .retain(|(existing, _)| !existing.eq_ignore_ascii_case(key));
    }

    pub fn to_header_map(&self) -> HeaderMap {
        let mut map = HeaderMap::with_capacity(self.0.len());
        for (key, value) in &self.0 {
            let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(key.as_bytes()),
                HeaderValue::from_str(value),
            ) else {
                continue;
            };
            map.insert(name, value);
        }
        map
    }

    /// 用于 `--verbose` 输出（隐藏会话语义较强的 `SID`）。
    pub fn describe(&self) -> Vec<(String, String)> {
        self.0
            .iter()
            .map(|(key, value)| {
                let masked = if key.eq_ignore_ascii_case("SID") {
                    "***".to_string()
                } else {
                    value.clone()
                };
                (key.clone(), masked)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_and_platform_are_applied() {
        let headers = Headers::new(Channel::Bilibili, Platform::Android, "abc", "11.7.2");
        assert_eq!(headers.get("DEVICE-ID"), Some("abc"));
        assert_eq!(headers.get("PLATFORM"), Some("2"));
        assert_eq!(headers.get("PLATFORM-ID"), Some("2"));
        assert_eq!(headers.get("CHANNEL-ID"), Some("1"));
        assert_eq!(
            headers.get("RES-KEY"),
            Some("ab00a0a6dd915a052a2ef7fd649083e5")
        );
    }

    #[test]
    fn channel_server_overrides_platform_id() {
        let headers = Headers::new(Channel::Qu, Platform::Ios, "abc", "11.7.2");
        assert_eq!(headers.get("PLATFORM-ID"), Some("4"));
        assert_eq!(headers.get("PLATFORM"), Some("1000"));
    }

    #[test]
    fn set_replaces_case_insensitively() {
        let mut headers = Headers::new(Channel::Bilibili, Platform::Android, "abc", "11.7.2");
        headers.set("app-ver", "11.8.0");
        assert_eq!(headers.get("APP-VER"), Some("11.8.0"));
        assert_eq!(
            headers
                .describe()
                .into_iter()
                .filter(|(key, _)| key.eq_ignore_ascii_case("app-ver"))
                .count(),
            1
        );
    }
}
