//! 公主连结（B服）登录工具的核心库。
//!
//! 分两层：
//!
//! - [`bili_sdk`]：B 站 SDK 登录，账号密码 → `uid` + `access_key`
//! - [`game`]：游戏服务器登录，`uid` + `access_key` → `viewer_id` + 会话头 `SID`
//!
//! 协议细节对齐 `autopcr` 与 `kanna_connection_redive_2` 两个已验证实现。

pub mod bili_sdk;
pub mod consts;
pub mod error;
pub mod game;
pub mod percent;
pub mod sign;
pub mod token;

pub use error::{Error, Result};
pub use token::Token;

/// B 服渠道：官服（B站账号登录）与渠道服（QSDK，直接用游戏账号）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// 官服：`platform` 头为 2（Android）/ 1000（iOS）
    Bilibili,
    /// 渠道服：`platform` 头固定 4
    Qu,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Bilibili => "bilibili",
            Channel::Qu => "qu",
        }
    }

    /// `PLATFORM-ID` 请求头。
    pub fn platform_id(self, platform: Platform) -> &'static str {
        match (self, platform) {
            (Channel::Qu, _) => "4",
            (Channel::Bilibili, Platform::Android) => "2",
            (Channel::Bilibili, Platform::Ios) => "1000",
        }
    }

    /// 默认网关。真实服务器列表由 `source_ini/index` 下发。
    pub fn default_apiroot(self) -> &'static str {
        match self {
            Channel::Bilibili => "https://l3-prod-all-gs-gzlj.bilibiligame.net/",
            Channel::Qu => "https://l1-prod-uo-gs-gzlj.bilibiligame.net/",
        }
    }

    /// 官方客户端使用的固定资源密钥；会被维护公告响应里的 `res_key` 覆盖。
    pub fn default_res_key(self) -> &'static str {
        match self {
            Channel::Bilibili => "ab00a0a6dd915a052a2ef7fd649083e5",
            Channel::Qu => "d145b29050641dac2f8b19df0afe0e59",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Android,
    Ios,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Android => "android",
            Platform::Ios => "ios",
        }
    }

    /// `PLATFORM` 请求头。
    pub fn header_value(self) -> &'static str {
        match self {
            Platform::Android => "2",
            Platform::Ios => "1000",
        }
    }
}
