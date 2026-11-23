mod error;
mod crypto;
pub mod repositories;
pub mod services;
mod commands;
pub mod proxy;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{MenuBuilder, SubmenuBuilder},
    Manager, AppHandle, Emitter,
};
use tauri_plugin_clipboard::Clipboard;
use tracing::field::{Field, Visit};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Registry};

/// 主 WebView 创建失败标记。
///
/// tauri-runtime-wry 在创建 webview 失败时只做 `log::error!`（见其
/// `Message::CreateWebview` 分支），不会把错误向上抛出：事件循环会继续运行、
/// `WebviewWindow` 也会被注册，但原生 webview 并不存在。表现就是"窗口闪退，
/// 但进程和代理线程仍在后台占用监听端口"，导致用户无法重新启动代理。
///
/// 这里通过日志捕获该致命错误，在 `setup` 中检测到后主动退出进程。
static WEBVIEW_CREATE_FAILED: AtomicBool = AtomicBool::new(false);

/// 判断日志消息是否属于"主 WebView 创建失败"这类致命错误。
fn is_fatal_webview_error(message: &str) -> bool {
    message.contains("failed to create webview")
        || message.contains("Could not find the webview runtime")
}

/// 从 tracing 事件中提取 `message` 字段并识别致命 webview 错误。
#[derive(Default)]
struct WebviewErrorVisitor {
    fatal: bool,
}

impl Visit for WebviewErrorVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" && is_fatal_webview_error(&format!("{:?}", value)) {
            self.fatal = true;
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" && is_fatal_webview_error(value) {
            self.fatal = true;
        }
    }
}

/// 监听日志、标记致命 webview 创建失败的 tracing Layer。
struct WebviewHealthLayer;

impl<S: tracing::Subscriber> Layer<S> for WebviewHealthLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = WebviewErrorVisitor::default();
        event.record(&mut visitor);
        if visitor.fatal {
            WEBVIEW_CREATE_FAILED.store(true, Ordering::SeqCst);
        }
    }
}

/// 日志时间戳格式器：使用本机本地时区输出 RFC3339，
/// 例如 `2026-08-23T23:49:55+08:00`。避免之前 UTC `Z` 时间戳
/// 与肉眼本地时间对不上的问题。
struct LocalTime;

impl FormatTime for LocalTime {
    fn format_time(&self, w: &mut fmt::format::Writer<'_>) -> std::fmt::Result {
        let now = chrono::Local::now();
        write!(w, "{}", now.format("%Y-%m-%dT%H:%M:%S%:z"))
    }
}

static APP_HANDLE: once_cell::sync::Lazy<Arc<Mutex<Option<AppHandle>>>> = 
    once_cell::sync::Lazy::new(|| Arc::new(Mutex::new(None)));

/// 保留日志目录的 WorkerGuard，避免 non-blocking writer 的后台线程被提前 drop。
/// guard 在进程退出时自然释放（tauri 启动后一直存活）。
static LOG_GUARD: once_cell::sync::Lazy<Mutex<Option<WorkerGuard>>> = 
    once_cell::sync::Lazy::new(|| Mutex::new(None));

/// 获取后端日志目录：
/// - Windows: `%APPDATA%\com.ciyuanproxy.desktop\logs`
/// - 其他平台: `dirs::data_dir()/com.ciyuanproxy.desktop/logs`
pub fn get_log_dir() -> PathBuf {
    let base = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.ciyuanproxy.desktop")
        .join("logs");
    std::fs::create_dir_all(&base).ok();
    base
}

/// 把字符串日志级别解析为 EnvFilter 兼容的指令。
/// 支持：trace / debug / info / warn / error；其它值回退到 info。
fn parse_log_level_filter(level: &str) -> &'static str {
    match level.to_lowercase().as_str() {
        "trace" => "trace",
        "debug" => "debug",
        "warn" | "warning" => "warn",
        "error" => "error",
        _ => "info",
    }
}

