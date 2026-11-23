use rusqlite::{Connection, params};
use std::sync::Mutex;
use once_cell::sync::Lazy;
use uuid::Uuid;
use chrono::Utc;

use crate::error::{AppError, AppResult};
use crate::crypto;

static DB: Lazy<Mutex<Option<Connection>>> = Lazy::new(|| Mutex::new(None));

/// 初始化数据库：按"数据库表结构调整执行计划"创建最终 3 张表
/// 并对老库（含 token_groups/tokens/models 旧表）做一次性迁移。
pub fn init_db(path: &str) -> AppResult<()> {
    let conn = Connection::open(path)?;

    // ============================ 1. providers 表（含 models 字段）============================
    conn.execute(
        "CREATE TABLE IF NOT EXISTS providers (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            api_url TEXT NOT NULL,
            api_anthropic_url TEXT,
            api_responses_url TEXT,
            description TEXT,
            models TEXT NOT NULL DEFAULT '',
            type INTEGER NOT NULL DEFAULT 2,
            website TEXT,
            tags TEXT NOT NULL DEFAULT '[]',
            pricing TEXT,
            status INTEGER NOT NULL DEFAULT 0,
            sort INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            auth_modes TEXT NOT NULL DEFAULT '[\"api_key\"]'
        )",
        [],
    )?;

    // 老库迁移：base_url -> api_url（已重命名则忽略）
    conn.execute("ALTER TABLE providers RENAME COLUMN base_url TO api_url", []).ok();
    // 老库迁移：丢弃已合并到 api_url 的 api_version 列（已删除则忽略）
    conn.execute("ALTER TABLE providers DROP COLUMN api_version", []).ok();
    // 老库迁移：为旧 providers 增加 models 列（已存在则忽略）
    conn.execute(
        "ALTER TABLE providers ADD COLUMN models TEXT NOT NULL DEFAULT ''",
        [],
    ).ok();
    // 老库迁移：为旧 providers 增加 api_anthropic_url 列（已存在则忽略），用于支持 Anthropic 协议 URL
    conn.execute(
        "ALTER TABLE providers ADD COLUMN api_anthropic_url TEXT",
        [],
    ).ok();
    // 老库迁移：为旧 providers 增加 api_responses_url 列（已存在则忽略），用于 Responses 兼容模式下覆盖 api_url
    conn.execute(
        "ALTER TABLE providers ADD COLUMN api_responses_url TEXT",
        [],
    ).ok();
    // 老库迁移：为旧 providers 增加 auth_modes 列（已存在则忽略），
    // 记录该服务商支持的授权方式：["api_key"] / ["browser"] / 两者都支持
    conn.execute(
        "ALTER TABLE providers ADD COLUMN auth_modes TEXT NOT NULL DEFAULT '[\"api_key\"]'",
        [],
    ).ok();

    // 老库迁移：从旧 models 表聚合回填到 providers.models（旧表不存在则跳过）
    let has_old_models: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='models'",
        [],
        |row| row.get(0),
    ).unwrap_or(0);
    if has_old_models > 0 {
        conn.execute(
            "UPDATE providers
             SET models = COALESCE(
                 (SELECT GROUP_CONCAT(name, ',') FROM models WHERE models.provider_id = providers.id),
                 ''
             )",
            [],
        ).ok();
    }

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_providers_type ON providers(type)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_providers_status ON providers(status)",
        [],
    )?;

    // ============================ 2. providers_keys 表 ============================
    // 原 token_groups → providers_keys；字段 provider → provider_id
    // auth_type：'api_key'=手动填写密钥，'browser'=浏览器授权（OAuth2）换取的令牌
    // encrypted_refresh_token：浏览器授权的 refresh_token（AES-256-GCM 加密）
    // auth_meta：授权元数据 JSON（账号 ID / 过期时间 / 登录邮箱）
    conn.execute(
        "CREATE TABLE IF NOT EXISTS providers_keys (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            encrypted_api_key TEXT NOT NULL,
            is_active INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            auth_type TEXT NOT NULL DEFAULT 'api_key',
            encrypted_refresh_token TEXT,
            auth_meta TEXT,
            FOREIGN KEY(provider_id) REFERENCES providers(id)
        )",
        [],
    )?;

    // 老库迁移：为旧 providers_keys 补充授权相关列（已存在则忽略）
    conn.execute(
        "ALTER TABLE providers_keys ADD COLUMN auth_type TEXT NOT NULL DEFAULT 'api_key'",
        [],
    ).ok();
    conn.execute(
        "ALTER TABLE providers_keys ADD COLUMN encrypted_refresh_token TEXT",
        [],
    ).ok();
    conn.execute(
        "ALTER TABLE providers_keys ADD COLUMN auth_meta TEXT",
        [],
    ).ok();

    // 老库迁移：从旧 token_groups 迁移数据到 providers_keys（旧表存在且 providers_keys 为空时执行）
    let has_old_tg: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='token_groups'",
        [],
        |row| row.get(0),
    ).unwrap_or(0);
    if has_old_tg > 0 {
        let new_pk_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM providers_keys",
            [],
            |row| row.get(0),
        ).unwrap_or(0);
        if new_pk_count == 0 {
            conn.execute(
                "INSERT INTO providers_keys (id, name, provider_id, encrypted_api_key, is_active, created_at)
                 SELECT id, name, provider, encrypted_api_key, is_active, created_at
                 FROM token_groups",
                [],
            ).ok();
        }
    }

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_providers_keys_provider_id ON providers_keys(provider_id)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_providers_keys_is_active ON providers_keys(is_active)",
        [],
    )?;

    // ============================ 3. proxy_keys 表 ============================
    // 原 tokens → proxy_keys；列 token → 更名为 key；保留 model（TEXT 可空）；token_group_id → provider_key_id
    // api_type：调用目标协议（'chat'=OpenAI 兼容，'anthropic'=Anthropic，'responses'=Responses），默认 'chat'。
    conn.execute(
        "CREATE TABLE IF NOT EXISTS proxy_keys (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            key TEXT NOT NULL UNIQUE,
            provider_key_id TEXT,
            model TEXT,
            api_type TEXT NOT NULL DEFAULT 'chat',
            expires_at TEXT,
            is_active INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            FOREIGN KEY(provider_key_id) REFERENCES providers_keys(id)
        )",
        [],
    )?;

    // 历史库兼容 1：若仍存在旧列 token 但无 key，则把 token 列重命名为 key
    let has_token_col: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('proxy_keys') WHERE name = 'token'",
        [],
        |row| row.get(0),
    ).unwrap_or(0);
    let has_key_col: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('proxy_keys') WHERE name = 'key'",
        [],
        |row| row.get(0),
    ).unwrap_or(0);
    if has_token_col > 0 && has_key_col == 0 {
        conn.execute("ALTER TABLE proxy_keys RENAME COLUMN token TO key", []).ok();
    }

    // 历史库兼容 2：若缺少 model 列则补上（旧表已创建过且没有该列的情况）
    conn.execute(
        "ALTER TABLE proxy_keys ADD COLUMN model TEXT",
        [],
    ).ok();

    // 历史库兼容：若缺少 api_type 列则补上，默认 'chat'（OpenAI 兼容）。
    conn.execute(
        "ALTER TABLE proxy_keys ADD COLUMN api_type TEXT NOT NULL DEFAULT 'chat'",
        [],
    ).ok();

    // 历史库兼容 3：把 provider_key_id 改为可空（早期版本该列被建成 NOT NULL，
    // 导致删除 Provider/ProviderKey 时无法把 proxy_keys 的引用置 NULL）。
    // SQLite 不支持直接 ALTER COLUMN ... DROP NOT NULL，按标准 12 步法重建表。
    let provider_key_id_notnull: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('proxy_keys')
         WHERE name = 'provider_key_id' AND \"notnull\" = 1",
        [],
        |row| row.get(0),
    ).unwrap_or(0);
    if provider_key_id_notnull > 0 {
        conn.execute("PRAGMA foreign_keys = OFF", []).ok();
        conn.execute(
            "CREATE TABLE proxy_keys_new (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                key TEXT NOT NULL UNIQUE,
                provider_key_id TEXT,
                model TEXT,
                api_type TEXT NOT NULL DEFAULT 'chat',
                expires_at TEXT,
                is_active INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL,
                FOREIGN KEY(provider_key_id) REFERENCES providers_keys(id)
            )",
            [],
        )?;
        conn.execute(
            "INSERT INTO proxy_keys_new
                (id, name, key, provider_key_id, model, api_type, expires_at, is_active, created_at)
             SELECT id, name, key, provider_key_id, model,
                CASE CAST(api_type AS TEXT)
                    WHEN '2' THEN 'anthropic'
                    WHEN '3' THEN 'responses'
                    WHEN 'anthropic' THEN 'anthropic'
                    WHEN 'responses' THEN 'responses'
                    ELSE 'chat'
                END,
                expires_at, is_active, created_at
             FROM proxy_keys",
            [],
        )?;
        conn.execute("DROP TABLE proxy_keys", [])?;
        conn.execute("ALTER TABLE proxy_keys_new RENAME TO proxy_keys", [])?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_proxy_keys_provider_key_id ON proxy_keys(provider_key_id)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_proxy_keys_is_active ON proxy_keys(is_active)",
            [],
        )?;
        conn.execute("PRAGMA foreign_keys = ON", []).ok();
    }

    // 历史库兼容 4：api_type 由数字（1=chat，2=anthropic，3=responses）改为字符串
    // （'chat' / 'anthropic' / 'responses'）。SQLite 动态类型虽然允许 TEXT 写进
    // INTEGER 列，但列声明与 DEFAULT 仍是数字，会让新插入行落回 1；按 12 步法重建表，
    // 顺带把存量 1/2/3 映射为对应字符串。
    let api_type_decl: String = conn.query_row(
        "SELECT type FROM pragma_table_info('proxy_keys') WHERE name = 'api_type'",
        [],
        |row| row.get(0),
    ).unwrap_or_default();
    if !api_type_decl.eq_ignore_ascii_case("TEXT") {
        conn.execute("PRAGMA foreign_keys = OFF", []).ok();
        conn.execute(
            "CREATE TABLE proxy_keys_new (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                key TEXT NOT NULL UNIQUE,
                provider_key_id TEXT,
                model TEXT,
                api_type TEXT NOT NULL DEFAULT 'chat',
                expires_at TEXT,
                is_active INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL,
                FOREIGN KEY(provider_key_id) REFERENCES providers_keys(id)
            )",
            [],
        )?;
        conn.execute(
            "INSERT INTO proxy_keys_new
                (id, name, key, provider_key_id, model, api_type, expires_at, is_active, created_at)
             SELECT id, name, key, provider_key_id, model,
                CASE CAST(api_type AS TEXT)
                    WHEN '2' THEN 'anthropic'
                    WHEN '3' THEN 'responses'
                    WHEN 'anthropic' THEN 'anthropic'
                    WHEN 'responses' THEN 'responses'
                    ELSE 'chat'
                END,
                expires_at, is_active, created_at
             FROM proxy_keys",
            [],
        )?;
        conn.execute("DROP TABLE proxy_keys", [])?;
        conn.execute("ALTER TABLE proxy_keys_new RENAME TO proxy_keys", [])?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_proxy_keys_provider_key_id ON proxy_keys(provider_key_id)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_proxy_keys_is_active ON proxy_keys(is_active)",
            [],
        )?;
        conn.execute("PRAGMA foreign_keys = ON", []).ok();
    }

    // 老库迁移：从旧 tokens 迁移数据到 proxy_keys（旧表存在且 proxy_keys 为空时执行）
    // 同时尝试从旧 models 表反查 model_name（若旧 models 表存在）。
    let has_old_tokens: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tokens'",
        [],
        |row| row.get(0),
    ).unwrap_or(0);
    if has_old_tokens > 0 {
        let new_px_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM proxy_keys",
            [],
            |row| row.get(0),
        ).unwrap_or(0);
        if new_px_count == 0 {
            let has_old_models: i64 = conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='models'",
                [],
                |row| row.get(0),
            ).unwrap_or(0);
            if has_old_models > 0 {
                conn.execute(
                    "INSERT INTO proxy_keys (id, name, key, provider_key_id, model, expires_at, is_active, created_at)
                     SELECT t.id, t.name, t.token, t.token_group_id, m.model_name, t.expires_at, t.is_active, t.created_at
                     FROM tokens t
                     LEFT JOIN models m ON t.model_id = m.id
                     WHERE t.token_group_id IS NOT NULL",
                    [],
                ).ok();
            } else {
                conn.execute(
                    "INSERT INTO proxy_keys (id, name, key, provider_key_id, expires_at, is_active, created_at)
                     SELECT id, name, token, token_group_id, expires_at, is_active, created_at
                     FROM tokens
                     WHERE token_group_id IS NOT NULL",
                    [],
                ).ok();
            }
        }
    }

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_proxy_keys_provider_key_id ON proxy_keys(provider_key_id)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_proxy_keys_is_active ON proxy_keys(is_active)",
        [],
    )?;

    // ============================ 4. 删除旧表 ============================
    // 校验通过后按 tokens → token_groups → models 顺序删除
    conn.execute("DROP TABLE IF EXISTS tokens", [])?;
    conn.execute("DROP TABLE IF EXISTS token_groups", [])?;
    conn.execute("DROP TABLE IF EXISTS models", [])?;

    // ============================ 5. config 表（保持不变）============================
    conn.execute(
        "CREATE TABLE IF NOT EXISTS config (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            listen_port INTEGER NOT NULL DEFAULT 20112,
            auto_start INTEGER NOT NULL DEFAULT 0,
            minimize_to_tray INTEGER NOT NULL DEFAULT 0,
            log_level TEXT NOT NULL DEFAULT 'info',
            server_url TEXT NOT NULL DEFAULT 'https://api.ciyuanproxy.com/',
            app_env TEXT NOT NULL DEFAULT 'prod'
        )",
        [],
    )?;
    conn.execute(
        "ALTER TABLE config ADD COLUMN server_url TEXT NOT NULL DEFAULT 'https://api.ciyuanproxy.com/'",
        [],
    ).ok();
    conn.execute(
        "ALTER TABLE config ADD COLUMN app_env TEXT NOT NULL DEFAULT 'prod'",
        [],
    ).ok();
    conn.execute(
        "INSERT OR IGNORE INTO config (id, listen_port, auto_start, minimize_to_tray, log_level, server_url, app_env)
         VALUES (1, 20112, 0, 0, 'info', 'https://api.ciyuanproxy.com/', 'prod')",
        [],
    )?;

    // ============================ 7. install_state 表（统计：客户端实例身份）============================
    // 单行表（id=1）：记录该客户端实例的 install_id、首次启动时间、累计启动次数、是否开启统计上报。
    // install_id 用于服务端去重（同一物理机/同一用户只算一个 install）；卸载时由安装包脚本
    // 调用 `--report-uninstall` 标记一次 uninstall 事件。
    conn.execute(
        "CREATE TABLE IF NOT EXISTS install_state (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            install_id TEXT NOT NULL,
            first_seen_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL,
            launch_count INTEGER NOT NULL DEFAULT 0,
            client_version TEXT NOT NULL,
            os TEXT NOT NULL,
            reporting_enabled INTEGER NOT NULL DEFAULT 1
        )",
        [],
    )?;

    // ============================ 8. events_buffer 表（统计：本地事件缓冲）============================
    // 所有埋点事件先写入本表，后台 reporter 任务每 5 分钟批量上报到服务端，成功后 DELETE。
    // 设计要点：
    //   - reported=0 表示待上报；reporter 拉取后通过 DELETE 移除（避免同事件重复上报）
    //   - payload_json 仅含维度信息，不携带任何密钥 / URL / 用户路径
    //   - occurred_at 使用 RFC3339
    //   - fail_count 累计上报失败次数；超过阈值则丢弃（避免坏事件永远阻塞队列）
    conn.execute(
        "CREATE TABLE IF NOT EXISTS events_buffer (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            event_type TEXT NOT NULL,
            payload_json TEXT NOT NULL DEFAULT '{}',
            occurred_at TEXT NOT NULL,
            reported INTEGER NOT NULL DEFAULT 0,
            fail_count INTEGER NOT NULL DEFAULT 0
        )",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_events_buffer_reported ON events_buffer(reported, occurred_at)",
        [],
    )?;
    // 老库迁移：补 fail_count 列（已存在则忽略）
    conn.execute(
        "ALTER TABLE events_buffer ADD COLUMN fail_count INTEGER NOT NULL DEFAULT 0",
        [],
    ).ok();

    // ============================ 9. stats_report_state 表（统计：上报游标）============================
    // 单行表（id=1）：记录上次上报时间、连续失败计数、下一可重试时间，用于指数退避。
    conn.execute(
        "CREATE TABLE IF NOT EXISTS stats_report_state (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            last_reported_at TEXT,
            last_success_at TEXT,
            consecutive_failures INTEGER NOT NULL DEFAULT 0,
            next_retry_at TEXT
        )",
        [],
    )?;

    // ============================ 6. 种子数据：providers（含 models 字段）============================
    // 全新数据库时插入预置服务商及模型（以逗号分隔写入 providers.models）
    let providers_count: i64 = conn.query_row("SELECT COUNT(*) FROM providers", [], |row| row.get(0))?;
    if providers_count == 0 {
        seed_providers(&conn)?;
    }

    // 老库升级：预置服务商在旧库里已存在（seed_providers 的 INSERT OR IGNORE 不生效），
    // 这里仅把仍是默认值的 auth_modes 升级为"支持浏览器授权"，
    // 避免覆盖用户手动改过的配置。
    conn.execute(
        "UPDATE providers SET auth_modes = '[\"api_key\",\"browser\"]'
         WHERE id = 'provider-openai' AND auth_modes = '[\"api_key\"]'",
        [],
    )?;

    let mut guard = DB.lock().unwrap();
    *guard = Some(conn);
    Ok(())
}

