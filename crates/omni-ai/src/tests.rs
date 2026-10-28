use std::{collections::BTreeMap, time::Duration};

use omni_core::{Action, MediaCommand};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
    task::JoinHandle,
};

use super::*;

const PROVIDERS: [AiProvider; 5] =
    [AiProvider::Openai, AiProvider::Anthropic, AiProvider::Ollama, AiProvider::Openrouter, AiProvider::Custom];

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Value,
}

struct Mock {
    url: String,
    requests: mpsc::Receiver<Request>,
    task: JoinHandle<()>,
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Mock {
    async fn new(responses: Vec<(u16, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (sender, requests) = mpsc::channel(16);
        let task = tokio::spawn(async move {
            for (status, response) in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let header_end = loop {
                    let mut buffer = [0; 4096];
                    let count = stream.read(&mut buffer).await.unwrap();
                    assert!(count > 0, "request ended before its headers");
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                        break end + 4;
                    }
                };
                let head = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
                let mut lines = head.lines();
                let mut first = lines.next().unwrap().split_whitespace();
                let method = first.next().unwrap().to_owned();
                let path = first.next().unwrap().to_owned();
                let headers: BTreeMap<_, _> = lines
                    .filter_map(|line| line.split_once(':'))
                    .map(|(name, value)| (name.to_lowercase(), value.trim().to_owned()))
                    .collect();
                let length = headers.get("content-length").map_or(0, |value| value.parse::<usize>().unwrap());
                while bytes.len() < header_end + length {
                    let mut buffer = [0; 4096];
                    let count = stream.read(&mut buffer).await.unwrap();
                    assert!(count > 0, "request ended before its body");
                    bytes.extend_from_slice(&buffer[..count]);
                }
                let body = if length == 0 {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap()
                };
                sender.send(Request { method, path, headers, body }).await.unwrap();
                let head = format!(
                    "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                );
                stream.write_all(head.as_bytes()).await.unwrap();
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.shutdown().await.unwrap();
            }
        });
        Self { url, requests, task }
    }

    async fn json(responses: Vec<Value>) -> Self {
        Self::new(responses.into_iter().map(|body| (200, body.to_string())).collect()).await
    }

    async fn next(&mut self) -> Request {
        tokio::time::timeout(Duration::from_secs(3), self.requests.recv()).await.unwrap().unwrap()
    }

    async fn done(&mut self) {
        tokio::time::timeout(Duration::from_secs(3), &mut self.task).await.unwrap().unwrap();
    }

    fn settings(&self, provider: AiProvider) -> AiSettings {
        let prefix = match provider {
            AiProvider::Openai | AiProvider::Openrouter | AiProvider::Custom => "/proxy/v1/",
            _ => "/proxy/",
        };
        AiSettings {
            provider,
            model: "test-model".into(),
            api_key: "test-key".into(),
            base_url: Some(format!("{}{prefix}", self.url)),
        }
    }
}

fn client() -> AiClient {
    AiClient::new(reqwest::Client::builder().no_proxy()).unwrap()
}

fn request() -> GenerateRequest {
    GenerateRequest {
        prompt: "Pause music".into(),
        history: vec![
            ChatMessage { role: ChatRole::User, content: "Control Spotify".into() },
            ChatMessage { role: ChatRole::Assistant, content: "Previous result".into() },
        ],
        current: Some(Action::Media { command: MediaCommand::Next }),
    }
}

fn control() -> Value {
    json!({"label":"Pause music","icon":"pause","action":{"type":"media","command":"playPause"},"explanation":"Pause or resume playback."})
}

fn reply(provider: AiProvider, text: &str) -> Value {
    match provider {
        AiProvider::Openai => json!({"status":"completed","output":[
            {"type":"reasoning","summary":[]},
            {"type":"message","content":[{"type":"output_text","text":text}]}
        ]}),
        AiProvider::Anthropic => {
            json!({"content":[{"type":"thinking","thinking":"ignored"},{"type":"text","text":text}],"stop_reason":"end_turn"})
        }
        AiProvider::Ollama => {
            json!({"message":{"role":"assistant","content":text},"done":true,"done_reason":"stop"})
        }
        _ => json!({"choices":[{"message":{"content":text},"finish_reason":"stop"}]}),
    }
}

fn generation_path(provider: AiProvider) -> &'static str {
    match provider {
        AiProvider::Openai => "/proxy/v1/responses",
        AiProvider::Anthropic => "/proxy/v1/messages",
        AiProvider::Ollama => "/proxy/api/chat",
        _ => "/proxy/v1/chat/completions",
    }
}

