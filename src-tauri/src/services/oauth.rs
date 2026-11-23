//! 浏览器授权（OAuth2 + PKCE）模块
//!
//! 目前只实现 OpenAI 的 "Sign in with ChatGPT" 授权流程：
//! 1. 本地随机生成 `code_verifier` / `code_challenge`（S256）与 `state`；
//! 2. 在本机回环端口监听授权回调（OpenAI 客户端登记的回调地址固定为
//!    `http://localhost:1455/auth/callback`）；
//! 3. 打开系统浏览器让用户登录 ChatGPT 账号；
//! 4. 回调拿到 `code` 后用 `code_verifier` 换取 access_token / refresh_token；
//! 5. 解析 JWT 得到账号 ID（`chatgpt_account_id`）与过期时间，供代理转发时
//!    注入 `chatgpt-account-id` 头并自动续期。
//!
//! 说明：ChatGPT 订阅令牌只能在 ChatGPT 后端使用，不能直接打
//! `api.openai.com/v1`，因此代理侧会把浏览器授权秘钥的转发目标切到
//! `https://chatgpt.com/backend-api/codex`（见 `services/proxy.rs`）。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

use crate::error::{AppError, AppResult};

/// OpenAI 公共客户端 ID（Codex CLI 使用的同一客户端，PKCE 模式无需 client_secret）。
pub const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// OpenAI 授权端点。
pub const OPENAI_AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
/// OpenAI 令牌端点。
pub const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
/// OpenAI 登记的回环回调端口（不可更改，否则授权服务端会拒绝 redirect_uri）。
pub const OPENAI_REDIRECT_PORT: u16 = 1455;
/// OpenAI 登记的回环回调地址。
pub const OPENAI_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
/// ChatGPT 订阅令牌对应的后端地址（Responses API）。
pub const CHATGPT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
/// 授权作用域：openid/profile/email 用于拿身份，offline_access 用于拿 refresh_token。
const OPENAI_SCOPE: &str = "openid profile email offline_access";
/// 授权等待超时（秒）。
pub const AUTH_WAIT_TIMEOUT_SECS: u64 = 300;
/// 令牌提前续期阈值（秒）：距离过期不足该值即视为需要刷新。
pub const TOKEN_REFRESH_THRESHOLD_SECS: i64 = 120;

/// 浏览器授权得到的凭据。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserAuthToken {
    /// access_token（作为代理转发时的 Bearer 令牌）
    pub access_token: String,
    /// refresh_token（用于后续静默续期）
    pub refresh_token: Option<String>,
    /// id_token（含账号身份信息）
    pub id_token: Option<String>,
    /// access_token 过期时间（Unix 秒）
    pub expires_at: Option<i64>,
    /// ChatGPT 账号 ID，转发时需要写入 `chatgpt-account-id` 头
    pub account_id: Option<String>,
    /// 登录账号邮箱（仅用于界面展示）
    pub email: Option<String>,
}

/// 当前时间戳（Unix 秒）。
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ============================ PKCE ============================

/// base64url（无 padding）编码。
fn base64url_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// 生成随机字符串（base64url）。
pub fn random_urlsafe(len: usize) -> String {
    let mut buf = vec![0u8; len];
    rand::thread_rng().fill_bytes(&mut buf);
    base64url_encode(&buf)
}

/// 生成 PKCE `code_verifier`（43 字符，符合 RFC 7636 长度要求）。
pub fn generate_code_verifier() -> String {
    random_urlsafe(32)
}

/// 计算 PKCE `code_challenge`（S256）。
pub fn code_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64url_encode(&digest)
}

/// 拼装 OpenAI 授权页地址。
pub fn build_authorize_url(state: &str, challenge: &str) -> String {
    format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&id_token_add_organizations=true&state={}",
        OPENAI_AUTHORIZE_URL,
        OPENAI_CLIENT_ID,
        url_encode(OPENAI_REDIRECT_URI),
        url_encode(OPENAI_SCOPE),
        url_encode(challenge),
        url_encode(state),
    )
}

/// 百分号编码（application/x-www-form-urlencoded）。
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// 百分号解码。
fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

// ============================ 回调监听 ============================

/// 授权会话的取消标记（全局保存当前会话的 flag，供 `cancel_pending_auth` 使用）。
static AUTH_CANCEL: once_cell::sync::Lazy<std::sync::Mutex<Option<Arc<AtomicBool>>>> =
    once_cell::sync::Lazy::new(|| std::sync::Mutex::new(None));

/// 取消当前正在等待的授权会话。
pub fn cancel_pending_auth() {
    if let Ok(mut guard) = AUTH_CANCEL.lock() {
        if let Some(flag) = guard.as_ref() {
            flag.store(true, Ordering::SeqCst);
        }
        *guard = None;
    }
}

