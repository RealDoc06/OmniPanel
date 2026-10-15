use std::time::Duration;

use omni_core::{AiProvider, AiSettings, ChatMessage};
use reqwest::{Method, RequestBuilder, Url};
use serde_json::{Value, json};

use crate::{AiClient, AiError, Result, default_base_url, default_model};

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

pub(crate) struct Config<'a> {
    pub(crate) provider: AiProvider,
    base: String,
    key: &'a str,
    model: &'a str,
}

impl<'a> Config<'a> {
    pub(crate) fn new(settings: &'a AiSettings) -> Result<Self> {
        let provider = settings.provider;
        let key = settings.api_key.trim();
        reqwest::header::HeaderValue::from_str(key)
            .map_err(|_| AiError::NotConfigured("The API key contains invalid header characters.".into()))?;
        if matches!(provider, AiProvider::Openai | AiProvider::Anthropic | AiProvider::Openrouter) && key.is_empty() {
            return Err(AiError::NotConfigured(format!(
                "Enter an API key for {} in AI settings.",
                crate::provider_name(provider)
            )));
        }
        let base = settings
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|base| !base.is_empty())
            .or_else(|| default_base_url(provider))
            .ok_or_else(|| {
                AiError::NotConfigured(
                    "Set the custom API base URL, including its API prefix (e.g. http://localhost:1234/v1).".into(),
                )
            })?;
        let url = Url::parse(base)
            .map_err(|_| AiError::NotConfigured("The API base URL must be an absolute HTTP or HTTPS URL.".into()))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(AiError::NotConfigured(
                "Use an HTTP or HTTPS API base URL without credentials, query parameters, or a fragment.".into(),
            ));
        }
        Ok(Self { provider, base: url.as_str().trim_end_matches('/').into(), key, model: settings.model.trim() })
    }

    pub(crate) fn model(&self) -> Result<&str> {
        let model = if self.model.is_empty() { default_model(self.provider) } else { self.model };
        if model.is_empty() {
            Err(AiError::NotConfigured("Select a model for the custom endpoint in AI settings.".into()))
        } else {
            Ok(model)
        }
    }

    fn url(&self, path: &str) -> String {
        // Anthropic commonly gets configured with either the root or /v1.
        let path = if self.provider == AiProvider::Anthropic && self.base.ends_with("/v1") {
            path.strip_prefix("v1/").unwrap_or(path)
        } else {
            path
        };
        format!("{}/{path}", self.base)
    }

    fn request(&self, client: &reqwest::Client, method: Method, path: &str, cursor: Option<&str>) -> RequestBuilder {
        let mut url = Url::parse(&self.url(path)).expect("base URL validated by Config");
        if let Some(cursor) = cursor {
            url.query_pairs_mut().append_pair("after_id", cursor);
        }
        let mut request = client.request(method, url);
        if self.provider == AiProvider::Anthropic {
            let mut key = reqwest::header::HeaderValue::from_str(self.key).expect("key validated by Config");
            key.set_sensitive(true);
            request = request.header("x-api-key", key).header("anthropic-version", "2023-06-01");
        } else if !self.key.is_empty() {
            request = request.bearer_auth(self.key);
        }
        request
    }
}

