//! 本地 Web UI。
//!
//! 设计要点：
//!
//! - 只监听回环地址，默认随机端口（`--port 0`），账号只在本机流动
//! - 静态页面用 `include_str!` 编进二进制，发布物就是一个可执行文件
//! - 极验页面与后端同源，浏览器里滑一下就能把 `validate` 回调给工作线程
//! - 账号与令牌写进同一个 JSON 文件（默认 `./pcr-accounts.json`），可导出 CSV

pub mod store;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Path, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Semaphore};

use pcr_auth::bili_sdk::{CaptchaAnswer, LoginOutcome, SdkClient};
use pcr_auth::game::Session;
use pcr_auth::{Error, Result, Token};

use crate::store::{Account, AccountPatch, Store};

const INDEX_HTML: &str = include_str!("static/index.html");
const CAPTCHA_HTML: &str = include_str!("static/captcha.html");

/// 同时登录的账号数上限。太高容易被风控盯上。
const MAX_CONCURRENT: usize = 2;

/// 等待用户在浏览器里过验证码的时限。
const CAPTCHA_TIMEOUT: Duration = Duration::from_secs(600);

pub struct Options {
    pub host: String,
    /// 0 表示随机空闲端口
    pub port: u16,
    pub open_browser: bool,
    pub data_path: PathBuf,
}

pub async fn serve(options: Options) -> std::io::Result<()> {
    let store = Store::open(&options.data_path)?;
    let data_path = std::env::current_dir()
        .map(|dir| dir.join(&options.data_path))
        .unwrap_or_else(|_| options.data_path.clone());

    let listener = bind_with_fallback(&options.host, options.port).await?;
    let address: SocketAddr = listener.local_addr()?;
    let url = format!("http://{}:{}/", display_host(&options.host), address.port());

    println!("登录页:   {url}");
    println!("数据文件: {}", data_path.display());
    println!("注意:     账号密码以明文保存在上面的文件里（权限 0600），不要分享或同步到公共目录。");
    println!("按 Ctrl+C 退出。");

    if options.open_browser {
        open_browser(&url);
    }

    axum::serve(listener, router(Arc::new(AppState::new(store))))
        .await
        .map_err(|error| std::io::Error::other(error.to_string()))
}

/// 端口 0 交给系统挑；固定端口被占用时向后顺延。
async fn bind_with_fallback(host: &str, port: u16) -> std::io::Result<TcpListener> {
    if port == 0 {
        return TcpListener::bind((host, 0)).await;
    }

    let mut last_error = None;
    for offset in 0..20u16 {
        let Some(candidate) = port.checked_add(offset) else {
            break;
        };
        match TcpListener::bind((host, candidate)).await {
            Ok(listener) => return Ok(listener),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error
        .unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::AddrInUse, "没有可用端口")))
}

fn display_host(host: &str) -> &str {
    match host {
        "0.0.0.0" => "127.0.0.1",
        "::" | "[::]" => "[::1]",
        other => other,
    }
}

fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    let (program, args) = ("cmd", vec!["/C", "start", "", url]);
    #[cfg(target_os = "macos")]
    let (program, args) = ("open", vec![url]);
    #[cfg(all(unix, not(target_os = "macos")))]
    let (program, args) = ("xdg-open", vec![url]);

    let _ = std::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/accounts", get(list_accounts).post(create_account))
        .route(
            "/api/accounts/{id}",
            post(update_account).delete(remove_account),
        )
        .route("/api/export.csv", get(export_csv))
        .route("/api/jobs", get(list_jobs).post(create_jobs))
        .route("/captcha/{id}", get(captcha_page))
        .route("/api/captcha/{id}", post(submit_captcha))
        .with_state(state)
}

struct AppState {
    jobs: Mutex<Vec<Job>>,
    store: Mutex<Store>,
    semaphore: Semaphore,
    counter: AtomicU64,
    sdk: SdkClient,
}

impl AppState {
    fn new(store: Store) -> Self {
        Self {
            jobs: Mutex::new(Vec::new()),
            store: Mutex::new(store),
            semaphore: Semaphore::new(MAX_CONCURRENT),
            counter: AtomicU64::new(0),
            sdk: SdkClient::new().expect("构造 HTTP 客户端失败"),
        }
    }

