//! 游戏服务器会话建立。
//!
//! 顺序（对齐 `autopcr/core/sessionmgr.py::_login`）：
//!
//! 1. `source_ini/index` 拿服务器列表（失败则退回渠道默认网关）
//! 2. `source_ini/get_maintenance_status` 拿 `RES-KEY` / `MANIFEST-VER` / 客户端版本
//! 3. `tool/sdk_login` 用 `uid` + `access_key` 换会话，`data_headers.viewer_id` 即游戏 UID
//! 4. `check/game_start` 校验账号是否可用（未过教程会被拒绝）
//! 5. 可选 `load/index` 取昵称（响应较大，默认不拉）

use std::time::Duration;

use rand::Rng;
use reqwest::Client;
use rmpv::Value;

use crate::bili_sdk::CaptchaAnswer;
use crate::consts;
use crate::game::codec::{self, Key};
use crate::game::headers::Headers;
use crate::game::{get, get_i64, get_str, get_u64};
use crate::sign::md5_hex;
use crate::{Channel, Error, Platform, Result};

/// 登录成功后的游戏侧信息。
#[derive(Debug, Clone)]
pub struct LoginInfo {
    /// 游戏内 UID（`data_headers.viewer_id`）
    pub viewer_id: u64,
    /// 昵称，未拉取时为 `None`
    pub user_name: Option<String>,
    /// 本次实际使用的 `APP-VER`
    pub app_ver: String,
}

pub struct Session {
    http: Client,
    headers: Headers,
    channel: Channel,
    platform: Platform,
    apiroot: String,
    uid: String,
    access_key: String,
    viewer_id: u64,
    device_index: u32,
    app_ver: String,
    verbose: bool,
    /// 过程中的非致命提示（版本更新、换设备指纹、昵称拉取失败等）。
    pub notes: Vec<String>,
}

struct Response {
    data: Value,
    header: Value,
}

impl Session {
    pub fn new(
        uid: impl Into<String>,
        access_key: impl Into<String>,
        channel: Channel,
        platform: Platform,
        verbose: bool,
    ) -> Result<Self> {
        let uid = uid.into().trim().to_string();
        let access_key = access_key.into().trim().to_string();
        if uid.is_empty() || access_key.is_empty() {
            return Err(Error::Other("uid 与 access_key 都不能为空".to_string()));
        }

        let app_ver = consts::DEFAULT_APP_VER.to_string();
        let headers = Headers::new(
            channel,
            platform,
            &Self::device_id(&uid, &access_key, 0),
            &app_ver,
        );
        let http = Client::builder().timeout(Duration::from_secs(20)).build()?;

        Ok(Self {
            http,
            headers,
            channel,
            platform,
            apiroot: channel.default_apiroot().to_string(),
            uid,
            access_key,
            viewer_id: 0,
            device_index: 0,
            app_ver,
            verbose,
            notes: Vec::new(),
        })
    }

    /// 设备指纹。遇到 203 时递增 `index` 换一个指纹重试。
    fn device_id(uid: &str, access_key: &str, index: u32) -> String {
        md5_hex(&format!("{uid}:{access_key}:{index}"))
    }

    pub fn apiroot(&self) -> &str {
        &self.apiroot
    }

    /// 用 `uid` + `access_key` 完成游戏服务器登录。
    pub async fn login(&mut self, fetch_name: bool) -> Result<LoginInfo> {
        self.login_with_captcha(None, fetch_name).await
    }