impl AiClient {
    pub(crate) async fn complete(
        &self,
        config: &Config<'_>,
        system: &str,
        messages: &[ChatMessage],
        generation: bool,
    ) -> Result<String> {
        let model = config.model()?;
        let mut chat = vec![json!({"role": "system", "content": system})];
        chat.extend(messages.iter().map(|message| json!(message)));
        let (path, mut body) = match config.provider {
            AiProvider::Openai => {
                let mut body = json!({
                    "model": model, "instructions": system, "input": messages,
                    "store": false, "stream": false,
                    "max_output_tokens": if generation { 8192 } else { 64 },
                });
                if generation {
                    // The domain schema permits arbitrary HA data. JSON mode preserves that
                    // contract without imposing OpenAI's restricted strict-schema subset.
                    body["text"] = json!({"format": {"type": "json_object"}});
                }
                ("responses", body)
            }
            AiProvider::Anthropic => (
                "v1/messages",
                json!({
                    "model": model, "system": system, "messages": messages,
                    "max_tokens": if generation { 4096 } else { 8 }, "stream": false,
                }),
            ),
            AiProvider::Ollama => {
                let mut body = json!({
                    "model": model, "messages": chat, "stream": false,
                    "options": {"num_predict": if generation { 4096 } else { 8 }},
                });
                if generation {
                    body["format"] = json!("json");
                }
                ("api/chat", body)
            }
            AiProvider::Openrouter | AiProvider::Custom => {
                let mut body = json!({
                    "model": model, "messages": chat, "stream": false,
                    "max_tokens": if generation { 4096 } else { 16 },
                });
                if generation {
                    body["response_format"] = json!({"type": "json_object"});
                }
                ("chat/completions", body)
            }
        };
        let send = |body: &Value| {
            config.request(&self.http, Method::POST, path, None).timeout(Duration::from_secs(60)).json(body)
        };
        let response = match self.json(config, send(&body)).await {
            Err(AiError::Provider { status, ref message })
                if generation
                    && matches!(config.provider, AiProvider::Custom | AiProvider::Openrouter)
                    && matches!(status, 400 | 422)
                    && unsupported_json_format(message) =>
            {
                // Some otherwise-compatible local servers/models lack JSON mode.
                body.as_object_mut().expect("request is an object").remove("response_format");
                self.json(config, send(&body)).await?
            }
            other => other?,
        };
        // Reasoning models may spend the entire tiny probe budget before emitting text.
        // Positive token usage still proves that the endpoint accepted the key and model.
        if !generation && truncated_probe(config.provider, &response) {
            return Ok("Model accepted the test request.".into());
        }
        response_text(config.provider, &response)
    }

    pub(crate) async fn models(&self, config: &Config<'_>) -> Result<Vec<String>> {
        let path = match config.provider {
            AiProvider::Anthropic => "v1/models",
            AiProvider::Ollama => "api/tags",
            _ => "models",
        };
        let mut ids = Vec::new();
        let mut cursor: Option<String> = None;
        // Anthropic paginates; bound requests even if an endpoint returns a broken cursor.
        for _ in 0..100 {
            let request =
                config.request(&self.http, Method::GET, path, cursor.as_deref()).timeout(Duration::from_secs(30));
            let response = self.json(config, request).await?;
            let (field, id_field) =
                if config.provider == AiProvider::Ollama { ("models", "name") } else { ("data", "id") };
            let models = response[field]
                .as_array()
                .ok_or_else(|| AiError::InvalidOutput(format!("Model list is missing {field}.")))?;
            for model in models {
                let id = model[id_field]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| AiError::InvalidOutput(format!("Model list entry is missing {id_field}.")))?;
                ids.push(id.to_owned());
            }
            if config.provider != AiProvider::Anthropic || response["has_more"] != true {
                ids.sort_unstable();
                ids.dedup();
                return Ok(ids);
            }
            let next = response["last_id"]
                .as_str()
                .filter(|id| !id.is_empty() && cursor.as_deref() != Some(*id))
                .ok_or_else(|| AiError::InvalidOutput("Model list has an invalid pagination cursor.".into()))?;
            cursor = Some(next.to_owned());
        }
        Err(AiError::InvalidOutput("Model list exceeded 100 pages.".into()))
    }

    async fn json(&self, config: &Config<'_>, request: RequestBuilder) -> Result<Value> {
        let mut response = request.send().await.map_err(http_error)?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(http_error)? {
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(AiError::Provider {
                    status: status.as_u16(),
                    message: "Provider response exceeded 1 MiB.".into(),
                });
            }
            bytes.extend_from_slice(&chunk);
        }
        let parsed = serde_json::from_slice::<Value>(&bytes);
        if !status.is_success()
            || parsed.as_ref().is_ok_and(|value| value.get("error").is_some_and(|error| !error.is_null()))
        {
            let message = parsed
                .as_ref()
                .ok()
                .and_then(|value| {
                    value
                        .pointer("/error/message")
                        .or_else(|| value.get("error"))
                        .or_else(|| value.get("message"))?
                        .as_str()
                })
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    let text = String::from_utf8_lossy(&bytes);
                    if text.trim().is_empty() { status.to_string() } else { text.trim().chars().take(1000).collect() }
                });
            let message = if config.key.is_empty() { message } else { message.replace(config.key, "[redacted]") };
            return Err(AiError::Provider { status: status.as_u16(), message });
        }
        parsed.map_err(|error| AiError::InvalidOutput(format!("Provider response is not JSON: {error}")))
    }
}

