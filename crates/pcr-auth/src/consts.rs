//! 协议常量。
//!
//! 设备指纹模板与请求头均来自已验证的客户端实现（`autopcr`、`kanna_connection_redive_2`），
//! 游戏更新时需要同步的是 `DEFAULT_APP_VER`、`ANDROID_HEADERS` 里的版本相关内容，
//! 以及 SDK 模板里的 `sdk_ver` / `apk_sign`。

pub mod templates {
    include!("consts_templates.rs");
}

pub use templates::{MODOL_CAPTCHA, MODOL_LOGIN, MODOL_RSA};

/// B 站 SDK 登录网关。
pub const SDK_BASE: &str = "https://line1-sdk-center-login-sh.biligame.net/";
pub const SDK_HOST: &str = "line1-sdk-center-login-sh.biligame.net";
pub const SDK_USER_AGENT: &str = "Mozilla/5.0 BSGameSDK";

/// 客户端版本。服务端维护公告/`store_url` 会下发新版本，见 [`crate::game::session`]。
pub const DEFAULT_APP_VER: &str = "11.7.2";

/// 官方 Android 客户端请求头（不含 DEVICE-ID / RES-KEY / PLATFORM 等动态项）。
pub const ANDROID_HEADERS: &[(&str, &str)] = &[
    ("Accept-Encoding", "gzip"),
    (
        "User-Agent",
        "Dalvik/2.1.0 (Linux, U, Android 5.1.1, PCRT00 Build/LMY48Z)",
    ),
    ("X-Unity-Version", "2021.3.20f1c1"),
    ("APP-VER", DEFAULT_APP_VER),
    ("BATTLE-LOGIC-VERSION", "4"),
    ("BUNDLE-VER", ""),
    ("DEVICE", "2"),
    ("DEVICE-NAME", "OPPO PCRT00"),
    ("EXCEL-VER", "1.0.0"),
    ("GRAPHICS-DEVICE-NAME", "Adreno (TM) 640"),
    ("IP-ADDRESS", "10.0.2.15"),
    ("KEYCHAIN", ""),
    ("LOCALE", "CN"),
    (
        "PLATFORM-OS-VERSION",
        "Android OS 5.1.1 / API-22 (LMY48Z/rel.se.infra.20200612.100533)",
    ),
    ("REGION-CODE", ""),
    ("RES-VER", "10002200"),
    ("SHORT-UDID", "0"),
];

/// 官方 iOS 客户端请求头。
pub const IOS_HEADERS: &[(&str, &str)] = &[
    ("Accept-Encoding", "gzip"),
    ("User-Agent", "priconne/24 CFNetwork/1492.0.1 Darwin/23.3.0"),
    ("X-Unity-Version", "2021.3.20f1c1"),
    ("APP-VER", DEFAULT_APP_VER),
    ("BATTLE-LOGIC-VERSION", "4"),
    ("BUNDLE-VER", ""),
    ("DEVICE", "1"),
    ("DEVICE-NAME", "iPad13,8"),
    ("EXCEL-VER", "1.0.0"),
    ("GRAPHICS-DEVICE-NAME", "Apple M1"),
    ("IP-ADDRESS", "172.26.62.98"),
    ("KEYCHAIN", ""),
    ("LOCALE", "CN"),
    ("PLATFORM-OS-VERSION", "iOS 17.3"),
    ("REGION-CODE", ""),
    ("RES-VER", "10002200"),
    ("SHORT-UDID", "0"),
];

/// 游戏服务器的固定 IV（AES-256-CBC）。
pub const AES_IV: [u8; 16] = *b"7Fk9Lm3Np8Qr4Sv2";
