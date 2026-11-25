//! 账号与本机令牌的持久化。
//!
//! 默认写在当前目录的 `pcr-accounts.json`（可用 `--data` 改路径）。
//!
//! - 账号密码是明文：文件权限设为 `0600`，启动时会把绝对路径打印出来提醒
//! - 写入是「临时文件 + rename」，进程被中断也不会写坏原文件
//! - 令牌结果与账号同一条记录，便于「同一条命令随时再复制」

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use pcr_auth::{Channel, Platform, Token};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct StoreData {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub accounts: Vec<Account>,
}

fn default_version() -> u32 {
    1
}

fn default_channel() -> String {
    "bilibili".to_string()
}

fn default_platform() -> String {
    "android".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub account: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_channel")]
    pub channel: String,
    #[serde(default = "default_platform")]
    pub platform: String,
    #[serde(default)]
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<TokenRecord>,
}

/// 最近一次成功取到的令牌。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRecord {
    pub uid: String,
    pub access_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_ver: Option<String>,
    /// 毫秒时间戳，前端按本地时区渲染
    pub fetched_at: i64,
}

impl Account {
    pub fn channel(&self) -> Channel {
        match self.channel.as_str() {
            "qu" => Channel::Qu,
            _ => Channel::Bilibili,
        }
    }

    pub fn platform(&self) -> Platform {
        match self.platform.as_str() {
            "ios" => Platform::Ios,
            _ => Platform::Android,
        }
    }

    /// 可以直接发给 kanna 机器人的绑定命令。
    pub fn bind_command(&self) -> Option<String> {
        let token = self.token.as_ref()?;
        let prefix = match self.channel() {
            Channel::Bilibili => "B绑定账号",
            Channel::Qu => "渠绑定账号",
        };
        Some(format!("{prefix} {} {}", token.uid, token.access_key))
    }

    pub fn ready(&self) -> bool {
        !self.account.trim().is_empty() && !self.password.is_empty()
    }
}

pub struct Store {
    path: PathBuf,
    data: StoreData,
    serial: u64,
}

#[derive(Debug, Default, Deserialize)]
pub struct AccountPatch {
    pub account: Option<String>,
    pub password: Option<String>,
    pub channel: Option<String>,
    pub platform: Option<String>,
    pub note: Option<String>,
}

impl Store {
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let data = match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} 不是合法 JSON: {error}", path.display()),
                )
            })?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => StoreData {
                version: 1,
                accounts: Vec::new(),
            },
            Err(error) => return Err(error),
        };

        Ok(Self {
            path,
            data,
            serial: 0,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn accounts(&self) -> &[Account] {
        &self.data.accounts
    }

    pub fn find(&self, id: &str) -> Option<&Account> {
        self.data.accounts.iter().find(|account| account.id == id)
    }

    pub fn create(&mut self, patch: AccountPatch) -> Account {
        self.serial += 1;
        let id = format!("a{:x}{:x}", self.now_ms(), self.serial);

        let account = Account {
            id,
            account: patch.account.unwrap_or_default().trim().to_string(),
            password: patch.password.unwrap_or_default(),
            channel: patch.channel.unwrap_or_else(default_channel),
            platform: patch.platform.unwrap_or_else(default_platform),
            note: patch.note.unwrap_or_default(),
            token: None,
        };
        self.data.accounts.push(account.clone());
        account
    }

    pub fn update(&mut self, id: &str, patch: AccountPatch) -> Option<Account> {
        let account = self
            .data
            .accounts
            .iter_mut()
            .find(|account| account.id == id)?;

        if let Some(value) = patch.account {
            account.account = value.trim().to_string();
        }
        if let Some(value) = patch.password {
            account.password = value;
        }
        if let Some(value) = patch.channel {
            account.channel = value;
        }
        if let Some(value) = patch.platform {
            account.platform = value;
        }
        if let Some(value) = patch.note {
            account.note = value;
        }

        Some(account.clone())
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.data.accounts.len();
        self.data.accounts.retain(|account| account.id != id);
        self.data.accounts.len() != before
    }

    /// 记录一次成功登录的结果。
    pub fn apply_token(&mut self, id: &str, token: &Token) -> bool {
        let fetched_at = self.now_ms() as i64;
        let Some(account) = self
            .data
            .accounts
            .iter_mut()
            .find(|account| account.id == id)
        else {
            return false;
        };

        account.token = Some(TokenRecord {
            uid: token.uid.clone(),
            access_key: token.access_key.clone(),
            viewer_id: token.viewer_id.clone(),
            user_name: token.user_name.clone(),
            app_ver: Some(token.app_ver.clone()),
            fetched_at,
        });
        true
    }

    /// 原子写回：临时文件 + rename，避免中断时写坏。
    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let text = serde_json::to_string_pretty(&self.data)
            .map_err(|error| io::Error::other(format!("序列化失败: {error}")))?;

        let temp = self.path.with_extension("json.tmp");
        {
            let mut file = File::create(&temp)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(fs::Permissions::from_mode(0o600))?;
            }
            file.write_all(text.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
        }
        fs::rename(&temp, &self.path)?;
        Ok(())
    }

    /// 导出 CSV（带 BOM，Excel 直接打开不乱码）。
    pub fn to_csv(&self) -> String {
        let mut out =
            String::from("\u{feff}账号,密码,渠道,平台,uid,access_key,viewer_id,昵称,获取时间\n");
        for account in &self.data.accounts {
            let empty = String::new();
            let token = account.token.as_ref();
            row(
                &mut out,
                &[
                    &account.account,
                    &account.password,
                    &account.channel,
                    &account.platform,
                    token.map(|t| &t.uid).unwrap_or(&empty),
                    token.map(|t| &t.access_key).unwrap_or(&empty),
                    token.and_then(|t| t.viewer_id.as_ref()).unwrap_or(&empty),
                    token.and_then(|t| t.user_name.as_ref()).unwrap_or(&empty),
                    &token.map(|t| t.fetched_at.to_string()).unwrap_or_default(),
                ],
            );
        }
        out
    }

    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or_default()
    }
}