/// 向 providers 表插入预置服务商及模型种子数据。
/// models 字段使用英文逗号分隔多个模型名。
/// 每个厂商覆盖"当代全系 + 上一代主力"两档，既能用最新模型，
/// 也不会把仍在服役的上一代（GPT-5.6 / Claude Opus 5 / qwen3.7 / GLM-5.x）排除在外。
/// api_anthropic_url / api_responses_url 留空表示该厂商不支持对应协议，
/// 前端会据此过滤代理密钥的接口类型选项。
fn seed_providers(conn: &Connection) -> AppResult<()> {
    // (id, name, api_url, api_anthropic_url, api_responses_url, description, models, type, sort, website, tags, auth_modes)
    const SEEDS: &[(
        &str,
        &str,
        &str,
        Option<&str>,
        Option<&str>,
        &str,
        &str,
        i32,
        i32,
        &str,
        &str,
        &str,
    )] = &[
        (
            "provider-openai",
            "OpenAI",
            "https://api.openai.com/v1",
            None,
            Some("https://api.openai.com/v1"),
            "GPT 系列官方直连，行业标杆",
            "gpt-6-astra,gpt-6.1-sol,gpt-6-sol,gpt-6-luna,gpt-5.6-sol,gpt-5.6-terra,gpt-5.6-luna",
            1,
            100,
            "https://openai.com",
            "[\"通用\",\"推理\",\"代码\",\"多模态\"]",
            "[\"api_key\",\"browser\"]",
        ),
        (
            "provider-anthropic",
            "Anthropic Claude",
            "https://api.anthropic.com/v1",
            Some("https://api.anthropic.com"),
            None,
            "Claude 系列官方，长文本与编码能力强",
            "claude-opus-5-5,claude-sonnet-5-5,claude-haiku-5-5,claude-opus-5,claude-sonnet-5,claude-haiku-4.5",
            1,
            90,
            "https://www.anthropic.com",
            "[\"通用\",\"推理\",\"代码\",\"长上下文\",\"Computer Use\"]",
            "[\"api_key\"]",
        ),
        (
            "provider-deepseek",
            "DeepSeek",
            "https://api.deepseek.com/v1",
            Some("https://api.deepseek.com/anthropic"),
            Some("https://api.deepseek.com/v1"),
            "深度求索官方，国产高性价比推理模型",
            "deepseek-v4-pro,deepseek-flash",
            1,
            80,
            "https://www.deepseek.com",
            "[\"通用\",\"推理\",\"代码\",\"Agent\",\"开源\",\"高性价比\"]",
            "[\"api_key\"]",
        ),
        (
            "provider-minimax",
            "MiniMax",
            "https://api.minimax.cn/v1",
            Some("https://api.minimax.cn/anthropic"),
            Some("https://api.minimax.cn/v1"),
            "MiniMax 大模型服务，按量计费，原生多模态与 1M 上下文",
            "MiniMax-M3,MiniMax-M2.7,MiniMax-M2.7-highspeed,MiniMax-M2.5,MiniMax-M2.5-highspeed",
            1,
            75,
            "https://www.minimaxi.com",
            "[\"通用\",\"中文\",\"多模态\",\"Agent\",\"视频\",\"语音\",\"长上下文\"]",
            "[\"api_key\"]",
        ),
        (
            "provider-minimax-token-plan",
            "MiniMax Token Plan",
            "https://api.minimax.cn/v1",
            Some("https://api.minimax.cn/anthropic"),
            Some("https://api.minimax.cn/v1"),
            "MiniMax M Plan 订阅专属模型，须使用订阅 Key 调用，不能与按量 Key 混用",
            "MiniMax-M3.1-Flash-Preview,MiniMax-M3,MiniMax-M2.7,MiniMax-M2.7-highspeed,MiniMax-M2.5,MiniMax-M2.5-highspeed",
            1,
            70,
            "https://www.minimaxi.com",
            "[\"通用\",\"中文\",\"多模态\",\"Agent\",\"长上下文\",\"订阅Key\"]",
            "[\"api_key\"]",
        ),
        (
            "provider-alibaba",
            "阿里通义千问",
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            None,
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1"),
            "阿里云百炼平台，国内企业级稳定服务",
            "qwen3.8-max,qwen3.7-max,qwen3.7-plus,qwen3.7-flash,qwen3.8-flash,qwen3-vl-plus,qwen3-coder-flash",
            1,
            65,
            "https://tongyi.aliyun.com",
            "[\"通用\",\"中文\",\"代码\",\"多模态\",\"Agent\"]",
            "[\"api_key\"]",
        ),
        (
            "provider-zhipu",
            "智谱 GLM",
            "https://open.bigmodel.cn/api/paas/v4",
            Some("https://open.bigmodel.cn/api/anthropic"),
            Some("https://open.bigmodel.cn/api/v1"),
            "智谱 AI 大模型服务，提供 GLM 系列模型",
            "glm-5.3,glm-5.3-flash,glm-5.2,glm-5.1,glm-5,glm-4.7,glm-4.7-flash",
            1,
            60,
            "https://www.zhipuai.cn",
            "[\"通用\",\"中文\",\"代码\",\"Agent\",\"开源\",\"工具调用\"]",
            "[\"api_key\"]",
        ),
    ];

    let created_at = "2024-01-01T00:00:00Z";
    for (
        id,
        name,
        api_url,
        api_anthropic_url,
        api_responses_url,
        description,
        models,
        r#type,
        sort,
        website,
        tags_json,
        auth_modes_json,
    ) in SEEDS
    {
        conn.execute(
            "INSERT OR IGNORE INTO providers
                (id, name, api_url, api_anthropic_url, api_responses_url, description, models, type, sort, website, tags, created_at, auth_modes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                id,
                name,
                api_url,
                api_anthropic_url,
                api_responses_url,
                description,
                models,
                r#type,
                sort,
                website,
                tags_json,
                created_at,
                auth_modes_json
            ],
        )?;
    }

    Ok(())
}

