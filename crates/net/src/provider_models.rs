use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::future::Future;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_BODY: usize = 2 * 1024 * 1024;
const MAX_MODELS: usize = 2000;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ProviderModel {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort_levels: Option<Vec<EffortLevel>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<EffortLevel>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum EffortLevel {
    None,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
    Ultra,
}

impl EffortLevel {
    pub const ALL: [Self; 8] = [
        Self::None, Self::Minimal, Self::Low, Self::Medium, Self::High, Self::XHigh, Self::Max, Self::Ultra,
    ];
    pub const STANDARD: [Self; 5] = [Self::Low, Self::Medium, Self::High, Self::XHigh, Self::Max];

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|level| level.as_str() == value)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
            Self::Ultra => "ultra",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ProviderCatalog {
    pub models: Vec<ProviderModel>,
    pub source_url: String,
    pub partial: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProviderModelCheck {
    pub healthy: bool,
    pub message: String,
    pub latency_ms: u64,
    pub checked_at_ms: u64,
}

pub struct ProviderReply { pub status: u16, pub body: Vec<u8> }

pub trait ProviderTransport {
    fn request(&self, url: &str, header: (&str, &str), body: Option<Value>) -> impl Future<Output = Result<ProviderReply, String>> + Send;
}

pub struct ReqwestProviderModels { client: Client }

impl ReqwestProviderModels {
    pub fn new(proxy: Option<&str>) -> Result<Self, String> {
        let mut builder = Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(8)).timeout(Duration::from_secs(30));
        if let Some(proxy) = proxy { builder = builder.proxy(reqwest::Proxy::all(proxy).map_err(|_| "本机代理地址无效")?); }
        Ok(Self { client: builder.build().map_err(|_| "无法创建模型请求客户端")? })
    }
}

impl ProviderTransport for ReqwestProviderModels {
    async fn request(&self, url: &str, header: (&str, &str), body: Option<Value>) -> Result<ProviderReply, String> {
        let mut request = if let Some(body) = body {
            let request = self.client.post(url).json(&body);
            if body["stream"] == true {
                request.header("accept", "text/event-stream")
            } else {
                request
            }
        } else {
            self.client.get(url)
        };
        request = request.header(header.0, header.1).header("anthropic-version", "2023-06-01");
        let mut response = request.send().await.map_err(|e| if e.is_timeout() { "连接超时，请检查网络后重试" } else { "无法连接模型接口，请检查地址、TLS 证书与代理" }.to_string())?;
        let status = response.status().as_u16();
        if response.content_length().is_some_and(|size| size > MAX_BODY as u64) { return Err("接口响应过大，已停止读取".into()); }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "读取模型接口响应失败")? {
            if body.len() + chunk.len() > MAX_BODY { return Err("接口响应过大，已停止读取".into()); }
            body.extend_from_slice(&chunk);
        }
        Ok(ProviderReply { status, body })
    }
}

pub fn provider_base(raw: &str) -> Result<Url, String> {
    let mut url = Url::parse(raw.trim()).map_err(|_| "请填写有效的 HTTPS 接口地址")?;
    if url.scheme() != "https" || url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
        return Err("模型接口必须是 HTTPS 地址，不能包含用户名、密钥、查询参数或片段".into());
    }
    let path = url.path().trim_end_matches('/').strip_suffix("/v1").unwrap_or(url.path().trim_end_matches('/')).to_string();
    if path.split('/').any(|segment| matches!(segment.to_ascii_lowercase().as_str(), "users" | "balance" | "billing" | "oauth" | "account")) {
        return Err("模型请求不能指向账户或账单接口".into());
    }
    url.set_path(&path);
    Ok(url)
}

pub fn is_model_studio(base: &Url) -> bool {
    base.scheme() == "https"
        && base.host_str().is_some_and(|host| {
            matches!(host, "dashscope.aliyuncs.com" | "dashscope-intl.aliyuncs.com")
                || host.ends_with(".maas.aliyuncs.com")
        })
}