    /// 同上，但可以把极验结果带进 `tool/sdk_login`。
    ///
    /// 游戏侧对部分账号会再要一次验证码（响应 `is_risk=true`），
    /// 这时需要把新的 `challenge` / `validate` 一起提交，见 `autopcr/core/sessionmgr.py::_ensure_token`。
    pub async fn login_with_captcha(
        &mut self,
        captcha: Option<&CaptchaAnswer>,
        fetch_name: bool,
    ) -> Result<LoginInfo> {
        self.discover_servers().await;
        self.check_maintenance().await?;

        let mut device_rotations = 0;
        loop {
            match self.sdk_login(captcha).await {
                Ok(()) => break,
                Err(Error::AccountBlocked) if device_rotations < 2 => {
                    device_rotations += 1;
                    self.device_index += 1;
                    let device_id = Self::device_id(&self.uid, &self.access_key, self.device_index);
                    self.headers.set("DEVICE-ID", &device_id);
                    self.notes.push(format!(
                        "服务端判定凭据无效，换用第 {device_rotations} 个设备指纹重试"
                    ));
                }
                Err(error) => return Err(error),
            }
        }

        self.check_game_start().await?;

        let user_name = if fetch_name {
            self.fetch_user_name().await
        } else {
            None
        };

        Ok(LoginInfo {
            viewer_id: self.viewer_id,
            user_name,
            app_ver: self.app_ver.clone(),
        })
    }

    /// 服务器列表。失败不致命：退回渠道默认网关。
    async fn discover_servers(&mut self) {
        match self
            .send("source_ini/index?format=json", false, Vec::new())
            .await
        {
            Ok(response) => {
                let root = get(&response.data, "server")
                    .and_then(|value| value.as_array())
                    .and_then(|servers| {
                        servers
                            .iter()
                            .filter_map(|v| v.as_str())
                            .find_map(normalize_root)
                    });
                if let Some(root) = root {
                    if self.verbose {
                        self.notes.push(format!("服务器网关: {root}"));
                    }
                    self.apiroot = root;
                }
            }
            Err(error) => self
                .notes
                .push(format!("获取服务器列表失败，使用默认网关: {error}")),
        }
    }

    async fn check_maintenance(&mut self) -> Result<()> {
        // 版本号变化时需要带新 APP-VER 重拉，最多三轮。
        for attempt in 0..3 {
            let response = self
                .send(
                    "source_ini/get_maintenance_status?format=json",
                    false,
                    Vec::new(),
                )
                .await?;

            let version_bumped = self.apply_version_hint(&response.header);

            if let Some(res_key) = get_str(&response.data, "res_key") {
                if !res_key.is_empty() {
                    self.headers.set("RES-KEY", res_key);
                }
            }

            let manifest = get_str(&response.data, "required_manifest_ver")
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    get_str(&response.data, "manifest_ver").filter(|value| !value.is_empty())
                });
            if let Some(manifest) = manifest {
                self.headers.set("MANIFEST-VER", manifest);
            }

            if let Some(message) = get_str(&response.data, "maintenance_message") {
                if !message.is_empty() {
                    self.notes.push(format!("维护公告: {message}"));
                }
            }