    fn next_id(&self) -> String {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        let counter = self.counter.fetch_add(1, Ordering::Relaxed);
        format!("{millis:x}{counter:x}")
    }

    fn persist(&self) {
        let store = self.store.lock().expect("store 锁中毒");
        if let Err(error) = store.save() {
            eprintln!("保存 {} 失败: {error}", store.path().display());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Pending,
    Running,
    NeedCaptcha,
    Done,
    Failed,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Running => "running",
            Status::NeedCaptcha => "need_captcha",
            Status::Done => "done",
            Status::Failed => "failed",
        }
    }
}

struct Job {
    id: String,
    account_id: String,
    account: String,
    status: Status,
    message: Option<String>,
    viewer_id: Option<String>,
    user_name: Option<String>,
    bind_command: Option<String>,
    notes: Vec<String>,
    captcha: Option<pcr_auth::bili_sdk::CaptchaChallenge>,
    captcha_tx: Option<oneshot::Sender<CaptchaAnswer>>,
    started: Instant,
    elapsed_ms: Option<u128>,
}

#[derive(Serialize)]
struct JobView {
    id: String,
    account_id: String,
    account: String,
    status: &'static str,
    message: Option<String>,
    viewer_id: Option<String>,
    user_name: Option<String>,
    bind_command: Option<String>,
    captcha_url: Option<String>,
    notes: Vec<String>,
    elapsed_ms: Option<u64>,
}

impl Job {
    fn view(&self) -> JobView {
        JobView {
            id: self.id.clone(),
            account_id: self.account_id.clone(),
            account: self.account.clone(),
            status: self.status.as_str(),
            message: self.message.clone(),
            viewer_id: self.viewer_id.clone(),
            user_name: self.user_name.clone(),
            bind_command: self.bind_command.clone(),
            notes: self.notes.clone(),
            captcha_url: (self.status == Status::NeedCaptcha)
                .then(|| format!("/captcha/{}", self.id)),
            elapsed_ms: self.elapsed_ms.map(|value| value as u64),
        }
    }
}

// ---------------------------------------------------------------- 页面与账号

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn list_accounts(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let store = state.store.lock().expect("store 锁中毒");
    Json(json!({
        "accounts": store.accounts(),
        "data_path": store.path().display().to_string(),
    }))
}