/// 授权会话监听器。
pub struct AuthCallbackListener {
    /// 实际监听端口
    pub port: u16,
    cancel: Arc<AtomicBool>,
}

impl Drop for AuthCallbackListener {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

/// 启动本机回环回调监听。
///
/// 返回监听器与接收端：接收端拿到 `Ok(code)` 表示授权成功，
/// `Err(msg)` 表示授权被拒绝 / 状态校验失败 / 超时。
pub fn spawn_callback_listener(
    expected_state: &str,
) -> AppResult<(
    AuthCallbackListener,
    oneshot::Receiver<Result<String, String>>,
)> {
    let listeners = bind_loopback(OPENAI_REDIRECT_PORT)?;

    for listener in &listeners {
        listener.set_nonblocking(true).map_err(|e| {
            AppError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                e.to_string(),
            ))
        })?;
    }

    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = oneshot::channel::<Result<String, String>>();

    // 记录当前会话的取消标记，供 cancel_pending_auth 中断等待
    if let Ok(mut guard) = AUTH_CANCEL.lock() {
        *guard = Some(cancel.clone());
    }

    let state = expected_state.to_string();
    let cancel_flag = cancel.clone();

    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(AUTH_WAIT_TIMEOUT_SECS);
        let mut result: Option<Result<String, String>> = None;

        'waiting: while Instant::now() < deadline {
            if cancel_flag.load(Ordering::SeqCst) {
                result = Some(Err("授权已取消".to_string()));
                break;
            }

            for listener in &listeners {
                match listener.accept() {
                    Ok((mut stream, _peer)) => {
                        let outcome = handle_callback_request(&mut stream, &state);
                        match outcome {
                            CallbackOutcome::Code(code) => {
                                result = Some(Ok(code));
                                break 'waiting;
                            }
                            CallbackOutcome::Error(msg) => {
                                result = Some(Err(msg));
                                break 'waiting;
                            }
                            // 浏览器可能先请求 /favicon.ico 之类，忽略并继续等待
                            CallbackOutcome::Ignore => continue,
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => {
                        result = Some(Err(format!("监听回调端口失败: {}", e)));
                        break 'waiting;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(120));
        }

        let final_result = match result {
            Some(r) => r,
            None => Err("等待浏览器授权超时".to_string()),
        };

        if let Ok(mut guard) = AUTH_CANCEL.lock() {
            if guard
                .as_ref()
                .is_some_and(|flag| Arc::ptr_eq(flag, &cancel_flag))
            {
                *guard = None;
            }
        }

        drop(listeners);
        let _ = tx.send(final_result);
    });

    Ok((
        AuthCallbackListener {
            port: OPENAI_REDIRECT_PORT,
            cancel,
        },
        rx,
    ))
}

/// 同时占用两个回环地址，避免 localhost 的另一地址将回调交给 OpenCode 等程序。
fn bind_loopback(port: u16) -> AppResult<Vec<TcpListener>> {
    let mut listeners = Vec::with_capacity(2);
    for addr in [
        SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        SocketAddr::from((Ipv6Addr::LOCALHOST, port)),
    ] {
        let listener = TcpListener::bind(addr).map_err(|e| AppError::Validation(format!(
            "无法监听授权回调地址 {}: {}。请关闭 OpenCode、Codex 等程序中正在进行的授权，释放端口 {} 后重试",
            addr, e, port
        )))?;
        listeners.push(listener);
    }
    Ok(listeners)
}

/// 回调请求的处理结果。
enum CallbackOutcome {
    /// 拿到授权码
    Code(String),
    /// 授权失败
    Error(String),
    /// 非回调请求，忽略
    Ignore,
}

/// 读取并解析一次 HTTP 回调请求。
fn handle_callback_request(stream: &mut TcpStream, expected_state: &str) -> CallbackOutcome {
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();

    let mut buf = [0u8; 8192];
    let n = match stream.read(&mut buf) {
        Ok(n) if n > 0 => n,
        _ => return CallbackOutcome::Ignore,
    };
    let request = String::from_utf8_lossy(&buf[..n]).to_string();

    let first_line = request.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let _method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");

    let (path, query) = match target.find('?') {
        Some(idx) => (&target[..idx], &target[idx + 1..]),
        None => (target, ""),
    };

    if path != "/auth/callback" {
        respond_page(stream, "忽略的请求", "该请求不是授权回调，已忽略。");
        return CallbackOutcome::Ignore;
    }

    let params = parse_query(query);

    if let Some(err) = params.get("error") {
        let desc = params
            .get("error_description")
            .cloned()
            .unwrap_or_else(|| err.clone());
        respond_page(stream, "授权失败", &desc);
        return CallbackOutcome::Error(format!("授权失败: {}", desc));
    }

    let state = params.get("state").cloned().unwrap_or_default();
    if state != expected_state {
        respond_page(stream, "校验失败", "state 校验失败，请重新发起授权。");
        return CallbackOutcome::Error("授权 state 校验失败".to_string());
    }

    match params.get("code") {
        Some(code) if !code.is_empty() => {
            respond_page(stream, "授权成功", "授权已完成，可以关闭此页面返回客户端。");
            CallbackOutcome::Code(code.clone())
        }
        _ => {
            respond_page(stream, "授权失败", "回调中缺少授权码。");
            CallbackOutcome::Error("回调中缺少授权码".to_string())
        }
    }
}

/// 解析 query string。
fn parse_query(query: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.find('=') {
            Some(idx) => (&pair[..idx], &pair[idx + 1..]),
            None => (pair, ""),
        };
        map.insert(url_decode(k), url_decode(v));
    }
    map
}

