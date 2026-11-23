/**
 * 代理服务器模块
 * 
 * 该模块实现了一个本地HTTP代理服务器，用于将前端请求转发到目标API。
 * 核心功能包括：
 * 1. 监听本地端口接收HTTP请求
 * 2. 解析请求中的Authorization令牌
 * 3. 根据令牌匹配对应的 ProviderKey 配置
 * 4. 将请求转发到目标服务器（自动注入正确的API Key）
 * 5. 处理CORS跨域请求
 * 6. 支持gzip压缩响应的解压
 */
use std::io::{Read, Write as IoWrite};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use flate2::read::GzDecoder;
use futures_util::StreamExt;
use once_cell::sync::Lazy;
use tokio::runtime::Runtime;

use crate::error::{AppError, AppResult};
use crate::crypto;
use crate::repositories;
use crate::repositories::ProviderKey;
use crate::services::oauth;

/// 代理服务器超时时间（秒）
const PROXY_TIMEOUT_SECS: u64 = 30;

/// Anthropic 协议要求的默认 `max_tokens`（Anthropic API 必填且过小会拒）
const ANTHROPIC_DEFAULT_MAX_TOKENS: u64 = 4096;
/// Anthropic 协议版本头
const ANTHROPIC_VERSION_HEADER: &str = "2023-06-01";
/// Anthropic Messages API 路径
const ANTHROPIC_MESSAGES_PATH: &str = "/v1/messages";

/// 协议枚举：决定转发时使用哪种 API 协议。
/// 当前区分 OpenAI 兼容与 Anthropic 原生；其它值视为 OpenAI 兼容。
///
/// 注意：OpenAI 的 Codex CLI 使用 Responses API（路径 `/v1/responses`），
/// 但请求体格式与 Chat Completions 接近（同 Bearer 鉴权、同 `model`/`stream` 字段），
/// 因此复用 `OpenAi` 模式处理，仅在 `strip_remote_url_prefix` 中保留 `/responses`
/// 路径段，由 `provider.api_url` 中已有的 `/v1` 前缀拼出最终上游 URL
/// `<api_url>/v1/responses`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApiProtocol {
    Chat,
    Anthropic,
    Responses,
}

impl ApiProtocol {
    /// 解析 `proxy_key.api_type`（'chat' / 'anthropic' / 'responses'）；
    /// 历史数字值 1/2/3 兼容，其余一律视为 chat。
    fn from_str_lossy(api_type: &str) -> Self {
        match api_type.trim() {
            "anthropic" | "2" => ApiProtocol::Anthropic,
            "responses" | "3" => ApiProtocol::Responses,
            _ => ApiProtocol::Chat,
        }
    }
}

/// 把 `proxy_key.api_type` 翻译为可读日志标签：chat / anthropic / responses
fn api_type_label(api_type: &str) -> &'static str {
    match ApiProtocol::from_str_lossy(api_type) {
        ApiProtocol::Anthropic => "Anthropic",
        ApiProtocol::Responses => "Responses",
        ApiProtocol::Chat => "Chat",
    }
}

/// 选择转发目标 URL：
/// - Responses 模式：优先使用 provider.api_responses_url；为空时回退到 api_url；
/// - Anthropic 模式：优先使用 provider.api_anthropic_url；为空或未配置时回退到 api_url；
/// - chat 模式：使用 provider.api_url，为空时回退到官方地址。
///
/// 注意：api_responses_url **仅**在 api_type=responses 时生效；其它模式不再
/// 受该字段影响，避免配置了 Responses 端点后误把 Anthropic / OpenAI 请求劫持过去。
fn select_remote_url(provider: &repositories::Provider, api_type: &str) -> String {
    let protocol = ApiProtocol::from_str_lossy(api_type);

    if protocol == ApiProtocol::Responses {
        if let Some(url) = provider
            .api_responses_url
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            return url.to_string();
        }
        return fallback_to_api_url(provider);
    }

    if protocol == ApiProtocol::Anthropic {
        if let Some(url) = provider
            .api_anthropic_url
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            return url.to_string();
        }
    }

    fallback_to_api_url(provider)
}

/// 共用回退逻辑：取 `api_url`，为空时用 OpenAI 官方地址。
fn fallback_to_api_url(provider: &repositories::Provider) -> String {
    if provider.api_url.trim().is_empty() {
        "https://api.openai.com".to_string()
    } else {
        provider.api_url.trim().to_string()
    }
}

/// 把客户端发来的 OpenAI 风格请求体转换为 Anthropic Messages 请求体。
/// 字段映射：
///   - `model`     → 与 `inject_model_to_body` 对齐：`proxy_key.model` 非空时强制用它，
///                   否则才回退到请求体中的 `model`；保证 Anthropic 模式也尊重秘钥配置
///   - `messages`  → 拆分出首条 role=system 作为顶层 `system`，其余保留为 `messages`
///   - `max_tokens` → 必须存在；缺失或过小时填默认值
/// 其余字段（如 temperature / top_p / tools 等）按需透传；流式字段 `stream` 保留给上游识别。
///
/// 解析失败时返回原 body，由上游按非 JSON 处理。
fn convert_openai_body_to_anthropic(body: &str, default_model: &str) -> String {
    let mut root: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return body.to_string(),
    };
    let obj = match root.as_object_mut() {
        Some(m) => m,
        None => return body.to_string(),
    };

    // 1. model：与 inject_model_to_body 一致——配置了 default_model 就覆盖 body 里的 model
    let model = if !default_model.trim().is_empty() {
        default_model.trim().to_string()
    } else {
        obj.get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_default()
    };
    obj.insert("model".to_string(), serde_json::Value::String(model));

    // 2. messages → system + messages
    let messages = obj.remove("messages");
    let mut system_value: Option<serde_json::Value> = None;
    let mut anthropic_messages: Vec<serde_json::Value> = Vec::new();
    if let Some(serde_json::Value::Array(arr)) = messages {
        for msg in arr {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
            if role == "system" {
                // 仅保留首个 system；OpenAI 允许多条，Anthropic 顶层 system 是字符串
                if system_value.is_none() {
                    let content = msg.get("content").cloned().unwrap_or(serde_json::Value::Null);
                    system_value = Some(content);
                }
                continue;
            }
            // user / assistant 直接透传
            anthropic_messages.push(msg);
        }
    }
    if let Some(sys) = system_value {
        obj.insert("system".to_string(), sys);
    }
    if !anthropic_messages.is_empty() {
        obj.insert(
            "messages".to_string(),
            serde_json::Value::Array(anthropic_messages),
        );
    }

    // 3. max_tokens：缺失或过小(<=0)时填默认值
    let needs_default_max = match obj.get("max_tokens") {
        Some(serde_json::Value::Number(n)) => n.as_u64().map(|v| v == 0).unwrap_or(true),
        _ => true,
    };
    if needs_default_max {
        obj.insert(
            "max_tokens".to_string(),
            serde_json::Value::Number(serde_json::Number::from(ANTHROPIC_DEFAULT_MAX_TOKENS)),
        );
    }

    serde_json::to_string(&root).unwrap_or_else(|_| body.to_string())
}

