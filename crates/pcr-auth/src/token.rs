//! 登录结果的数据结构。
//!
//! 只承载「可以重新获取」的凭据：`uid` + `access_key`。
//! 不保存 B 站帐密，access_key 失效时由用户重新运行一次 `login`。

use serde::Serialize;

use crate::{Channel, Platform};

#[derive(Debug, Clone, Serialize)]
pub struct Token {
    /// B 站数字账号
    pub uid: String,
    /// 长期凭据，等价于「登录令牌」
    pub access_key: String,
    /// 游戏内 UID（`data_headers.viewer_id`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewer_id: Option<String>,
    /// 游戏昵称（需要 `--name` 才会拉取）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    pub channel: &'static str,
    pub platform: &'static str,
    /// 本次实际使用的客户端版本
    pub app_ver: String,
    /// 直接发给 kanna_connection_redive_2 机器人的绑定命令
    pub kanna_bind_command: String,
}

impl Token {
    pub fn new(
        uid: impl Into<String>,
        access_key: impl Into<String>,
        channel: Channel,
        platform: Platform,
        app_ver: impl Into<String>,
    ) -> Self {
        let uid = uid.into();
        let access_key = access_key.into();
        let kanna_bind_command = match channel {
            Channel::Bilibili => format!("B绑定账号 {uid} {access_key}"),
            Channel::Qu => format!("渠绑定账号 {uid} {access_key}"),
        };

        Self {
            uid,
            access_key,
            viewer_id: None,
            user_name: None,
            channel: channel.as_str(),
            platform: platform.as_str(),
            app_ver: app_ver.into(),
            kanna_bind_command,
        }
    }

    pub fn with_game(mut self, viewer_id: u64, user_name: Option<String>) -> Self {
        self.viewer_id = Some(viewer_id.to_string());
        self.user_name = user_name;
        self
    }

    /// kanna 的 `B绑定账号` 要求 uid 是纯数字，否则会直接拒绝绑定。
    pub fn uid_is_numeric(&self) -> bool {
        !self.uid.is_empty() && self.uid.chars().all(|c| c.is_ascii_digit())
    }

    pub fn to_json(&self) -> crate::Result<String> {
        serde_json::to_string_pretty(self)
            .map_err(|error| crate::Error::Other(format!("序列化结果失败: {error}")))
    }
}