pub fn provider_catalog_urls(base: &Url) -> Vec<String> {
    let mut url = base.clone();
    let path = base.path().trim_end_matches('/');
    match (base.host_str().unwrap_or(""), path) {
        ("api.deepseek.com", "/anthropic") => { url.set_path("/models"); vec![url.to_string()] }
        ("api.moonshot.cn" | "api.moonshot.ai", "/anthropic") => { url.set_path("/v1/models"); vec![url.to_string()] }
        ("token-plan.cn-beijing.maas.aliyuncs.com", "/apps/anthropic") => {
            url.set_path("/compatible-mode/v1/models");
            vec![url.to_string()]
        }
        ("open.bigmodel.cn", "/api/anthropic") => {
            url.set_path("/api/anthropic/v1/models"); let compatible = url.to_string();
            url.set_path("/api/paas/v4/models"); vec![compatible, url.to_string()]
        }
        _ => { url.set_path(&format!("{path}/v1/models")); vec![url.to_string()] }
    }
}

pub fn provider_api_model(raw: &str) -> Result<String, String> {
    let model = raw.trim();
    if model.is_empty() || model.len() > 256 || model.chars().any(char::is_control) { return Err("请填写有效的模型 ID".into()); }
    Ok(model.strip_suffix("[1m]").unwrap_or(model).to_string())
}

pub fn is_kimi_code_endpoint(url: &Url) -> bool {
    url.scheme() == "https"
        && matches!(url.host_str(), Some("api.kimi.com" | "api.kimi.ai"))
        && (url.path() == "/coding" || url.path().starts_with("/coding/"))
}

pub fn kimi_code_concurrency_message(body: &[u8]) -> Option<&'static str> {
    let error: Value = serde_json::from_slice(body).ok()?;
    error
        .pointer("/error/message")?
        .as_str()?
        .trim_start()
        .starts_with("You've reached your concurrent request limit")
        .then_some("Kimi Code 返回并发请求限制，请稍后手动重试")
}

fn http_error(status: u16) -> String {
    let text = match status {
        301..=399 => "接口返回重定向，为保护密钥没有继续跳转，请核对接口地址",
        401 => "认证失败，请检查 API 密钥及所属平台",
        402 => "余额或额度不足，请检查服务商账户",
        403 => "接口拒绝访问，请检查模型权限、地区或套餐",
        404 | 405 => "接口不支持此请求，或该模型不存在",
        429 => "请求频率或额度受限，请稍后重试",
        400 | 422 => "服务商拒绝了模型或参数，请核对模型 ID 与兼容接口",
        500..=599 => "服务商暂时不可用，请稍后重试",
        _ => "模型接口返回异常状态",
    };
    format!("{text}（HTTP {status}）")
}

fn parse_efforts(item: &Value, openrouter: bool) -> Option<Vec<EffortLevel>> {
    let levels = EffortLevel::ALL;
    let mandatory = item.pointer("/reasoning/mandatory") == Some(&Value::Bool(true));
    if let Some(effort) = item.pointer("/capabilities/effort") {
        if effort["supported"] == false {
            return Some(Vec::new());
        }
        if levels.iter().any(|level| effort.get(level.as_str()).is_some()) {
            return Some(levels.into_iter()
                .filter(|level| effort[level.as_str()]["supported"] == true)
                .collect());
        }
    }
    let supported = match item.pointer("/reasoning/supported_efforts") {
        // OpenRouter's unrestricted list uses its public gateway ladder.
        Some(Value::Null) if openrouter => return Some(levels.into_iter()
            .filter(|&level| level != EffortLevel::Ultra && (!mandatory || level != EffortLevel::None))
            .collect()),
        Some(Value::Array(supported)) => supported,
        _ if openrouter && item.get("reasoning").is_some_and(Value::is_object) => return Some(Vec::new()),
        _ => item.pointer("/effort/supported_levels")?.as_array()?,
    };
    Some(levels.into_iter()
        .filter(|&level| !mandatory || level != EffortLevel::None)
        .filter(|level| supported.iter().any(|candidate| candidate == level.as_str()))
        .collect())
}