/// 解析客户端请求路径，决定转发到上游的最终路径：
/// - Anthropic 模式：无视客户端具体 path，统一发到 `/v1/messages`；
///   若 `remote_url` 已包含 `/v1`（如 `https://api.minimaxi.com/v1` 或
///   `https://api.minimaxi.com/anthropic/v1`），则剥除 `/v1/messages` 中
///   的重叠前缀以避免 `/v1/v1/messages` 的双前缀；
/// - OpenAI / Responses 模式：透传客户端 path 并去掉与 base url 重复的版本段。
///
/// 客户端 path 中可能携带 query string（如 Anthropic SDK 的 `?beta=true`、
/// OpenAI Codex CLI 的 `?stream=true` 等），本函数始终透传：
/// - 先把 query 段从 `client_path` 剥离；
/// - path 段参与上述归一化；
/// - 最终结果 = 归一化后的 path + 原 query。
///
/// OpenAI Codex CLI 调用的 `/v1/responses`（Responses API）即走 OpenAI 模式：
/// 客户端 path = `/v1/responses`，provider.api_url 含 `/v1` 时，
/// `strip_remote_url_prefix` 会先剥掉重复的 `v1`，再透传 `/responses`，
/// 最终上游 URL 为 `<api_url>/v1/responses`。
fn resolve_target_path(remote_url: &str, client_path: &str, api_type: &str) -> String {
    // 剥离 query string（若有）：path 段走归一化，query 段在末尾原样拼回
    let (path_only, query) = match client_path.find('?') {
        Some(idx) => (&client_path[..idx], &client_path[idx..]),
        None => (client_path, ""),
    };

    // ChatGPT 后端（浏览器授权秘钥）只提供 Responses API，
    // 无论客户端请求的是 /v1/responses 还是 /v1/chat/completions，
    // 都统一收敛到 /responses，避免打到不存在的端点。
    if remote_url.starts_with(oauth::CHATGPT_CODEX_BASE_URL) {
        return format!("/responses{}", query);
    }

    let normalized = match ApiProtocol::from_str_lossy(api_type) {
        ApiProtocol::Anthropic => {
            strip_overlapping_prefix(remote_url, ANTHROPIC_MESSAGES_PATH)
        }
        _ => strip_remote_url_prefix(remote_url, path_only),
    };
    format!("{}{}", normalized, query)
}

/// 把 `target_path` 头部与 `remote_url` 路径段重叠的部分剥除，避免 Anthropic
/// 模式固定 `/v1/messages` 出现双前缀。与 `strip_remote_url_prefix` 不同，
/// 本函数只在 `remote_url` 路径里**确实包含某个版本号段**时，才剥除 target
/// 头部的版本号段——避免误伤 `https://api.anthropic.com` 这种不带任何版本号的
/// 官方地址（需要保留 `/v1/messages` 才能命中 Anthropic 官方 `/v1/messages`）。
///
/// 规则：
///   1. Step 1：剥除与 `remote_url` 路径段**完全重叠**的前缀
///      （如 remote=`/v1` + target=`/v1/messages` → `/messages`）；
///   2. Step 2：若 target 头部仍是版本号段（v1、v2 等）且 `remote_url` 路径里
///      出现过**任意**版本号段（不限同名），再剥掉一段，避免
///      `/api/paas/v4/v1/messages` 这类拼接。
///
/// 例子：
///   - remote_url=`https://api.minimaxi.com/v1`            + target=`/v1/messages`  → `/messages`
///   - remote_url=`https://api.minimaxi.com/anthropic/v1`  + target=`/v1/messages`  → `/messages`
///   - remote_url=`https://api.anthropic.com`              + target=`/v1/messages`  → `/v1/messages`
///   - remote_url=`https://example.com/api/paas/v4`        + target=`/v1/messages`  → `/messages`
fn strip_overlapping_prefix(remote_url: &str, target_path: &str) -> String {
    // 取出 remote_url 的 path 段（去掉 host 部分）："https://host/v1" → "/v1"
    let remote_path = match remote_url.find("://") {
        Some(idx) => &remote_url[idx + 3..],
        None => remote_url,
    };
    let remote_path = match remote_path.find('/') {
        Some(idx) => &remote_path[idx..],
        None => return target_path.to_string(),
    };
    let remote_path = remote_path.trim_end_matches('/');

    let target_trimmed = target_path.trim_start_matches('/');
    if target_trimmed.is_empty() {
        return "/".to_string();
    }

    let remote_segments: Vec<&str> = remote_path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let target_segments: Vec<&str> = target_trimmed
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    // remote 是否至少有一段是"协议版本号"（v1、v2、v1beta 等）
    let remote_has_version = remote_segments.iter().any(|s| is_version_segment(s));

    // Step 1: 剥除与 remote_url 路径段完全重叠的前缀（如 remote=`/v1` + target=`/v1/messages` → `/messages`）
    let mut result_segments: Vec<&str> = target_segments.clone();
    if !remote_segments.is_empty()
        && remote_segments.len() < result_segments.len()
        && result_segments[..remote_segments.len()] == remote_segments[..]
    {
        result_segments = result_segments[remote_segments.len()..].to_vec();
    }

    // Step 2: 若 target 头部仍是版本号段且 remote_url 路径里出现过任意版本号段，
    // 继续剥除。覆盖诸如 `/api/paas/v4/v1/messages` → `/api/paas/v4/messages` 的场景。
    // 仅在 remote 路径里**完全不含**版本号时才保留 target 自带的版本号，
    // 以保护 `https://api.anthropic.com` + `/v1/messages` 这种官方地址。
    if let Some(&head) = result_segments.first() {
        if is_version_segment(head) && remote_has_version {
            result_segments.remove(0);
        }
    }

    if result_segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", result_segments.join("/"))
    }
}

/// 代理服务器全局状态（懒加载单例）
static PROXY_SERVER: Lazy<Mutex<Option<ProxyServerState>>> = Lazy::new(|| Mutex::new(None));

/// 代理服务器状态结构体
/// 用于管理服务器的启动/停止状态
pub struct ProxyServerState {
    shutdown_flag: Arc<Mutex<bool>>,  // 关闭标志（原子操作）
}

impl ProxyServerState {
    /// 创建新的服务器状态
    pub fn new() -> Self {
        Self {
            shutdown_flag: Arc::new(Mutex::new(false)),
        }
    }

    /// 设置关闭标志
    pub fn shutdown(&self) {
        let mut flag = self.shutdown_flag.lock().unwrap();
        *flag = true;
    }

    /// 检查是否需要关闭
    pub fn is_shutdown(&self) -> bool {
        *self.shutdown_flag.lock().unwrap()
    }
}

/// 代理调试日志：默认仅写 debug 级别（不出现在 info 级 stdout / 日志文件）。
/// 通过 `RUST_LOG=ciyuan_proxy=debug` 可重新打开。
/// 不再 emit 到前端"日志"页，避免默认值下还是被前端读到。
fn log_proxy_message(message: &str) {
    tracing::debug!("{}", message);
}

/// 代理状态摘要：默认 info 级，输出到文件 / stdout 并推给前端。
/// 用于仅保留"每个请求的关键里程碑"——如匹配上的 provider、转发开始/结束等。
fn log_proxy_summary(message: &str) {
    tracing::info!("{}", message);
    crate::emit_proxy_log(message);
}

/// 判断错误是否属于"对端提前关闭 / 临时忙 / 被中断"这一类常见非致命情形，
/// 用于避免一条 `Error handling connection: IO error ... os error 10035`
/// 把单请求场景下的偶发情况当成故障推到前端日志页。
fn is_benign_io_error(err: &AppError) -> bool {
    match err {
        AppError::Io(io) => matches!(
            io.kind(),
            std::io::ErrorKind::WouldBlock
                | std::io::ErrorKind::Interrupted
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::UnexpectedEof
        ),
        _ => false,
    }
}

/// 把 `Write::write_all` 在 `WouldBlock` 时的行为包装为短睡重试，
/// 并最终把其它错误原样上抛。
/// 仅在需要把"non-blocking 流式写出"走阻塞语义时使用。
fn write_all_tolerant(stream: &mut TcpStream, buf: &[u8]) -> AppResult<()> {
    let mut offset = 0;
    let mut retries = 0;
    while offset < buf.len() {
        match stream.write(&buf[offset..]) {
            Ok(0) => {
                return Err(AppError::Io(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "write returned 0",
                )));
            }
            Ok(n) => {
                offset += n;
                retries = 0;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                retries += 1;
                if retries > 5_000 {
                    return Err(AppError::Io(e));
                }
                thread::sleep(Duration::from_millis(2));
            }
            Err(e) => return Err(AppError::Io(e)),
        }
    }
    Ok(())
}

/// 获取代理服务器监听端口
pub fn get_listen_port() -> AppResult<u16> {
    let config = repositories::get_config()?;
    Ok(config.listen_port as u16)
}