/// 初始化文件日志：同时输出到 stdout 和按日滚动的日志文件。
/// 由 `run()` 在启动早期调用一次。多次调用是 no-op（依靠 guard 是否已被设置判断）。
pub fn init_logger(level: &str) {
    // 防止多次初始化（比如测试场景下重复调用 run()）。
    if let Ok(guard) = LOG_GUARD.lock() {
        if guard.is_some() {
            return;
        }
    }

    let log_dir = get_log_dir();
    // 文件名形如 `app.YYYY-MM-DD.log`，tracing-appender 会在跨天时自动 rollover。
    let file_appender = tracing_appender::rolling::daily(&log_dir, "app.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = match EnvFilter::try_from_default_env() {
        Ok(filter) => filter,
        Err(_) => EnvFilter::new("info").add_directive(
            format!("ciyuan_proxy={}", parse_log_level_filter(level))
                .parse()
                .unwrap(),
        ),
    };

    let file_layer = fmt::layer()
        .with_timer(LocalTime)
        .with_writer(file_writer)
        .with_ansi(false)
        .with_target(false);

    // stdout 同步输出仅在 dev 调试时使用；发布版本避免污染用户终端。
    // 这里仍然输出，便于 `tauri dev` 直接看到日志。
    let stdout_layer = fmt::layer()
        .with_timer(LocalTime)
        .with_writer(std::io::stdout)
        .with_ansi(true)
        .with_target(false);

    let _ = Registry::default()
        .with(env_filter)
        .with(WebviewHealthLayer)
        .with(file_layer)
        .with(stdout_layer)
        .try_init();

    if let Ok(mut g) = LOG_GUARD.lock() {
        *g = Some(guard);
    }

    tracing::info!(
        log_dir = %log_dir.display(),
        level = %level,
        "backend logger initialized"
    );
}

pub fn emit_proxy_log(message: &str) {
    if let Ok(mutex) = APP_HANDLE.lock() {
        if let Some(app) = &*mutex {
            let _ = app.emit("proxy_log", message).unwrap_or(());
        }
    }
}

fn get_db_path() -> PathBuf {
    get_db_path_for_stats()
}