fn with_db<F, T>(f: F) -> AppResult<T>
where F: FnOnce(&Connection) -> AppResult<T> {
    let guard = DB.lock().unwrap();
    let conn = guard.as_ref().ok_or(AppError::Database(rusqlite::Error::InvalidQuery))?;
    f(conn)
}

/// 暴露给 services::stats 等模块直接使用同一个全局连接，避免再多开一个 Connection 引发锁竞争。
/// 返回的引用生命周期与本次锁内一致，调用方必须在调用栈内完成所有 SQL 后再释放。
pub fn raw_db_conn() -> AppResult<std::sync::MutexGuard<'static, Option<Connection>>> {
    // 注意：这里我们故意 lock 住 mutex 并返回 guard。调用方在持有期间独占访问 DB。
    let guard = DB.lock().unwrap();
    if guard.is_none() {
        return Err(AppError::Database(rusqlite::Error::InvalidQuery));
    }
    Ok(guard)
}

// ============================ ProviderKey ============================

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProviderKey {
    pub id: String,
    pub name: String,
    pub provider_id: String,
    pub encrypted_api_key: String,
    pub is_active: bool,
    pub created_at: String,
    /// 授权方式：'api_key'（手动填写）/ 'browser'（浏览器授权）
    pub auth_type: String,
    /// 浏览器授权的 refresh_token（密文）
    pub encrypted_refresh_token: Option<String>,
    /// 授权元数据 JSON（账号 ID / 过期时间 / 登录邮箱）
    pub auth_meta: Option<String>,
}

