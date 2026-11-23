use crate::repositories::{AppConfig, Provider, ProxyKey};
use crate::services::{
    CreateProviderKeyRequest, CreateProviderRequest, UpdateProviderKeyRequest, UpdateProviderRequest, UpdateConfigRequest, ProviderKeyWithStatus,
    create_provider_key as svc_create_provider_key,
    get_all_provider_keys as svc_get_all_provider_keys,
    update_provider_key as svc_update_provider_key,
    delete_provider_key as svc_delete_provider_key,
    toggle_provider_key_active as svc_toggle_provider_key_active,
    create_provider as svc_create_provider,
    get_all_providers as svc_get_all_providers,
    update_provider as svc_update_provider,
    delete_provider as svc_delete_provider,
    toggle_provider_status as svc_toggle_provider_status,
    get_config as svc_get_config,
    update_config as svc_update_config,
    apply_build_env as svc_apply_build_env,
};
use crate::services::oauth;
use crate::services::remote::{
    fetch_remote_providers as svc_fetch_remote_providers,
    fetch_remote_provider_detail as svc_fetch_remote_provider_detail,
    RemoteProvider, RemoteProviderDetail,
};
use crate::services::feedback::{
    submit_feedback as svc_submit_feedback,
    FeedbackSubmitInput,
};
use clipboard::{ClipboardContext, ClipboardProvider};

// ======================== ProviderKey Commands ========================

