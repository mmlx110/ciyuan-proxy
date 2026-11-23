use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// 意见反馈 客户端提交入参
///
/// <p>对齐服务端 {@code WebFeedbackDto}
/// (POST /common/web/feedback/submit)。
/// 字段保持与服务端一致：服务端会强制覆盖 source/status/priority，
/// 这里不发送这些字段。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackSubmitInput {
    /// 用户ID，匿名提交时为 null
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<i64>,
    /// 反馈类型：1-Bug 2-建议 3-商务
    pub r#type: i32,
    /// 标题
    pub title: String,
    /// 内容
    pub content: String,
    /// 联系方式（邮箱/手机/微信）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
    /// 客户端版本号
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_version: Option<String>,
    /// OS / 架构
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_info: Option<String>,
}

/// RuoYi 通用响应包装：{ code, msg, data }
///
/// <p>本项目 common-core 全局 Jackson 配置：
/// <ul>
///   <li>{@code code} 序列化为字符串 {@code "success"} / {@code "error"}（不是数字 200）</li>
///   <li>{@code Long} 类型序列化为字符串（如 {@code "9"}）</li>
/// </ul>
/// 与 {@code remote.rs} 中的 {@code R} 结构保持一致。
#[derive(Debug, Deserialize)]
struct R {
    #[serde(default)]
    code: String,
    #[serde(default)]
    msg: String,
    #[serde(default, deserialize_with = "deserialize_lenient_long")]
    data: Option<i64>,
}

/// 兼容 long 字段既可能序列化为数字，也可能序列化为字符串（"42"）。
fn deserialize_lenient_long<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Lenient {
        I64(i64),
        S(String),
    }
    match Lenient::deserialize(deserializer)? {
        Lenient::I64(v) => Ok(Some(v)),
        // null 或空字符串按 None 处理
        Lenient::S(s) if s.trim().is_empty() => Ok(None),
        Lenient::S(s) => s
            .trim()
            .parse::<i64>()
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

/// 提交意见反馈到服务端
///
/// <p>调用 POST {serverUrl}/common/web/feedback/submit，
/// 入参为 {@link FeedbackSubmitInput}，返回服务端生成的新记录 ID。
pub async fn submit_feedback(
    server_url: &str,
    input: &FeedbackSubmitInput,
) -> AppResult<i64> {
    let base = server_url.trim_end_matches('/');
    let url = format!("{}/common/web/feedback/submit", base);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| AppError::Validation(format!("构建 HTTP 客户端失败: {}", e)))?;

    let resp = client
        .post(&url)
        .json(input)
        .send()
        .await
        .map_err(|e| AppError::Validation(format!("请求服务端失败: {}", e)))?
        .error_for_status()
        .map_err(|e| AppError::Validation(format!("服务端返回错误: {}", e)))?;

    // 与现有 remote.rs 保持一致：服务端 Content-Type 未带 charset 时按 GBK 兼容解码
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| AppError::Validation(format!("读取响应内容失败: {}", e)))?;
    let body = decode_response_body(&bytes);

    let parsed: R = serde_json::from_str(&body).map_err(|e| {
        AppError::Validation(format!("解析服务端响应失败: {}", e))
    })?;

    if parsed.code != "success" {
        return Err(AppError::Validation(format!(
            "服务端业务错误: {}",
            if parsed.msg.is_empty() { "未知错误" } else { &parsed.msg }
        )));
    }

    parsed.data.ok_or_else(|| {
        AppError::Validation("服务端未返回反馈记录 ID".to_string())
    })
}

/// 优先按 UTF-8 解码；纯 ASCII/UTF-8 内容直接返回，
/// 失败时回退 GBK（与服务端 application/json 未带 charset 的输出兼容）。
fn decode_response_body(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    let (cow, _, _) = encoding_rs::GBK.decode(bytes);
    cow.into_owned()
}