/// 服务商秘钥的授权信息（写入时的载体，refresh_token 为明文，落库前加密）。
#[derive(Debug, Clone, Default)]
pub struct ProviderKeyAuth {
    /// "api_key"（默认）/ "browser"
    pub auth_type: String,
    /// refresh_token 明文
    pub refresh_token: Option<String>,
    /// auth_meta JSON
    pub auth_meta: Option<String>,
}

impl ProviderKeyAuth {
    /// 归一化授权方式：只认 'browser'，其余一律视为 'api_key'。
    pub fn normalize_auth_type(value: &str) -> String {
        if value.trim().eq_ignore_ascii_case("browser") {
            "browser".to_string()
        } else {
            "api_key".to_string()
        }
    }
}

/// 授权方式常量。
pub const AUTH_TYPE_API_KEY: &str = "api_key";
pub const AUTH_TYPE_BROWSER: &str = "browser";

pub fn create_provider_key(
    name: &str,
    api_key: &str,
    provider_id: &str,
    auth: Option<&ProviderKeyAuth>,
) -> AppResult<ProviderKey> {
    let id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();
    let encrypted_api_key = crypto::encrypt(api_key)?;
    let auth_type = match auth {
        Some(a) => ProviderKeyAuth::normalize_auth_type(&a.auth_type),
        None => AUTH_TYPE_API_KEY.to_string(),
    };
    let encrypted_refresh_token = match auth.and_then(|a| a.refresh_token.as_deref()) {
        Some(token) if !token.trim().is_empty() => Some(crypto::encrypt(token)?),
        _ => None,
    };
    let auth_meta = auth
        .and_then(|a| a.auth_meta.clone())
        .filter(|s| !s.trim().is_empty());

    with_db(|conn| {
        conn.execute(
            "INSERT INTO providers_keys
                 (id, name, provider_id, encrypted_api_key, is_active, created_at,
                  auth_type, encrypted_refresh_token, auth_meta)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7, ?8)",
            params![
                id,
                name,
                provider_id,
                encrypted_api_key,
                created_at,
                auth_type,
                encrypted_refresh_token,
                auth_meta,
            ],
        )?;

        Ok(ProviderKey {
            id,
            name: name.to_string(),
            provider_id: provider_id.to_string(),
            encrypted_api_key,
            is_active: true,
            created_at,
            auth_type,
            encrypted_refresh_token,
            auth_meta,
        })
    })
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProviderKeyWithStatus {
    pub id: String,
    pub name: String,
    pub api_key: String,
    pub provider_id: String,
    pub provider_name: String,
    pub is_active: bool,
    pub created_at: String,
    /// 授权方式：'api_key' / 'browser'
    pub auth_type: String,
}

pub fn get_all_provider_keys() -> AppResult<Vec<ProviderKeyWithStatus>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT pk.id, pk.name, pk.encrypted_api_key, pk.provider_id, p.name AS provider_name,
                    pk.is_active, pk.created_at, pk.auth_type
             FROM providers_keys pk
             LEFT JOIN providers p ON pk.provider_id = p.id
             ORDER BY pk.created_at DESC"
        )?;

        let mut results = Vec::new();
        let rows = stmt.query_map([], |row| {
            let api_key = crypto::decrypt(&row.get::<_, String>(2)?).unwrap_or_else(|_| "".to_string());
            Ok(ProviderKeyWithStatus {
                id: row.get(0)?,
                name: row.get(1)?,
                api_key,
                provider_id: row.get(3)?,
                provider_name: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                is_active: row.get::<_, i32>(5)? != 0,
                created_at: row.get(6)?,
                auth_type: row.get::<_, Option<String>>(7)?.unwrap_or_else(|| AUTH_TYPE_API_KEY.to_string()),
            })
        })?;

        for row in rows {
            results.push(row.map_err(AppError::Database)?);
        }

        Ok(results)
    })
}

#[allow(dead_code)]
pub fn get_provider_key_by_id(id: &str) -> AppResult<Option<ProviderKey>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, name, provider_id, encrypted_api_key, is_active, created_at,
                    auth_type, encrypted_refresh_token, auth_meta
             FROM providers_keys WHERE id = ?1"
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(ProviderKey {
                id: row.get(0)?,
                name: row.get(1)?,
                provider_id: row.get(2)?,
                encrypted_api_key: row.get(3)?,
                is_active: row.get::<_, i32>(4)? != 0,
                created_at: row.get(5)?,
                auth_type: row
                    .get::<_, Option<String>>(6)?
                    .unwrap_or_else(|| AUTH_TYPE_API_KEY.to_string()),
                encrypted_refresh_token: row.get(7)?,
                auth_meta: row.get(8)?,
            }))
        } else {
            Ok(None)
        }
    })
}