/// 启动本地代理服务器
/// 
/// 绑定到配置的监听端口，开始接受客户端连接。
/// 每个连接会在独立的线程中处理，支持并发请求。
pub fn start_proxy_server() -> AppResult<()> {
    let port = get_listen_port()?;
    let addr = format!("0.0.0.0:{}", port);
    log_proxy_message(&format!("Starting proxy server on {}", addr));

    // 绑定监听端口
    let listener = TcpListener::bind(&addr).map_err(|e| {
        AppError::Io(std::io::Error::new(std::io::ErrorKind::AddrInUse, e.to_string()))
    })?;

    // 设置为非阻塞模式，以便定期检查关闭标志
    listener.set_nonblocking(true).map_err(|e| {
        AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    })?;

    // 保存服务器状态（用于后续关闭）
    {
        let mut server = PROXY_SERVER.lock().unwrap();
        *server = Some(ProxyServerState::new());
    }

    log_proxy_summary(&format!("Proxy server listening on {}", addr));

    // 主循环：接受连接
    loop {
        // 检查关闭标志
        {
            let server = PROXY_SERVER.lock().unwrap();
            if let Some(ref state) = *server {
                if state.is_shutdown() {
                    log_proxy_message("Proxy server shutting down");
                    break;
                }
            }
        }

        // 接受客户端连接
        match listener.accept() {
            Ok((stream, client_addr)) => {
                log_proxy_message(&format!("Accepted connection from {}", client_addr));
                // 每个连接在独立线程中处理
                thread::spawn(move || {
                    if let Err(e) = handle_connection(stream) {
                        if is_benign_io_error(&e) {
                            // 对端提前关闭 / 临时忙等常见场景，仅 debug! 一行，不再推前端
                            tracing::debug!(error = %e, "connection closed/benign io error");
                        } else {
                            log_proxy_message(&format!("Error handling connection: {}", e));
                        }
                    }
                });
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // 无连接等待，短暂休眠
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                log_proxy_message(&format!("Accept error: {}", e));
                thread::sleep(Duration::from_millis(100));
            }
        }
    }

    // 清除服务器状态（标记为已停止）
    {
        let mut server = PROXY_SERVER.lock().unwrap();
        *server = None;
    }

    Ok(())
}

/// 停止代理服务器
pub fn stop_proxy_server() {
    let server = PROXY_SERVER.lock().unwrap();
    if let Some(ref state) = *server {
        state.shutdown();
    }
}

/// 获取代理服务器运行状态
pub fn is_proxy_running() -> bool {
    let server = PROXY_SERVER.lock().unwrap();
    server.is_some()
}

/// 处理单个客户端连接
///
/// 主要流程：
/// 1. 设置超时时间
/// 2. 读取并解析HTTP请求
/// 3. 处理CORS预检请求
/// 4. 根据Authorization令牌匹配 ProviderKey
/// 5. 转发请求到目标服务器
/// 6. 返回响应给客户端
fn handle_connection(mut stream: TcpStream) -> AppResult<()> {
    // Windows 上 listener 通过 set_nonblocking(true) 启动 accept 主循环，
    // accept() 返回的子 stream 会继承 non-blocking 状态，导致后续阻塞
    // 的 read/write 偶发触发 WSAEWOULDBLOCK (os error 10035)。
    // 这里强制恢复阻塞模式并设置读写超时，主循环的 non-blocking 仅作用于 listener。
    stream.set_nonblocking(false).ok();
    // 设置读写超时
    stream.set_read_timeout(Some(Duration::from_secs(PROXY_TIMEOUT_SECS)))?;
    stream.set_write_timeout(Some(Duration::from_secs(PROXY_TIMEOUT_SECS)))?;

    log_proxy_message("\n═══════════════════════════════════════════════════════════════");
    log_proxy_message("New connection received");

    // 读取HTTP请求数据
    let mut request_data = Vec::new();
    let mut buffer = [0u8; 4096];
    
    // 第一次读取
    let mut bytes_read = stream.read(&mut buffer).map_err(|e| {
        AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    })?;
    
    if bytes_read == 0 {
        log_proxy_message("Empty request, closing connection");
        log_proxy_message("═══════════════════════════════════════════════════════════════");
        return Ok(());
    }
    
    request_data.extend_from_slice(&buffer[..bytes_read]);
    
    // 根据Content-Length判断是否需要继续读取
    let request_str = String::from_utf8_lossy(&request_data);
    if let Some(content_length) = extract_content_length(&request_str) {
        let total_needed = content_length as usize + request_str.find("\r\n\r\n").unwrap_or(0) + 4;
        while request_data.len() < total_needed {
            bytes_read = stream.read(&mut buffer).map_err(|e| {
                AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
            })?;
            if bytes_read == 0 {
                break;
            }
            request_data.extend_from_slice(&buffer[..bytes_read]);
        }
    }
    
    let request_str = String::from_utf8_lossy(&request_data);
    
    log_proxy_message(&format!("Total bytes read: {}", request_data.len()));

    // 解析HTTP请求
    let (method, path, headers, body) = parse_http_request(&request_str)?;

    // 打印请求摘要
    log_proxy_message("╔══════════════════════════════════════════════════════════╗");
    log_proxy_message("║ Incoming Request                                        ║");
    log_proxy_message("╠══════════════════════════════════════════════════════════╣");
    log_proxy_message(&format!("║ Method: {}                                               ", method));
    log_proxy_message(&format!("║ Path: {}                                                 ", path));
    log_proxy_message(&format!("║ Headers count: {}                                        ", headers.len()));
    if let Some(b) = &body {
        log_proxy_message(&format!("║ Body size: {} bytes                                      ", b.len()));
    } else {
        log_proxy_message("║ Body: (empty)                                           ");
    }
    log_proxy_message("╚══════════════════════════════════════════════════════════╝");

    // 处理CORS预检请求（OPTIONS方法）
    if method == "OPTIONS" {
        log_proxy_message("\nHandling CORS preflight request");
        let cors_response = build_cors_response(&headers);
        stream.write_all(cors_response.as_bytes()).map_err(|e| {
            AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
        })?;
        log_proxy_message("CORS response sent");
        log_proxy_message("═══════════════════════════════════════════════════════════════");
        return Ok(());
    }

    // 打印所有请求头（Authorization会脱敏显示）
    log_proxy_message("Headers:");
    for (name, value) in &headers {
        let display_value = if name.to_lowercase() == "authorization" {
            mask_token(value)
        } else {
            value.clone()
        };
        log_proxy_message(&format!("  {}: {}", name, display_value));
    }

    // 从Authorization头提取代理 key
    let auth_key = extract_token_from_headers(&headers);

    // 根据代理 key 匹配 ProviderKey（原 TokenGroup）
    let matched_group = match auth_key {
        Some(key) => {
            // 仅打印脱敏后的代理 key，避免明文泄露到日志文件 / 前端日志页。
            log_proxy_message(&format!("\nProxy key found: {}", mask_proxy_key(&key)));
            log_proxy_message("Looking up provider key by proxy key...");
            match repositories::get_proxy_key_with_provider_key_by_key(&key) {
                Ok(Some(proxy_with_pk)) => {
                    let provider_key = proxy_with_pk.provider_key;
                    log_proxy_message(&format!("✓ Found provider key: {}", provider_key.name));

                    // 查询关联的 Provider，从中取 api_url、api_anthropic_url 与 models（逗号分隔）
                    match repositories::get_provider_by_id(&provider_key.provider_id) {
                        Ok(Some(provider)) => {
                            let configured_api_type = proxy_with_pk.proxy_key.api_type.clone();
                            let mut remote_url = select_remote_url(&provider, &configured_api_type);
                            let mut api_key = crypto::decrypt(&provider_key.encrypted_api_key)?;
                            let mut api_type = configured_api_type;
                            let mut extra_headers: Vec<(String, String)> = Vec::new();

                            // 浏览器授权（ChatGPT 订阅令牌）：令牌只在 ChatGPT 后端可用，
                            // 转发目标切到 backend-api/codex，并注入账号 ID 头。
                            if provider_key.auth_type == repositories::AUTH_TYPE_BROWSER {
                                let credential = match oauth::resolve_browser_credential(&provider_key) {
                                    Ok(c) => c,
                                    Err(e) => {
                                        log_proxy_summary(&format!(
                                            "✗ 浏览器授权凭据不可用: {}",
                                            e
                                        ));
                                        let response = format!(
                                            "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{}",
                                            e
                                        );
                                        let _ = stream.write_all(response.as_bytes());
                                        return Ok(());
                                    }
                                };
                                api_key = credential.access_token;
                                remote_url = oauth::CHATGPT_CODEX_BASE_URL.to_string();
                                // ChatGPT 后端只提供 Responses API
                                api_type = "responses".to_string();
                                if let Some(account_id) = credential.account_id {
                                    extra_headers.push((
                                        "chatgpt-account-id".to_string(),
                                        account_id,
                                    ));
                                }
                                extra_headers.push((
                                    "originator".to_string(),
                                    "codex_cli_rs".to_string(),
                                ));
                            }

                            // 优先使用 proxy_key 显式配置的模型，否则回退到 provider.models 首个
                            let model = proxy_with_pk
                                .proxy_key
                                .model
                                .clone()
                                .filter(|m| !m.trim().is_empty())
                                .or_else(|| {
                                    provider
                                        .models
                                        .split(',')
                                        .map(|s| s.trim())
                                        .find(|s| !s.is_empty())
                                        .map(|s| s.to_string())
                                })
                                .unwrap_or_default();
                            log_proxy_message(&format!(
                                "✓ Found provider: {} -> {} (api_type={})",
                                provider.name, remote_url, api_type
                            ));
                            Some(ProviderKeyWithDecryptedKey {
                                name: provider_key.name,
                                remote_url,
                                api_key,
                                model,
                                route_path: path.clone(),
                                api_type,
                                extra_headers,
                            })
                        }
                        Ok(None) => {
                            log_proxy_message(&format!("✗ Provider not found for provider_id: {}", provider_key.provider_id));
                            None
                        }
                        Err(e) => {
                            log_proxy_message(&format!("✗ Error looking up provider: {}", e));
                            None
                        }
                    }
                }
                Ok(None) => {
                    log_proxy_message("✗ No matching proxy key found");
                    None
                }
                Err(e) => {
                    log_proxy_message(&format!("✗ Error looking up proxy key: {}", e));
                    None
                }
            }
        }
        None => {
            // 如果没有代理 key，尝试基于路由匹配
            log_proxy_message("\nNo authorization proxy key found in headers");
            log_proxy_message("Falling back to route-based matching...");
            let active_keys = repositories::get_active_provider_keys()?;
            log_proxy_message(&format!("Active provider keys count: {}", active_keys.len()));
            find_matching_provider_key(&active_keys, &path)?
        }
    };

    // 如果没有匹配的 ProviderKey，返回502错误
    if matched_group.is_none() {
        log_proxy_message("✗ No matching provider key found");
        let response = "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\n\r\nNo active provider key matching this request";
        stream.write_all(response.as_bytes()).map_err(|e| {
            AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
        })?;
        log_proxy_message("Sent 502 response");
        log_proxy_message("═══════════════════════════════════════════════════════════════");
        return Ok(());
    }

    // 获取匹配的 ProviderKey 配置
    let group_with_key = matched_group.unwrap();
    log_proxy_summary(&format!("\n✓ Matched provider key: {}", group_with_key.name));
    log_proxy_summary(&format!("  Remote URL: {}", group_with_key.remote_url));
    log_proxy_summary(&format!("  Model: {}", group_with_key.model));
    log_proxy_summary(&format!(
        "  API type: {}",
        api_type_label(&group_with_key.api_type)
    ));

    let remote_url = &group_with_key.remote_url;
    let api_key = &group_with_key.api_key;
    let model = &group_with_key.model;
    let api_type = &group_with_key.api_type;
    let extra_headers = &group_with_key.extra_headers;

    // 转发请求到目标服务器
    log_proxy_summary("\n→ Forwarding request to remote server...");

    // 根据请求类型选择流式或非流式转发
    if is_streaming_request(&body) {
        forward_streaming_request(
            &method,
            &path,
            &headers,
            &body,
            remote_url,
            api_key,
            model,
            api_type,
            extra_headers,
            &mut stream,
        )?;
    } else {
        let forwarded_response = forward_request(
            &method,
            &path,
            &headers,
            &body,
            remote_url,
            api_key,
            model,
            api_type,
            extra_headers,
        )?;

        // 将响应返回给客户端
        log_proxy_message("\n← Response received from remote server");
        log_proxy_message(&format!("Response size: {} bytes", forwarded_response.len()));

        stream.write_all(forwarded_response.as_bytes()).map_err(|e| {
            AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
        })?;
        
        stream.flush().map_err(|e| {
            AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("Flush error: {}", e)))
        })?;

        log_proxy_message("→ Response sent to client");
    }
    
    log_proxy_message("═══════════════════════════════════════════════════════════════");

    Ok(())
}