fn row(out: &mut String, cells: &[&str]) {
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('"');
        out.push_str(&cell.replace('"', "\"\""));
        out.push('"');
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("pcr-web-test-{name}.json"));
        let _ = fs::remove_file(&path);
        path
    }

    #[test]
    fn create_update_remove_roundtrip() {
        let path = temp_path("crud");
        let mut store = Store::open(&path).expect("open");

        let created = store.create(AccountPatch {
            account: Some(" 13800000000 ".to_string()),
            password: Some("secret".to_string()),
            ..Default::default()
        });
        assert_eq!(created.account, "13800000000");
        assert_eq!(created.channel, "bilibili");
        assert_eq!(created.platform, "android");
        assert!(created.ready());

        let updated = store
            .update(
                &created.id,
                AccountPatch {
                    platform: Some("ios".to_string()),
                    ..Default::default()
                },
            )
            .expect("update");
        assert_eq!(updated.platform, "ios");
        assert_eq!(updated.password, "secret");

        store.save().expect("save");
        let reloaded = Store::open(&path).expect("reload");
        assert_eq!(reloaded.accounts().len(), 1);
        assert_eq!(reloaded.accounts()[0].platform, "ios");

        assert!(reloaded_remove(&path));
    }

    fn reloaded_remove(path: &Path) -> bool {
        let mut store = Store::open(path).expect("open");
        let id = store.accounts()[0].id.clone();
        let removed = store.remove(&id);
        store.save().expect("save");
        removed
    }

    #[test]
    fn token_is_stored_per_account() {
        let path = temp_path("token");
        let mut store = Store::open(&path).expect("open");
        let account = store.create(AccountPatch {
            account: Some("100".to_string()),
            password: Some("p".to_string()),
            ..Default::default()
        });

        let token = Token::new(
            "100",
            "access",
            Channel::Bilibili,
            Platform::Android,
            "11.7.2",
        )
        .with_game(999, Some("骑士君".to_string()));
        assert!(store.apply_token(&account.id, &token));

        let stored = store.find(&account.id).expect("found");
        assert_eq!(
            stored.bind_command().as_deref(),
            Some("B绑定账号 100 access")
        );
        assert!(stored.token.as_ref().is_some_and(|t| t.fetched_at > 0));
    }

    #[test]
    fn csv_escapes_quotes_and_commas() {
        let path = temp_path("csv");
        let mut store = Store::open(&path).expect("open");
        store.create(AccountPatch {
            account: Some("acc,with,comma".to_string()),
            password: Some("pa\"ss".to_string()),
            ..Default::default()
        });

        let csv = store.to_csv();
        assert!(csv.contains("\"acc,with,comma\""));
        assert!(csv.contains("\"pa\"\"ss\""));
        assert_eq!(csv.lines().count(), 2);
    }

    #[test]
    fn missing_file_is_an_empty_store() {
        let path = temp_path("missing");
        let store = Store::open(&path).expect("open");
        assert!(store.accounts().is_empty());
    }
}