pub fn update_provider_key(
    id: &str,
    name: &str,
    api_key: Option<&str>,
    provider_id: &str,
    auth: Option<&ProviderKeyAuth>,
) -> AppResult<()> {
    with_db(|conn| {
        match api_key {
            Some(key) => {
                let encrypted = crypto::encrypt(key)?;
                match auth {
                    Some(a) => {
                        // 显式传入授权信息：覆盖授权方式与附带凭据
                        let auth_type = ProviderKeyAuth::normalize_auth_type(&a.auth_type);
                        let refresh_token = match a.refresh_token.as_deref() {
                            Some(token) if !token.trim().is_empty() => Some(crypto::encrypt(token)?),
                            _ => None,
                        };
                        let auth_meta = a.auth_meta.clone().filter(|s| !s.trim().is_empty());
                        conn.execute(
                            "UPDATE providers_keys
                             SET name = ?1, encrypted_api_key = ?2, provider_id = ?3,
                                 auth_type = ?4, encrypted_refresh_token = ?5, auth_meta = ?6
                             WHERE id = ?7",
                            params![name, encrypted, provider_id, auth_type, refresh_token, auth_meta, id],
                        )?;
                    }
                    None => {
                        // 未传入授权信息：保留原有授权方式与刷新凭据
                        conn.execute(
                            "UPDATE providers_keys
                             SET name = ?1, encrypted_api_key = ?2, provider_id = ?3
                             WHERE id = ?4",
                            params![name, encrypted, provider_id, id],
                        )?;
                    }
                }
            }
            None => {
                conn.execute(
                    "UPDATE providers_keys SET name = ?1, provider_id = ?2 WHERE id = ?3",
                    params![name, provider_id, id],
                )?;
            }
        }
        Ok(())
    })
}

/// 浏览器授权令牌续期后回写凭据（代理在转发前自动续期时调用）。
pub fn update_provider_key_credentials(
    id: &str,
    api_key: &str,
    refresh_token: Option<&str>,
    auth_meta: Option<&str>,
) -> AppResult<()> {
    let encrypted = crypto::encrypt(api_key)?;
    let encrypted_refresh = match refresh_token {
        Some(token) if !token.trim().is_empty() => Some(crypto::encrypt(token)?),
        _ => None,
    };
    with_db(|conn| {
        conn.execute(
            "UPDATE providers_keys
             SET encrypted_api_key = ?1, encrypted_refresh_token = ?2, auth_meta = ?3
             WHERE id = ?4",
            params![encrypted, encrypted_refresh, auth_meta, id],
        )?;
        Ok(())
    })
}

pub fn delete_provider_key(id: &str) -> AppResult<()> {
    // 级联清理：proxy_keys.provider_key_id → providers_keys.id（硬外键）。
    // 删除前先把引用该 ProviderKey 的 proxy_keys 行解绑（置 NULL），
    // 这样代理 key 仍保留在系统中，只是不再关联到该 ProviderKey。
    with_db(|conn| {
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE proxy_keys SET provider_key_id = NULL WHERE provider_key_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM providers_keys WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    })
}

pub fn toggle_provider_key_active(id: &str, is_active: bool) -> AppResult<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE providers_keys SET is_active = ?1 WHERE id = ?2",
            params![is_active as i32, id],
        )?;
        Ok(())
    })
}

pub fn get_active_provider_keys() -> AppResult<Vec<ProviderKey>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, name, provider_id, encrypted_api_key, is_active, created_at,
                    auth_type, encrypted_refresh_token, auth_meta
             FROM providers_keys
             WHERE is_active = 1
             ORDER BY created_at DESC"
        )?;

        let mut results = Vec::new();
        let rows = stmt.query_map([], |row| {
            Ok(ProviderKey {
                id: row.get(0)?,
                name: row.get(1)?,
                provider_id: row.get(2)?,
                encrypted_api_key: row.get(3)?,
                is_active: true,
                created_at: row.get(5)?,
                auth_type: row
                    .get::<_, Option<String>>(6)?
                    .unwrap_or_else(|| AUTH_TYPE_API_KEY.to_string()),
                encrypted_refresh_token: row.get(7)?,
                auth_meta: row.get(8)?,
            })
        })?;

        for row in rows {
            results.push(row.map_err(AppError::Database)?);
        }

        Ok(results)
    })
}

// ============================ ProxyKey ============================

/// api_type 归一化：仅接受 "chat" / "anthropic" / "responses"；
/// 历史数字值（1/2/3）兼容映射，其余一律回退为 "chat"（OpenAI 兼容）。
pub fn normalize_api_type(value: &str) -> &'static str {
    match value.trim() {
        "anthropic" | "2" => "anthropic",
        "responses" | "3" => "responses",
        _ => "chat",
    }
}

/// 读取 api_type 列并归一化：兼容存量库中 INTEGER(1/2/3)、NULL 等历史值。
fn read_api_type(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<String> {
    let raw: rusqlite::types::Value = row.get(idx)?;
    let text = match raw {
        rusqlite::types::Value::Text(s) => s,
        rusqlite::types::Value::Integer(i) => i.to_string(),
        _ => String::new(),
    };
    Ok(normalize_api_type(&text).to_string())
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProxyKey {
    pub id: String,
    pub name: String,
    pub key: String,
    pub provider_key_id: Option<String>,
    pub provider_key_name: Option<String>,
    pub provider_name: Option<String>,
    pub model: Option<String>,
    pub api_type: String,
    pub expires_at: Option<String>,
    pub is_active: bool,
    pub created_at: String,
}

fn generate_unique_key() -> String {
    let uuid = Uuid::new_v4().to_string().replace("-", "");
    let timestamp = Utc::now().timestamp().to_string();
    format!("CY_{}_{}", timestamp, &uuid[0..16]).to_uppercase()
}

pub fn create_proxy_key(name: &str, provider_key_id: Option<&str>, model: Option<&str>, api_type: &str) -> AppResult<ProxyKey> {
    let id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();
    let key = generate_unique_key();
    // 'chat'=OpenAI 兼容（默认），'anthropic'=Anthropic Messages，'responses'=Responses
    let api_type = normalize_api_type(api_type).to_string();

    with_db(|conn| {
        conn.execute(
            "INSERT INTO proxy_keys (id, name, key, provider_key_id, model, api_type, is_active, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7)",
            params![id, name, key, provider_key_id, model, api_type, created_at],
        )?;

        let (provider_key_name, provider_name) = if let Some(pk_id) = provider_key_id {
            let mut stmt = conn.prepare(
                "SELECT pk2.name, p.name
                 FROM providers_keys pk2
                 LEFT JOIN providers p ON pk2.provider_id = p.id
                 WHERE pk2.id = ?1"
            )?;
            stmt.query_row(params![pk_id], |row| {
                Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?))
            }).ok().unwrap_or((None, None))
        } else {
            (None, None)
        };

        Ok(ProxyKey {
            id,
            name: name.to_string(),
            key,
            provider_key_id: provider_key_id.map(|s| s.to_string()),
            provider_key_name,
            provider_name,
            model: model.map(|s| s.to_string()),
            api_type,
            expires_at: None,
            is_active: true,
            created_at,
        })
    })
}