fn assert_auth(request: &Request, provider: AiProvider, key: bool) {
    if provider == AiProvider::Anthropic {
        assert_eq!(request.headers.get("x-api-key").unwrap(), "test-key");
        assert_eq!(request.headers.get("anthropic-version").unwrap(), "2023-06-01");
        assert!(!request.headers.contains_key("authorization"));
    } else if key {
        assert_eq!(request.headers.get("authorization").unwrap(), "Bearer test-key");
    } else {
        assert!(!request.headers.contains_key("authorization"));
    }
}

#[test]
fn extracts_fences_prose_nested_objects_and_escaped_braces() {
    let text = r#"{"label":"a } { \"quote\"","action":{"type":"delay","ms":0}}"#;
    for wrapped in [
        text.to_owned(),
        format!("```json\n{text}\n```"),
        format!("Here is your control:\n{text}\nExplanation {{ignored}}"),
    ] {
        assert_eq!(extract_json(&wrapped), text);
        assert!(serde_json::from_str::<Value>(extract_json(&wrapped)).is_ok());
    }
    assert_eq!(extract_json("no JSON here"), "no JSON here");
    assert_eq!(extract_json("```json\n{\"broken\": "), "{\"broken\":");
}

#[test]
fn prompt_contains_actual_schema_every_action_and_machine_facts() {
    let context = AiContext {
        os: "windows".into(),
        obs_scenes: vec!["BRB".into()],
        obs_inputs: vec!["Desk mic".into()],
        audio_apps: vec!["Discord".into()],
        voice_presets: vec!["robot".into()],
        home_assistant: true,
    };
    let system = prompt::system(&context);
    let schema: Value = serde_json::from_str(system.split_once("GeneratedControl JSON schema:\n").unwrap().1).unwrap();
    assert_eq!(schema, serde_json::to_value(schemars::schema_for!(GeneratedControl)).unwrap());
    assert!(system.contains(&serde_json::to_string(&context).unwrap()));
    for action in [
        "hotkey",
        "typeText",
        "openApp",
        "open",
        "runScript",
        "media",
        "volume",
        "voice",
        "obs",
        "homeAssistant",
        "http",
        "system",
        "navigate",
        "multi",
        "toggle",
        "delay",
    ] {
        assert!(system.contains(&format!("- {action}:")), "undocumented {action}");
    }
    assert!(system.contains("powershell"));
    assert!(system.contains("harmless action"));
}

#[test]
fn configuration_defaults_and_validation() {
    assert_eq!(default_model(AiProvider::Openai), "gpt-6-luna");
    assert_eq!(default_model(AiProvider::Anthropic), "claude-sonnet-5-5");
    assert_eq!(default_base_url(AiProvider::Ollama), Some("http://localhost:11434"));
    assert_eq!(default_base_url(AiProvider::Custom), None);
    for provider in [AiProvider::Openai, AiProvider::Anthropic, AiProvider::Openrouter] {
        assert!(matches!(
            provider::Config::new(&AiSettings { provider, ..AiSettings::default() }),
            Err(AiError::NotConfigured(_))
        ));
    }
    for base in [
        None,
        Some(""),
        Some("file:///tmp"),
        Some("http://user:secret@localhost/v1"),
        Some("http://localhost/v1?key=secret"),
        Some("http://localhost/#x"),
    ] {
        let settings =
            AiSettings { provider: AiProvider::Custom, base_url: base.map(str::to_owned), ..AiSettings::default() };
        assert!(matches!(provider::Config::new(&settings), Err(AiError::NotConfigured(_))));
    }
    let settings = AiSettings {
        provider: AiProvider::Custom,
        base_url: Some("http://localhost:1234/v1".into()),
        model: "".into(),
        ..AiSettings::default()
    };
    assert!(matches!(provider::Config::new(&settings).unwrap().model(), Err(AiError::NotConfigured(_))));
    assert!(provider::Config::new(&AiSettings { api_key: "key\nbad".into(), ..AiSettings::default() }).is_err());
}

