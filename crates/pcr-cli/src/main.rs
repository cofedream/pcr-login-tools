use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use pcr_auth::bili_sdk::{CaptchaAnswer, CaptchaChallenge, LoginOutcome, SdkClient};
use pcr_auth::error::EXIT_NEED_CAPTCHA;
use pcr_auth::game::Session;
use pcr_auth::{Channel, Error, Platform, Result, Token};
use pcr_web::Options;

#[derive(Debug, Parser)]
#[command(
    name = "pcr-auth",
    version,
    about = "公主连结（B服）登录工具：浏览器批量取令牌，或命令行换取 uid / access_key"
)]
struct Cli {
    /// 不带子命令时直接启动本地 Web UI
    #[command(subcommand)]
    command: Option<Command>,

    /// 以 JSON 输出，便于脚本消费
    #[arg(long, global = true)]
    json: bool,

    /// 把结果写入文件（Unix 下权限 0600）
    #[arg(long, global = true, value_name = "PATH")]
    save: Option<PathBuf>,

    /// 打印请求过程与服务端提示
    #[arg(long, global = true)]
    verbose: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ChannelArg {
    Bilibili,
    Qu,
}

impl From<ChannelArg> for Channel {
    fn from(value: ChannelArg) -> Self {
        match value {
            ChannelArg::Bilibili => Channel::Bilibili,
            ChannelArg::Qu => Channel::Qu,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PlatformArg {
    Android,
    Ios,
}

impl From<PlatformArg> for Platform {
    fn from(value: PlatformArg) -> Self {
        match value {
            PlatformArg::Android => Platform::Android,
            PlatformArg::Ios => Platform::Ios,
        }
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 启动本地 Web UI（默认行为），在浏览器里批量取令牌
    Serve {
        /// 监听端口；默认 0 = 每次由系统分配一个随机空闲端口
        #[arg(long, default_value_t = 0)]
        port: u16,

        /// 监听地址，默认只允许本机访问
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// 账号/令牌数据文件，默认当前目录的 pcr-accounts.json
        #[arg(long, value_name = "PATH")]
        data: Option<PathBuf>,

        /// 不自动打开浏览器
        #[arg(long)]
        no_open: bool,
    },

    /// 账号密码登录，换取 uid / access_key（可能触发极验）
    Login {
        /// B 站账号（手机号 / 邮箱）
        #[arg(short = 'u', long)]
        account: String,

        /// 密码；不传则交互读取
        #[arg(short, long)]
        password: Option<String>,

        #[arg(long, value_enum, default_value = "bilibili")]
        channel: ChannelArg,

        #[arg(long, value_enum, default_value = "android")]
        platform: PlatformArg,

        /// 同时拉取游戏昵称（多一次较大的 load/index 响应）
        #[arg(long)]
        name: bool,

        /// 已解出的极验 challenge
        #[arg(long)]
        captcha_challenge: Option<String>,

        /// 已解出的极验 gt_user_id
        #[arg(long)]
        captcha_gt_user_id: Option<String>,

        /// 已解出的极验 validate
        #[arg(long)]
        captcha_validate: Option<String>,
    },

    /// 用已有 uid / access_key 验证凭据并取 viewer_id
    Check {
        /// B 站 uid（或渠道服 login_id）
        #[arg(long)]
        uid: String,

        /// access_key（或渠道服 token）
        #[arg(long)]
        access_key: String,

        #[arg(long, value_enum, default_value = "bilibili")]
        channel: ChannelArg,

        #[arg(long, value_enum, default_value = "android")]
        platform: PlatformArg,

        /// 同时拉取游戏昵称
        #[arg(long)]
        name: bool,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    init_console();
    let cli = Cli::parse();
    let json = cli.json;

    match run(cli).await {
        Ok(code) => code,
        Err(error) => {
            if json {
                let payload = serde_json::json!({
                    "ok": false,
                    "error": error.to_string(),
                    "hint": hint_for(&error),
                });
                println!("{payload}");
            }
            eprintln!("✗ {error}");
            if let Some(hint) = hint_for(&error) {
                eprintln!("  → {hint}");
            }
            ExitCode::from(2)
        }
    }
}

/// Windows 控制台默认用 OEM 代码页（简体中文下是 936），直接输出 UTF-8 会乱码。
#[cfg(target_os = "windows")]
fn init_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn SetConsoleOutputCP(code_page: u32) -> i32;
        fn SetConsoleCP(code_page: u32) -> i32;
    }

    // 65001 = UTF-8
    unsafe {
        SetConsoleOutputCP(65001);
        SetConsoleCP(65001);
    }
}

#[cfg(not(target_os = "windows"))]
fn init_console() {}

async fn run(cli: Cli) -> Result<ExitCode> {
    let Cli {
        command,
        json,
        save,
        verbose,
    } = cli;

    match command.unwrap_or(Command::Serve {
        port: 0,
        host: "127.0.0.1".to_string(),
        data: None,
        no_open: false,
    }) {
        Command::Serve {
            port,
            host,
            data,
            no_open,
        } => {
            pcr_web::serve(Options {
                host,
                port,
                open_browser: !no_open,
                data_path: data.unwrap_or_else(|| PathBuf::from("pcr-accounts.json")),
            })
            .await
            .map_err(|error| Error::Other(format!("启动 Web 服务失败: {error}")))?;
            Ok(ExitCode::SUCCESS)
        }

        Command::Login {
            account,
            password,
            channel,
            platform,
            name,
            captcha_challenge,
            captcha_gt_user_id,
            captcha_validate,
        } => {
            let password = match password {
                Some(password) => password,
                None => read_password()?,
            };
            let answer = match (captcha_challenge, captcha_gt_user_id, captcha_validate) {
                (Some(challenge), Some(gt_user_id), Some(validate)) => Some(CaptchaAnswer {
                    challenge,
                    gt_user_id,
                    validate,
                }),
                (None, None, None) => None,
                _ => return Err(Error::Other(
                    "--captcha-challenge / --captcha-gt-user-id / --captcha-validate 需要同时提供"
                        .to_string(),
                )),
            };

            let sdk = SdkClient::new()?;
            let (uid, access_key) = match sdk.login(&account, &password, answer.as_ref()).await? {
                LoginOutcome::Authorized { uid, access_key } => (uid, access_key),
                LoginOutcome::NeedCaptcha => {
                    let challenge = sdk.start_captcha().await?;
                    print_captcha_help(&challenge);
                    return Ok(ExitCode::from(EXIT_NEED_CAPTCHA));
                }
            };

            finish(
                uid,
                access_key,
                channel.into(),
                platform.into(),
                name,
                json,
                save,
                verbose,
            )
            .await
        }

        Command::Check {
            uid,
            access_key,
            channel,
            platform,
            name,
        } => {
            finish(
                uid,
                access_key,
                channel.into(),
                platform.into(),
                name,
                json,
                save,
                verbose,
            )
            .await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn finish(
    uid: String,
    access_key: String,
    channel: Channel,
    platform: Platform,
    name: bool,
    json: bool,
    save: Option<PathBuf>,
    verbose: bool,
) -> Result<ExitCode> {
    let mut session = Session::new(uid.clone(), access_key.clone(), channel, platform, verbose)?;
    let info = match session.login(name).await {
        Ok(info) => info,
        Err(error) => {
            if verbose {
                eprintln!("· 网关: {}", session.apiroot());
                for note in &session.notes {
                    eprintln!("· {note}");
                }
            }
            return Err(error);
        }
    };

    let token = Token::new(uid, access_key, channel, platform, info.app_ver)
        .with_game(info.viewer_id, info.user_name.clone());

    if verbose {
        eprintln!("· 网关: {}", session.apiroot());
        for note in &session.notes {
            eprintln!("· {note}");
        }
    }

    if let Some(path) = save.as_deref() {
        write_token(&token, path)?;
        eprintln!("已写入 {}", path.display());
    }

    if json {
        println!("{}", token.to_json()?);
    } else {
        print_human(&token);
    }

    Ok(ExitCode::SUCCESS)
}

fn print_human(token: &Token) {
    println!("uid:         {}", token.uid);
    println!("access_key:  {}", token.access_key);
    if let Some(viewer_id) = &token.viewer_id {
        println!("viewer_id:   {viewer_id}");
    }
    if let Some(user_name) = &token.user_name {
        println!("user_name:   {user_name}");
    }
    println!("channel:     {} ({})", token.channel, token.platform);
    println!("app_ver:     {}", token.app_ver);

    if token.channel == Channel::Bilibili.as_str() && !token.uid_is_numeric() {
        println!();
        println!("⚠ uid 不是纯数字，kanna 的「B绑定账号」会拒绝；请确认用的是 B 站数字账号。");
    }

    println!();
    println!("发给 kanna_connection_redive_2 机器人（复制整行）：");
    println!("  {}", token.kanna_bind_command);
}

fn print_captcha_help(challenge: &CaptchaChallenge) {
    eprintln!("需要过一次极验验证码才能继续登录。");
    eprintln!();
    eprintln!("gt:          {}", challenge.gt);
    eprintln!("challenge:   {}", challenge.challenge);
    eprintln!("gt_user_id:  {}", challenge.gt_user_id);
    eprintln!();
    eprintln!("用能完成极验 v3 的页面（第三方或本地页面）解出 validate 后，重跑：");
    eprintln!();
    eprintln!("  pcr-auth login -u <账号> -p <密码> \\");
    eprintln!("    --captcha-challenge {} \\", challenge.challenge);
    eprintln!("    --captcha-gt-user-id {} \\", challenge.gt_user_id);
    eprintln!("    --captcha-validate <validate>");
    eprintln!();
    eprintln!("下一步会把「本地过码页面 + 回调」内置进来，届时无需第三方服务。");
}

fn read_password() -> Result<String> {
    eprint!("密码: ");
    let _ = io::stderr().flush();

    let mut buffer = String::new();
    io::stdin()
        .read_line(&mut buffer)
        .map_err(|error| Error::Other(format!("读取密码失败: {error}")))?;

    let password = buffer.trim_end_matches(&['\r', '\n'][..]).to_string();
    if password.is_empty() {
        return Err(Error::Other("密码不能为空".to_string()));
    }
    Ok(password)
}

fn write_token(token: &Token, path: &Path) -> Result<()> {
    let text = token.to_json()?;
    std::fs::write(path, text)
        .map_err(|error| Error::Other(format!("写入 {} 失败: {error}", path.display())))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn hint_for(error: &Error) -> Option<&'static str> {
    match error {
        Error::Password => Some("确认账号是 B 站数字账号或绑定手机号，密码为 B 站登录密码"),
        Error::AccessKeyExpired | Error::AccountBlocked => {
            Some("access_key 已失效或被服务端拒绝，请重新运行 `pcr-auth login` 获取")
        }
        Error::Tutorial => Some("该账号还没通过新手教程，游戏服务器拒绝登录"),
        Error::RiskControl(_) => Some("触发风控：换网络/稍后再试，或先在手机客户端正常登录一次"),
        Error::VersionOutdated(_) => {
            Some("客户端版本更新了，请更新 crates/pcr-auth/src/consts.rs 里的 DEFAULT_APP_VER")
        }
        Error::Maintenance(_) => Some("游戏在维护，维护结束后重试"),
        Error::NeedCaptcha { .. } => Some("用 --captcha-* 参数回填解出的验证码结果"),
        _ => None,
    }
}