async fn create_account(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(patch): Json<AccountPatch>,
) -> Response {
    if !same_origin(&headers) {
        return (StatusCode::FORBIDDEN, "跨站请求被拒绝").into_response();
    }

    let account = {
        let mut store = state.store.lock().expect("store 锁中毒");
        let account = store.create(patch);
        store.save().map(|_| account)
    };

    match account {
        Ok(account) => Json(account).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

async fn update_account(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(patch): Json<AccountPatch>,
) -> Response {
    if !same_origin(&headers) {
        return (StatusCode::FORBIDDEN, "跨站请求被拒绝").into_response();
    }

    let mut store = state.store.lock().expect("store 锁中毒");
    let Some(account) = store.update(&id, patch) else {
        return (StatusCode::NOT_FOUND, "账号不存在").into_response();
    };
    if let Err(error) = store.save() {
        return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
    }
    Json(account).into_response()
}

async fn remove_account(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !same_origin(&headers) {
        return (StatusCode::FORBIDDEN, "跨站请求被拒绝").into_response();
    }

    let mut store = state.store.lock().expect("store 锁中毒");
    if !store.remove(&id) {
        return (StatusCode::NOT_FOUND, "账号不存在").into_response();
    }
    if let Err(error) = store.save() {
        return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
    }
    Json(json!({ "ok": true })).into_response()
}

async fn export_csv(State(state): State<Arc<AppState>>) -> Response {
    let csv = state.store.lock().expect("store 锁中毒").to_csv();
    (
        [
            (CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                CONTENT_DISPOSITION,
                "attachment; filename=\"pcr-accounts.csv\"",
            ),
        ],
        csv,
    )
        .into_response()
}

// ---------------------------------------------------------------- 任务

#[derive(Deserialize)]
struct RunRequest {
    ids: Vec<String>,
    #[serde(default)]
    fetch_name: bool,
}

async fn create_jobs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<RunRequest>,
) -> Response {
    if !same_origin(&headers) {
        return (StatusCode::FORBIDDEN, "跨站请求被拒绝").into_response();
    }

    let mut started = Vec::new();
    for id in request.ids {
        let account = {
            let store = state.store.lock().expect("store 锁中毒");
            store.find(&id).cloned()
        };
        let Some(account) = account else {
            continue;
        };
        if !account.ready() {
            continue;
        }

        let job_id = state.next_id();
        let job = Job {
            id: job_id.clone(),
            account_id: account.id.clone(),
            account: account.account.clone(),
            status: Status::Pending,
            message: None,
            viewer_id: None,
            user_name: None,
            bind_command: None,
            notes: Vec::new(),
            captcha: None,
            captcha_tx: None,
            started: Instant::now(),
            elapsed_ms: None,
        };
        state.jobs.lock().expect("jobs 锁中毒").push(job);
        started.push(job_id.clone());

        let state = Arc::clone(&state);
        tokio::spawn(async move {
            run_job(state, job_id, account, request.fetch_name).await;
        });
    }

    Json(json!({ "started": started })).into_response()
}

async fn list_jobs(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let jobs = state.jobs.lock().expect("jobs 锁中毒");
    let views: Vec<JobView> = jobs.iter().map(Job::view).collect();
    Json(json!({ "jobs": views }))
}

/// 极验页面。把 gt / challenge 注入模板，浏览器里滑完回调 `validate`。
async fn captcha_page(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let jobs = state.jobs.lock().expect("jobs 锁中毒");
    let Some(job) = jobs.iter().find(|job| job.id == id) else {
        return (StatusCode::NOT_FOUND, "任务不存在").into_response();
    };
    let Some(challenge) = job.captcha.as_ref() else {
        return (StatusCode::CONFLICT, "该任务当前不需要验证码").into_response();
    };

    let page = CAPTCHA_HTML
        .replace("__JOB_ID__", &escape_js(&job.id))
        .replace("__ACCOUNT__", &escape_html(&job.account))
        .replace("__GT__", &escape_js(&challenge.gt))
        .replace("__CHALLENGE__", &escape_js(&challenge.challenge));

    Html(page).into_response()
}

#[derive(Deserialize)]
struct CaptchaSubmission {
    /// 极验回调里的 `geetest_challenge`（滑动流程服务端会换新的）
    #[serde(default)]
    challenge: Option<String>,
    /// 极验回调里的 `geetest_validate`
    validate: String,
}

async fn submit_captcha(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<CaptchaSubmission>,
) -> Response {
    if !same_origin(&headers) {
        return (StatusCode::FORBIDDEN, "跨站请求被拒绝").into_response();
    }

    let mut jobs = state.jobs.lock().expect("jobs 锁中毒");
    let Some(job) = jobs.iter_mut().find(|job| job.id == id) else {
        return (StatusCode::NOT_FOUND, "任务不存在").into_response();
    };
    let Some(challenge) = job.captcha.clone() else {
        return (StatusCode::CONFLICT, "该任务当前不需要验证码").into_response();
    };
    let Some(tx) = job.captcha_tx.take() else {
        return (StatusCode::CONFLICT, "验证码结果已被接收").into_response();
    };

    let answer = CaptchaAnswer {
        challenge: body.challenge.unwrap_or(challenge.challenge),
        gt_user_id: challenge.gt_user_id,
        validate: body.validate,
    };
    if tx.send(answer).is_err() {
        return (StatusCode::CONFLICT, "任务已结束").into_response();
    }

    job.status = Status::Running;
    job.message = Some("已收到验证码，继续登录…".to_string());
    Json(json!({ "ok": true })).into_response()
}

// ---------------------------------------------------------------- 任务执行

async fn run_job(state: Arc<AppState>, id: String, account: Account, fetch_name: bool) {
    let _permit = state.semaphore.acquire().await;
    if !update(&state, &id, |job| {
        job.status = Status::Running;
        job.message = Some("正在登录…".to_string());
    }) {
        return;
    }

    let mut notes = Vec::new();
    let result = execute(&state, &id, &account, fetch_name, &mut notes).await;

    if let Err(error) = &result {
        eprintln!("[{}] 登录失败: {error}", account.account);
        for note in &notes {
            eprintln!("  · {note}");
        }
    }

    update(&state, &id, |job| {
        job.elapsed_ms = Some(job.started.elapsed().as_millis());
        match &result {
            Ok(token) => {
                job.status = Status::Done;
                job.message = None;
                job.viewer_id = token.viewer_id.clone();
                job.user_name = token.user_name.clone();
                job.bind_command = Some(token.kanna_bind_command.clone());
            }
            Err(error) => {
                job.status = Status::Failed;
                job.message = Some(error.to_string());
                job.notes = notes;
            }
        }
        job.captcha = None;
        job.captcha_tx = None;
    });

    if let Ok(token) = &result {
        let applied = state
            .store
            .lock()
            .expect("store 锁中毒")
            .apply_token(&account.id, token);
        if applied {
            state.persist();
        }
    }
}

async fn execute(
    state: &Arc<AppState>,
    id: &str,
    account: &Account,
    fetch_name: bool,
    notes: &mut Vec<String>,
) -> Result<Token> {
    notes.push(format!(
        "登录方式: {} / {}",
        account.channel.as_str(),
        account.platform.as_str()
    ));

    let (uid, access_key) =
        credentials(state, id, &account.account, &account.password, notes).await?;
    notes.push(format!(
        "拿到 L1 凭据: uid={uid}, access_key 长度={}",
        access_key.len()
    ));

    let mut session = Session::new(
        uid.clone(),
        access_key.clone(),
        account.channel(),
        account.platform(),
        true,
    )?;

    // 游戏侧风控（is_risk）要求把验证码带进 tool/sdk_login，可能需要重试几轮。
    let mut captcha: Option<CaptchaAnswer> = None;
    let mut risk_rounds = 0u32;
    let info = loop {
        match session
            .login_with_captcha(captcha.as_ref(), fetch_name)
            .await
        {
            Ok(info) => break info,
            Err(Error::RiskControl(message)) if risk_rounds < 3 => {
                risk_rounds += 1;
                notes.push(format!(
                    "游戏侧风控（{message}），第 {risk_rounds} 次在游戏登录里补验证码"
                ));
                match ask_captcha(state, id, risk_rounds, notes).await {
                    Ok(answer) => captcha = Some(answer),
                    Err(error) => {
                        notes.append(&mut session.notes);
                        return Err(error);
                    }
                }
            }
            Err(error) => {
                notes.append(&mut session.notes);
                return Err(error);
            }
        }
    };
    notes.append(&mut session.notes);

    Ok(Token::new(
        uid,
        access_key,
        account.channel(),
        account.platform(),
        info.app_ver,
    )
    .with_game(info.viewer_id, info.user_name))
}

/// 账号密码换 `(uid, access_key)`；需要验证码时挂起等待浏览器回调。
async fn credentials(
    state: &Arc<AppState>,
    id: &str,
    account: &str,
    password: &str,
    notes: &mut Vec<String>,
) -> Result<(String, String)> {
    /// 最多让用户解几次验证码（validate 被拒会重新发一张）。
    const MAX_CAPTCHA_ATTEMPTS: u32 = 3;

    let mut answer: Option<CaptchaAnswer> = None;
    let mut attempts = 0u32;

    loop {
        match state.sdk.login(account, password, answer.as_ref()).await {
            Ok(LoginOutcome::Authorized { uid, access_key }) => {
                notes.push(format!("L1 登录成功（第 {} 次尝试）", attempts + 1));
                return Ok((uid, access_key));
            }
            Ok(LoginOutcome::NeedCaptcha) => {
                notes.push(format!("L1 第 {} 次尝试：服务端要求验证码", attempts + 1));
            }
            // validate 被服务端拒绝：换一张验证码再来，而不是直接判失败
            Err(Error::CaptchaFailed) if attempts < MAX_CAPTCHA_ATTEMPTS => {
                notes.push(format!(
                    "L1 第 {} 次尝试：服务端拒绝了这次验证码（200001），重新申请",
                    attempts + 1
                ));
                answer = None;
                continue;
            }
            Err(error) => {
                notes.push(format!("L1 第 {} 次尝试失败: {error}", attempts + 1));
                return Err(error);
            }
        }

        if attempts >= MAX_CAPTCHA_ATTEMPTS {
            return Err(Error::Other("验证码多次未通过，请稍后再试".to_string()));
        }
        attempts += 1;

        answer = Some(ask_captcha(state, id, attempts, notes).await?);
    }
}

/// 申请一次极验并把任务挂起，等浏览器回调。
async fn ask_captcha(
    state: &Arc<AppState>,
    id: &str,
    round: u32,
    notes: &mut Vec<String>,
) -> Result<CaptchaAnswer> {
    let challenge = state.sdk.start_captcha().await?;
    let original = challenge.challenge.clone();

    let (tx, rx) = oneshot::channel();
    if !update(state, id, |job| {
        job.status = Status::NeedCaptcha;
        job.message = Some(format!("请在浏览器里完成验证（第 {round} 次）"));
        job.captcha = Some(challenge);
        job.captcha_tx = Some(tx);
    }) {
        return Err(Error::Other("任务已被移除".to_string()));
    }

    let received = tokio::time::timeout(CAPTCHA_TIMEOUT, rx)
        .await
        .map_err(|_| Error::Other("等待验证码超时".to_string()))?
        .map_err(|_| Error::Other("验证码通道已关闭".to_string()))?;

    notes.push(format!(
        "收到浏览器回调：validate 长度={}，challenge {}",
        received.validate.len(),
        if received.challenge == original {
            "未变"
        } else {
            "已由极验更新"
        }
    ));

    update(state, id, |job| {
        job.status = Status::Running;
        job.captcha = None;
        job.message = Some("已收到验证码，继续登录…".to_string());
    });

    Ok(received)
}

fn update(state: &Arc<AppState>, id: &str, apply: impl FnOnce(&mut Job)) -> bool {
    let mut jobs = state.jobs.lock().expect("jobs 锁中毒");
    match jobs.iter_mut().find(|job| job.id == id) {
        Some(job) => {
            apply(job);
            true
        }
        None => false,
    }
}

/// 简单的同源校验：带 `Origin` 时必须与 `Host` 一致，挡掉跨站表单提交。
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get("origin") else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let host = headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    origin == format!("http://{host}") || origin == format!("https://{host}")
}