#[tokio::test]
async fn generation_wire_format_for_every_provider() {
    for provider in PROVIDERS {
        let mut mock = Mock::json(vec![reply(provider, &control().to_string())]).await;
        let mut settings = mock.settings(provider);
        if matches!(provider, AiProvider::Ollama | AiProvider::Custom) {
            settings.api_key.clear();
        }
        let result = client().generate_control(&settings, &AiContext::default(), &request()).await.unwrap();
        assert_eq!(result.label, "Pause music");
        let received = mock.next().await;
        assert_eq!(received.method, "POST");
        assert_eq!(received.path, generation_path(provider));
        assert_eq!(received.headers["content-type"], "application/json");
        assert_auth(&received, provider, !settings.api_key.is_empty());
        assert_eq!(received.body["model"], "test-model");
        assert_eq!(received.body["stream"], false);
        let (system, messages) = match provider {
            AiProvider::Openai => {
                assert_eq!(received.body["store"], false);
                assert_eq!(received.body["text"]["format"]["type"], "json_object");
                assert_eq!(received.body["max_output_tokens"], 8192);
                (received.body["instructions"].as_str().unwrap(), &received.body["input"])
            }
            AiProvider::Anthropic => {
                assert_eq!(received.body["max_tokens"], 4096);
                (received.body["system"].as_str().unwrap(), &received.body["messages"])
            }
            _ => {
                if provider == AiProvider::Ollama {
                    assert_eq!(received.body["format"], "json");
                    assert_eq!(received.body["options"]["num_predict"], 4096);
                } else {
                    assert_eq!(received.body["response_format"]["type"], "json_object");
                }
                (received.body["messages"][0]["content"].as_str().unwrap(), &received.body["messages"])
            }
        };
        assert!(system.contains("GeneratedControl JSON schema"));
        let messages = messages.as_array().unwrap();
        let end = messages.last().unwrap()["content"].as_str().unwrap();
        assert!(end.contains("Pause music"));
        assert!(end.contains(r#"{"type":"media","command":"next"}"#));
        assert!(
            messages.iter().any(|message| message["role"] == "assistant" && message["content"] == "Previous result")
        );
        mock.done().await;
    }
}

#[tokio::test]
async fn retries_invalid_control_once_with_serde_feedback_for_every_provider() {
    for provider in PROVIDERS {
        let mut mock = Mock::json(vec![
            reply(provider, "{\"label\":7}"),
            reply(provider, &format!("```json\n{}\n```", control())),
        ])
        .await;
        client().generate_control(&mock.settings(provider), &AiContext::default(), &request()).await.unwrap();
        mock.next().await;
        let second = mock.next().await;
        let messages = if provider == AiProvider::Openai { &second.body["input"] } else { &second.body["messages"] };
        let messages = messages.as_array().unwrap();
        assert_eq!(messages[messages.len() - 2]["content"], "{\"label\":7}");
        let correction = messages.last().unwrap()["content"].as_str().unwrap();
        assert!(correction.contains("invalid type: integer"));
        assert!(correction.contains("schema"));
        mock.done().await;
    }
}

#[tokio::test]
async fn stops_after_second_invalid_control() {
    let provider = AiProvider::Openai;
    let mut mock = Mock::json(vec![reply(provider, "not json"), reply(provider, "still not json")]).await;
    assert!(matches!(
        client().generate_control(&mock.settings(provider), &AiContext::default(), &request()).await,
        Err(AiError::InvalidOutput(_))
    ));
    mock.next().await;
    mock.next().await;
    mock.done().await;
}

#[tokio::test]
async fn lists_sorted_models_for_every_provider() {
    for provider in PROVIDERS {
        let body = if provider == AiProvider::Ollama {
            json!({"models":[{"name":"z-model"},{"name":"a-model"},{"name":"a-model"}]})
        } else {
            json!({"data":[{"id":"z-model"},{"id":"a-model"},{"id":"a-model"}]})
        };
        let mut mock = Mock::json(vec![body]).await;
        let mut settings = mock.settings(provider);
        settings.model.clear(); // Listing does not require a selected model, even for custom.
        assert_eq!(client().list_models(&settings).await.unwrap(), ["a-model", "z-model"]);
        let received = mock.next().await;
        assert_eq!(received.method, "GET");
        assert_eq!(
            received.path,
            match provider {
                AiProvider::Anthropic => "/proxy/v1/models",
                AiProvider::Ollama => "/proxy/api/tags",
                _ => "/proxy/v1/models",
            }
        );
        assert_auth(&received, provider, true);
        mock.done().await;
    }
}

#[tokio::test]
async fn anthropic_pagination_and_v1_override() {
    let mut mock = Mock::json(vec![
        json!({"data":[{"id":"z"}],"has_more":true,"last_id":"z"}),
        json!({"data":[{"id":"a"}],"has_more":false}),
    ])
    .await;
    let mut settings = mock.settings(AiProvider::Anthropic);
    settings.base_url = Some(format!("{}/v1/", mock.url));
    assert_eq!(client().list_models(&settings).await.unwrap(), ["a", "z"]);
    assert_eq!(mock.next().await.path, "/v1/models");
    assert_eq!(mock.next().await.path, "/v1/models?after_id=z");
    mock.done().await;
}

#[tokio::test]
async fn connection_test_is_small_and_uses_selected_model_for_every_provider() {
    for provider in PROVIDERS {
        let mut mock = Mock::json(vec![reply(provider, "OK")]).await;
        assert_eq!(
            client().test(&mock.settings(provider)).await.unwrap(),
            "Connected. Model test-model is responding."
        );
        let received = mock.next().await;
        assert_eq!(received.path, generation_path(provider));
        assert_auth(&received, provider, true);
        assert_eq!(received.body["model"], "test-model");
        assert!(received.body.to_string().len() < 300);
        assert!(received.body.get("text").is_none());
        assert!(received.body.get("response_format").is_none());
        assert!(received.body.get("format").is_none());
        mock.done().await;
    }
}

#[tokio::test]
async fn empty_models_use_provider_defaults() {
    for provider in [AiProvider::Openai, AiProvider::Anthropic, AiProvider::Ollama, AiProvider::Openrouter] {
        let mut mock = Mock::json(vec![reply(provider, "OK")]).await;
        let mut settings = mock.settings(provider);
        settings.model = "  ".into();
        client().test(&settings).await.unwrap();
        assert_eq!(mock.next().await.body["model"], default_model(provider));
        mock.done().await;
    }
}

#[tokio::test]
async fn maps_http_errors_and_redacts_echoed_key_for_every_provider() {
    for provider in PROVIDERS {
        let error = if provider == AiProvider::Ollama {
            json!({"error":"missing model test-key"})
        } else {
            json!({"error":{"message":"invalid key test-key"}})
        };
        let mut mock = Mock::new(vec![(401, error.to_string())]).await;
        let result = client().test(&mock.settings(provider)).await;
        match result {
            Err(AiError::Provider { status, message }) => {
                assert_eq!(status, 401);
                assert!(message.contains("[redacted]"));
                assert!(!message.contains("test-key"));
            }
            _ => panic!("incorrect HTTP error mapping"),
        }
        mock.next().await;
        mock.done().await;
    }
    let mut mock = Mock::new(vec![(503, "upstream unavailable".into())]).await;
    assert!(
        matches!(client().test(&mock.settings(AiProvider::Custom)).await, Err(AiError::Provider { status:503, message }) if message == "upstream unavailable")
    );
    mock.next().await;
    mock.done().await;
    // A key straddling the length cut must not stay partly visible.
    let mut mock = Mock::new(vec![(500, format!("{}test-key", "x".repeat(996)))]).await;
    assert!(
        matches!(client().test(&mock.settings(AiProvider::Custom)).await, Err(AiError::Provider { message, .. }) if !message.contains("test"))
    );
    mock.next().await;
    mock.done().await;
}

#[tokio::test]
async fn maps_network_errors() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let settings = AiSettings {
        provider: AiProvider::Custom,
        model: "test".into(),
        base_url: Some(base_url),
        ..AiSettings::default()
    };
    assert!(matches!(client().test(&settings).await, Err(AiError::Http(_))));
}

#[tokio::test]
async fn compatible_servers_can_decline_json_mode_without_breaking_generation() {
    for provider in [AiProvider::Custom, AiProvider::Openrouter] {
        let mut mock = Mock::new(vec![
            (400, json!({"error":{"message":"Unsupported response_format json_object"}}).to_string()),
            (200, reply(provider, &control().to_string()).to_string()),
        ])
        .await;
        client().generate_control(&mock.settings(provider), &AiContext::default(), &request()).await.unwrap();
        assert!(mock.next().await.body.get("response_format").is_some());
        assert!(mock.next().await.body.get("response_format").is_none());
        mock.done().await;
    }
}

#[tokio::test]
async fn unsafe_output_is_rejected_without_retry_and_labels_are_unicode_safe() {
    let provider = AiProvider::Ollama;
    let mut bad = control();
    bad["action"] = json!({"type":"multi","steps":[{"type":"runScript","interpreter":"shell","script":"rm -rf /"}]});
    let mut mock = Mock::json(vec![reply(provider, &bad.to_string())]).await;
    assert!(
        matches!(client().generate_control(&mock.settings(provider), &AiContext::default(), &request()).await, Err(AiError::InvalidOutput(message)) if message.contains("destructive"))
    );
    mock.next().await;
    mock.done().await;
    let mut long = control();
    long["label"] = json!("  音楽を一時停止するボタンの長い名前  ");
    long["icon"] = json!("not-a-lucide-icon");
    let mut mock = Mock::json(vec![reply(provider, &long.to_string())]).await;
    let result = client().generate_control(&mock.settings(provider), &AiContext::default(), &request()).await.unwrap();
    assert_eq!(result.label.chars().count(), 14);
    assert_eq!(result.label, long["label"].as_str().unwrap().trim().chars().take(14).collect::<String>());
    assert_eq!(result.icon, None);
    mock.next().await;
    mock.done().await;
}

#[test]
fn rejects_refusals_truncation_and_missing_text() {
    for (provider, body) in [
        (AiProvider::Openai, json!({"status":"incomplete","output":[]})),
        (
            AiProvider::Openai,
            json!({"output":[{"type":"message","content":[{"type":"refusal","refusal":"Cannot do that"}]}]}),
        ),
        (AiProvider::Anthropic, json!({"stop_reason":"max_tokens","content":[{"type":"text","text":"partial"}]})),
        (AiProvider::Ollama, json!({"done":false,"message":{"content":"partial"}})),
        (AiProvider::Custom, json!({"choices":[{"finish_reason":"length","message":{"content":"partial"}}]})),
    ] {
        assert!(matches!(provider::response_text(provider, &body), Err(AiError::InvalidOutput(_))));
    }
    for provider in PROVIDERS {
        assert!(matches!(provider::response_text(provider, &reply(provider, " ")), Err(AiError::InvalidOutput(_))));
    }
}

#[tokio::test]
async fn malformed_response_and_model_list_are_errors() {
    let mut mock = Mock::new(vec![(200, "not JSON".into())]).await;
    assert!(matches!(client().test(&mock.settings(AiProvider::Openai)).await, Err(AiError::InvalidOutput(_))));
    mock.next().await;
    mock.done().await;
    let mut mock = Mock::json(vec![json!({"data":[{"name":"missing-id"}]})]).await;
    assert!(matches!(client().list_models(&mock.settings(AiProvider::Custom)).await, Err(AiError::InvalidOutput(_))));
    mock.next().await;
    mock.done().await;
}

#[tokio::test]
#[ignore = "requires OPENAI_API_KEY and makes a billable live request"]
async fn live_openai_generation() {
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set for the live test");
    let settings = AiSettings { api_key, ..AiSettings::default() };
    let request = GenerateRequest {
        prompt: "Make a button that pauses or resumes system media playback.".into(),
        history: vec![],
        current: None,
    };
    let result =
        client().generate_control(&settings, &AiContext { os: "macos".into(), ..AiContext::default() }, &request).await;
    // Avoid printing a server error that could echo credentials.
    assert!(result.is_ok(), "live OpenAI generation failed");
    assert!(matches!(result.unwrap().action, Action::Media { command: MediaCommand::PlayPause }));
}

#[tokio::test]
async fn tiny_probe_accepts_token_usage_but_generation_rejects_truncation() {
    for provider in PROVIDERS {
        let response = match provider {
            AiProvider::Openai => {
                json!({"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"usage":{"output_tokens":64},"output":[]})
            }
            AiProvider::Anthropic => {
                json!({"stop_reason":"max_tokens","usage":{"output_tokens":8},"content":[]})
            }
            AiProvider::Ollama => {
                json!({"done":true,"done_reason":"length","eval_count":8,"message":{"content":""}})
            }
            _ => {
                json!({"choices":[{"finish_reason":"length","message":{"content":""}}],"usage":{"completion_tokens":16}})
            }
        };
        let mut mock = Mock::json(vec![response.clone(), response]).await;
        let ai = client();
        let settings = mock.settings(provider);
        assert!(ai.test(&settings).await.is_ok());
        assert!(matches!(
            ai.generate_control(&settings, &AiContext::default(), &request()).await,
            Err(AiError::InvalidOutput(_))
        ));
        mock.next().await;
        mock.next().await;
        mock.done().await;
    }
}

#[tokio::test]
async fn detects_errors_in_successful_http_envelopes_and_bounds_response_size() {
    let mut mock = Mock::json(vec![json!({"error":{"message":"rejected test-key"}})]).await;
    assert!(
        matches!(client().test(&mock.settings(AiProvider::Openai)).await, Err(AiError::Provider { status:200, message }) if message == "rejected [redacted]")
    );
    mock.next().await;
    mock.done().await;
    let mut mock = Mock::new(vec![(200, " ".repeat(1024 * 1024 + 1))]).await;
    assert!(
        matches!(client().test(&mock.settings(AiProvider::Custom)).await, Err(AiError::Provider { message, .. }) if message.contains("exceeded 1 MiB"))
    );
    mock.next().await;
    // The client may close the oversized response before the server finishes sending it.
}

#[tokio::test]
async fn redirects_are_not_followed_so_the_key_stays_with_the_configured_origin() {
    let mut target = Mock::json(vec![json!({"data": []})]).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let location = format!("{}/v1/models", target.url);
    let redirect = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = stream.read(&mut [0; 4096]).await.unwrap();
        let head =
            format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        stream.write_all(head.as_bytes()).await.unwrap();
    });
    let settings = AiSettings { base_url: Some(origin), ..target.settings(AiProvider::Anthropic) };
    let result = client().list_models(&settings).await;
    assert!(
        matches!(&result, Err(AiError::Provider { status: 302, message }) if message.contains("redirects")),
        "{result:?}"
    );
    redirect.await.unwrap();
    assert!(target.requests.try_recv().is_err(), "the API key was sent to the redirect target");
}
