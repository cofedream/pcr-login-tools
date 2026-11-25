//! B 站 SDK 登录：账号密码 → `uid` + `access_key`。
//!
//! 流程（对齐 `autopcr/autopcr/sdk/bsgamesdk.py`）：
//!
//! ```text
//! POST api/client/rsa    → rsa_key, hash
//! pwd = base64(PKCS1v1.5(hash + password))
//! POST api/client/login  → { code, uid, access_key }
//! code=200000 时需要验证码：
//! POST api/client/start_captcha → gt, challenge, gt_user_id
//! 带着 challenge/validate/seccode 再请求一次 login
//! ```

use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use rand::rngs::ThreadRng;
use reqwest::Client;
use rsa::pkcs1::DecodeRsaPublicKey;
use rsa::pkcs1v15::Pkcs1v15Encrypt;
use rsa::pkcs8::DecodePublicKey;
use rsa::RsaPublicKey;
use serde_json::Value;

use crate::consts;
use crate::error::codes;
use crate::sign;
use crate::{Error, Result};

/// `/api/client/start_captcha` 下发的极验参数。
#[derive(Debug, Clone)]
pub struct CaptchaChallenge {
    pub gt: String,
    pub challenge: String,
    pub gt_user_id: String,
}

/// 人工/外部求解后的极验结果，回填给登录接口。
#[derive(Debug, Clone)]
pub struct CaptchaAnswer {
    pub challenge: String,
    pub gt_user_id: String,
    pub validate: String,
}

impl CaptchaAnswer {
    /// `seccode` 字段的固定拼法。
    pub fn seccode(&self) -> String {
        format!("{}|jordan", self.validate)
    }
}

#[derive(Debug)]
pub enum LoginOutcome {
    Authorized {
        uid: String,
        access_key: String,
    },
    /// 需要过一次极验，调用 [`SdkClient::start_captcha`] 取参数。
    NeedCaptcha,
}

pub struct SdkClient {
    http: Client,
}

impl SdkClient {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .user_agent(consts::SDK_USER_AGENT)
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self { http })
    }

    async fn post_form(&self, path: &str, body: String) -> Result<Value> {
        let url = format!("{}{}", consts::SDK_BASE, path);
        let response = self
            .http
            .post(&url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await?;

        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            return Err(Error::Other(format!(
                "SDK 接口 {path} 返回 HTTP {status}: {}",
                truncate(&text)
            )));
        }
        serde_json::from_str(&text).map_err(|error| {
            Error::Decode(format!(
                "SDK 接口 {path} 返回非 JSON: {error}: {}",
                truncate(&text)
            ))
        })
    }

    /// 取 RSA 公钥与 `hash`。
    pub async fn rsa(&self) -> Result<(String, String)> {
        let body = sign::signed_body(consts::MODOL_RSA, &[]);
        let response = self.post_form("api/client/rsa", body).await?;

        let rsa_key = response
            .get("rsa_key")
            .and_then(|value| value.as_str())
            .ok_or_else(|| Error::Decode(format!("rsa 接口缺少 rsa_key: {response}")))?;
        let hash = response
            .get("hash")
            .and_then(|value| value.as_str())
            .ok_or_else(|| Error::Decode(format!("rsa 接口缺少 hash: {response}")))?;

        Ok((rsa_key.to_string(), hash.to_string()))
    }

    /// 申请一次极验挑战。
    pub async fn start_captcha(&self) -> Result<CaptchaChallenge> {
        let body = sign::signed_body(consts::MODOL_CAPTCHA, &[]);
        let response = self.post_form("api/client/start_captcha", body).await?;

        let field = |name: &str| -> Result<String> {
            response
                .get(name)
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
                .ok_or_else(|| Error::Decode(format!("start_captcha 缺少 {name}: {response}")))
        };

        Ok(CaptchaChallenge {
            gt: field("gt")?,
            challenge: field("challenge")?,
            gt_user_id: field("gt_user_id")?,
        })
    }

    /// 用账号密码登录。已知验证码结果时传 `captcha`。
    pub async fn login(
        &self,
        account: &str,
        password: &str,
        captcha: Option<&CaptchaAnswer>,
    ) -> Result<LoginOutcome> {
        let (rsa_key, hash) = self.rsa().await?;
        let pwd = rsa_encrypt(&format!("{hash}{password}"), &rsa_key)?;

        let mut overrides: Vec<(&str, Value)> = vec![
            ("access_key", Value::from("")),
            ("uid", Value::from("")),
            ("user_id", Value::from(account)),
            ("pwd", Value::from(pwd)),
        ];
        match captcha {
            Some(answer) => {
                overrides.push(("gt_user_id", Value::from(answer.gt_user_id.as_str())));
                overrides.push(("challenge", Value::from(answer.challenge.as_str())));
                overrides.push(("validate", Value::from(answer.validate.as_str())));
                overrides.push(("seccode", Value::from(answer.seccode())));
            }
            None => {
                overrides.push(("gt_user_id", Value::from("")));
                overrides.push(("challenge", Value::from("")));
                overrides.push(("validate", Value::from("")));
                overrides.push(("seccode", Value::from("")));
            }
        }

        let body = sign::signed_body(consts::MODOL_LOGIN, &overrides);
        let response = self.post_form("api/client/login", body).await?;
        interpret_login(&response)
    }
}

