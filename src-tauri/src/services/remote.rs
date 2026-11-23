use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RemoteProvider {
    // 服务端字段名为 "id"，前端 TS 类型期望 "remoteId"。
    // 用 alias 而非 rename: 反序列化时接受 "id"（来自服务端），序列化时输出 "remoteId"（与前端 TS 接口一致）。
    #[serde(alias = "id")]
    pub remote_id: String,
    pub name: String,
    #[serde(default)]
    pub website: Option<String>,
    /// API 基础地址。兼容服务端旧字段 base_url 与新字段 api_url。
    ///
    /// 服务端 Jackson 全局配置了 `NON_NULL`，`api_url` 为空的条目（如自建网关 One-API）
    /// 会直接省略该字段；这里必须是 `Option` + `default`，否则整页解析会因
    /// `missing field apiUrl` 失败。
    #[serde(default, alias = "base_url", alias = "baseUrl", alias = "api_url")]
    pub api_url: Option<String>,
    /// 服务端可选字段：Anthropic 模式使用的 API 地址
    #[serde(default, alias = "apiAnthropicUrl", alias = "api_anthropic_url")]
    pub api_anthropic_url: Option<String>,
    /// 服务端可选字段：用于无条件覆盖 api_url 的 Responses API URL
    #[serde(default, alias = "apiResponsesUrl", alias = "api_responses_url")]
    pub api_responses_url: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(
        rename = "type",
        default = "default_type",
        deserialize_with = "deserialize_lenient_int"
    )]
    pub r#type: i32,
    #[serde(default)]
    pub tag_list: Vec<String>,
    #[serde(default)]
    pub pricing: Option<String>,
    #[serde(default, deserialize_with = "deserialize_lenient_int")]
    pub sort: i32,
    /// 选择器接口(/picker)在该字段回填服务端关联模型;老接口可能为空
    #[serde(default)]
    pub models: Vec<RemoteProviderModel>,
}

#[derive(Debug, Default, Deserialize, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteProviderModel {
    #[serde(default, alias = "model_name")]
    pub model_name: String,
    #[serde(default = "default_type", deserialize_with = "deserialize_lenient_int")]
    pub r#type: i32,
    #[serde(default)]
    pub type_desc: Option<String>,
    #[serde(default)]
    pub context_window: Option<i64>,
    #[serde(default)]
    pub alias_name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_lenient_int")]
    pub sort: i32,
}

#[derive(Debug, Deserialize, Clone, Serialize)]
pub struct RemoteProviderDetail {
    pub models: Vec<RemoteProviderModel>,
}

fn default_type() -> i32 {
    2
}

/// 站点端分页响应结构（com.ruoyi.common.core.web.page.TableDataInfo）
/// 字段顺序与 Spring 序列化一致：rows/code/msg/total
/// 注意：该分支对 long 类型全局序列化为字符串，total 可能是 "18" 而非 18
#[derive(Debug, Deserialize)]
struct TableDataInfo<T> {
    #[serde(default)]
    rows: Vec<T>,
    #[serde(default)]
    code: String,
    #[serde(default)]
    msg: String,
    #[serde(default, deserialize_with = "deserialize_lenient_int")]
    total: i64,
}

/// 兼容服务端把 long 序列化为字符串（"18"）或数字（18）两种情况
fn deserialize_lenient_int<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + TryFrom<i64> + TryFrom<u64>,
    <T as TryFrom<i64>>::Error: std::fmt::Display,
    <T as TryFrom<u64>>::Error: std::fmt::Display,
    T: std::str::FromStr,
    <T as std::str::FromStr>::Err: std::fmt::Display,
{
    use serde::Deserialize;
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Lenient {
        I64(i64),
        U64(u64),
        S(String),
        /// 服务端偶发显式 null
        Null(()),
    }
    match Lenient::deserialize(deserializer)? {
        Lenient::I64(v) => T::try_from(v).map_err(serde::de::Error::custom),
        Lenient::U64(v) => T::try_from(v).map_err(serde::de::Error::custom),
        Lenient::S(s) => s
            .trim()
            .parse::<T>()
            .map_err(serde::de::Error::custom),
        Lenient::Null(()) => T::try_from(0i64).map_err(serde::de::Error::custom),
    }
}