/// 回给浏览器一个简单的 HTML 页面。
fn respond_page(stream: &mut TcpStream, title: &str, message: &str) {
    let body = format!(
        "<!DOCTYPE html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><title>{}</title></head>\
         <body style=\"font-family:system-ui,-apple-system,'Segoe UI',sans-serif;padding:48px;text-align:center\">\
         <h2 style=\"margin-bottom:12px\">{}</h2><p style=\"color:#555\">{}</p></body></html>",
        title, title, message
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.as_bytes().len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

// ============================ 令牌交换 ============================

/// OpenAI 令牌端点返回体。
#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

/// 用授权码换取令牌（阻塞版，供代理线程与命令共用）。
pub fn exchange_authorization_code(code: &str, code_verifier: &str) -> AppResult<BrowserAuthToken> {
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("client_id", OPENAI_CLIENT_ID),
        ("code", code),
        ("redirect_uri", OPENAI_REDIRECT_URI),
        ("code_verifier", code_verifier),
    ];
    form.push(("scope", OPENAI_SCOPE));

    request_token(&form)
}

/// 用 refresh_token 续期（阻塞版）。
pub fn refresh_access_token(refresh_token: &str) -> AppResult<BrowserAuthToken> {
    let form = vec![
        ("grant_type", "refresh_token"),
        ("client_id", OPENAI_CLIENT_ID),
        ("refresh_token", refresh_token),
        ("scope", OPENAI_SCOPE),
    ];

    request_token(&form)
}

/// 请求令牌端点。
fn request_token(form: &[(&str, &str)]) -> AppResult<BrowserAuthToken> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| AppError::Validation(format!("构建 HTTP 客户端失败: {}", e)))?;

    let resp = client
        .post(OPENAI_TOKEN_URL)
        .form(form)
        .send()
        .map_err(|e| AppError::Validation(format!("请求 OpenAI 令牌端点失败: {}", e)))?;

    let status = resp.status();
    let body = resp
        .text()
        .map_err(|e| AppError::Validation(format!("读取令牌响应失败: {}", e)))?;

    let parsed: TokenResponse = serde_json::from_str(&body).map_err(|e| {
        AppError::Validation(format!(
            "解析令牌响应失败: {}（响应: {}）",
            e,
            truncate(&body, 300)
        ))
    })?;

    if let Some(err) = parsed.error {
        let desc = parsed.error_description.unwrap_or_default();
        return Err(AppError::Validation(format!(
            "OpenAI 令牌端点返回错误: {} {}",
            err, desc
        )));
    }

    if !status.is_success() {
        return Err(AppError::Validation(format!(
            "OpenAI 令牌端点返回 HTTP {}",
            status.as_u16()
        )));
    }

    if parsed.access_token.is_empty() {
        return Err(AppError::Validation(
            "OpenAI 令牌端点未返回 access_token".to_string(),
        ));
    }

    Ok(build_auth_token(parsed))
}

/// 把令牌端点响应组装成 `BrowserAuthToken`。
fn build_auth_token(parsed: TokenResponse) -> BrowserAuthToken {
    let claims = parse_jwt_claims(&parsed.access_token);
    let account_id = claims.as_ref().and_then(extract_account_id);
    let email = claims.as_ref().and_then(|c| {
        c.get("email")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    });

    let expires_at = parsed.expires_in.map(|secs| now_secs() + secs);

    BrowserAuthToken {
        access_token: parsed.access_token,
        refresh_token: parsed.refresh_token,
        id_token: parsed.id_token,
        expires_at,
        account_id,
        email,
    }
}

/// 截断长文本，用于错误提示。
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "..."
}

// ============================ JWT 解析 ============================

