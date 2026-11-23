// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::OnceLock;

/// 卸载钩子信号：检测到 --report-uninstall 参数后置 true，run() 会短路直接退出。
/// 卸载脚本（NSIS uninstall section / sign-installer.ps1）会先调用我们一次
/// `ciyuan-proxy.exe --report-uninstall`，进程快速写入 uninstall 事件后退出，
/// 不进入 GUI / proxy 启动流程。
static UNINSTALL_MODE: OnceLock<bool> = OnceLock::new();

pub fn is_uninstall_mode() -> bool {
    *UNINSTALL_MODE.get().unwrap_or(&false)
}

fn main() {
    // 提前解析 argv，识别 uninstall 钩子。
    let mut uninstall = false;
    for arg in std::env::args().skip(1) {
        if arg == "--report-uninstall" || arg == "--uninstall" {
            uninstall = true;
            break;
        }
    }
    let _ = UNINSTALL_MODE.set(uninstall);

    if uninstall {
        run_uninstall_hook();
        return;
    }

    ciyuan_proxy_lib::run()
}

/// 卸载钩子流程：
///   1) 初始化 logger
///   2) 初始化 DB（确保 install_state 存在，便于写入 uninstall 事件）
///   3) 写入 app_uninstall 事件
///   4) 尝试立即 flush 到服务端（同步阻塞，最多 8s 超时）
///   5) 退出进程
///
/// 注意：此路径不进入 run() 的完整启动（不会起 GUI / proxy）。
fn run_uninstall_hook() {
    use ciyuan_proxy_lib as lib;
    lib::init_logger("info");
    tracing::info!("uninstall hook triggered");

    let db_path = lib::get_db_path_for_stats();
    if let Err(e) = lib::repositories::init_db(db_path.to_str().unwrap_or("data.db")) {
        tracing::error!(error = %e, "uninstall hook: init_db failed");
    }
    if let Err(e) = lib::repositories::init_config() {
        tracing::error!(error = %e, "uninstall hook: init_config failed");
    }
    if let Err(e) = lib::services::stats::on_app_uninstall() {
        tracing::warn!(error = %e, "uninstall hook: on_app_uninstall failed");
    }

    // 尝试立即上报
    let server_url = lib::repositories::get_config()
        .map(|c| c.server_url)
        .unwrap_or_else(|_| "https://api.ciyuanproxy.com/".to_string());
    match lib::services::stats::flush_to_server(&server_url) {
        Ok(report) => {
            tracing::info!(
                uploaded = report.uploaded,
                server_reachable = report.server_reachable,
                "uninstall hook: flush result"
            );
        }
        Err(e) => {
            tracing::warn!(error = %e, "uninstall hook: flush error");
        }
    }
}