/// 从请求头中提取Bearer令牌
/// 
/// 从Authorization头中提取Bearer令牌（格式：Bearer <token>）
fn extract_token_from_headers(headers: &[(String, String)]) -> Option<String> {
    for (name, value) in headers {
        if name.to_lowercase() == "authorization" {
            let value_lower = value.to_lowercase();
            if value_lower.starts_with("bearer ") {
                return Some(value[7..].trim().to_string());
            }
        }
    }
    None
}

/// 令牌脱敏处理
///
/// 将Authorization令牌中间部分用*号替换，仅保留前后各4个字符
/// 默认使用4个*号
fn mask_token(value: &str) -> String {
    if value.to_lowercase().starts_with("bearer ") {
        let token = value[7..].trim();
        let len = token.len();
        if len <= 8 {
            // 令牌太短，不脱敏
            format!("Bearer {}", token)
        } else {
            let prefix = &token[..4];
            let suffix = &token[len - 4..];
            let stars = "****";
            format!("Bearer {}{}{}", prefix, stars, suffix)
        }
    } else {
        value.to_string()
    }
}

/// 代理 key 脱敏处理，与 mask_token 保持一致风格：
/// 仅保留前 4 后 4，中间用 `****` 替代；过短时整体隐藏。
fn mask_proxy_key(key: &str) -> String {
    if key.len() <= 8 {
        return "****".to_string();
    }
    // 代理 key 仅由 ASCII 字符构成，按字节切 4 / 后 4 是安全的。
    let prefix = &key[..4];
    let suffix = &key[key.len() - 4..];
    format!("{}{}{}", prefix, "****", suffix)
}

/// 从HTTP请求中提取Content-Length
fn extract_content_length(request_str: &str) -> Option<u64> {
    for line in request_str.lines() {
        let line_lower = line.to_lowercase();
        if line_lower.starts_with("content-length:") {
            let value = line.split(':').nth(1)?.trim();
            return value.parse().ok();
        }
    }
    None
}

/// 解析HTTP请求
/// 
/// 从原始HTTP请求字符串中解析出：方法、路径、请求头和请求体
fn parse_http_request(request_str: &str) -> AppResult<(String, String, Vec<(String, String)>, Option<String>)> {
    // 查找头部和体之间的分隔符
    let header_body_separator = "\r\n\r\n";
    let (header_part, body_part) = if let Some(idx) = request_str.find(header_body_separator) {
        let split_at = idx + header_body_separator.len();
        (&request_str[..idx], Some(&request_str[split_at..]))
    } else {
        (request_str, None)
    };
    
    let mut lines = header_part.lines();
    
    // 解析请求行：GET /path HTTP/1.1
    let request_line = lines.next().ok_or_else(|| {
        AppError::Validation("Invalid HTTP request: missing request line".to_string())
    })?;
    
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 3 {
        return Err(AppError::Validation("Invalid HTTP request line".to_string()));
    }
    
    let method = parts[0].to_string();
    let path = parts[1].to_string();
    
    // 解析请求头
    let mut headers = Vec::new();
    
    for line in lines {
        if line.is_empty() {
            continue;
        }
        
        if let Some(colon_pos) = line.find(':') {
            let name = line[..colon_pos].trim().to_string();
            let value = line[colon_pos + 1..].trim().to_string();
            headers.push((name, value));
        }
    }
    
    // 提取请求体（如果存在）
    let body = body_part.and_then(|body_str| {
        if body_str.is_empty() { None } else { Some(body_str.to_string()) }
    });
    
    Ok((method, path, headers, body))
}

