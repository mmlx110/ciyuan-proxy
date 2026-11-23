//! 客户端使用统计服务
//!
//! 数据流：
//!   1. 进程启动 / 关键事件触发 → 调用 `record_event()` 写入本地 SQLite 缓冲表
//!   2. 后台 `reporter` 任务每 5 分钟（或手动触发）批量 POST 到服务端
//!   3. 成功上报的事件立即 DELETE（避免重复上报）
//!
//! 关闭上报：在设置页关闭 `reporting_enabled` 后，reporter 跳过 pull，但本地事件
//! 仍写入缓冲。下次开启时会一并补传。

use std::time::Duration;

use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

// ============================ 数据结构 ============================

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InstallState {
    pub install_id: String,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub launch_count: i32,
    pub client_version: String,
    pub os: String,
    pub reporting_enabled: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BufferedEvent {
    pub id: i64,
    pub event_type: String,
    pub payload: Value,
    pub occurred_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FlushReport {
    pub uploaded: i32,
    pub attempted: i32,
    pub server_reachable: bool,
}

// ============================ 安装/启动识别 ============================

/// 进程启动时调用一次：
///   - 首次启动（install_state 为空）：生成 install_id，写入 install 事件
///   - 后续启动：launch_count += 1，更新 last_seen_at，always 写一条 launch 事件
///
/// 返回最终的 InstallState，供调用方决定是否启动 reporter。
pub fn on_app_launch(client_version: &str) -> AppResult<InstallState> {
    let os = detect_os();
    let now = now_rfc3339_millis();

    let state = with_db(|conn| {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM install_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if exists == 0 {
            // 全新安装
            let install_id = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO install_state
                    (id, install_id, first_seen_at, last_seen_at, launch_count, client_version, os, reporting_enabled)
                 VALUES (1, ?1, ?2, ?2, 1, ?3, ?4, 1)",
                params![install_id, now, client_version, os],
            )?;
            insert_event(conn, "app_install", serde_json::json!({
                "version": client_version,
                "os": os,
            }))?;
            Ok(InstallState {
                install_id,
                first_seen_at: now.clone(),
                last_seen_at: now,
                launch_count: 1,
                client_version: client_version.to_string(),
                os,
                reporting_enabled: true,
            })
        } else {
            // 已存在：launch_count + 1，更新 last_seen / version / os
            conn.execute(
                "UPDATE install_state
                 SET launch_count = launch_count + 1,
                     last_seen_at = ?1,
                     client_version = ?2,
                     os = ?3
                 WHERE id = 1",
                params![now, client_version, os],
            )?;
            let st = load_install_state(conn)?;
            insert_event(conn, "app_launch", serde_json::json!({
                "version": client_version,
                "os": os,
                "launch_count": st.launch_count,
            }))?;
            Ok(st)
        }
    })?;

    Ok(state)
}

/// 卸载钩子调用：写入一条 app_uninstall 事件后立即尝试 flush 一次。
/// 安装包脚本（如 NSIS uninstall section）可通过 `ciyuan-proxy.exe --report-uninstall`
/// 触发，进程在 main 启动时短路检测 argv。
pub fn on_app_uninstall() -> AppResult<()> {
    with_db(|conn| {
        insert_event(conn, "app_uninstall", serde_json::json!({}))?;
        Ok(())
    })
}

pub fn get_install_state() -> AppResult<Option<InstallState>> {
    with_db(|conn| {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM install_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if exists == 0 {
            Ok(None)
        } else {
            Ok(Some(load_install_state(conn)?))
        }
    })
}

// ============================ 用户开关 ============================

pub fn is_reporting_enabled() -> AppResult<bool> {
    Ok(get_install_state()?.map(|s| s.reporting_enabled).unwrap_or(true))
}

pub fn set_reporting_enabled(enabled: bool) -> AppResult<()> {
    with_db(|conn| {
        // install_state 可能在极早期还未写入；此时不影响行为，下次启动会落到默认值。
        conn.execute(
            "UPDATE install_state SET reporting_enabled = ?1 WHERE id = 1",
            params![enabled as i32],
        )
        .ok();
        Ok(())
    })
}

// ============================ 事件埋点接口 ============================

/// 业务事件埋点（应用层随时调用）。失败不阻塞主流程，最多记一条 warn 日志。
pub fn record_event(event_type: &str, payload: Value) {
    if let Err(e) = with_db(|conn| insert_event(conn, event_type, payload)) {
        tracing::warn!(error = %e, event_type = %event_type, "stats: failed to record event");
    }
}

// ============================ 本地表查询 ============================

pub fn get_buffered_events(limit: i32) -> AppResult<Vec<BufferedEvent>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, event_type, payload_json, occurred_at FROM events_buffer
             WHERE reported = 0 ORDER BY id ASC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
                let payload_str: String = row.get(2)?;
                let payload: Value = serde_json::from_str(&payload_str).unwrap_or(Value::Null);
                Ok(BufferedEvent {
                    id: row.get(0)?,
                    event_type: row.get(1)?,
                    payload,
                    occurred_at: row.get(3)?,
                })
            })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(AppError::Database)?);
        }
        Ok(out)
    })
}