pub fn get_all_proxy_keys() -> AppResult<Vec<ProxyKey>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT pk.id, pk.name, pk.key, pk.provider_key_id, pk2.name AS provider_key_name, p.name AS provider_name, pk.model, pk.api_type, pk.expires_at, pk.is_active, pk.created_at
             FROM proxy_keys pk
             LEFT JOIN providers_keys pk2 ON pk.provider_key_id = pk2.id
             LEFT JOIN providers p ON pk2.provider_id = p.id
             ORDER BY pk.created_at DESC"
        )?;

        let mut results = Vec::new();
        let rows = stmt.query_map([], |row| {
            Ok(ProxyKey {
                id: row.get(0)?,
                name: row.get(1)?,
                key: row.get(2)?,
                provider_key_id: row.get(3)?,
                provider_key_name: row.get(4)?,
                provider_name: row.get(5)?,
                model: row.get(6)?,
                api_type: read_api_type(row, 7)?,
                expires_at: row.get(8)?,
                is_active: row.get::<_, i32>(9)? != 0,
                created_at: row.get(10)?,
            })
        })?;

        for row in rows {
            results.push(row?);
        }

        Ok(results)
    })
}

#[allow(dead_code)]
pub fn get_proxy_key_by_id(id: &str) -> AppResult<Option<ProxyKey>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT pk.id, pk.name, pk.key, pk.provider_key_id, pk2.name AS provider_key_name, p.name AS provider_name, pk.model, pk.api_type, pk.expires_at, pk.is_active, pk.created_at
             FROM proxy_keys pk
             LEFT JOIN providers_keys pk2 ON pk.provider_key_id = pk2.id
             LEFT JOIN providers p ON pk2.provider_id = p.id
             WHERE pk.id = ?1"
        )?;
        let row = stmt.query_row(params![id], |row| {
            Ok(ProxyKey {
                id: row.get(0)?,
                name: row.get(1)?,
                key: row.get(2)?,
                provider_key_id: row.get(3)?,
                provider_key_name: row.get(4)?,
                provider_name: row.get(5)?,
                model: row.get(6)?,
                api_type: read_api_type(row, 7)?,
                expires_at: row.get(8)?,
                is_active: row.get::<_, i32>(9)? != 0,
                created_at: row.get(10)?,
            })
        });

        match row {
            Ok(proxy_key) => Ok(Some(proxy_key)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e)),
        }
    })
}

pub fn update_proxy_key(id: &str, name: &str, provider_key_id: Option<&str>, model: Option<&str>, api_type: &str) -> AppResult<()> {
    // 'chat'=OpenAI 兼容（默认），'anthropic'=Anthropic Messages，'responses'=Responses
    let api_type = normalize_api_type(api_type).to_string();
    with_db(|conn| {
        conn.execute(
            "UPDATE proxy_keys SET name = ?1, provider_key_id = ?2, model = ?3, api_type = ?4 WHERE id = ?5",
            params![name, provider_key_id, model, api_type, id],
        )?;
        Ok(())
    })
}

pub fn delete_proxy_key(id: &str) -> AppResult<()> {
    with_db(|conn| {
        conn.execute("DELETE FROM proxy_keys WHERE id = ?1", params![id])?;
        Ok(())
    })
}

pub fn toggle_proxy_key_active(id: &str, is_active: bool) -> AppResult<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE proxy_keys SET is_active = ?1 WHERE id = ?2",
            params![is_active as i32, id],
        )?;
        Ok(())
    })
}

pub fn regenerate_proxy_key(id: &str) -> AppResult<String> {
    let new_key = generate_unique_key();
    with_db(|conn| {
        conn.execute(
            "UPDATE proxy_keys SET key = ?1 WHERE id = ?2",
            params![new_key, id],
        )?;
        Ok(new_key)
    })
}

/// 代理转发使用：根据传入的代理 key 值反查关联的 ProviderKey + Provider 完整信息。
#[derive(Debug, Clone)]
pub struct ProxyKeyWithProviderKey {
    pub proxy_key: ProxyKey,
    pub provider_key: ProviderKey,
}

pub fn get_proxy_key_with_provider_key_by_key(key: &str) -> AppResult<Option<ProxyKeyWithProviderKey>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT pk.id, pk.name, pk.key, pk.provider_key_id, pk2.name AS provider_key_name, pk.model, pk.api_type, pk.expires_at, pk.is_active, pk.created_at,
                    pk2.id, pk2.name, pk2.provider_id, pk2.encrypted_api_key, pk2.is_active, pk2.created_at,
                    pk2.auth_type, pk2.encrypted_refresh_token, pk2.auth_meta
             FROM proxy_keys pk
             JOIN providers_keys pk2 ON pk.provider_key_id = pk2.id
             WHERE pk.key = ?1 AND pk.is_active = 1 AND pk2.is_active = 1"
        )?;

        let row = stmt.query_row(params![key], |row| {
            Ok(ProxyKeyWithProviderKey {
                proxy_key: ProxyKey {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    key: row.get(2)?,
                    provider_key_id: row.get(3)?,
                    provider_key_name: row.get(4)?,
                    provider_name: None,
                    model: row.get(5)?,
                    api_type: read_api_type(row, 6)?,
                    expires_at: row.get(7)?,
                    is_active: row.get::<_, i32>(8)? != 0,
                    created_at: row.get(9)?,
                },
                provider_key: ProviderKey {
                    id: row.get(10)?,
                    name: row.get(11)?,
                    provider_id: row.get(12)?,
                    encrypted_api_key: row.get(13)?,
                    is_active: row.get::<_, i32>(14)? != 0,
                    created_at: row.get(15)?,
                    auth_type: row
                        .get::<_, Option<String>>(16)?
                        .unwrap_or_else(|| AUTH_TYPE_API_KEY.to_string()),
                    encrypted_refresh_token: row.get(17)?,
                    auth_meta: row.get(18)?,
                },
            })
        });

        match row {
            Ok(pk) => Ok(Some(pk)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e)),
        }
    })
}

// ============================ Config ============================

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub id: i32,
    pub listen_port: i32,
    pub auto_start: bool,
    pub minimize_to_tray: bool,
    pub log_level: String,
    pub server_url: String,
    /// 构建环境标识（dev / test / prod），由前端构建时通过 init_build_env 推送，
    /// 设置页用于提示当前安装包对应哪个环境。
    pub app_env: String,
}

pub fn init_config() -> AppResult<()> {
    with_db(|conn| {
        conn.execute(
            "INSERT OR IGNORE INTO config (id, listen_port, auto_start, minimize_to_tray, log_level, server_url, app_env) VALUES (1, 20112, 0, 0, 'info', 'https://api.ciyuanproxy.com/', 'prod')",
            [],
        )?;
        Ok(())
    })
}

pub fn get_config() -> AppResult<AppConfig> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, listen_port, auto_start, minimize_to_tray, log_level, server_url, app_env FROM config WHERE id = 1"
        )?;
        stmt.query_row([], |row| {
            Ok(AppConfig {
                id: row.get(0)?,
                listen_port: row.get(1)?,
                auto_start: row.get::<_, i32>(2)? != 0,
                minimize_to_tray: row.get::<_, i32>(3)? != 0,
                log_level: row.get(4)?,
                server_url: row.get(5)?,
                app_env: row.get(6)?,
            })
        }).map_err(AppError::Database)
    })
}

