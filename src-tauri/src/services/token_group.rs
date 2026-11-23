use serde::{Deserialize, Serialize};

use crate::error::AppResult;
use crate::repositories::{self, TokenGroup};

// ======================== TokenGroup Service ========================

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTokenGroupRequest {
    pub name: String,
    pub token_id: Option<String>,
    pub provider: String,
    pub route_path: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTokenGroupRequest {
    pub id: String,
    pub name: String,
    pub token_id: Option<String>,
    pub provider: String,
    pub route_path: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenGroupWithStatus {
    pub id: String,
    pub name: String,
    pub token_id: Option<String>,
    pub provider: String,
    pub route_path: String,
    pub is_active: bool,
    pub created_at: String,
}

impl From<TokenGroup> for TokenGroupWithStatus {
    fn from(tg: TokenGroup) -> Self {
        TokenGroupWithStatus {
            id: tg.id,
            name: tg.name,
            token_id: tg.token_id,
            provider: tg.provider,
            route_path: tg.route_path,
            is_active: tg.is_active,
            created_at: tg.created_at,
        }
    }
}

pub fn create_token_group(req: CreateTokenGroupRequest) -> AppResult<TokenGroupWithStatus> {
    let group = repositories::create_token_group(
        &req.name,
        req.token_id.as_deref(),
        &req.provider,
        &req.route_path,
    )?;
    Ok(group.into())
}

pub fn get_all_token_groups() -> AppResult<Vec<TokenGroupWithStatus>> {
    let groups = repositories::get_all_token_groups()?;
    Ok(groups.into_iter().map(|g| g.into()).collect())
}

pub fn update_token_group(req: UpdateTokenGroupRequest) -> AppResult<()> {
    repositories::update_token_group(
        &req.id,
        &req.name,
        req.token_id.as_deref(),
        &req.provider,
        &req.route_path,
    )
}

pub fn delete_token_group(id: &str) -> AppResult<()> {
    repositories::delete_token_group(id)
}

pub fn activate_token_group(id: &str) -> AppResult<()> {
    repositories::activate_token_group(id)
}

pub fn deactivate_token_group(id: &str) -> AppResult<()> {
    repositories::deactivate_token_group(id)
}

pub fn get_active_token_groups() -> AppResult<Vec<TokenGroupWithStatus>> {
    let groups = repositories::get_active_token_groups()?;
    Ok(groups.into_iter().map(|g| g.into()).collect())
}