fn parse_catalog(bytes: &[u8], openrouter: bool) -> Result<(Vec<ProviderModel>, bool, String), String> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| "接口没有返回有效 JSON 模型列表")?;
    if value.get("error").is_some() { return Err("接口返回错误，未获取模型列表".into()); }
    let data = value.get("data").or_else(|| value.get("models")).and_then(Value::as_array).or_else(|| value.as_array()).ok_or("接口响应中没有模型列表")?;
    let mut models = BTreeMap::new();
    for item in data.iter().take(MAX_MODELS) {
        let id = item.as_str().or_else(|| item.get("id").and_then(Value::as_str)).unwrap_or("");
        if id.is_empty() || id.len() > 256 || id.chars().any(|ch| ch.is_whitespace() || ch.is_control()) { continue; }
        let name = item.get("display_name").or_else(|| item.get("name")).and_then(Value::as_str).unwrap_or(id).chars().filter(|c| !c.is_control()).take(160).collect();
        let effort_levels = parse_efforts(item, openrouter);
        let default_effort = item.pointer("/reasoning/default_effort").or_else(|| item.pointer("/effort/default_level"))
            .and_then(Value::as_str).and_then(EffortLevel::parse)
            .filter(|level| effort_levels.as_ref().is_some_and(|levels| levels.contains(level)));
        models.insert(id.to_string(), ProviderModel { id: id.into(), name, effort_levels, default_effort });
    }
    let last = value.get("last_id").and_then(Value::as_str).or_else(|| data.last().and_then(|item| item.get("id")).and_then(Value::as_str)).unwrap_or("").to_string();
    Ok((models.into_values().collect(), value.get("has_more").and_then(Value::as_bool).unwrap_or(false) || data.len() > MAX_MODELS, last))
}

pub async fn fetch_provider_catalog<T: ProviderTransport + Sync>(transport: &T, base: &Url, header: (&str, &str)) -> Result<ProviderCatalog, String> {
    let urls = provider_catalog_urls(base);
    fetch_provider_catalog_from_urls(transport, &urls, header).await
}

pub async fn fetch_provider_catalog_from_urls<T: ProviderTransport + Sync>(transport: &T, urls: &[String], header: (&str, &str)) -> Result<ProviderCatalog, String> {
    for (index, endpoint) in urls.iter().enumerate() {
        let openrouter = Url::parse(endpoint).is_ok_and(|url| {
            url.scheme() == "https" && url.host_str() == Some("openrouter.ai")
        });
        let mut url = endpoint.clone();
        let mut models = BTreeMap::new();
        let mut cursors = Vec::new();
        for page in 0..10 {
            let reply = transport.request(&url, header, None).await?;
            if matches!(reply.status, 404 | 405) && index + 1 < urls.len() && page == 0 { break; }
            if !(200..300).contains(&reply.status) {
                if matches!(reply.status, 404 | 405) { return Err("此服务商未提供可用的模型列表接口，可继续手动填写模型 ID 并验证可用性".into()); }
                return Err(http_error(reply.status));
            }
            let (items, more, last) = parse_catalog(&reply.body, openrouter)?;
            let secret = header.1.strip_prefix("Bearer ").unwrap_or(header.1);
            for mut item in items {
                if !secret.is_empty() {
                    if item.id.contains(secret) { continue; }
                    item.name = item.name.replace(secret, "***");
                }
                if models.len() < MAX_MODELS { models.insert(item.id.clone(), item); }
            }
            let stop = !more || page == 9 || models.len() >= MAX_MODELS || last.is_empty() || cursors.contains(&last);
            if stop {
                if models.is_empty() { return Err("服务商返回的模型列表为空，可手动填写模型 ID 后验证".into()); }
                return Ok(ProviderCatalog { models: models.into_values().collect(), source_url: endpoint.clone(), partial: more });
            }
            cursors.push(last.clone());
            let mut next = Url::parse(endpoint).map_err(|_| "模型列表地址无效")?;
            next.query_pairs_mut().append_pair("after_id", &last);
            url = next.to_string();
        }
    }
    Err("此服务商未提供可用的模型列表接口，可手动填写模型 ID 后验证".into())
}