pub fn delete_events(ids: &[i64]) -> AppResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    with_db(|conn| {
        let placeholders = std::iter::repeat("?")
            .take(ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("DELETE FROM events_buffer WHERE id IN ({})", placeholders);
        let mut params_vec: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(ids.len());
        for id in ids {
            params_vec.push(id);
        }
        conn.execute(&sql, &params_vec[..])?;
        Ok(())
    })
}

pub fn buffered_count() -> AppResult<i32> {
    with_db(|conn| {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events_buffer WHERE reported = 0",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(n as i32)
    })
}

// ============================ 上报状态（用于指数退避）============================

#[derive(Debug, Default, Clone)]
pub(crate) struct ReportState {
    consecutive_failures: i32,
    last_success_at: Option<String>,
    next_retry_at: Option<String>,
}

pub(crate) fn load_report_state() -> AppResult<ReportState> {
    with_db(|conn| {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM stats_report_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if exists == 0 {
            return Ok(ReportState::default());
        }
        let r = conn.query_row(
            "SELECT consecutive_failures, last_success_at, next_retry_at
             FROM stats_report_state WHERE id = 1",
            [],
            |row| {
                Ok(ReportState {
                    consecutive_failures: row.get::<_, i32>(0)?,
                    last_success_at: row.get::<_, Option<String>>(1)?,
                    next_retry_at: row.get::<_, Option<String>>(2)?,
                })
            },
        );
        match r {
            Ok(s) => Ok(s),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(ReportState::default()),
            Err(e) => Err(AppError::Database(e)),
        }
    })
}

pub fn mark_report_success() -> AppResult<()> {
    let now = now_rfc3339_millis();
    with_db(|conn| {
        conn.execute(
            "INSERT INTO stats_report_state (id, last_reported_at, last_success_at, consecutive_failures, next_retry_at)
             VALUES (1, ?1, ?1, 0, NULL)
             ON CONFLICT(id) DO UPDATE SET
                last_reported_at = excluded.last_reported_at,
                last_success_at  = excluded.last_success_at,
                consecutive_failures = 0,
                next_retry_at = NULL",
            params![now],
        )?;
        Ok(())
    })
}

