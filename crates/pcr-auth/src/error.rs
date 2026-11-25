/// 错误类型。区分「可操作」与「环境问题」，CLI 据此给出下一步提示。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("账号或密码错误")]
    Password,

    /// 登录被风控拦截，需要过一次极验。`gt` / `challenge` / `gt_user_id`
    /// 来自 `/api/client/start_captcha`，交给调用方决定怎么解。
    #[error("需要验证码（gt={gt} challenge={challenge} gt_user_id={gt_user_id}）")]
    NeedCaptcha {
        gt: String,
        challenge: String,
        gt_user_id: String,
    },

    #[error("验证码验证未通过")]
    CaptchaFailed,

    #[error("账号触发风控: {0}")]
    RiskControl(String),

    #[error("账号未完成教程，无法进入游戏")]
    Tutorial,

    #[error("access_key 已失效或未被服务端接受，请重新获取")]
    AccessKeyExpired,

    /// 服务端返回 203：账号被限制 / 会话被拒。换设备指纹可能绕过。
    #[error("服务端拒绝该账号（result_code 203）")]
    AccountBlocked,

    #[error("客户端版本过期（{0}），服务端拒绝该 APP-VER")]
    VersionOutdated(String),

    #[error("服务器维护中: {0}")]
    Maintenance(String),

    #[error("服务端返回错误 {code}: {message}")]
    Api { code: i64, message: String },

    #[error("网络请求失败: {0}")]
    Http(#[from] reqwest::Error),

    #[error("响应解析失败: {0}")]
    Decode(String),

    #[error("签名/加解密失败: {0}")]
    Crypto(String),

    #[error("{0}")]
    Other(String),
}

/// 业务错误码（`data_headers.result_code` 与 SDK 返回的 `code`）。
pub mod codes {
    /// SDK 登录要求过极验
    pub const SDK_NEED_CAPTCHA: i64 = 200000;
    /// SDK 验证码校验未通过
    pub const SDK_CAPTCHA_FAILED: i64 = 200001;
    /// SDK 判定账号存在风险
    pub const SDK_RISK_CONTROL: i64 = 500053;

    /// 会话错误（需重新登录）
    pub const API_SESSION_ERROR: i64 = 201;
    /// 账号认证失败（access_key 无效）
    pub const API_AUTH_ERROR: i64 = 202;
    /// 账号被限制 / access_key 失效，换设备指纹可重试
    pub const API_ACCOUNT_BLOCK: i64 = 203;
    /// 客户端版本错误
    pub const API_VERSION_ERROR: i64 = 204;
    /// APP 资源版本错误
    pub const API_APPRES_VERSION_ERROR: i64 = 217;
    /// 请求体解码失败（通常是 AES/msgpack 写错）
    pub const API_REQUEST_DECODE_ERROR: i64 = 218;
    /// 响应体解码失败
    pub const API_RESPONSE_DECODE_ERROR: i64 = 219;
}

pub type Result<T> = std::result::Result<T, Error>;

/// 把 `rmpv` 的 Value 转成可读 JSON，仅用于日志与错误信息。
pub(crate) fn value_to_json(value: &rmpv::Value) -> serde_json::Value {
    use rmpv::Value as V;
    match value {
        V::Nil => serde_json::Value::Null,
        V::Boolean(b) => serde_json::Value::Bool(*b),
        V::Integer(i) => {
            if let Some(u) = i.as_u64() {
                serde_json::Value::from(u)
            } else if let Some(i) = i.as_i64() {
                serde_json::Value::from(i)
            } else {
                serde_json::Value::Null
            }
        }
        V::F32(f) => serde_json::Value::from(*f as f64),
        V::F64(f) => serde_json::Value::from(*f),
        V::String(s) => serde_json::Value::String(s.as_str().unwrap_or_default().to_string()),
        V::Binary(b) => serde_json::Value::String(String::from_utf8_lossy(b).into_owned()),
        V::Array(items) => serde_json::Value::Array(items.iter().map(value_to_json).collect()),
        V::Map(entries) => {
            let mut map = serde_json::Map::new();
            for (k, v) in entries {
                let key = match k {
                    V::String(s) => s.as_str().unwrap_or_default().to_string(),
                    other => value_to_json(other).to_string(),
                };
                map.insert(key, value_to_json(v));
            }
            serde_json::Value::Object(map)
        }
        V::Ext(_, data) => serde_json::Value::String(String::from_utf8_lossy(data).into_owned()),
    }
}

/// 生成给用户看的诊断摘要（截断，避免把整个响应打出来）。
pub(crate) fn summarize(value: &rmpv::Value) -> String {
    let mut text =
        serde_json::to_string(&value_to_json(value)).unwrap_or_else(|_| "<unprintable>".into());
    const LIMIT: usize = 600;
    if text.chars().count() > LIMIT {
        text = text.chars().take(LIMIT).collect::<String>() + "…";
    }
    text
}

impl Error {
    /// 服务端错误码 → 语义化错误。
    pub(crate) fn from_result_code(code: i64, message: String) -> Self {
        match code {
            codes::API_SESSION_ERROR | codes::API_AUTH_ERROR => Error::AccessKeyExpired,
            codes::API_ACCOUNT_BLOCK => Error::AccountBlocked,
            codes::API_VERSION_ERROR | codes::API_APPRES_VERSION_ERROR => {
                Error::VersionOutdated(message)
            }
            codes::API_REQUEST_DECODE_ERROR => Error::Crypto(format!(
                "服务端无法解码请求体（{message}）。通常是 AES/msgpack 编码与客户端不一致"
            )),
            codes::API_RESPONSE_DECODE_ERROR => {
                Error::Decode(format!("服务端返回了无法解码的响应体（{message}）"))
            }
            _ => Error::Api { code, message },
        }
    }
}

/// CLI 退出码约定：0 成功、2 失败、3 需要过验证码。
pub const EXIT_NEED_CAPTCHA: u8 = 3;