pub async fn verify_provider_model<T: ProviderTransport + Sync>(transport: &T, base: &Url, header: (&str, &str), raw_model: &str) -> Result<ProviderModelCheck, String> {
    let model = provider_api_model(raw_model)?;
    let mut endpoint = base.clone(); endpoint.set_path(&format!("{}/v1/messages", base.path().trim_end_matches('/')));
    let mut body = json!({ "model": model, "max_tokens": 32, "stream": false, "messages": [{ "role": "user", "content": "Reply with OK only." }] });
    if model == "kimi-k2.7-code" { body["thinking"] = json!({"type":"enabled","budget_tokens":1024}); body["max_tokens"] = json!(1056); }
    if model == "qwen4-max" && base.path() == "/apps/anthropic" && is_model_studio(base) {
        body["thinking"] = json!({"type": "disabled"});
    }
    let start = Instant::now();
    let result = match transport.request(endpoint.as_str(), header, Some(body)).await {
        Err(err) => Err(err),
        Ok(reply) if matches!(reply.status, 403 | 429) && is_kimi_code_endpoint(base) => {
            let message = kimi_code_concurrency_message(&reply.body)
                .map(|message| format!("{message}（上游 HTTP {}）", reply.status))
                .unwrap_or_else(|| http_error(reply.status));
            Err(message)
        }
        Ok(reply) if !(200..300).contains(&reply.status) => Err(http_error(reply.status)),
        Ok(reply) => {
            let value: Value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
            let content = value.get("content").and_then(Value::as_array);
            let valid = value.get("error").is_none() && value.get("type").and_then(Value::as_str) == Some("message")
                && value.get("role").and_then(Value::as_str) == Some("assistant")
                && value.get("model").and_then(Value::as_str).is_some_and(|m| !m.is_empty())
                && content.is_some_and(|items| items.iter().any(|item| match item.get("type").and_then(Value::as_str) {
                    Some("text") => item.get("text").and_then(Value::as_str).is_some_and(|text| !text.trim().is_empty()),
                    Some("thinking") => item.get("thinking").and_then(Value::as_str).is_some_and(|text| !text.trim().is_empty()),
                    _ => false,
                }));
            if valid { Ok("模型调用通过".to_string()) } else { Err("接口有响应，但未返回有效的模型生成结果，尚不能确认可用".into()) }
        }
    };
    Ok(ProviderModelCheck { healthy: result.is_ok(), message: result.unwrap_or_else(|error| error), latency_ms: start.elapsed().as_millis() as u64, checked_at_ms: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    struct Stub { replies: Mutex<VecDeque<ProviderReply>>, calls: Mutex<Vec<(String, Option<Value>)>> }
    impl ProviderTransport for Stub {
        async fn request(&self, url: &str, _: (&str, &str), body: Option<Value>) -> Result<ProviderReply, String> {
            self.calls.lock().unwrap().push((url.into(), body));
            self.replies.lock().unwrap().pop_front().ok_or("test timeout".into())
        }
    }
    fn stub(replies: Vec<(u16, Value)>) -> Stub { Stub { replies: Mutex::new(replies.into_iter().map(|(status, body)| ProviderReply { status, body: serde_json::to_vec(&body).unwrap() }).collect()), calls: Mutex::new(vec![]) } }

    #[tokio::test]
    async fn catalog_keeps_only_explicit_effort_levels() {
        use EffortLevel::{High, Low, Max, XHigh};
        let directory = json!({"data":[
            {"id":"native", "capabilities":{"effort":{"supported":true, "high":{"supported":true}, "max":{"supported":true}, "low":{"supported":false}}}},
            {"id":"disabled", "capabilities":{"effort":{"supported":false, "high":{"supported":true}}}},
            {"id":"toggle-only", "reasoning":{"default_enabled":true}},
            {"id":"unified", "reasoning":{"supported_efforts":["none","xhigh","low","xhigh","no-such-level"]}},
            {"id":"unrestricted", "reasoning":{"supported_efforts":null}},
            {"id":"mandatory", "reasoning":{"mandatory":true,"supported_efforts":["none","low","high"]}},
            {"id":"undocumented", "supported_parameters":["reasoning"]}
        ]});
        let transport = stub(vec![(200, directory.clone()), (200, directory)]);
        let levels = async |url| {
            let catalog = fetch_provider_catalog(
                &transport, &provider_base(url).unwrap(), ("x-api-key", "fixture-key")
            ).await.unwrap();
            catalog.models.into_iter().map(|model| (model.id, model.effort_levels)).collect::<BTreeMap<_,_>>()
        };
        let router = levels("https://openrouter.ai/api").await;
        assert_eq!(router["native"], Some(vec![High, Max]));
        assert_eq!(router["disabled"], Some(Vec::new()));
        assert_eq!(router["toggle-only"], Some(Vec::new()));
        assert_eq!(router["unified"], Some(vec![EffortLevel::None, Low, XHigh]));
        assert_eq!(router["unrestricted"], Some(EffortLevel::ALL[..7].to_vec()));
        assert_eq!(router["mandatory"], Some(vec![Low, High]));
        assert_eq!(router["undocumented"], None);
        let custom = levels("https://example.invalid/anthropic").await;
        assert_eq!(custom["native"], Some(vec![High, Max]));
        assert_eq!(custom["unrestricted"], None);
    }
    #[test]
    fn catalog_routes_preserve_custom_paths_and_use_vendor_catalogs() {
        assert_eq!(provider_catalog_urls(&provider_base("https://api.deepseek.com/anthropic/v1/").unwrap()), ["https://api.deepseek.com/models"]);
        assert_eq!(provider_catalog_urls(&provider_base("https://api.moonshot.cn/anthropic").unwrap()), ["https://api.moonshot.cn/v1/models"]);
        assert_eq!(provider_catalog_urls(&provider_base("https://gateway.example/anthropic").unwrap()), ["https://gateway.example/anthropic/v1/models"]);
        assert_eq!(provider_catalog_urls(&provider_base("https://gateway.example/apps/anthropic").unwrap()), ["https://gateway.example/apps/anthropic/v1/models"]);
        for url in ["http://example.com", "https://user:secret@example.com", "https://example.com?key=secret", "https://example.com/billing"] { assert!(provider_base(url).is_err()); }
    }

    #[tokio::test]
    async fn token_plan_lists_models_via_openai_and_verifies_via_anthropic() {
        let base = provider_base("https://token-plan.cn-beijing.maas.aliyuncs.com/apps/anthropic/v1/").unwrap();
        let header = ("Authorization", "Bearer test-key");
        let client = stub(vec![
            (200, json!({"object":"list","data":[{"id":"qwen4-max","object":"model"}]})),
            (200, json!({"type":"message","role":"assistant","model":"qwen4-max","content":[{"type":"text","text":"OK"}]})),
        ]);

        let catalog = fetch_provider_catalog(&client, &base, header).await.unwrap();
        assert_eq!(catalog.models[0].id, "qwen4-max");
        assert!(!catalog.partial);
        assert!(verify_provider_model(&client, &base, header, &catalog.models[0].id).await.unwrap().healthy);

        let calls = client.calls.lock().unwrap();
        assert_eq!(calls[0], ("https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/models".into(), None));
        assert_eq!(calls[1].0, "https://token-plan.cn-beijing.maas.aliyuncs.com/apps/anthropic/v1/messages");
        let body = calls[1].1.as_ref().unwrap();
        assert_eq!(body["model"], "qwen4-max");
        assert_eq!(body["thinking"], json!({"type": "disabled"}));
    }

    #[tokio::test]
    async fn openrouter_uses_its_catalog_and_anthropic_message_endpoints() {
        let base = provider_base("https://openrouter.ai/api/v1/").unwrap();
        let client = stub(vec![
            (200, json!({"data":[{"id":"anthropic/claude-sonnet-4.6","name":"Claude Sonnet"},{"id":"qwen/qwen3-coder","name":"Qwen Coder"}]})),
            (200, json!({"type":"message","role":"assistant","model":"anthropic/claude-sonnet-4.6","content":[{"type":"text","text":"OK"}]})),
        ]);
        let catalog = fetch_provider_catalog(&client, &base, ("Authorization", "Bearer test-key")).await.unwrap();
        assert_eq!(catalog.source_url, "https://openrouter.ai/api/v1/models");
        assert_eq!(catalog.models.len(), 2);
        assert!(verify_provider_model(&client, &base, ("Authorization", "Bearer test-key"), "~anthropic/claude-sonnet-latest[1m]").await.unwrap().healthy);
        let calls = client.calls.lock().unwrap();
        assert_eq!(calls[1].0, "https://openrouter.ai/api/v1/messages");
        assert_eq!(calls[1].1.as_ref().unwrap()["model"], "~anthropic/claude-sonnet-latest");
    }

    #[tokio::test]
    async fn catalog_paginates_deduplicates_and_never_follows_remote_next_urls() {
        let client = stub(vec![(200,json!({"data":[{"id":"b"},{"id":"b"}],"has_more":true,"last_id":"b","next":"https://evil.example"})),(200,json!({"data":[{"id":"a","display_name":"Model A"}],"has_more":false}))]);
        let catalog = fetch_provider_catalog(&client,&provider_base("https://gateway.example").unwrap(),("Authorization","Bearer secret")).await.unwrap();
        assert_eq!(catalog.models.iter().map(|m|m.id.as_str()).collect::<Vec<_>>(),["a","b"]);
        assert!(!catalog.partial);
        let calls = client.calls.lock().unwrap(); assert!(calls[1].0.contains("after_id=b")); assert!(!calls[1].0.contains("evil"));
        assert!(!serde_json::to_string(&catalog).unwrap().contains("secret"));
    }
    #[tokio::test]
    async fn auth_errors_do_not_retry_other_endpoints_or_echo_response_secrets() {
        let client = stub(vec![(401,json!({"error":"secret-from-service"}))]);
        let error = fetch_provider_catalog(&client,&provider_base("https://open.bigmodel.cn/api/anthropic").unwrap(),("Authorization","Bearer secret")).await.unwrap_err();
        assert!(error.contains("401") && !error.contains("secret")); assert_eq!(client.calls.lock().unwrap().len(),1);
    }
    #[tokio::test]
    async fn unsupported_or_empty_catalog_is_never_a_successful_fabricated_list() {
        for (status,body) in [(404,json!({})),(200,json!({"data":[]})),(200,json!({"error":"failure"})),(200,json!({"html":"login"}))] {
            assert!(fetch_provider_catalog(&stub(vec![(status,body)]),&provider_base("https://gateway.example").unwrap(),("x-api-key","secret")).await.is_err());
        }
    }
    #[tokio::test]
    async fn only_a_real_generated_message_is_healthy_and_context_suffix_is_not_sent() {
        let client = stub(vec![(200,json!({"type":"message","role":"assistant","model":"model-a","content":[{"type":"text","text":"OK"}]}))]);
        let check = verify_provider_model(&client,&provider_base("https://gateway.example/anthropic").unwrap(),("Authorization","Bearer secret"),"model-a[1m]").await.unwrap();
        assert!(check.healthy);
        let calls = client.calls.lock().unwrap(); assert!(calls[0].0.ends_with("/anthropic/v1/messages")); assert_eq!(calls[0].1.as_ref().unwrap()["model"],"model-a");
        assert!(!serde_json::to_string(&check).unwrap().contains("secret"));
    }
    #[tokio::test]
    async fn status_200_alone_and_rejected_models_never_turn_green() {
        for (status,body) in [(401,json!({})),(429,json!({})),(200,json!({"data":[{"id":"a"}]})),(200,json!({"type":"message","role":"assistant","model":"a","content":[]}))] {
            let result = verify_provider_model(&stub(vec![(status,body)]),&provider_base("https://gateway.example").unwrap(),("x-api-key","secret"),"a").await.unwrap(); assert!(!result.healthy);
        }
    }

    #[tokio::test]
    async fn kimi_model_verification_explains_concurrency_rejection() {
        let message = "You've reached your concurrent request limit. Please wait for your ongoing requests to finish and try again.";
        let client = stub(vec![(403, json!({"error": {"type": "permission_error", "message": message}}))]);
        let result = verify_provider_model(
            &client,
            &provider_base("https://api.kimi.com/coding").unwrap(),
            ("x-api-key", "fixture-key"),
            "k3.1-pro",
        )
        .await
        .unwrap();
        assert!(!result.healthy);
        assert_eq!(result.message, "Kimi Code 返回并发请求限制，请稍后手动重试（上游 HTTP 403）");
        assert_eq!(client.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn thinking_only_models_keep_their_required_protocol() {
        let client = stub(vec![(200,json!({"type":"message","role":"assistant","model":"kimi-k2.7-code","content":[{"type":"thinking","thinking":"response"}]}))]);
        assert!(verify_provider_model(&client,&provider_base("https://api.moonshot.cn/anthropic").unwrap(),("Authorization","Bearer secret"),"kimi-k2.7-code").await.unwrap().healthy);
        assert_eq!(client.calls.lock().unwrap()[0].1.as_ref().unwrap()["thinking"]["type"],"enabled");
    }
    #[tokio::test]
    async fn catalog_never_returns_a_credential_echoed_as_a_model() {
        let client=stub(vec![(200,json!({"data":[{"id":"top-secret"},{"id":"real-model","name":"Echo top-secret"}]}))]);
        let catalog=fetch_provider_catalog(&client,&provider_base("https://gateway.example").unwrap(),("Authorization","Bearer top-secret")).await.unwrap();
        assert_eq!(catalog.models.len(),1); assert!(!serde_json::to_string(&catalog).unwrap().contains("top-secret"));
    }
    #[tokio::test]
    async fn real_transport_sends_authentication_and_never_forwards_it_to_redirects() {
        use std::io::{Read,Write};
        let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port=listener.local_addr().unwrap().port();
        let server=std::thread::spawn(move || {
            let (mut stream,_)=listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut bytes=Vec::new(); let mut chunk=[0;4096];
            while !bytes.windows(4).any(|part|part==b"\r\n\r\n") { let count=stream.read(&mut chunk).unwrap(); if count==0 { break; } bytes.extend_from_slice(&chunk[..count]); }
            stream.write_all(format!("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/target\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").as_bytes()).unwrap();
            String::from_utf8_lossy(&bytes).to_lowercase()
        });
        let client=ReqwestProviderModels::new(None).unwrap();
        let result=client.request(&format!("http://127.0.0.1:{port}/models"),("Authorization","Bearer test-only-secret"),None).await.unwrap();
        assert_eq!(result.status,302); assert!(server.join().unwrap().contains("authorization: bearer test-only-secret"));
    }
}