pub fn update_config(listen_port: i32, auto_start: bool, minimize_to_tray: bool, log_level: &str, server_url: &str) -> AppResult<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE config SET listen_port = ?1, auto_start = ?2, minimize_to_tray = ?3, log_level = ?4, server_url = ?5 WHERE id = 1",
            params![listen_port, auto_start as i32, minimize_to_tray as i32, log_level, server_url],
        )?;
        Ok(())
    })
}

/// 前端构建时把 `VITE_SERVER_URL` 和 `VITE_APP_ENV` 推过来。
/// 行为：
/// - `app_env` 始终覆盖为构建时传入的值（仅供展示，不影响功能）。
/// - `server_url` 仅在当前值仍是出厂默认 `https://api.ciyuanproxy.com/` 时才覆盖，
///   避免破坏用户在设置页手动修改过的远程服务端地址。
pub fn apply_build_env(server_url: &str, app_env: &str) -> AppResult<()> {
    const FACTORY_DEFAULT_SERVER_URL: &str = "https://api.ciyuanproxy.com/";
    with_db(|conn| {
        let current: String = conn
            .query_row(
                "SELECT server_url FROM config WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .map_err(AppError::Database)?;
        let new_server_url = if current == FACTORY_DEFAULT_SERVER_URL {
            server_url
        } else {
            current.as_str()
        };
        conn.execute(
            "UPDATE config SET server_url = ?1, app_env = ?2 WHERE id = 1",
            params![new_server_url, app_env],
        )?;
        Ok(())
    })
}

// ============================ Provider ============================

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub website: Option<String>,
    pub api_url: String,
    pub api_anthropic_url: Option<String>,
    pub api_responses_url: Option<String>,
    pub description: Option<String>,
    pub models: String,
    #[serde(rename = "type")]
    pub r#type: i32,
    pub tags: Vec<String>,
    pub pricing: Option<String>,
    pub status: i32,
    pub sort: i32,
    pub created_at: String,
    /// 支持的授权方式：["api_key"] / ["browser"] / ["api_key","browser"]
    pub auth_modes: Vec<String>,
}

fn tags_to_json(tags: &[String]) -> String {
    serde_json::to_string(tags).unwrap_or_else(|_| "[]".to_string())
}

fn tags_from_json(s: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(s).unwrap_or_default()
}

/// 授权方式归一化：只保留 "api_key" / "browser"，去重并保证 api_key 在前。
fn auth_modes_to_json(modes: &[String]) -> String {
    let mut normalized: Vec<String> = Vec::new();
    for mode in modes {
        let m = mode.trim();
        if m.eq_ignore_ascii_case("browser") {
            if !normalized.iter().any(|x| x == AUTH_TYPE_BROWSER) {
                normalized.push(AUTH_TYPE_BROWSER.to_string());
            }
        } else if m.eq_ignore_ascii_case("api_key") {
            if !normalized.iter().any(|x| x == AUTH_TYPE_API_KEY) {
                normalized.push(AUTH_TYPE_API_KEY.to_string());
            }
        }
    }
    if normalized.is_empty() {
        normalized.push(AUTH_TYPE_API_KEY.to_string());
    }
    serde_json::to_string(&normalized).unwrap_or_else(|_| "[\"api_key\"]".to_string())
}

fn auth_modes_from_json(s: &str) -> Vec<String> {
    let modes: Vec<String> = serde_json::from_str(s).unwrap_or_default();
    let mut normalized: Vec<String> = Vec::new();
    for mode in modes {
        let m = mode.trim().to_lowercase();
        if m == AUTH_TYPE_BROWSER {
            if !normalized.iter().any(|x| x == AUTH_TYPE_BROWSER) {
                normalized.push(AUTH_TYPE_BROWSER.to_string());
            }
        } else if m == AUTH_TYPE_API_KEY {
            if !normalized.iter().any(|x| x == AUTH_TYPE_API_KEY) {
                normalized.push(AUTH_TYPE_API_KEY.to_string());
            }
        }
    }
    if normalized.is_empty() {
        normalized.push(AUTH_TYPE_API_KEY.to_string());
    }
    normalized
}

fn provider_row_to_struct(row: &rusqlite::Row) -> rusqlite::Result<Provider> {
    Ok(Provider {
        id: row.get(0)?,
        name: row.get(1)?,
        website: row.get(2)?,
        api_url: row.get(3)?,
        api_anthropic_url: row.get(4)?,
        api_responses_url: row.get(5)?,
        description: row.get(6)?,
        models: row.get(7)?,
        r#type: row.get(8)?,
        tags: tags_from_json(&row.get::<_, String>(9)?),
        pricing: row.get(10)?,
        status: row.get(11)?,
        sort: row.get(12)?,
        created_at: row.get(13)?,
        auth_modes: auth_modes_from_json(&row.get::<_, String>(14)?),
    })
}

const PROVIDER_COLUMNS: &str = "id, name, website, api_url, api_anthropic_url, api_responses_url, description, models, type, tags, pricing, status, sort, created_at, auth_modes";

pub fn create_provider(
    name: &str,
    website: Option<&str>,
    api_url: &str,
    api_anthropic_url: Option<&str>,
    api_responses_url: Option<&str>,
    description: Option<&str>,
    models: &str,
    r#type: i32,
    tags: &[String],
    pricing: Option<&str>,
    status: i32,
    sort: i32,
    auth_modes: &[String],
) -> AppResult<Provider> {
    let id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();
    let tags_json = tags_to_json(tags);
    let auth_modes_json = auth_modes_to_json(auth_modes);

    with_db(|conn| {
        conn.execute(
            "INSERT INTO providers (id, name, website, api_url, api_anthropic_url, api_responses_url, description, models, type, tags, pricing, status, sort, created_at, auth_modes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                id,
                name,
                website,
                api_url,
                api_anthropic_url,
                api_responses_url,
                description,
                models,
                r#type,
                tags_json,
                pricing,
                status,
                sort,
                created_at,
                auth_modes_json,
            ],
        )?;

        Ok(Provider {
            id,
            name: name.to_string(),
            website: website.map(|s| s.to_string()),
            api_url: api_url.to_string(),
            api_anthropic_url: api_anthropic_url.map(|s| s.to_string()),
            api_responses_url: api_responses_url.map(|s| s.to_string()),
            description: description.map(|s| s.to_string()),
            models: models.to_string(),
            r#type,
            tags: tags.to_vec(),
            pricing: pricing.map(|s| s.to_string()),
            status,
            sort,
            created_at,
            auth_modes: auth_modes_from_json(&auth_modes_json),
        })
    })
}

pub fn get_all_providers() -> AppResult<Vec<Provider>> {
    with_db(|conn| {
        let sql = format!(
            "SELECT {} FROM providers ORDER BY sort ASC, type ASC, created_at DESC",
            PROVIDER_COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;

        let mut results = Vec::new();
        let rows = stmt.query_map([], provider_row_to_struct)?;

        for row in rows {
            results.push(row.map_err(AppError::Database)?);
        }

        Ok(results)
    })
}

#[allow(dead_code)]
pub fn get_provider_by_id(id: &str) -> AppResult<Option<Provider>> {
    with_db(|conn| {
        let sql = format!(
            "SELECT {} FROM providers WHERE id = ?1",
            PROVIDER_COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Provider {
                id: row.get(0)?,
                name: row.get(1)?,
                website: row.get(2)?,
                api_url: row.get(3)?,
                api_anthropic_url: row.get(4)?,
                api_responses_url: row.get(5)?,
                description: row.get(6)?,
                models: row.get(7)?,
                r#type: row.get(8)?,
                tags: tags_from_json(&row.get::<_, String>(9)?),
                pricing: row.get(10)?,
                status: row.get(11)?,
                sort: row.get(12)?,
                created_at: row.get(13)?,
                auth_modes: auth_modes_from_json(&row.get::<_, String>(14)?),
            }))
        } else {
            Ok(None)
        }
    })
}