/// 公开的 DB 路径解析：供 main.rs 的卸载钩子复用，避免复制逻辑。
pub fn get_db_path_for_stats() -> PathBuf {
    let app_data = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.ciyuanproxy.desktop")
        .join("data");

    std::fs::create_dir_all(&app_data).ok();
    app_data.join("data.db")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let db_path = get_db_path();

    // 先以兜底 info 级别初始化日志器，确保启动流程均被记录。
    init_logger("info");

    tracing::info!(path = %db_path.display(), "database path resolved");

    // Initialize database
    repositories::init_db(db_path.to_str().unwrap())
        .expect("Failed to initialize database");

    // Initialize config with default values
    repositories::init_config()
        .expect("Failed to initialize config");

    // 统计：进程启动埋点 + 首次安装识别。
    // 注意 argv 可能在 main 里被 --report-uninstall 短路，但 run() 这里仍先识别 install_state；
    // 卸载分支在 main.rs 处理。
    let client_version = env!("CARGO_PKG_VERSION").to_string();
    if let Err(e) = services::stats::on_app_launch(&client_version) {
        tracing::warn!(error = %e, "stats: failed to record app_launch");
    } else {
        tracing::info!(version = %client_version, "stats: app launch recorded");
    }

    // 读取用户持久化的日志级别，再次设置（init_logger 内部做了幂等处理，
    // 第一次调用真正生效；这里读取 config 只是为了记录信息并确保文件存在）。
    let stored_log_level = repositories::get_config()
        .map(|c| c.log_level)
        .unwrap_or_else(|_| "info".to_string());
    tracing::info!(level = %stored_log_level, "log level from config");

    // Set default encryption key (in production, derive from user password)
    let default_key = [0u8; 32];
    crypto::set_master_key(&default_key);

    // Check auto_start config and start proxy server if enabled
    tracing::info!("starting proxy server automatically");
    std::thread::spawn(|| {
        match crate::services::proxy::start_proxy_server() {
            Ok(()) => {
                // 统计：代理服务成功启动
                let port = crate::repositories::get_config()
                    .map(|c| c.listen_port)
                    .unwrap_or(0);
                crate::services::stats::record_event(
                    "proxy_started",
                    serde_json::json!({ "port": port }),
                );
            }
            Err(e) => {
                tracing::error!(error = %e, "proxy server error");
                crate::services::stats::record_event(
                    "proxy_started_failed",
                    serde_json::json!({ "error": e.to_string() }),
                );
            }
        }
    });

    // 统计：启动后台 reporter 任务（每 5 分钟尝试批量上报一次）
    std::thread::spawn(|| {
        crate::services::stats::start_reporter_loop();
    });
    
    #[cfg(desktop)]
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // 二次启动时把已有实例的主窗口拉到前台，避免重复进程占用代理端口。
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard::init())
        .plugin(tauri_plugin_updater::Builder::new().build());
    #[cfg(not(desktop))]
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard::init());

    builder
        .setup(|app| {
            // 配置中的窗口在此闭包之前由 Tauri 创建。若 webview 创建失败，
            // tauri-runtime-wry 只记录日志、不抛错，事件循环会继续运行但没有窗口，
            // 代理线程也会一直存活并占用端口。这里检测到后主动退出，释放端口。
            if WEBVIEW_CREATE_FAILED.load(Ordering::SeqCst) {
                tracing::error!(
                    "main webview failed to create; exiting to release the proxy port"
                );
                crate::services::proxy::stop_proxy_server();
                app.handle().exit(1);
                return Ok(());
            }

            *APP_HANDLE.lock().unwrap() = Some(app.handle().clone());
            
            // Build Chinese menu using MenuBuilder
            let app_submenu = SubmenuBuilder::new(app, "词元代理")
                .id("app")
                .text("show", "显示主窗口")
                .text("tokens", "代理秘钥")
                .text("token-groups", "服务商秘钥")
                .separator()
                .text("providers", "服务商")
                .separator()
                .text("test-proxy", "代理测试")
                .text("logs", "日志")
                .text("settings", "设置")
                .separator()
                .text("quit", "退出")
                .build()?;
            
            let _menu = MenuBuilder::new(app)
                .item(&app_submenu)
                .build()?;

            // 不设置全局菜单,避免在窗口左上角显示菜单栏
            // app.set_menu(menu)?;

            // Handle menu events
            app.on_menu_event(|app_handle, event| {
                match event.id().0.as_str() {
                    "show" => {
                        if let Some(window) = app_handle.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "tokens" => {
                        if let Some(window) = app_handle.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                            let _ = window.eval("window.location.hash = '#/'");
                        }
                    }
                    "token-groups" => {
                        if let Some(window) = app_handle.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                            let _ = window.eval("window.location.hash = '#/token-groups'");
                        }
                    }
                    "providers" => {
                        if let Some(window) = app_handle.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                            let _ = window.eval("window.location.hash = '#/providers'");
                        }
                    }
                    "test-proxy" => {
                        if let Some(window) = app_handle.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                            let _ = window.eval("window.location.hash = '#/test-proxy'");
                        }
                    }
                    "logs" => {
                        if let Some(window) = app_handle.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                            let _ = window.eval("window.location.hash = '#/logs'");
                        }
                    }
                    "settings" => {
                        if let Some(window) = app_handle.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                            let _ = window.eval("window.location.hash = '#/settings'");
                        }
                    }
                    "quit" => {
                        app_handle.exit(0);
                    }
                    _ => {}
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::create_provider_key,
            commands::get_all_provider_keys,
            commands::update_provider_key,
            commands::delete_provider_key,
            commands::toggle_provider_key_active,
            commands::start_openai_browser_auth,
            commands::cancel_openai_browser_auth,
            commands::create_proxy_key,
            commands::get_all_proxy_keys,
            commands::update_proxy_key,
            commands::delete_proxy_key,
            commands::toggle_proxy_key_active,
            commands::regenerate_proxy_key,
            commands::start_proxy_server,
            commands::stop_proxy_server,
            commands::get_proxy_status,
            commands::get_config,
            commands::update_config,
            commands::apply_log_level,
            commands::init_build_env,
            commands::create_provider,
            commands::get_all_providers,
            commands::update_provider,
            commands::delete_provider,
            commands::toggle_provider_status,
            commands::fetch_remote_providers,
            commands::fetch_remote_provider_detail,
            commands::submit_feedback,
            commands::read_clipboard,
            commands::write_clipboard,
            commands::get_current_version,
            commands::get_tauri_version,
            commands::get_os_version,
            commands::log_updater_endpoints,
            commands::get_stats_enabled,
            commands::set_stats_enabled,
            commands::flush_stats_now,
            commands::get_stats_buffered_count,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}