fn http_error(error: reqwest::Error) -> AiError {
    AiError::Http(error.without_url().to_string())
}

fn unsupported_json_format(message: &str) -> bool {
    let message = message.to_lowercase();
    (message.contains("response_format") || message.contains("json_object"))
        && ["unsupported", "not support", "unknown", "unrecognized", "unexpected"]
            .iter()
            .any(|word| message.contains(word))
}

fn truncated_probe(provider: AiProvider, response: &Value) -> bool {
    let (truncated, tokens) = match provider {
        AiProvider::Openai => (
            response["status"] == "incomplete" && response["incomplete_details"]["reason"] == "max_output_tokens",
            &response["usage"]["output_tokens"],
        ),
        AiProvider::Anthropic => (response["stop_reason"] == "max_tokens", &response["usage"]["output_tokens"]),
        AiProvider::Ollama => {
            (response["done"] == true && response["done_reason"] == "length", &response["eval_count"])
        }
        AiProvider::Custom | AiProvider::Openrouter => {
            (response["choices"][0]["finish_reason"] == "length", &response["usage"]["completion_tokens"])
        }
    };
    truncated && tokens.as_u64().is_some_and(|tokens| tokens > 0)
}

pub(crate) fn response_text(provider: AiProvider, response: &Value) -> Result<String> {
    if let Some(error) = response.get("error").filter(|error| !error.is_null()) {
        return Err(AiError::Provider {
            status: 200,
            message: error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
                .unwrap_or("Provider returned an error.")
                .into(),
        });
    }
    let invalid = |reason: &str| AiError::InvalidOutput(reason.into());
    let text = match provider {
        AiProvider::Openai => {
            if let Some(status) = response["status"].as_str()
                && status != "completed"
            {
                return Err(invalid(&format!(
                    "Response did not complete ({status}); check the model's output-token limit."
                )));
            }
            let output = response["output"].as_array().ok_or_else(|| invalid("Responses API reply has no output."))?;
            let mut text = String::new();
            for message in output.iter().filter(|item| item["type"] == "message") {
                if let Some(content) = message["content"].as_array() {
                    for block in content {
                        if block["type"] == "refusal" {
                            return Err(invalid(
                                block["refusal"].as_str().unwrap_or("The model refused this request."),
                            ));
                        }
                        if block["type"] == "output_text"
                            && let Some(part) = block["text"].as_str()
                        {
                            text.push_str(part);
                        }
                    }
                }
            }
            text
        }
        AiProvider::Anthropic => {
            if response["stop_reason"] == "max_tokens" || response["stop_reason"] == "refusal" {
                return Err(invalid("Anthropic response was truncated or refused."));
            }
            response["content"]
                .as_array()
                .ok_or_else(|| invalid("Messages API reply has no content."))?
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .collect()
        }
        AiProvider::Ollama => {
            if response["done"] == false || response["done_reason"] == "length" {
                return Err(invalid("Ollama response did not complete."));
            }
            response.pointer("/message/content").and_then(Value::as_str).unwrap_or_default().into()
        }
        AiProvider::Openrouter | AiProvider::Custom => {
            let choice = &response["choices"][0];
            if matches!(choice["finish_reason"].as_str(), Some("length" | "content_filter")) {
                return Err(invalid("Chat completion was truncated or filtered."));
            }
            if let Some(refusal) = choice.pointer("/message/refusal").and_then(Value::as_str) {
                return Err(invalid(refusal));
            }
            choice.pointer("/message/content").and_then(Value::as_str).unwrap_or_default().into()
        }
    };
    if text.trim().is_empty() { Err(invalid("Provider returned no text.")) } else { Ok(text) }
}