            if !version_bumped || attempt == 2 {
                return Ok(());
            }
        }
        Ok(())
    }

    async fn sdk_login(&mut self, captcha: Option<&CaptchaAnswer>) -> Result<()> {
        let platform_id = self.channel.platform_id(self.platform);
        let mut fields = vec![
            ("uid", Value::from(self.uid.as_str())),
            ("access_key", Value::from(self.access_key.as_str())),
            ("platform", Value::from(platform_id)),
            ("channel_id", Value::from("1")),
        ];
        if let Some(answer) = captcha {
            fields.extend([
                ("challenge", Value::from(answer.challenge.as_str())),
                ("validate", Value::from(answer.validate.as_str())),
                ("seccode", Value::from(answer.seccode())),
                ("captcha_type", Value::from("1")),
                ("image_token", Value::from("")),
                ("captcha_code", Value::from("")),
            ]);
        }

        let response = self.send("tool/sdk_login", true, fields).await?;

        if let Some(is_risk) = get(&response.data, "is_risk") {
            let risky = is_risk
                .as_bool()
                .unwrap_or_else(|| is_risk.as_i64().unwrap_or(0) != 0);
            if risky {
                self.notes
                    .push("tool/sdk_login 返回 is_risk：游戏侧要求在登录时提交验证码".to_string());
                return Err(Error::RiskControl(
                    "tool/sdk_login 返回 is_risk，需要过一次极验".to_string(),
                ));
            }
        }
        Ok(())
    }

    async fn check_game_start(&mut self) -> Result<()> {
        for _ in 0..2 {
            let campaign_user = (rand::thread_rng().gen_range(0..100_000) & !1) as i64;
            let response = self
                .send(
                    "check/game_start",
                    true,
                    vec![
                        ("apptype", Value::from(0)),
                        ("campaign_data", Value::from("")),
                        ("campaign_user", Value::from(campaign_user)),
                    ],
                )
                .await?;

            if self.apply_version_hint(&response.header) {
                continue;
            }

            if let Some(now_tutorial) = get(&response.data, "now_tutorial") {
                let passed = now_tutorial
                    .as_bool()
                    .unwrap_or_else(|| now_tutorial.as_i64().unwrap_or(1) != 0);
                if !passed {
                    return Err(Error::Tutorial);
                }
            }
            return Ok(());
        }
        Err(Error::VersionOutdated(self.app_ver.clone()))
    }

    async fn fetch_user_name(&mut self) -> Option<String> {
        match self
            .send("load/index", true, vec![("carrier", Value::from("OPPO"))])
            .await
        {
            Ok(response) => get(&response.data, "user_info")
                .and_then(|info| get_str(info, "user_name"))
                .map(|name| name.to_string()),
            Err(error) => {
                self.notes
                    .push(format!("拉取昵称失败（不影响令牌）: {error}"));
                None
            }
        }
    }

    /// 从 `store_url` 里同步客户端版本，返回是否发生了更新。
    fn apply_version_hint(&mut self, header: &Value) -> bool {
        let Some(store_url) = get_str(header, "store_url") else {
            return false;
        };
        let Some(version) = extract_app_ver(store_url) else {
            return false;
        };
        if version == self.app_ver {
            return false;
        }
        self.notes
            .push(format!("客户端版本更新: {} → {version}", self.app_ver));
        self.app_ver = version.clone();
        self.headers.set("APP-VER", &version);
        true
    }

    async fn send(
        &mut self,
        url: &str,
        crypted: bool,
        fields: Vec<(&str, Value)>,
    ) -> Result<Response> {
        let key = Key::random();

        let (body, content_type) = if crypted {
            let mut entries: Vec<(Value, Value)> = fields
                .into_iter()
                .map(|(field, value)| (Value::from(field), value))
                .collect();
            entries.push((
                Value::from("viewer_id"),
                Value::from(codec::encrypt_viewer_id(self.viewer_id, &key)),
            ));
            (
                codec::encrypt_payload(&Value::Map(entries), &key),
                "application/octet-stream",
            )
        } else {
            let payload = serde_json::json!({ "viewer_id": self.viewer_id.to_string() });
            (
                serde_json::to_vec(&payload)
                    .map_err(|error| Error::Decode(format!("序列化请求体失败: {error}")))?,
                "application/json",
            )
        };

        let full_url = format!("{}{}", self.apiroot, url);
        if self.verbose {
            self.notes.push(format!("POST {full_url}"));
        }

        let response = self
            .http
            .post(&full_url)
            .headers(self.headers.to_header_map())
            .header("Content-Type", content_type)
            .body(body)
            .send()
            .await?;

        let status = response.status();
        let bytes = response.bytes().await?;
        if !status.is_success() {
            return Err(Error::Other(format!(
                "请求 {url} 返回 HTTP {status}（{} 字节）",
                bytes.len()
            )));
        }

        let value = if crypted {
            codec::no_null_key(codec::decrypt_response(&bytes)?)
        } else {
            let json: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|error| Error::Decode(format!("{url} 返回的不是 JSON: {error}")))?;
            codec::no_null_key(codec::json_to_rmpv(&json))
        };

        let header = match get(&value, "data_headers") {
            Some(header) => header.clone(),
            None => {
                return Err(Error::Decode(format!(
                    "{url} 响应缺少 data_headers: {}",
                    crate::error::summarize(&value)
                )))
            }
        };

        if let Some(sid) = get_str(&header, "sid") {
            if !sid.is_empty() {
                self.headers.set("SID", &md5_hex(&format!("{sid}c!SID!n")));
            }
        }
        if let Some(request_id) = get_str(&header, "request_id") {
            if !request_id.is_empty() {
                self.headers.set("REQUEST-ID", request_id);
            }
        }
        if let Some(viewer_id) = get_u64(&header, "viewer_id") {
            if viewer_id > 0 {
                self.viewer_id = viewer_id;
            }
        }

        let data = get(&value, "data").cloned().unwrap_or(Value::Nil);
        if let Some(server_error) = get(&data, "server_error") {
            let code = get_i64(&header, "result_code").unwrap_or(-1);
            let message = get_str(server_error, "message")
                .unwrap_or("未知错误")
                .to_string();
            if message.contains("维护") {
                let detail = get_str(&data, "maintenance_message").unwrap_or_default();
                let text = if detail.is_empty() {
                    message
                } else {
                    format!("{message}（{detail}）")
                };
                self.notes
                    .push(format!("{url} 返回 result_code={code}: {text}"));
                return Err(Error::Maintenance(text));
            }
            self.notes
                .push(format!("{url} 返回 result_code={code}: {message}"));
            return Err(Error::from_result_code(code, message));
        }

        Ok(Response { data, header })
    }
}