/// 基于路由路径匹配 ProviderKey
///
/// 当无法通过令牌匹配时，尝试基于请求路径匹配活跃的 ProviderKey。
/// 模型取自关联 Provider 的 models 字段（逗号分隔，取第一个非空项）。
/// 回退匹配没有代理秘钥上下文，固定按 OpenAI 兼容协议转发。
/// 同时识别 OpenAI Codex CLI 的 `/v1/responses` 入口。
fn find_matching_provider_key(keys: &[ProviderKey], path: &str) -> AppResult<Option<ProviderKeyWithDecryptedKey>> {
    // Codex / Responses API 与 Chat Completions 视为同一类入口（OpenAI 兼容）。
    const OPENAI_KNOWN_PATHS: &[&str] = &[
        "/v1/chat/completions",
        "/v1/responses",
    ];
    for key in keys {
        if let Ok(Some(provider)) = repositories::get_provider_by_id(&key.provider_id) {
            let api_url = provider.api_url.clone();
            let matches_api_url = !api_url.is_empty() && path.starts_with(&api_url);
            let matches_known = OPENAI_KNOWN_PATHS.iter().any(|p| path.starts_with(p));
            if matches_api_url || matches_known {
                let mut api_key = crypto::decrypt(&key.encrypted_api_key)?;
                let mut remote_url = if api_url.is_empty() {
                    "https://api.openai.com".to_string()
                } else {
                    api_url
                };
                let mut api_type = "chat".to_string();
                let mut extra_headers: Vec<(String, String)> = Vec::new();

                // 浏览器授权（ChatGPT 订阅令牌）：转发目标切到 ChatGPT 后端
                if key.auth_type == repositories::AUTH_TYPE_BROWSER {
                    let credential = oauth::resolve_browser_credential(key)?;
                    api_key = credential.access_token;
                    remote_url = oauth::CHATGPT_CODEX_BASE_URL.to_string();
                    api_type = "responses".to_string();
                    if let Some(account_id) = credential.account_id {
                        extra_headers.push(("chatgpt-account-id".to_string(), account_id));
                    }
                    extra_headers.push(("originator".to_string(), "codex_cli_rs".to_string()));
                }

                let model = provider
                    .models
                    .split(',')
                    .map(|s| s.trim())
                    .find(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                // route_path 字段目前仅用于日志，回退场景下仍记录 chat/completions
                // 作为"OpenAI 兼容"的代表路径。
                return Ok(Some(ProviderKeyWithDecryptedKey {
                    name: key.name.clone(),
                    remote_url,
                    api_key,
                    model,
                    route_path: "/v1/chat/completions".to_string(),
                    api_type,
                    extra_headers,
                }));
            }
        }
    }
    Ok(None)
}

/// ProviderKey 配置（包含解密后的 API Key）
///
/// 用于存储匹配到的 ProviderKey 配置，包含解密后的 API Key
#[derive(Debug, Clone)]
pub struct ProviderKeyWithDecryptedKey {
    pub name: String,         // ProviderKey 名称
    pub remote_url: String,   // 目标 API 地址
    pub api_key: String,      // 解密后的 API Key
    pub model: String,        // 目标模型名称
    #[allow(dead_code)]
    pub route_path: String,   // 路由路径
    /// 转发使用的协议："chat"=OpenAI 兼容，"anthropic"=Anthropic，"responses"=Responses
    pub api_type: String,
    /// 额外注入的请求头（如 ChatGPT 后端需要的 chatgpt-account-id）
    pub extra_headers: Vec<(String, String)>,
}

/// 读取并处理HTTP响应体
/// 
/// 支持gzip压缩响应的解压，确保响应体正确解码为UTF-8字符串
fn read_response_body(mut response: reqwest::blocking::Response) -> AppResult<String> {
    // 检查Content-Encoding头，判断是否需要解压
    let content_encoding = response.headers()
        .get("content-encoding")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    
    // 读取响应字节
    let bytes = response.bytes().map_err(|e| {
        AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("Read response bytes error: {}", e)))
    })?;
    
    // 如果是gzip压缩，进行解压
    if content_encoding == "gzip" {
        let mut decoder = GzDecoder::new(&bytes[..]);
        let mut decoded = String::new();
        decoder.read_to_string(&mut decoded).map_err(|e| {
            AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("Gzip decompression error: {}", e)))
        })?;
        Ok(decoded)
    } else {
        // 直接转换为UTF-8字符串
        String::from_utf8(bytes.to_vec()).map_err(|e| {
            AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("UTF-8 decode error: {}", e)))
        })
    }
}

/// 判断请求是否为流式请求
fn is_streaming_request(body: &Option<String>) -> bool {
    if let Some(body_str) = body {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(body_str) {
            return json.get("stream").map(|v| v.as_bool().unwrap_or(false)).unwrap_or(false);
        }
    }
    false
}

/// 从客户端请求 path 中剥除与 `remote_url` 路径段重复的前缀，并额外剥除客户端 path 中
/// 所有"语义为协议版本号"的段（`vN`、`vNbeta`、`vN.M` 等），从而让 `providers.api_url`
/// 中已包含的版本路径段（如 `/v1`、`/v1beta`、`/compatible-mode/v1`、`/api/paas/v4`）
/// 成为最终请求 URL 唯一的版本号来源。
///
/// 例如：
///   - remote_url=`https://api.openai.com/v1`             + path=`/v1/chat/completions`  → `/chat/completions`
///   - remote_url=`https://open.bigmodel.cn/api/paas/v4`  + path=`/v1/chat/completions`  → `/chat/completions`
///   - remote_url=`https://dashscope.aliyuncs.com/compatible-mode/v1` + path=`/v1/chat/completions` → `/chat/completions`
///
/// 不会误删业务路径段（如 `chat/completions`、`embeddings`、`models`），因为它们不是版本号格式。
fn strip_remote_url_prefix(remote_url: &str, path: &str) -> String {
    // 取 remote_url 的 path 段(去掉 host 部分),例如 "https://host.example/v1" -> "/v1"
    let remote_path = match remote_url.find("://") {
        Some(idx) => &remote_url[idx + 3..],
        None => remote_url,
    };
    let remote_path = match remote_path.find('/') {
        Some(idx) => &remote_path[idx..],
        None => return path.to_string(),
    };
    let remote_path = remote_path.trim_end_matches('/');

    let path_trimmed = path.trim_start_matches('/');
    if path_trimmed.is_empty() {
        return "/".to_string();
    }

    // 1) 先剥除与 remote_url 完全重复的路径段
    let mut result: Vec<String> = path_trimmed.split('/').map(|s| s.to_string()).collect();
    if !remote_path.is_empty() {
        let remote_segments: Vec<&str> = remote_path.trim_start_matches('/').split('/').collect();
        if remote_segments.len() < result.len()
            && result[..remote_segments.len()]
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                == remote_segments
        {
            result = result[remote_segments.len()..].to_vec();
        }
    }

    // 2) 从客户端 path 开头连续剥除所有"协议版本号"段（v1 / v1beta / v2.5 等），
    //    直到遇到非版本号段为止
    while let Some(head) = result.first() {
        if is_version_segment(head) {
            result.remove(0);
        } else {
            break;
        }
    }

    if result.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", result.join("/"))
    }
}

/// 判断单段路径是否为"协议版本号"，如 `v1`、`v1beta`、`v2.5`、`v4preview`。
/// 注意只匹配 OpenAI 兼容协议里常见的 `v` 开头版本号，避免误删业务路径。
fn is_version_segment(seg: &str) -> bool {
    let s = seg.trim();
    if s.is_empty() || !s.starts_with('v') {
        return false;
    }
    let rest = &s[1..];
    if rest.is_empty() {
        return false;
    }
    // 段内字符必须是 [a-zA-Z0-9.]（如 "1"、"1beta"、"2.5"、"4preview"），否则视为业务段
    rest.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.')
}