fn interpret_login(response: &Value) -> Result<LoginOutcome> {
    let code = response
        .get("code")
        .and_then(|value| value.as_i64())
        .unwrap_or(-1);
    let message = response
        .get("message")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();

    // 注意：`uid` 在不同账号上可能是字符串也可能是数字（旧 B 站账号实测返回数字），
    // 只按字符串取值会把成功响应判成失败。
    if let (Some(uid), Some(access_key)) = (
        text_field(response, "uid"),
        text_field(response, "access_key"),
    ) {
        return Ok(LoginOutcome::Authorized { uid, access_key });
    }

    match code {
        codes::SDK_NEED_CAPTCHA => Ok(LoginOutcome::NeedCaptcha),
        // code=0 但没有 uid/access_key：账号可能需要短信/实名等额外验证
        0 => Err(Error::Other(format!(
            "登录成功但没有返回 access_key（账号可能有额外验证要求）: {}",
            preview(response)
        ))),
        codes::SDK_CAPTCHA_FAILED => Err(Error::CaptchaFailed),
        codes::SDK_RISK_CONTROL => Err(Error::RiskControl(if message.is_empty() {
            "SDK 判定账号存在风险".to_string()
        } else {
            message
        })),
        _ if message.contains("密码错误") || message.contains("账号不存在") => {
            Err(Error::Password)
        }
        _ => Err(Error::Api { code, message }),
    }
}

/// PKCS#1 v1.5 加密（与 PyCryptodome 的 `PKCS1_v1_5` 一致）。
pub fn rsa_encrypt(message: &str, public_key: &str) -> Result<String> {
    let key: RsaPublicKey = RsaPublicKey::from_pkcs1_pem(public_key.trim())
        .or_else(|_| RsaPublicKey::from_public_key_pem(public_key.trim()))
        .map_err(|error| Error::Crypto(format!("解析 RSA 公钥失败: {error}")))?;

    let mut rng: ThreadRng = rand::thread_rng();
    let encrypted = key
        .encrypt(&mut rng, Pkcs1v15Encrypt, message.as_bytes())
        .map_err(|error| Error::Crypto(format!("RSA 加密失败: {error}")))?;

    Ok(B64.encode(encrypted))
}

/// 取字符串或数字字段（空字符串视为缺失）。
fn text_field(value: &Value, key: &str) -> Option<String> {
    match value.get(key) {
        Some(Value::String(text)) => Some(text.clone()).filter(|text| !text.is_empty()),
        Some(Value::Number(number)) => Some(number.to_string()),
        _ => None,
    }
}

/// 截断后的响应预览，用于错误信息。
fn preview(value: &Value) -> String {
    let text = value.to_string();
    const LIMIT: usize = 400;
    if text.chars().count() <= LIMIT {
        text
    } else {
        text.chars().take(LIMIT).collect::<String>() + "…"
    }
}

fn truncate(text: &str) -> String {
    const LIMIT: usize = 300;
    if text.chars().count() <= LIMIT {
        return text.to_string();
    }
    text.chars().take(LIMIT).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn successful_login_returns_credentials() {
        let response = json!({ "code": 0, "uid": "123456789", "access_key": "abc" });
        let outcome = interpret_login(&response).expect("ok");
        assert!(matches!(
            outcome,
            LoginOutcome::Authorized { uid, access_key } if uid == "123456789" && access_key == "abc"
        ));
    }

    /// 线上真实响应：uid 是数字、access_key 带 `_sh` 后缀。
    #[test]
    fn numeric_uid_is_accepted() {
        let response = json!({
            "code": 0,
            "need_captch": "1",
            "uid": 481952,
            "access_key": "d250d4d15902feaca47182ea57df2761_sh",
            "expires": 1798183257000_i64
        });
        match interpret_login(&response).expect("ok") {
            LoginOutcome::Authorized { uid, access_key } => {
                assert_eq!(uid, "481952");
                assert_eq!(access_key, "d250d4d15902feaca47182ea57df2761_sh");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// code=0 但确实没有 access_key 时，错误信息要带上响应，便于定位。
    #[test]
    fn missing_access_key_reports_body() {
        let error = interpret_login(&json!({ "code": 0, "uid": 1 })).unwrap_err();
        assert!(format!("{error}").contains("没有返回 access_key"));
    }

    #[test]
    fn captcha_codes_map_to_semantic_errors() {
        assert!(matches!(
            interpret_login(&json!({ "code": 200000 })).expect("need captcha"),
            LoginOutcome::NeedCaptcha
        ));
        assert!(matches!(
            interpret_login(&json!({ "code": 200001, "message": "验证码错误" })),
            Err(Error::CaptchaFailed)
        ));
        assert!(matches!(
            interpret_login(&json!({ "code": 500053, "message": "风险验证" })),
            Err(Error::RiskControl(_))
        ));
        assert!(matches!(
            interpret_login(&json!({ "code": 100009, "message": "用户名或密码错误" })),
            Err(Error::Password)
        ));
    }

    #[test]
    fn seccode_appends_jordan() {
        let answer = CaptchaAnswer {
            challenge: "c".into(),
            gt_user_id: "u".into(),
            validate: "v".into(),
        };
        assert_eq!(answer.seccode(), "v|jordan");
    }
}