pub fn mark_report_failure() -> AppResult<()> {
    with_db(|conn| {
        let cur: i32 = conn
            .query_row(
                "SELECT consecutive_failures FROM stats_report_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let next_failures = cur + 1;
        // 指数退避：1m, 2m, 5m, 15m, 60m, 60m ...
        let backoff_secs = match next_failures {
            1 => 60,
            2 => 120,
            3 => 300,
            4 => 900,
            _ => 3600,
        };
        let next_retry_at = (Utc::now() + chrono::Duration::seconds(backoff_secs)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let now = now_rfc3339_millis();
        conn.execute(
            "INSERT INTO stats_report_state (id, last_reported_at, consecutive_failures, next_retry_at)
             VALUES (1, ?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET
                last_reported_at = excluded.last_reported_at,
                consecutive_failures = excluded.consecutive_failures,
                next_retry_at = excluded.next_retry_at",
            params![now, next_failures, next_retry_at],
        )?;
        Ok(())
    })
}

pub fn should_retry_now() -> AppResult<bool> {
    let s = load_report_state()?;
    match s.next_retry_at {
        None => Ok(true),
        Some(iso) => {
            // RFC3339 解析失败时默认允许重试，避免永久卡住
            let parsed = chrono::DateTime::parse_from_rfc3339(&iso).ok();
            Ok(match parsed {
                Some(t) => Utc::now() >= t.with_timezone(&Utc),
                None => true,
            })
        }
    }
}

// ============================ 实际 HTTP 上报 ============================

/// 单次 flush：拉取本地缓冲 → POST 服务端 → 成功后 DELETE 已上报行。
/// 返回 FlushReport 给前端展示。
pub fn flush_to_server(server_url: &str) -> AppResult<FlushReport> {
    if !is_reporting_enabled()? {
        return Ok(FlushReport {
            uploaded: 0,
            attempted: 0,
            server_reachable: false,
        });
    }
    let events = get_buffered_events(200)?;
    if events.is_empty() {
        return Ok(FlushReport {
            uploaded: 0,
            attempted: 0,
            server_reachable: true,
        });
    }
    let state = get_install_state()?.ok_or_else(|| {
        AppError::Database(rusqlite::Error::InvalidQuery)
    })?;

    let url = build_stats_url(server_url);
    let body = serde_json::json!({
        "installId": state.install_id,
        "clientVersion": state.client_version,
        "os": state.os,
        "firstSeenAt": state.first_seen_at,
        "lastSeenAt": state.last_seen_at,
        "launchCount": state.launch_count,
        "events": events.iter().map(|e| serde_json::json!({
            "type": e.event_type,
            "ts": e.occurred_at,
            "payload": e.payload,
        })).collect::<Vec<_>>(),
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!(
            "ciyuan-proxy/",
            env!("CARGO_PKG_VERSION"),
            " (stats reporter)"
        ))
        .build()
        .map_err(|e| AppError::Validation(format!("reqwest build: {e}")))?;

    let attempted = events.len() as i32;
    match client.post(&url).json(&body).send() {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                let ids: Vec<i64> = events.iter().map(|e| e.id).collect();
                delete_events(&ids)?;
                mark_report_success()?;
                Ok(FlushReport {
                    uploaded: attempted,
                    attempted,
                    server_reachable: true,
                })
            } else {
                tracing::warn!(%status, "stats: server returned non-success");
                mark_report_failure()?;
                let ids: Vec<i64> = events.iter().map(|e| e.id).collect();
                bump_fail_count_and_drop(&ids)?;
                Ok(FlushReport {
                    uploaded: 0,
                    attempted,
                    server_reachable: true,
                })
            }
        }
        Err(e) => {
            // 网络层失败（超时 / DNS / 连接拒绝）：服务端可能根本没收到这批事件，
            // 不能 bump fail_count（会误丢）；只更新 stats_report_state 的退避时间。
            tracing::warn!(error = %e, "stats: HTTP send failed");
            mark_report_failure()?;
            Ok(FlushReport {
                uploaded: 0,
                attempted,
                server_reachable: false,
            })
        }
    }
}

/// 服务端 4xx/5xx 时调用：把这批事件的 fail_count += 1；
/// 超过 {@link MAX_EVENT_FAIL_COUNT} 的事件直接 DELETE 丢弃，
/// 防止一条坏事件永远阻塞 reporter 队列。
const MAX_EVENT_FAIL_COUNT: i32 = 10;

fn bump_fail_count_and_drop(ids: &[i64]) -> AppResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    with_db(|conn| {
        let tx = conn.unchecked_transaction()?;
        // 1) fail_count += 1
        for id in ids {
            tx.execute(
                "UPDATE events_buffer SET fail_count = fail_count + 1 WHERE id = ?1",
                params![id],
            )?;
        }
        // 2) 超阈值的直接删
        let placeholders = std::iter::repeat("?")
            .take(ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "DELETE FROM events_buffer WHERE fail_count > ?1 AND id IN ({})",
            placeholders
        );
        let mut p: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(ids.len() + 1);
        p.push(&MAX_EVENT_FAIL_COUNT);
        for id in ids {
            p.push(id);
        }
        let dropped = tx.execute(&sql, &p[..])?;
        if dropped > 0 {
            tracing::warn!(dropped = dropped, "stats: dropped events that exceeded max fail count");
        }
        tx.commit()?;
        Ok(())
    })
}

// ============================ helpers ============================

/// 后台 reporter 主循环：由 lib.rs 在启动时 spawn 到 std::thread。
///
/// 时序：
/// 1) 启动后等 2 秒（让 DB / on_app_launch 落盘）→ 立刻 flush 一次（首次 flush）
///   2) 之后每 30 分钟（PERIOD）唤醒一次，按以下条件触发实际 flush：
///      - 用户开启了 reporting_enabled
///      - 距离上次 flush >= 30 分钟
///      - 距离 next_retry_at 已到（指数退避）
///
/// 阻塞 + sleep 实现，避免引入 tokio runtime；flush_to_server 内部使用
/// reqwest blocking client，单次调用最多 15 秒超时。
pub fn start_reporter_loop() {
    use std::time::Duration;

    const PERIOD: Duration = Duration::from_secs(1800); // 30 分钟
    const TICK: Duration = Duration::from_secs(5);

    // 启动后给 2 秒缓冲（让 init_db + on_app_launch + proxy start 落盘），
    // 之后立刻尝试第一次 flush，避免用户要等 5 分钟才看到数据进服务端。
    let mut startup_done = false;

    loop {
        if !startup_done {
            std::thread::sleep(Duration::from_secs(2));
            startup_done = true;
            perform_periodic_flush();
            continue;
        }

        std::thread::sleep(TICK);

        // 退避检查
        if !should_retry_now().unwrap_or(true) {
            continue;
        }

        // 距上次 flush 至少 30 分钟
        if !passes_period_check(PERIOD) {
            continue;
        }

        perform_periodic_flush();
    }
}

/// 实际执行一次 flush（包含用户开关、HTTP 调用、状态标记）
fn perform_periodic_flush() {
    if !is_reporting_enabled().unwrap_or(true) {
        return;
    }

    let server_url = crate::repositories::get_config()
        .map(|c| c.server_url)
        .unwrap_or_else(|_| "https://api.ciyuanproxy.com/".to_string());

    match flush_to_server(&server_url) {
        Ok(report) => {
            if report.attempted > 0 || report.server_reachable {
                tracing::info!(
                    uploaded = report.uploaded,
                    attempted = report.attempted,
                    reachable = report.server_reachable,
                    "stats: periodic flush"
                );
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "stats: periodic flush error");
        }
    }
    mark_last_flush_now();
}

/// 距上次 flush 至少 period 才返回 true。首次返回 true（无记录）。
fn passes_period_check(period: std::time::Duration) -> bool {
    let now_secs = chrono::Utc::now().timestamp();
    let last_secs: Option<i64> = with_db(|conn| {
        let r = conn.query_row(
            "SELECT last_reported_at FROM stats_report_state WHERE id = 1",
            [],
            |row| row.get::<_, Option<String>>(0),
        );
        match r {
            Ok(Some(iso)) => Ok(chrono::DateTime::parse_from_rfc3339(&iso)
                .ok()
                .map(|t| t.timestamp())),
            Ok(None) => Ok(None),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(crate::error::AppError::Database(e)),
        }
    })
    .ok()
    .flatten();
    match last_secs {
        None => true,
        Some(last) => now_secs - last >= period.as_secs() as i64,
    }
}

fn mark_last_flush_now() {
    with_db(|conn| {
        let _ = conn.execute(
            "INSERT INTO stats_report_state (id, last_reported_at)
             VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET last_reported_at = excluded.last_reported_at",
            rusqlite::params![now_rfc3339_millis()],
        );
        Ok(())
    })
    .ok();
}

fn build_stats_url(server_url: &str) -> String {
    let base = server_url.trim_end_matches('/');
    format!("{}/stats/web/client/events", base)
}

/// RFC3339 时间字符串，精度抹平到毫秒（3 位小数）。
///
/// Jackson 默认的 Date 反序列化只支持到毫秒（RFC3339 形式如 `2026-08-30T02:03:16.912+00:00`），
/// 而 chrono `Utc::now().to_rfc3339()` 输出的是 7 位小数（纳秒精度），
/// 会触发 "Unparseable date" 错误。本函数把纳秒截断到毫秒，
/// 并保留时区偏移。例：2026-08-30T02:03:16.912632600+00:00 → 2026-08-30T02:03:16.912+00:00
pub(crate) fn now_rfc3339_millis() -> String {
    let n = chrono::Utc::now();
    // to_rfc3339_opts(SecondsFormat::Millis, true) 自动输出毫秒精度
    n.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn detect_os() -> String {
    if cfg!(target_os = "windows") {
        format!("windows-{}", std::env::consts::ARCH)
    } else if cfg!(target_os = "macos") {
        format!("macos-{}", std::env::consts::ARCH)
    } else if cfg!(target_os = "linux") {
        format!("linux-{}", std::env::consts::ARCH)
    } else {
        "unknown".to_string()
    }
}

fn insert_event(conn: &Connection, event_type: &str, payload: Value) -> AppResult<()> {
    let now = now_rfc3339_millis();
    let payload_str = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
    conn.execute(
        "INSERT INTO events_buffer (event_type, payload_json, occurred_at, reported)
         VALUES (?1, ?2, ?3, 0)",
        params![event_type, payload_str, now],
    )?;
    Ok(())
}

fn load_install_state(conn: &Connection) -> AppResult<InstallState> {
    conn.query_row(
        "SELECT install_id, first_seen_at, last_seen_at, launch_count, client_version, os, reporting_enabled
         FROM install_state WHERE id = 1",
        [],
        |row| {
            Ok(InstallState {
                install_id: row.get(0)?,
                first_seen_at: row.get(1)?,
                last_seen_at: row.get(2)?,
                launch_count: row.get(3)?,
                client_version: row.get(4)?,
                os: row.get(5)?,
                reporting_enabled: row.get::<_, i32>(6)? != 0,
            })
        },
    )
    .map_err(AppError::Database)
}

fn with_db<F, T>(f: F) -> AppResult<T>
where
    F: FnOnce(&Connection) -> AppResult<T>,
{
    // 直接复用 repositories 的全局 DB 句柄（避免再开一条连接导致锁竞争）
    let guard = crate::repositories::raw_db_conn()?;
    let conn = guard.as_ref().expect("checked in raw_db_conn");
    f(conn)
}