/// 转发HTTP请求到目标服务器（流式响应）
///
/// 支持SSE流式响应，实时将数据返回给客户端。
/// `api_type`："chat"=OpenAI 兼容，"anthropic"=Anthropic；后者会强制把请求路径改为
/// `/v1/messages`，并把 OpenAI 风格 body 转 Anthropic Messages 格式（保留 stream 字段）。
fn forward_streaming_request(
    method: &str,
    path: &str,
    headers: &[(String, String)],
    body: &Option<String>,
    remote_url: &str,
    api_key: &str,
    model: &str,
    api_type: &str,
    extra_headers: &[(String, String)],
    stream: &mut TcpStream,
) -> AppResult<()> {
    let normalized_path = resolve_target_path(remote_url, path, api_type);
    let target_url = format!("{}{}", remote_url.trim_end_matches('/'), normalized_path);

    let origin = headers.iter()
        .find(|(name, _)| name.to_lowercase() == "origin")
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| "*".to_string());

    // 拼出客户端发起的完整 URL，便于对照「输入 URL」与「最终转发 URL」。
    let host_header = headers.iter()
        .find(|(name, _)| name.to_lowercase() == "host")
        .map(|(_, value)| value.as_str())
        .unwrap_or("localhost");
    let client_url = format!("http://{}{}", host_header, path);

    log_proxy_summary("\n=== Forwarding Streaming Request ===");
    log_proxy_summary(&format!("Method: {}", method));
    log_proxy_summary(&format!("输入 URL: {}", client_url));
    log_proxy_summary(&format!("目标 URL: {}", target_url));
    log_proxy_summary(&format!("Model: {}", model));
    log_proxy_summary(&format!(
        "API 类型: {}",
        api_type_label(api_type)
    ));

    let protocol = ApiProtocol::from_str_lossy(api_type);

    let final_body = if let Some(ref body_str) = body {
        let injected = if protocol == ApiProtocol::Anthropic {
            convert_openai_body_to_anthropic(body_str, model)
        } else {
            inject_model_to_body(body_str, model)
        };
        Some(normalize_codex_body(&injected, remote_url))
    } else {
        None
    };

    // 记录请求体日志（streaming 路径此前缺失；补全便于联调时直接看到客户端发起的 body 内容）
    match &final_body {
        Some(body_str) => {
            log_proxy_message(&format!("Body ({} bytes):", body_str.len()));
            log_proxy_message(body_str);
        }
        None => {
            log_proxy_message("Body: (empty)");
        }
    }
    log_proxy_message("=== End Forwarding Streaming Request ===");

    let runtime = Runtime::new().map_err(|e| {
        AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("Failed to create runtime: {}", e)))
    })?;

    let result: AppResult<()> = runtime.block_on(async move {
        let client = reqwest::Client::builder()
            // 流式场景下不能设全局 timeout（会覆盖整个流式读取阶段），
            // 改为只限制连接建立和 HTTPS 握手阶段，流式体读取由上游自身的超时控制
            .connect_timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| {
                AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("Client build error: {}", e)))
            })?;

        let mut request = client.request(reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| {
            AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("Invalid method: {}", e)))
        })?, &target_url);

        for (name, value) in headers {
            let name_lower = name.to_lowercase();
            if protocol == ApiProtocol::Anthropic {
                // Anthropic 协议：剥离 Authorization 与 Anthropic 专属头，统一由代理注入
                match name_lower.as_str() {
                    "authorization" | "x-api-key" | "anthropic-version" | "anthropic-beta" => continue,
                    _ => {}
                }
            }
            if name_lower == "authorization" {
                request = request.header("Authorization", &format!("Bearer {}", api_key));
            } else if name_lower == "host" || name_lower == "content-length" || name_lower == "connection" {
                continue;
            } else if name_lower == "accept-encoding" {
                // 客户端声明的 br/gzip/deflate 转发给上游后，会导致上游返回压缩字节流，
                // 而代理严格按 UTF-8 解码响应体时会崩溃。代理自身会重写响应体，
                // 不需要透传压缩，统一剥掉让上游返回明文。
                continue;
            } else {
                request = request.header(name, value);
            }
        }

        if protocol == ApiProtocol::Anthropic {
            // 注入 Anthropic 鉴权头
            request = request.header("x-api-key", api_key);
            request = request.header("anthropic-version", ANTHROPIC_VERSION_HEADER);
        }

        // 注入额外请求头（如 ChatGPT 后端需要的 chatgpt-account-id）
        for (name, value) in extra_headers {
            request = request.header(name.as_str(), value.as_str());
        }

        let request = match final_body {
            Some(body_str) => request.body(body_str),
            None => request,
        };

        let start_time = std::time::Instant::now();
        let response = request.send().await.map_err(|e| {
            AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("Request error: {}", e)))
        })?;

        let duration = start_time.elapsed();
        let status_line = format!("HTTP/1.1 {} {}", response.status().as_u16(), response.status().canonical_reason().unwrap_or("OK"));

        let content_encoding = response.headers()
            .get("content-encoding")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .to_lowercase();

        let mut response_str = format!(
            "{}\r\n\
            Access-Control-Allow-Origin: {}\r\n\
            Access-Control-Allow-Methods: GET, POST, PUT, DELETE, OPTIONS\r\n\
            Access-Control-Allow-Headers: *\r\n\
            Access-Control-Allow-Credentials: true\r\n\
            Content-Type: text/event-stream\r\n\
            Connection: keep-alive\r\n\
            Cache-Control: no-cache\r\n\
            X-Accel-Buffering: no\r\n\
            Pragma: no-cache\r\n",
            status_line, origin
        );

        if !content_encoding.is_empty() {
            response_str.push_str(&format!("Content-Encoding: {}\r\n", content_encoding));
        }
        response_str.push_str("\r\n");

        if let Err(e) = write_all_tolerant(stream, response_str.as_bytes()) {
            return Err(AppError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Write header error: {}", e),
            )));
        }
        if let Err(e) = stream.flush() {
            // flush 失败时若只是 WouldBlock/对端关闭，按 benign 错误处理
            if !matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::Interrupted
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::BrokenPipe
            ) {
                return Err(AppError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Flush header error: {}", e),
                )));
            }
        }

        log_proxy_summary("\n=== Streaming Response Started ===");
        log_proxy_summary(&format!("Time to first byte: {:?}", duration));
        log_proxy_summary(&format!("Status: {}", status_line));
        log_proxy_message(&format!("Content-Encoding: {}", content_encoding));

        let mut total_bytes = 0;
        let mut response_body = Vec::new();
        let mut gzip_buffer = Vec::new();

        let mut stream_reader = response.bytes_stream();
        while let Some(chunk) = stream_reader.next().await {
            let bytes = chunk.map_err(|e| {
                AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("Stream read error: {}", e)))
            })?;

            total_bytes += bytes.len();

            if content_encoding == "gzip" {
                gzip_buffer.extend_from_slice(&bytes);

                let mut decoder = flate2::read::GzDecoder::new(&gzip_buffer[..]);
                let mut decoded = Vec::new();
                match decoder.read_to_end(&mut decoded) {
                    Ok(_) => {
                        response_body.extend_from_slice(&decoded);
                        if let Err(e) = write_all_tolerant(stream, &decoded) {
                            return Err(AppError::Io(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                format!("Write chunk error: {}", e),
                            )));
                        }
                        if let Err(e) = stream.flush() {
                            if !matches!(
                                e.kind(),
                                std::io::ErrorKind::WouldBlock
                                    | std::io::ErrorKind::Interrupted
                                    | std::io::ErrorKind::ConnectionReset
                                    | std::io::ErrorKind::BrokenPipe
                            ) {
                                return Err(AppError::Io(std::io::Error::new(
                                    std::io::ErrorKind::Other,
                                    format!("Flush chunk error: {}", e),
                                )));
                            }
                        }
                        gzip_buffer.clear();
                    }
                    Err(_) => {
                    }
                }
            } else {
                response_body.extend_from_slice(&bytes);

                if let Err(e) = write_all_tolerant(stream, &bytes) {
                    return Err(AppError::Io(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Write chunk error: {}", e),
                    )));
                }
                if let Err(e) = stream.flush() {
                    if !matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::Interrupted
                            | std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::BrokenPipe
                    ) {
                        return Err(AppError::Io(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            format!("Flush chunk error: {}", e),
                        )));
                    }
                }
            }
        }

        let total_duration = start_time.elapsed();

        log_proxy_summary(&format!("Total bytes streamed: {}", total_bytes));
        log_proxy_summary(&format!("Total duration: {:?}", total_duration));

        // 对 Responses API 检查流是否以 response.completed 正常结束
        if protocol == ApiProtocol::Responses {
            let raw = String::from_utf8_lossy(&response_body);
            if !raw.contains("response.completed") {
                log_proxy_summary("WARN: Responses stream ended WITHOUT 'response.completed' event — client SDK may see 'disconnected before completion'");
            }
        }

        if content_encoding.is_empty() {
            // 非压缩响应可以原样打日志；压缩响应原始字节是 br/gzip 字节流，
            // 强转 UTF-8 会出现大量乱码，因此只输出元信息而不打印 Body。
            let body_str = String::from_utf8_lossy(&response_body);
            if body_str.len() > 2000 {
                log_proxy_message(&format!("Response Body (truncated): {}\n...", &body_str[..2000]));
            } else {
                log_proxy_message(&format!("Response Body: {}", body_str));
            }
        } else {
            log_proxy_message(&format!(
                "Response Body omitted ({} bytes, content-encoding={})",
                response_body.len(),
                content_encoding
            ));
        }

        log_proxy_summary("=== End Streaming Response ===");

        Ok(())
    });

    result
}