#[tauri::command]
pub fn create_provider_key(
    name: String,
    api_key: String,
    provider_id: String,
    auth_type: Option<String>,
    refresh_token: Option<String>,
    auth_meta: Option<String>,
) -> Result<ProviderKeyWithStatus, String> {
    svc_create_provider_key(CreateProviderKeyRequest {
        name,
        api_key,
        provider_id,
        auth_type,
        refresh_token,
        auth_meta,
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_all_provider_keys() -> Result<Vec<ProviderKeyWithStatus>, String> {
    svc_get_all_provider_keys()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_provider_key(
    id: String,
    name: String,
    api_key: Option<String>,
    provider_id: String,
    auth_type: Option<String>,
    refresh_token: Option<String>,
    auth_meta: Option<String>,
) -> Result<(), String> {
    svc_update_provider_key(UpdateProviderKeyRequest {
        id,
        name,
        api_key,
        provider_id,
        auth_type,
        refresh_token,
        auth_meta,
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_provider_key(id: String) -> Result<(), String> {
    svc_delete_provider_key(&id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn toggle_provider_key_active(id: String, is_active: bool) -> Result<(), String> {
    svc_toggle_provider_key_active(&id, is_active)
        .map_err(|e| e.to_string())
}

// ======================== 浏览器授权（OAuth2）Commands ========================

/// 浏览器授权结果（前端据此写入服务商秘钥）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiBrowserAuthResult {
    /// access_token（作为服务商秘钥的 api_key）
    pub access_token: String,
    /// refresh_token（用于后续自动续期）
    pub refresh_token: Option<String>,
    /// access_token 过期时间（Unix 秒）
    pub expires_at: Option<i64>,
    /// ChatGPT 账号 ID
    pub account_id: Option<String>,
    /// 登录邮箱
    pub email: Option<String>,
    /// 授权元数据 JSON（accountId / expiresAt / email）
    pub auth_meta: String,
}

/// 发起 OpenAI 浏览器授权（"Sign in with ChatGPT"）。
///
/// 流程：本机监听 1455 端口 → 打开系统浏览器 → 用户登录 → 回调拿到授权码 →
/// 换取 access_token / refresh_token。令牌仅返回给前端，由前端随表单一起保存。
#[tauri::command]
pub async fn start_openai_browser_auth(
    app: tauri::AppHandle,
) -> Result<OpenAiBrowserAuthResult, String> {
    // 中断上一次未完成的授权
    oauth::cancel_pending_auth();

    let state = oauth::random_urlsafe(16);
    let code_verifier = oauth::generate_code_verifier();
    let challenge = oauth::code_challenge(&code_verifier);
    let authorize_url = oauth::build_authorize_url(&state, &challenge);

    // 监听回环回调端口（OpenAI 登记的回调地址固定为 localhost:1455）
    let (listener, rx) = oauth::spawn_callback_listener(&state)
        .map_err(|e| format!("启动授权回调监听失败: {}", e))?;

    tracing::info!(port = listener.port, "openai browser auth: 等待浏览器回调");

    // 用系统默认浏览器打开授权页
    {
        use tauri_plugin_opener::OpenerExt;
        if let Err(e) = app.opener().open_url(authorize_url.clone(), None::<&str>) {
            tracing::warn!(error = %e, "openai browser auth: 打开浏览器失败");
            oauth::cancel_pending_auth();
            return Err(format!(
                "打开浏览器失败: {}。请手动访问 {} 完成授权",
                e, authorize_url
            ));
        }
    }

    // 等待回调（最多 5 分钟）
    let code = match tokio::time::timeout(
        std::time::Duration::from_secs(oauth::AUTH_WAIT_TIMEOUT_SECS),
        rx,
    )
    .await
    {
        Ok(Ok(Ok(code))) => code,
        Ok(Ok(Err(msg))) => return Err(msg),
        Ok(Err(_)) => return Err("授权回调通道已关闭".to_string()),
        Err(_) => {
            oauth::cancel_pending_auth();
            return Err("等待浏览器授权超时".to_string());
        }
    };

    // 换取令牌（阻塞式 HTTP，放到独立线程避免阻塞运行时）
    let verifier = code_verifier.clone();
    let token = tokio::task::spawn_blocking(move || {
        oauth::exchange_authorization_code(&code, &verifier)
    })
    .await
    .map_err(|e| format!("令牌交换任务失败: {}", e))?
    .map_err(|e| format!("换取令牌失败: {}", e))?;

    tracing::info!(
        account_id = token.account_id.as_deref().unwrap_or("-"),
        expires_at = token.expires_at.unwrap_or(0),
        "openai browser auth: 授权成功"
    );

    let meta = oauth::build_auth_meta(&token);

    Ok(OpenAiBrowserAuthResult {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_at: token.expires_at,
        account_id: token.account_id,
        email: token.email,
        auth_meta: meta.to_json(),
    })
}

/// 取消正在等待的浏览器授权。
#[tauri::command]
pub fn cancel_openai_browser_auth() -> Result<(), String> {
    oauth::cancel_pending_auth();
    Ok(())
}

// ======================== Proxy Server Commands ========================

#[tauri::command]
pub fn start_proxy_server() -> Result<(), String> {
    // Start proxy server in a background thread
    std::thread::spawn(|| {
        if let Err(e) = crate::services::proxy::start_proxy_server() {
            tracing::error!(error = %e, "proxy server error");
        }
    });
    Ok(())
}

#[tauri::command]
pub fn stop_proxy_server() -> Result<(), String> {
    crate::services::proxy::stop_proxy_server();
    Ok(())
}

#[tauri::command]
pub fn get_proxy_status() -> bool {
    crate::services::proxy::is_proxy_running()
}

// ======================== Config Commands ========================

#[tauri::command]
pub fn get_config() -> Result<AppConfig, String> {
    svc_get_config()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_config(
    listen_port: i32,
    auto_start: bool,
    minimize_to_tray: bool,
    log_level: String,
    server_url: String,
) -> Result<(), String> {
    svc_update_config(UpdateConfigRequest {
        listen_port,
        auto_start,
        minimize_to_tray,
        log_level,
        server_url,
    })
    .map_err(|e| e.to_string())
}

/// 当前后端 logger 是一次性初始化的，启动后无法热修改 EnvFilter；
/// 此命令仅用于测试 runner 在前端设置页保存时确认 log_level 已被持久化，
/// 实际生效需要重启进程。如未来切换到 reload-based subscriber，
/// 在此处调用 tracing_subscriber::reload 即可热更新。
#[tauri::command]
pub fn apply_log_level(level: String) -> Result<(), String> {
    crate::repositories::get_config()
        .map(|_| ())
        .map_err(|e| e.to_string())?;
    tracing::info!(requested_level = %level, "log level change requires restart to take effect");
    Ok(())
}

/// 前端启动时调用：把构建期注入的 `VITE_SERVER_URL` / `VITE_APP_ENV` 写入数据库。
/// server_url 仅在仍是出厂默认 `https://api.ciyuanproxy.com/` 时被覆盖，
/// 避免覆盖用户在设置页手动修改过的地址。
#[tauri::command]
pub fn init_build_env(server_url: String, app_env: String) -> Result<(), String> {
    svc_apply_build_env(&server_url, &app_env).map_err(|e| e.to_string())
}

// ======================== Provider Commands ========================

#[tauri::command]
pub fn create_provider(
    name: String,
    website: Option<String>,
    api_url: String,
    api_anthropic_url: Option<String>,
    api_responses_url: Option<String>,
    description: Option<String>,
    models: String,
    r#type: i32,
    tags: Vec<String>,
    pricing: Option<String>,
    status: i32,
    sort: i32,
    auth_modes: Option<Vec<String>>,
) -> Result<Provider, String> {
    svc_create_provider(CreateProviderRequest {
        name,
        website,
        api_url,
        api_anthropic_url,
        api_responses_url,
        description,
        models,
        r#type,
        tags,
        pricing,
        status,
        sort,
        auth_modes: auth_modes.unwrap_or_default(),
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_all_providers() -> Result<Vec<Provider>, String> {
    svc_get_all_providers()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_provider(
    id: String,
    name: String,
    website: Option<String>,
    api_url: String,
    api_anthropic_url: Option<String>,
    api_responses_url: Option<String>,
    description: Option<String>,
    models: String,
    r#type: i32,
    tags: Vec<String>,
    pricing: Option<String>,
    status: i32,
    sort: i32,
    auth_modes: Option<Vec<String>>,
) -> Result<(), String> {
    svc_update_provider(UpdateProviderRequest {
        id,
        name,
        website,
        api_url,
        api_anthropic_url,
        api_responses_url,
        description,
        models,
        r#type,
        tags,
        pricing,
        status,
        sort,
        auth_modes: auth_modes.unwrap_or_default(),
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_provider(id: String) -> Result<(), String> {
    svc_delete_provider(&id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn toggle_provider_status(id: String, is_active: bool) -> Result<(), String> {
    svc_toggle_provider_status(&id, is_active)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn fetch_remote_providers(
    server_url: String,
    r#type: Option<i32>,
    keyword: Option<String>,
) -> Result<Vec<RemoteProvider>, String> {
    svc_fetch_remote_providers(&server_url, r#type, keyword)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn fetch_remote_provider_detail(
    server_url: String,
    provider_id: String,
) -> Result<RemoteProviderDetail, String> {
    svc_fetch_remote_provider_detail(&server_url, &provider_id)
        .await
        .map_err(|e| e.to_string())
}

// ======================== Feedback Commands ========================

#[tauri::command]
pub async fn submit_feedback(
    server_url: String,
    input: FeedbackSubmitInput,
) -> Result<i64, String> {
    svc_submit_feedback(&server_url, &input)
        .await
        .map_err(|e| e.to_string())
}

// ======================== Update Commands ========================

#[tauri::command]
pub async fn get_current_version() -> Result<String, String> {
    Ok(env!("CARGO_PKG_VERSION").to_string())
}

#[tauri::command]
pub async fn get_tauri_version() -> Result<String, String> {
    Ok(tauri::VERSION.to_string())
}

#[tauri::command]
pub async fn get_os_version() -> Result<String, String> {
    Ok(detect_os_version())
}

/// 解析编译期生效的 updater endpoint，替换掉 `{{target}}` / `{{arch}}` /
/// `{{current_version}}` / `{{bundle_type}}` 占位符，并把最终请求地址写进日志。
///
/// <p>tauri-plugin-updater 的报错信息里不含请求 URL（例如 "update endpoint did not
/// respond with a successful status code" 只给结论），排查时完全不知道打的是哪个地址。
/// 这里在前端调用 `check()` 之前先把真实 URL 落到
/// `%APPDATA%\com.ciyuanproxy.desktop\logs\app.log` 和「日志」页，并返回给前端
/// 拼进错误提示。
///
/// <p>占位符取值与插件内部 `updater_os()` / `updater_arch()` 保持一致
/// （见 tauri-plugin-updater/src/updater.rs）。若插件改了这两个函数，这里要跟着改。
#[tauri::command]
pub async fn log_updater_endpoints(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    let config = app.config();
    let templates: Vec<String> = config
        .plugins
        .0
        .get("updater")
        .and_then(|value| value.get("endpoints"))
        .and_then(|value| serde_json::from_value::<Vec<String>>(value.clone()).ok())
        .unwrap_or_default();

    if templates.is_empty() {
        tracing::error!("updater: plugins.updater.endpoints 未配置或不是字符串数组");
        crate::emit_proxy_log("[updater] 未配置 plugins.updater.endpoints");
        return Ok(Vec::new());
    }

    let current_version = app.package_info().version.to_string();
    let resolved: Vec<String> = templates
        .iter()
        .map(|t| {
            t.replace("{{current_version}}", &current_version)
                .replace("{{target}}", updater_os())
                .replace("{{arch}}", updater_arch())
                .replace("{{bundle_type}}", bundle_type())
        })
        .collect();

    for url in &resolved {
        tracing::info!(url = %url, version = %current_version, "updater: 即将请求更新清单");
        crate::emit_proxy_log(&format!("[updater] GET {url}"));
    }

    Ok(resolved)
}

/// 与 tauri-plugin-updater 的 `updater_os()` 保持一致。注意 macOS 返回 `darwin`。
fn updater_os() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}

/// 与 tauri-plugin-updater 的 `updater_arch()` 保持一致。
fn updater_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86") {
        "i686"
    } else if cfg!(target_arch = "arm") {
        "armv7"
    } else {
        "unknown"
    }
}

/// 与 tauri-plugin-updater 的 `Installer::name()` 保持一致（Windows 用 NSIS 安装包）。
fn bundle_type() -> &'static str {
    if cfg!(target_os = "windows") {
        "nsis"
    } else if cfg!(target_os = "macos") {
        "dmg"
    } else {
        "unknown"
    }
}

/// 检测操作系统版本字符串。Windows 走 RtlGetVersion(获取真实 Build 号,
/// 不受 manifest compatibility 干扰)区分 Win10/Win11;其他平台用 cfg! 给出稳定描述。
fn detect_os_version() -> String {
    #[cfg(target_os = "windows")]
    {
        windows_os_version()
    }
    #[cfg(target_os = "macos")]
    {
        "macOS".to_string()
    }
    #[cfg(target_os = "linux")]
    {
        "Linux".to_string()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        "Unknown".to_string()
    }
}

#[cfg(target_os = "windows")]
fn windows_os_version() -> String {
    // OSVERSIONINFOEXW 真实反映运行中系统的版本,不受可执行文件 manifest 中
    // supportedOS 列表的 compatibility 行为影响(那个 GetVersionEx 会撒谎)。
    #[repr(C)]
    struct OsVersionInfoExW {
        dw_os_version_info_size: u32,
        dw_major_version: u32,
        dw_minor_version: u32,
        dw_build_number: u32,
        dw_platform_id: u32,
        sz_version_string: [u16; 128],
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn RtlGetVersion(info: *mut OsVersionInfoExW) -> i32;
    }

    unsafe {
        let mut info: OsVersionInfoExW = std::mem::zeroed();
        info.dw_os_version_info_size = std::mem::size_of::<OsVersionInfoExW>() as u32;
        let status = RtlGetVersion(&mut info);
        if status != 0 {
            return "Windows".to_string();
        }

        // NT 10.0 同时覆盖 Win10 与 Win11;以 Build 号区分:>= 22000 即 Win11。
        let friendly = match (info.dw_major_version, info.dw_minor_version) {
            (10, 0) => {
                if info.dw_build_number >= 22000 {
                    "Windows 11"
                } else {
                    "Windows 10"
                }
            }
            (6, 3) => "Windows 8.1",
            (6, 2) => "Windows 8",
            (6, 1) => "Windows 7",
            _ => "Windows",
        };

        // sz_version_string 是 UTF-16 缓冲区,截断到第一个 NUL 后转 String。
        let nul = info
            .sz_version_string
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.sz_version_string.len());
        let extra = String::from_utf16_lossy(&info.sz_version_string[..nul]);
        if extra.is_empty() {
            friendly.to_string()
        } else {
            format!("{} ({})", friendly, extra)
        }
    }
}

// ======================== Clipboard Commands ========================

#[tauri::command]
pub fn read_clipboard() -> Result<String, String> {
    let mut ctx = ClipboardContext::new().map_err(|e| e.to_string())?;
    ctx.get_contents().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn write_clipboard(text: String) -> Result<(), String> {
    let mut ctx = ClipboardContext::new().map_err(|e| e.to_string())?;
    ctx.set_contents(text).map_err(|e| e.to_string())
}

// ======================== ProxyKey Commands ========================

#[tauri::command]
pub fn create_proxy_key(
    name: String,
    provider_key_id: Option<String>,
    model: Option<String>,
    api_type: Option<String>,
) -> Result<ProxyKey, String> {
    let provider_key_id = provider_key_id.filter(|s| !s.trim().is_empty());
    let model = model.filter(|s| !s.trim().is_empty());
    let provider_key_id = provider_key_id.ok_or_else(|| "服务商令牌为必填项".to_string())?;
    let model = model.ok_or_else(|| "模型为必填项".to_string())?;
    // 'chat'=OpenAI 兼容（默认），'anthropic'=Anthropic，'responses'=Responses
    let api_type = api_type.unwrap_or_else(|| "chat".to_string());

    crate::repositories::create_proxy_key(&name, Some(&provider_key_id), Some(&model), &api_type)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_all_proxy_keys() -> Result<Vec<ProxyKey>, String> {
    crate::repositories::get_all_proxy_keys()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_proxy_key(
    id: String,
    name: String,
    provider_key_id: Option<String>,
    model: Option<String>,
    api_type: Option<String>,
) -> Result<(), String> {
    let provider_key_id = provider_key_id.filter(|s| !s.trim().is_empty());
    let model = model.filter(|s| !s.trim().is_empty());
    let provider_key_id = provider_key_id.ok_or_else(|| "服务商令牌为必填项".to_string())?;
    let model = model.ok_or_else(|| "模型为必填项".to_string())?;
    // 'chat'=OpenAI 兼容（默认），'anthropic'=Anthropic，'responses'=Responses
    let api_type = api_type.unwrap_or_else(|| "chat".to_string());

    crate::repositories::update_proxy_key(
        &id,
        &name,
        Some(&provider_key_id),
        Some(&model),
        &api_type,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_proxy_key(id: String) -> Result<(), String> {
    crate::repositories::delete_proxy_key(&id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn toggle_proxy_key_active(id: String, is_active: bool) -> Result<(), String> {
    crate::repositories::toggle_proxy_key_active(&id, is_active)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn regenerate_proxy_key(id: String) -> Result<String, String> {
    crate::repositories::regenerate_proxy_key(&id)
        .map_err(|e| e.to_string())
}

// ======================== Stats Commands ========================

/// 获取当前是否开启匿名统计上报（默认 true）。
#[tauri::command]
pub fn get_stats_enabled() -> Result<bool, String> {
    crate::services::stats::is_reporting_enabled().map_err(|e| e.to_string())
}

/// 设置是否开启匿名统计上报。
#[tauri::command]
pub fn set_stats_enabled(enabled: bool) -> Result<(), String> {
    crate::services::stats::set_reporting_enabled(enabled).map_err(|e| e.to_string())
}

/// 立即尝试把本地缓冲事件批量上报到服务端（基于当前 config.server_url）。
/// 返回上传条数；服务端不可达时 uploaded=0、server_reachable=false。
#[tauri::command]
pub fn flush_stats_now() -> Result<crate::services::stats::FlushReport, String> {
    let server_url = crate::repositories::get_config()
        .map(|c| c.server_url)
        .unwrap_or_else(|_| "https://api.ciyuanproxy.com/".to_string());
    crate::services::stats::flush_to_server(&server_url).map_err(|e| e.to_string())
}

/// 查询本地缓冲中尚未上报的事件条数（用于设置页展示）。
#[tauri::command]
pub fn get_stats_buffered_count() -> Result<i32, String> {
    crate::services::stats::buffered_count().map_err(|e| e.to_string())
}
