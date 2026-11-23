use serde::{Deserialize, Serialize};

use crate::crypto;
use crate::error::AppResult;
use crate::repositories::{AppConfig, Provider, ProviderKey, ProviderKeyAuth};

pub mod feedback;
pub mod oauth;
pub mod proxy;
pub mod remote;
pub mod stats;

// ============================ Config Service ========================

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateConfigRequest {
    pub listen_port: i32,
    pub auto_start: bool,
    pub minimize_to_tray: bool,
    pub log_level: String,
    pub server_url: String,
}

pub fn get_config() -> AppResult<AppConfig> {
    crate::repositories::get_config()
}

pub fn update_config(req: UpdateConfigRequest) -> AppResult<()> {
    crate::repositories::update_config(
        req.listen_port,
        req.auto_start,
        req.minimize_to_tray,
        &req.log_level,
        &req.server_url,
    )
}

/// 前端构建时调用：把 `VITE_SERVER_URL` / `VITE_APP_ENV` 推送到后端持久化。
/// server_url 仅在仍是出厂默认时覆盖，app_env 始终覆盖。
pub fn apply_build_env(server_url: &str, app_env: &str) -> AppResult<()> {
    crate::repositories::apply_build_env(server_url, app_env)
}

// ============================ ProviderKey Service ========================

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProviderKeyRequest {
    pub name: String,
    pub api_key: String,
    pub provider_id: String,
    /// 授权方式："api_key"（默认）/ "browser"
    #[serde(default)]
    pub auth_type: Option<String>,
    /// 浏览器授权的 refresh_token（明文，后端加密存储）
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// 授权元数据 JSON
    #[serde(default)]
    pub auth_meta: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProviderKeyRequest {
    pub id: String,
    pub name: String,
    pub api_key: Option<String>,
    pub provider_id: String,
    /// 授权方式："api_key"（默认）/ "browser"
    #[serde(default)]
    pub auth_type: Option<String>,
    /// 浏览器授权的 refresh_token（明文，后端加密存储）
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// 授权元数据 JSON
    #[serde(default)]
    pub auth_meta: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProviderKeyWithStatus {
    pub id: String,
    pub name: String,
    pub api_key: String,
    pub provider_id: String,
    pub provider_name: String,
    pub is_active: bool,
    pub created_at: String,
    /// 授权方式："api_key" / "browser"
    pub auth_type: String,
}

impl From<ProviderKey> for ProviderKeyWithStatus {
    fn from(pk: ProviderKey) -> Self {
        let api_key = crypto::decrypt(&pk.encrypted_api_key).unwrap_or_else(|_| "".to_string());
        ProviderKeyWithStatus {
            id: pk.id,
            name: pk.name,
            api_key,
            provider_id: pk.provider_id,
            provider_name: String::new(),
            is_active: pk.is_active,
            created_at: pk.created_at,
            auth_type: pk.auth_type,
        }
    }
}

impl From<crate::repositories::ProviderKeyWithStatus> for ProviderKeyWithStatus {
    fn from(item: crate::repositories::ProviderKeyWithStatus) -> Self {
        ProviderKeyWithStatus {
            id: item.id,
            name: item.name,
            api_key: item.api_key,
            provider_id: item.provider_id,
            provider_name: item.provider_name,
            is_active: item.is_active,
            created_at: item.created_at,
            auth_type: item.auth_type,
        }
    }
}

/// 把请求里的授权字段转成仓储层结构。
///
/// `auth_type` 为空表示"不改动授权信息"（编辑场景下保留原有 refresh_token / auth_meta）。
fn to_auth_payload(
    auth_type: &Option<String>,
    refresh_token: &Option<String>,
    auth_meta: &Option<String>,
) -> Option<ProviderKeyAuth> {
    match auth_type {
        Some(t) => Some(ProviderKeyAuth {
            auth_type: t.clone(),
            refresh_token: refresh_token.clone(),
            auth_meta: auth_meta.clone(),
        }),
        None => None,
    }
}

pub fn create_provider_key(req: CreateProviderKeyRequest) -> AppResult<ProviderKeyWithStatus> {
    let auth = to_auth_payload(&req.auth_type, &req.refresh_token, &req.auth_meta);
    let key = crate::repositories::create_provider_key(
        &req.name,
        &req.api_key,
        &req.provider_id,
        auth.as_ref(),
    )?;
    Ok(key.into())
}

pub fn get_all_provider_keys() -> AppResult<Vec<ProviderKeyWithStatus>> {
    let keys = crate::repositories::get_all_provider_keys()?;
    Ok(keys.into_iter().map(ProviderKeyWithStatus::from).collect())
}

pub fn update_provider_key(req: UpdateProviderKeyRequest) -> AppResult<()> {
    let auth = to_auth_payload(&req.auth_type, &req.refresh_token, &req.auth_meta);
    crate::repositories::update_provider_key(
        &req.id,
        &req.name,
        req.api_key.as_deref(),
        &req.provider_id,
        auth.as_ref(),
    )
}

pub fn delete_provider_key(id: &str) -> AppResult<()> {
    crate::repositories::delete_provider_key(id)
}

pub fn toggle_provider_key_active(id: &str, is_active: bool) -> AppResult<()> {
    crate::repositories::toggle_provider_key_active(id, is_active)
}

// ============================ Provider Service ========================

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateProviderRequest {
    pub name: String,
    pub website: Option<String>,
    pub api_url: String,
    pub api_anthropic_url: Option<String>,
    pub api_responses_url: Option<String>,
    pub description: Option<String>,
    pub models: String,
    pub r#type: i32,
    pub tags: Vec<String>,
    pub pricing: Option<String>,
    pub status: i32,
    pub sort: i32,
    /// 支持的授权方式：["api_key"] / ["browser"] / 两者都支持
    #[serde(default)]
    pub auth_modes: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateProviderRequest {
    pub id: String,
    pub name: String,
    pub website: Option<String>,
    pub api_url: String,
    pub api_anthropic_url: Option<String>,
    pub api_responses_url: Option<String>,
    pub description: Option<String>,
    pub models: String,
    pub r#type: i32,
    pub tags: Vec<String>,
    pub pricing: Option<String>,
    pub status: i32,
    pub sort: i32,
    /// 支持的授权方式：["api_key"] / ["browser"] / 两者都支持
    #[serde(default)]
    pub auth_modes: Vec<String>,
}

pub fn create_provider(req: CreateProviderRequest) -> AppResult<Provider> {
    crate::repositories::create_provider(
        &req.name,
        req.website.as_deref(),
        &req.api_url,
        req.api_anthropic_url.as_deref(),
        req.api_responses_url.as_deref(),
        req.description.as_deref(),
        &req.models,
        req.r#type,
        &req.tags,
        req.pricing.as_deref(),
        req.status,
        req.sort,
        &req.auth_modes,
    )
}

pub fn get_all_providers() -> AppResult<Vec<Provider>> {
    crate::repositories::get_all_providers()
}

pub fn update_provider(req: UpdateProviderRequest) -> AppResult<()> {
    crate::repositories::update_provider(
        &req.id,
        &req.name,
        req.website.as_deref(),
        &req.api_url,
        req.api_anthropic_url.as_deref(),
        req.api_responses_url.as_deref(),
        req.description.as_deref(),
        &req.models,
        req.r#type,
        &req.tags,
        req.pricing.as_deref(),
        req.status,
        req.sort,
        &req.auth_modes,
    )
}

pub fn delete_provider(id: &str) -> AppResult<()> {
    crate::repositories::delete_provider(id)
}

pub fn toggle_provider_status(id: &str, status: bool) -> AppResult<()> {
    crate::repositories::toggle_provider_status(id, status)
}