pub fn update_provider(
    id: &str,
    name: &str,
    website: Option<&str>,
    api_url: &str,
    api_anthropic_url: Option<&str>,
    api_responses_url: Option<&str>,
    description: Option<&str>,
    models: &str,
    r#type: i32,
    tags: &[String],
    pricing: Option<&str>,
    status: i32,
    sort: i32,
    auth_modes: &[String],
) -> AppResult<()> {
    let tags_json = tags_to_json(tags);
    let auth_modes_json = auth_modes_to_json(auth_modes);
    with_db(|conn| {
        conn.execute(
            "UPDATE providers SET name = ?1, website = ?2, api_url = ?3, api_anthropic_url = ?4, api_responses_url = ?5, description = ?6, models = ?7, type = ?8, tags = ?9, pricing = ?10, status = ?11, sort = ?12, auth_modes = ?13 WHERE id = ?14",
            params![
                name,
                website,
                api_url,
                api_anthropic_url,
                api_responses_url,
                description,
                models,
                r#type,
                tags_json,
                pricing,
                status,
                sort,
                auth_modes_json,
                id,
            ],
        )?;
        Ok(())
    })
}

pub fn toggle_provider_status(id: &str, status: bool) -> AppResult<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE providers SET status = ?1 WHERE id = ?2",
            params![status as i32, id],
        )?;
        Ok(())
    })
}

pub fn delete_provider(id: &str) -> AppResult<()> {
    // 级联清理依赖：
    //   providers_keys.provider_id → providers.id（硬外键）
    //   proxy_keys.provider_key_id → providers_keys.id（硬外键，列可空）
    // SQLite 默认不开启 ON DELETE CASCADE，因此这里手动按依赖顺序处理：
    //   1) 先解除 proxy_keys 对这些 providers_keys 的引用（置 NULL，避免悬空引用）
    //   2) 删除这些 providers_keys（解除 providers_keys → providers 的引用）
    //   3) 删除 provider 本身
    with_db(|conn| {
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE proxy_keys
             SET provider_key_id = NULL
             WHERE provider_key_id IN (SELECT id FROM providers_keys WHERE provider_id = ?1)",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM providers_keys WHERE provider_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM providers WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 浏览器授权相关字段的落库 / 读取 / 更新语义
    #[test]
    fn test_provider_key_browser_auth_roundtrip() {
        let dir = std::env::temp_dir().join(format!("ciyuan-proxy-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let db_path = dir.join("test.db");

        // 预置一个"旧结构"数据库（缺少 auth_modes / auth_type 等新列），
        // 验证升级迁移不会丢数据
        {
            let legacy = Connection::open(&db_path).expect("open legacy db");
            legacy
                .execute(
                    "CREATE TABLE providers (
                        id TEXT PRIMARY KEY, name TEXT NOT NULL, api_url TEXT NOT NULL,
                        api_anthropic_url TEXT, api_responses_url TEXT, description TEXT,
                        models TEXT NOT NULL DEFAULT '', type INTEGER NOT NULL DEFAULT 2,
                        website TEXT, tags TEXT NOT NULL DEFAULT '[]', pricing TEXT,
                        status INTEGER NOT NULL DEFAULT 0, sort INTEGER NOT NULL DEFAULT 0,
                        created_at TEXT NOT NULL)",
                    [],
                )
                .expect("create legacy providers");
            legacy
                .execute(
                    "INSERT INTO providers
                        (id, name, api_url, models, type, tags, status, sort, created_at)
                     VALUES ('provider-openai', 'OpenAI', 'https://api.openai.com/v1', '', 1, '[]', 0, 0, '2024-01-01T00:00:00Z')",
                    [],
                )
                .expect("insert legacy provider");
            legacy
                .execute(
                    "CREATE TABLE providers_keys (
                        id TEXT PRIMARY KEY, name TEXT NOT NULL, provider_id TEXT NOT NULL,
                        encrypted_api_key TEXT NOT NULL, is_active INTEGER NOT NULL DEFAULT 1,
                        created_at TEXT NOT NULL)",
                    [],
                )
                .expect("create legacy providers_keys");
            legacy
                .execute(
                    "INSERT INTO providers_keys
                        (id, name, provider_id, encrypted_api_key, is_active, created_at)
                     VALUES ('legacy-key', '旧秘钥', 'provider-openai', 'cipher', 1, '2024-01-01T00:00:00Z')",
                    [],
                )
                .expect("insert legacy provider key");
        }

        init_db(db_path.to_str().unwrap()).expect("init db");

        // 旧数据仍在
        let legacy_keys = get_all_provider_keys().expect("list provider keys");
        assert!(legacy_keys.iter().any(|k| k.id == "legacy-key"));

        // 预置的 OpenAI 服务商应同时支持 API 密钥与浏览器授权
        let providers = get_all_providers().expect("list providers");
        let openai = providers
            .iter()
            .find(|p| p.id == "provider-openai")
            .expect("openai provider");
        assert!(openai.auth_modes.contains(&AUTH_TYPE_API_KEY.to_string()));
        assert!(openai.auth_modes.contains(&AUTH_TYPE_BROWSER.to_string()));

        // 浏览器授权写入：refresh_token 加密存储，auth_meta 存 JSON
        let auth = ProviderKeyAuth {
            auth_type: AUTH_TYPE_BROWSER.to_string(),
            refresh_token: Some("refresh-token-xxx".to_string()),
            auth_meta: Some("{\"accountId\":\"acc-1\"}".to_string()),
        };
        let key = create_provider_key("测试秘钥", "access-token", "provider-openai", Some(&auth))
            .expect("create provider key");
        assert_eq!(key.auth_type, AUTH_TYPE_BROWSER);
        assert!(key.encrypted_refresh_token.is_some());

        let listed = get_all_provider_keys().expect("list provider keys");
        let found = listed.iter().find(|k| k.id == key.id).expect("key in list");
        assert_eq!(found.auth_type, AUTH_TYPE_BROWSER);
        assert_eq!(found.api_key, "access-token");

        // 不带授权信息更新时，保留原有 refresh_token / auth_meta
        update_provider_key(
            &key.id,
            "改名",
            Some("access-token"),
            "provider-openai",
            None,
        )
        .expect("update without auth");
        let raw = get_provider_key_by_id(&key.id)
            .expect("query key")
            .expect("key exists");
        assert_eq!(raw.auth_type, AUTH_TYPE_BROWSER);
        assert!(raw.encrypted_refresh_token.is_some());
        assert!(raw.auth_meta.is_some());

        // 切回 API 密钥模式时清空刷新凭据
        let clear = ProviderKeyAuth {
            auth_type: AUTH_TYPE_API_KEY.to_string(),
            refresh_token: None,
            auth_meta: None,
        };
        update_provider_key(
            &key.id,
            "改名",
            Some("sk-manual"),
            "provider-openai",
            Some(&clear),
        )
        .expect("switch to api key");
        let raw = get_provider_key_by_id(&key.id)
            .expect("query key")
            .expect("key exists");
        assert_eq!(raw.auth_type, AUTH_TYPE_API_KEY);
        assert!(raw.encrypted_refresh_token.is_none());
        assert!(raw.auth_meta.is_none());

        std::fs::remove_dir_all(&dir).ok();
    }
}