fn escape_js(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '0'..='9' | 'a'..='z' | 'A'..='Z' | '-' | '_' | '.' => out.push(ch),
            _ => out.push_str(&format!("\\u{{{:x}}}", ch as u32)),
        }
    }
    out
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn origin_check_rejects_cross_site() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("127.0.0.1:17870"));
        assert!(same_origin(&headers));

        headers.insert("origin", HeaderValue::from_static("https://evil.example"));
        assert!(!same_origin(&headers));

        headers.insert("origin", HeaderValue::from_static("http://127.0.0.1:17870"));
        assert!(same_origin(&headers));
    }

    #[test]
    fn escape_js_keeps_hex_ids_and_escapes_the_rest() {
        assert_eq!(escape_js("01af-2b"), "01af-2b");
        assert_eq!(escape_js("a'b"), "a\\u{27}b");
    }

    #[test]
    fn escape_html_stops_tag_injection() {
        assert_eq!(escape_html("<script>"), "&lt;script&gt;");
        assert_eq!(escape_html("a&b\"c"), "a&amp;b&quot;c");
    }

    #[test]
    fn ids_are_unique_and_hex() {
        let state = AppState::new(Store::open("/tmp/pcr-web-test-ids.json").expect("store"));
        let first = state.next_id();
        let second = state.next_id();
        assert_ne!(first, second);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn display_host_maps_wildcards() {
        assert_eq!(display_host("0.0.0.0"), "127.0.0.1");
        assert_eq!(display_host("127.0.0.1"), "127.0.0.1");
    }
}