pub async fn fetch_remote_providers(
    server_url: &str,
    type_filter: Option<i32>,
    keyword: Option<String>,
) -> AppResult<Vec<RemoteProvider>> {
    let base = server_url.trim_end_matches('/');
    // 选择器专用接口:一次返回服务商 + 各自关联模型,免去选中后再二次详情请求
    let mut url = format!(
        "{}/provider/web/service_provider/picker?currentPage=1&pageSize=200",
        base
    );
    if let Some(t) = type_filter {
        url.push_str(&format!("&type={}", t));
    }
    if let Some(kw) = keyword.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        url.push_str(&format!("&name={}", urlencode(kw)));
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| AppError::Validation(format!("构建 HTTP 客户端失败: {}", e)))?;

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| AppError::Validation(format!("请求远程服务端失败: {}", e)))?
        .error_for_status()
        .map_err(|e| AppError::Validation(format!("远程服务端返回错误: {}", e)))?;

    // 服务端 Content-Type 未带 charset 且输出 GBK，需按字节解码
    // （reqwest .text() 默认按 UTF-8 解码，中文会乱码）
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| AppError::Validation(format!("读取响应内容失败: {}", e)))?;

    let body = decode_response_body(&bytes);

    let parsed: TableDataInfo<RemoteProvider> = serde_json::from_str(&body)
        .map_err(|e| AppError::Validation(format!("解析远程响应失败: {}", e)))?;

    if parsed.code != "success" {
        return Err(AppError::Validation(format!(
            "远程服务端业务错误: {} ({}条)",
            if parsed.msg.is_empty() { "未知错误" } else { &parsed.msg },
            parsed.total
        )));
    }

    Ok(parsed.rows)
}

/// 服务端以 GBK 输出（application/json 未带 charset），
/// 优先按 GBK 解码，纯 ASCII/UTF-8 内容同样兼容
fn decode_response_body(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    // 若本身是合法 UTF-8（如纯英文/空响应），直接按 UTF-8
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    let (cow, _, _) = encoding_rs::GBK.decode(bytes);
    cow.into_owned()
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

pub async fn fetch_remote_provider_detail(
    server_url: &str,
    provider_id: &str,
) -> AppResult<RemoteProviderDetail> {
    let base = server_url.trim_end_matches('/');
    let url = format!(
        "{}/provider/web/service_provider/info/{}",
        base,
        urlencode(provider_id),
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| AppError::Validation(format!("构建 HTTP 客户端失败：{}", e)))?;

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| AppError::Validation(format!("请求远程服务端失败：{}", e)))?
        .error_for_status()
        .map_err(|e| AppError::Validation(format!("远程服务端返回错误：{}", e)))?;

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| AppError::Validation(format!("读取响应内容失败：{}", e)))?;

    let body = decode_response_body(&bytes);

    // 实际响应: { code: "success", msg: <nullable>, data: { ..., models: [...] } }
    // RuoYi common-core 全局把 int code / long id 序列化为字符串,所以 code = "success" / "error"
    #[derive(Deserialize)]
    struct R<T> {
        #[serde(default)]
        code: String,
        #[serde(default)]
        msg: String,
        data: T,
    }

    #[derive(Deserialize)]
    struct DetailWrapper {
        #[serde(default)]
        models: Vec<RemoteProviderModel>,
    }

    match serde_json::from_str::<R<DetailWrapper>>(&body) {
        Ok(parsed) if parsed.code == "success" => Ok(RemoteProviderDetail {
            models: parsed.data.models,
        }),
        Ok(parsed) => Err(AppError::Validation(format!(
            "远程服务端业务错误: {}",
            if parsed.msg.is_empty() { "未知错误" } else { &parsed.msg }
        ))),
        Err(e) => Err(AppError::Validation(format!(
            "解析远程响应失败: {}",
            e
        ))),
    }
}