fn normalize_root(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches('\t').trim();
    if trimmed.is_empty() {
        return None;
    }
    let with_scheme = if trimmed.starts_with("http") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    Some(if with_scheme.ends_with('/') {
        with_scheme
    } else {
        format!("{with_scheme}/")
    })
}

/// 从 `store_url` 里取客户端版本：优先 `gzlj_` 之后，其次第一个 `x.y.z`。
fn extract_app_ver(store_url: &str) -> Option<String> {
    const MARKER: &str = "gzlj_";
    if let Some(index) = store_url.find(MARKER) {
        if let Some(version) = first_version(&store_url[index + MARKER.len()..]) {
            return Some(version);
        }
    }
    first_version(store_url)
}

fn first_version(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let start = index;
        let mut dots = 0;
        while index < bytes.len() && (bytes[index].is_ascii_digit() || bytes[index] == b'.') {
            if bytes[index] == b'.' {
                dots += 1;
            }
            index += 1;
        }
        let candidate = input[start..index].trim_end_matches('.');
        if dots >= 2 && !candidate.is_empty() {
            return Some(candidate.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_root_gets_scheme_and_trailing_slash() {
        assert_eq!(
            normalize_root("l3-prod-all-gs-gzlj.bilibiligame.net").as_deref(),
            Some("https://l3-prod-all-gs-gzlj.bilibiligame.net/")
        );
        assert_eq!(normalize_root("").as_deref(), None);
        assert_eq!(
            normalize_root("https://a.b.com\t").as_deref(),
            Some("https://a.b.com/")
        );
    }

    #[test]
    fn version_is_read_from_store_url() {
        let url = "https://example.com/pcr_gzlj_11.7.2_20240101.apk";
        assert_eq!(extract_app_ver(url).as_deref(), Some("11.7.2"));
        assert_eq!(
            extract_app_ver("https://x/11.8.0/foo").as_deref(),
            Some("11.8.0")
        );
        assert_eq!(extract_app_ver("https://x/foo"), None);
    }

    #[test]
    fn device_id_is_stable_per_index() {
        let a = Session::device_id("123", "key", 0);
        let b = Session::device_id("123", "key", 0);
        let c = Session::device_id("123", "key", 1);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 32);
    }
}