/// 转发HTTP请求到目标服务器（非流式）
///
/// 核心功能：
/// 1. 构建目标URL
/// 2. 处理请求头（注入正确的API Key）
/// 3. 注入模型参数到请求体
/// 4. 发送请求到目标服务器
/// 5. 处理响应（解压gzip等）
/// 6. 构建完整的HTTP响应返回给客户端
///
/// `api_type`："chat"=OpenAI 兼容（默认），"anthropic"=Anthropic。
///   Anthropic 模式下，会强制把请求路径改为 `/v1/messages`、使用 `x-api-key` 头，
///   并把 OpenAI 风格 body 转 Anthropic Messages 格式。
fn forward_request(
    method: &str,
    path: &str,
    headers: &[(String, String)],
    body: &Option<String>,
    remote_url: &str,
    api_key: &str,
    model: &str,
    api_type: &str,
    extra_headers: &[(String, String)],
) -> AppResult<String> {
    // 构建目标URL：Anthropic 模式统一发到 /v1/messages；
    // OpenAI 模式下剥除与 remote_url 重复的路径前缀以及版本号段。
    let normalized_path = resolve_target_path(remote_url, path, api_type);
    let target_url = format!("{}{}", remote_url.trim_end_matches('/'), normalized_path);

    // 提取Origin头用于CORS响应
    let origin = headers.iter()
        .find(|(name, _)| name.to_lowercase() == "origin")
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| "*".to_string());

    // 拼出客户端发起的完整 URL（含 Host 头），便于日志对照「输入 URL」与「最终转发 URL」。
    let host_header = headers.iter()
        .find(|(name, _)| name.to_lowercase() == "host")
        .map(|(_, value)| value.as_str())
        .unwrap_or("localhost");
    let client_url = format!("http://{}{}", host_header, path);

    // 记录转发请求日志
    log_proxy_summary("\n=== Forwarding Request ===");
    log_proxy_summary(&format!("Method: {}", method));
    log_proxy_summary(&format!("输入 URL: {}", client_url));
    log_proxy_summary(&format!("目标 URL: {}", target_url));
    log_proxy_summary(&format!("Model: {}", model));
    log_proxy_summary(&format!(
        "API 类型: {}",
        api_type_label(api_type)
    ));

    let protocol = ApiProtocol::from_str_lossy(api_type);

    let mut forwarded_headers = Vec::new();

    // 如果有请求体，按协议决定注入策略：
    //   OpenAI：仅替换 model
    //   Anthropic：先把 OpenAI 风格 body 转 Anthropic Messages 格式，再替换 model
    let final_body = if let Some(ref body_str) = body {
        let injected = if protocol == ApiProtocol::Anthropic {
            convert_openai_body_to_anthropic(body_str, model)
        } else {
            inject_model_to_body(body_str, model)
        };
        Some(normalize_codex_body(&injected, remote_url))
    } else {
        None
    };

    // 创建HTTP客户端（60秒超时）
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| {
            AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("Client build error: {}", e)))
        })?;

    // 创建请求对象
    let mut request = client.request(reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| {
        AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("Invalid method: {}", e)))
    })?, &target_url);

    // 处理请求头
    let mut has_auth = false;
    let mut has_content_type = false;

    for (name, value) in headers {
        let name_lower = name.to_lowercase();
        if protocol == ApiProtocol::Anthropic {
            // Anthropic 协议使用 x-api-key，忽略客户端原 Authorization；
            // 同时剥离 anthropic-version / anthropic-beta，统一由代理注入。
            match name_lower.as_str() {
                "authorization" => continue,
                "x-api-key" | "anthropic-version" | "anthropic-beta" => continue,
                _ => {}
            }
        }
        if name_lower == "authorization" {
            // 替换为正确的API Key（OpenAI 模式）
            has_auth = true;
            forwarded_headers.push(("Authorization".to_string(), mask_token(&format!("Bearer {}", api_key))));
            request = request.header("Authorization", &format!("Bearer {}", api_key));
        } else if name_lower == "content-type" {
            has_content_type = true;
            forwarded_headers.push((name.clone(), value.clone()));
            request = request.header(name, value);
        } else if name_lower == "host" {
            // 跳过Host头，由客户端自动处理
            continue;
        } else if name_lower == "content-length" {
            // 跳过Content-Length，由客户端自动处理
            continue;
        } else if name_lower == "accept-encoding" {
            // 客户端声明的 br/gzip/deflate 转发给上游后会导致上游返回压缩字节流，
            // 而 read_response_body 严格按 UTF-8 解码时会崩溃。代理重写响应体，
            // 不需要透传压缩，统一剥掉让上游返回明文。
            continue;
        } else {
            forwarded_headers.push((name.clone(), value.clone()));
            request = request.header(name, value);
        }
    }

    // 协议级认证头注入
    if protocol == ApiProtocol::Anthropic {
        // x-api-key 头是 Anthropic 的鉴权方式
        forwarded_headers.push(("x-api-key".to_string(), mask_token(api_key)));
        request = request.header("x-api-key", api_key);
        // anthropic-version 必填，使用较新的稳定版本
        forwarded_headers.push((
            "anthropic-version".to_string(),
            ANTHROPIC_VERSION_HEADER.to_string(),
        ));
        request = request.header("anthropic-version", ANTHROPIC_VERSION_HEADER);
    } else if !has_auth {
        forwarded_headers.push(("Authorization".to_string(), mask_token(&format!("Bearer {}", api_key))));
        request = request.header("Authorization", &format!("Bearer {}", api_key));
    }

    // 注入额外请求头（如 ChatGPT 后端需要的 chatgpt-account-id）
    for (name, value) in extra_headers {
        forwarded_headers.push((name.clone(), value.clone()));
        request = request.header(name.as_str(), value.as_str());
    }

    // 如果有请求体但没有Content-Type，添加默认值
    if final_body.is_some() && !has_content_type {
        forwarded_headers.push(("Content-Type".to_string(), "application/json".to_string()));
        request = request.header("Content-Type", "application/json");
    }

    // 记录请求头日志
    log_proxy_message("Headers:");
    for (name, value) in &forwarded_headers {
        log_proxy_message(&format!("  {}: {}", name, value));
    }
    
    // 记录请求体日志
    match final_body {
        Some(ref body_str) => {
            log_proxy_message(&format!("Body ({} bytes):", body_str.len()));
            log_proxy_message(body_str);
        }
        None => {
            log_proxy_message("Body: (empty)");
        }
    }
    
    // 记录完整请求日志
    log_proxy_message("\n=== Full Request ===");
    log_proxy_message(&format!("{} {} HTTP/1.1", method, path));
    for (name, value) in &forwarded_headers {
        log_proxy_message(&format!("{}: {}", name, value));
    }
    log_proxy_message("");
    if let Some(body_str) = &final_body {
        log_proxy_message(body_str);
    }
    log_proxy_message("=== End Full Request ===");

    // 发送请求到目标服务器
    let start_time = std::time::Instant::now();
    let response = match final_body {
        Some(body_str) => request.body(body_str.clone()).send(),
        None => request.send(),
    }.map_err(|e| {
        AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("Request error: {}", e)))
    })?;
    let duration = start_time.elapsed();

    // 构建响应状态行
    let status_line = format!("HTTP/1.1 {} {}", response.status().as_u16(), response.status().canonical_reason().unwrap_or("OK"));
    
    // 提取响应头
    let response_headers: Vec<(String, String)> = response.headers()
        .iter()
        .map(|(name, value)| (name.as_str().to_string(), value.to_str().unwrap_or("").to_string()))
        .collect();
    
    // 读取并处理响应体（支持gzip解压）
    let response_body = read_response_body(response)?;

    // 构建完整的HTTP响应
    let mut response_str = String::new();
    response_str.push_str(&status_line);
    response_str.push_str("\r\n");
    
    // 添加CORS响应头
    response_str.push_str(&format!("Access-Control-Allow-Origin: {}\r\n", origin));
    response_str.push_str("Access-Control-Allow-Methods: GET, POST, PUT, DELETE, OPTIONS\r\n");
    response_str.push_str("Access-Control-Allow-Headers: *\r\n");
    response_str.push_str("Access-Control-Allow-Credentials: true\r\n");
    response_str.push_str("Content-Type: application/json\r\n");
    
    // 添加Content-Length
    let body_bytes = response_body.as_bytes();
    response_str.push_str(&format!("Content-Length: {}\r\n", body_bytes.len()));
    response_str.push_str("Connection: close\r\n");
    
    // 添加其他响应头（过滤已处理的头）
    for (name, value) in &response_headers {
        let name_lower = name.to_lowercase();
        if name_lower != "content-encoding" && 
           name_lower != "transfer-encoding" && 
           name_lower != "content-length" && 
           name_lower != "connection" &&
           name_lower != "content-type" &&
           name_lower != "access-control-allow-origin" &&
           name_lower != "access-control-allow-methods" &&
           name_lower != "access-control-allow-headers" &&
           name_lower != "access-control-allow-credentials" {
            response_str.push_str(&format!("{}: {}\r\n", name, value));
        }
    }
    
    // 添加响应体
    response_str.push_str("\r\n");
    response_str.push_str(&response_body);

    // 记录响应日志
    log_proxy_summary("\n=== Response Received ===");
    log_proxy_summary(&format!("Time: {:?}", duration));
    log_proxy_summary(&format!("Status: {}", status_line));

    log_proxy_message("Response Headers:");
    for (name, value) in response_headers {
        log_proxy_message(&format!("  {}: {}", name, value));
    }
    
    log_proxy_message(&format!("Response Body ({} bytes):", response_body.len()));
    if response_body.len() > 2000 {
        // 长响应截断显示
        log_proxy_message(&format!("{}\n... (truncated)", &response_body[..2000]));
    } else {
        log_proxy_message(&response_body);
    }
    
    log_proxy_message("=== End Response ===");

    Ok(response_str)
}