/// 解析 JWT 的 payload 段（不做签名校验——令牌由 OpenAI 签发，此处只读取声明）。
pub fn parse_jwt_claims(token: &str) -> Option<serde_json::Value> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.as_bytes())
        .ok()?;
    serde_json::from_slice(&raw).ok()
}

/// 从 JWT 声明中取 ChatGPT 账号 ID。
///
/// 优先取 `https://api.openai.com/auth` 里的 `chatgpt_account_id`，
/// 再退回顶层 `organization_id` / `chatgpt_account_id`。
pub fn extract_account_id(claims: &serde_json::Value) -> Option<String> {
    let auth = claims.get("https://api.openai.com/auth");
    auth.and_then(|a| a.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            claims
                .get("organization_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            claims
                .get("chatgpt_account_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .filter(|s| !s.is_empty())
}

/// 授权元数据（落库到 `providers_keys.auth_meta`）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthMeta {
    /// ChatGPT 账号 ID
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// access_token 过期时间（Unix 秒）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    /// 登录邮箱
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

impl AuthMeta {
    /// 序列化为 JSON。
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// 解析 JSON。
    pub fn from_json(s: &str) -> Option<AuthMeta> {
        serde_json::from_str(s).ok()
    }
}

/// 由令牌构造授权元数据。
pub fn build_auth_meta(token: &BrowserAuthToken) -> AuthMeta {
    AuthMeta {
        account_id: token.account_id.clone(),
        expires_at: token.expires_at,
        email: token.email.clone(),
    }
}

/// 解析后可直接用于转发的浏览器授权凭据。
#[derive(Debug, Clone)]
pub struct ResolvedBrowserCredential {
    /// 可用的 access_token
    pub access_token: String,
    /// ChatGPT 账号 ID（转发时写入 `chatgpt-account-id` 头）
    pub account_id: Option<String>,
    /// access_token 过期时间
    pub expires_at: Option<i64>,
}

/// 解析浏览器授权秘钥的可用凭据。
///
/// 距过期不足 `TOKEN_REFRESH_THRESHOLD_SECS` 时用 refresh_token 静默续期，
/// 并把新凭据回写数据库（加密存储），保证重启后仍可继续使用。
pub fn resolve_browser_credential(
    provider_key: &crate::repositories::ProviderKey,
) -> AppResult<ResolvedBrowserCredential> {
    let access_token = crate::crypto::decrypt(&provider_key.encrypted_api_key)?;
    let meta = provider_key
        .auth_meta
        .as_deref()
        .and_then(AuthMeta::from_json)
        .unwrap_or_default();

    let need_refresh = match meta.expires_at {
        Some(exp) => oauth_now() >= exp - TOKEN_REFRESH_THRESHOLD_SECS,
        None => false,
    };

    if !need_refresh {
        return Ok(ResolvedBrowserCredential {
            access_token,
            account_id: meta.account_id,
            expires_at: meta.expires_at,
        });
    }

    // 需要续期：取出 refresh_token
    let refresh_token = match provider_key.encrypted_refresh_token.as_deref() {
        Some(enc) if !enc.trim().is_empty() => crate::crypto::decrypt(enc)?,
        _ => {
            return Err(AppError::Validation(
                "浏览器授权已过期且缺少 refresh_token，请重新进行浏览器授权".to_string(),
            ))
        }
    };

    let token = refresh_access_token(&refresh_token)?;
    let new_meta = build_auth_meta(&token);

    // 回写数据库：access_token / refresh_token（可能轮换）/ 元数据
    crate::repositories::update_provider_key_credentials(
        &provider_key.id,
        &token.access_token,
        token
            .refresh_token
            .as_deref()
            .or(Some(refresh_token.as_str())),
        Some(&new_meta.to_json()),
    )?;

    tracing::info!(
        provider_key_id = %provider_key.id,
        "浏览器授权令牌已自动续期"
    );

    Ok(ResolvedBrowserCredential {
        access_token: token.access_token,
        account_id: new_meta.account_id.or(meta.account_id),
        expires_at: new_meta.expires_at,
    })
}

/// 当前时间戳（Unix 秒）。
fn oauth_now() -> i64 {
    now_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_verifier_and_challenge_are_stable() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            code_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn authorize_url_contains_expected_params() {
        let url = build_authorize_url("st-123", "chal");
        assert!(url.starts_with(OPENAI_AUTHORIZE_URL));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("client_id=app_EMoamEEZ73f0CkXaXp7hrann"));
        assert!(url.contains("state=st-123"));
    }

    #[test]
    fn query_parsing_decodes_values() {
        let params = parse_query("code=abc%2Fdef&state=xyz&error_description=a+b");
        assert_eq!(params.get("code").unwrap(), "abc/def");
        assert_eq!(params.get("state").unwrap(), "xyz");
        assert_eq!(params.get("error_description").unwrap(), "a b");
    }
}