/// 构建CORS响应
/// 
/// 处理浏览器的CORS预检请求（OPTIONS方法），返回适当的CORS响应头
fn build_cors_response(headers: &[(String, String)]) -> String {
    // 提取Origin头（或使用默认值）
    let default_origin = "*".to_string();
    let origin = headers.iter()
        .find(|(name, _)| name.to_lowercase() == "origin")
        .map(|(_, value)| value)
        .unwrap_or(&default_origin);
    
    // 提取Access-Control-Request-Headers头（或使用默认值）
    let default_headers = "*".to_string();
    let request_headers = headers.iter()
        .find(|(name, _)| name.to_lowercase() == "access-control-request-headers")
        .map(|(_, value)| value)
        .unwrap_or(&default_headers);
    
    // 提取Access-Control-Request-Method头（或使用默认值）
    let default_method = "*".to_string();
    let request_method = headers.iter()
        .find(|(name, _)| name.to_lowercase() == "access-control-request-method")
        .map(|(_, value)| value)
        .unwrap_or(&default_method);

    // 构建CORS响应
    let response = format!(
        "HTTP/1.1 200 OK\r\n\
Access-Control-Allow-Origin: {}\r\n\
Access-Control-Allow-Methods: {}\r\n\
Access-Control-Allow-Headers: {}\r\n\
Access-Control-Allow-Credentials: true\r\n\
Content-Length: 0\r\n\
\r\n",
        origin, request_method, request_headers
    );
    
    log_proxy_message(&format!("CORS response built for origin: {}", origin));
    response
}

/// 向请求体中注入模型参数
///
/// 将请求体中的 model 字段替换为 ProviderKey 关联 Provider 的默认模型。
/// 当 model 为空时（Provider 未配置 models），保留请求体原有 model 字段不做改动。
///
/// 仅做"替换顶层 `model`"这一种最小改动，因此兼容 OpenAI Chat Completions
/// 与 Responses API（Codex CLI 使用）两种请求体格式。
fn inject_model_to_body(body: &str, model: &str) -> String {
    // 未配置默认模型时不覆盖请求体的 model 字段
    if model.is_empty() {
        return body.to_string();
    }
    // 尝试解析 JSON 并注入/替换 model 参数
    if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(body) {
        json["model"] = serde_json::Value::String(model.to_string());
        serde_json::to_string(&json).unwrap_or_else(|_| body.to_string())
    } else {
        // 如果解析失败，返回原始 body
        body.to_string()
    }
}

/// ChatGPT 订阅后端禁止保存响应；覆盖客户端显式传入的 true。
/// 普通 API 服务商保留原有 store 语义。
fn normalize_codex_body(body: &str, remote_url: &str) -> String {
    if remote_url.trim_end_matches('/') != oauth::CHATGPT_CODEX_BASE_URL {
        return body.to_string();
    }
    let Ok(mut json) = serde_json::from_str::<serde_json::Value>(body) else {
        return body.to_string();
    };
    let Some(obj) = json.as_object_mut() else {
        return body.to_string();
    };
    obj.insert("store".to_string(), serde_json::Value::Bool(false));
    serde_json::to_string(&json).unwrap_or_else(|_| body.to_string())
}

#[cfg(test)]
mod codex_body_tests {
    use super::*;

    #[test]
    fn subscription_requests_always_disable_storage() {
        for store in [None, Some(true), Some(false)] {
            let mut body = serde_json::json!({
                "model": "test-model", "input": "hello", "stream": true
            });
            if let Some(value) = store {
                body["store"] = serde_json::json!(value);
            }
            let result: serde_json::Value = serde_json::from_str(
                &normalize_codex_body(&body.to_string(), oauth::CHATGPT_CODEX_BASE_URL)
            ).unwrap();
            body["store"] = serde_json::json!(false);
            assert_eq!(result, body);
        }
    }

    #[test]
    fn other_providers_keep_storage_setting() {
        let body = r#"{"model":"test-model","store":true}"#;
        assert_eq!(normalize_codex_body(body, "https://api.openai.com/v1"), body);
        assert_eq!(normalize_codex_body("invalid", oauth::CHATGPT_CODEX_BASE_URL), "invalid");
    }
}
