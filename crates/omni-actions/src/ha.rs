//! Home Assistant integration over its REST API.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use omni_core::{ActionOutcome, ConnState, ConnectionStatus, HaEntity, HaOp, HomeAssistantSettings};
use reqwest::{Client, Method, Response, StatusCode, Url};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use tokio::sync::watch;
use tokio::task::AbortHandle;

use crate::http::{preview, request_error};
use crate::{ActionError, Result};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Largest answer read; the entity list of a big installation stays well below it.
const MAX_BODY: usize = 16 << 20;
/// Status and error when the URL answers, but not like Home Assistant.
const NOT_HOME_ASSISTANT: &str = "not a Home Assistant URL (another website or a login page answered)";
/// Delay between connection checks while Home Assistant is unreachable.
const RETRY_MIN: Duration = Duration::from_secs(5);
const RETRY_MAX: Duration = Duration::from_secs(60);

/// Home Assistant REST client: connection status, entity list and service calls.
pub struct HaClient {
    http: Client,
    status: Arc<watch::Sender<ConnectionStatus>>,
    config: Mutex<Config>,
}

/// What `configure` last applied.
#[derive(Default)]
struct Config {
    settings: Option<HomeAssistantSettings>,
    /// `None` when disabled or the settings are unusable (reflected in the status).
    endpoint: Option<Endpoint>,
    /// Connection check still running.
    check: Option<AbortHandle>,
}

#[derive(Clone)]
struct Endpoint {
    /// Always ends with `/`, so `join("api/")` keeps a sub-path like `/ha/`.
    base: Url,
    token: String,
}

impl Endpoint {
    fn new(settings: &HomeAssistantSettings) -> std::result::Result<Self, String> {
        let url = settings.url.trim();
        if url.is_empty() {
            return Err("Home Assistant URL is empty".into());
        }
        if settings.token.trim().is_empty() {
            return Err("access token is empty".into());
        }
        let url = if url.contains("://") { url.to_owned() } else { format!("http://{url}") };
        let mut base = Url::parse(&url).map_err(|e| format!("invalid Home Assistant URL: {e}"))?;
        if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
            return Err("Home Assistant URL must start with http:// or https://".into());
        }
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        Ok(Self { base, token: settings.token.trim().to_owned() })
    }

    fn url(&self, path: &str) -> Url {
        // `path` is always one of our own relative constants or validated identifiers.
        self.base.join(path).unwrap_or_else(|_| self.base.clone())
    }
}

impl Drop for HaClient {
    fn drop(&mut self) {
        if let Some(check) = self.config.get_mut().unwrap_or_else(PoisonError::into_inner).check.take() {
            check.abort();
        }
    }
}

impl HaClient {
    pub fn new(http: Client) -> Self {
        Self { http, status: Arc::new(watch::Sender::new(ConnectionStatus::default())), config: Mutex::default() }
    }

    /// Validate the URL and token (`GET /api/`) and keep the status current.
    /// Unreachable servers are retried with a growing delay; a rejected token is not.
    pub fn configure(&self, settings: &HomeAssistantSettings) {
        let mut config = self.config.lock().unwrap_or_else(PoisonError::into_inner);
        if config.settings.as_ref() == Some(settings) {
            return;
        }
        config.settings = Some(settings.clone());
        config.endpoint = None;
        if let Some(check) = config.check.take() {
            check.abort();
        }
        if !settings.enabled {
            set_status(&self.status, ConnState::Disabled, None);
            return;
        }
        match Endpoint::new(settings) {
            Err(message) => set_status(&self.status, ConnState::Error, Some(message)),
            Ok(endpoint) => {
                config.endpoint = Some(endpoint.clone());
                set_status(&self.status, ConnState::Connecting, None);
                config.check = Some(
                    crate::rt::spawn(check_until_settled(self.http.clone(), endpoint, self.status.clone()))
                        .abort_handle(),
                );
            }
        }
    }

    pub fn status(&self) -> ConnectionStatus {
        self.status.borrow().clone()
    }

    /// Notified whenever the connection status changes.
    pub fn subscribe(&self) -> watch::Receiver<ConnectionStatus> {
        self.status.subscribe()
    }

    /// All entities sorted by id, for the editor picker.
    pub async fn entities(&self) -> Result<Vec<HaEntity>> {
        #[derive(Deserialize)]
        struct Raw {
            entity_id: String,
            state: String,
            #[serde(default)]
            attributes: Attributes,
        }
        #[derive(Deserialize, Default)]
        struct Attributes {
            friendly_name: Option<Value>,
        }

        let raw: Vec<Raw> = self.send(Method::GET, "api/states", None).await?;
        let mut entities: Vec<HaEntity> = raw
            .into_iter()
            .map(|r| {
                let name = r.attributes.friendly_name.as_ref().and_then(Value::as_str).filter(|n| !n.is_empty());
                HaEntity { name: name.unwrap_or(&r.entity_id).to_owned(), entity_id: r.entity_id, state: r.state }
            })
            .collect();
        entities.sort_by(|a, b| a.entity_id.cmp(&b.entity_id));
        Ok(entities)
    }

    pub(crate) async fn run(&self, op: &HaOp) -> Result<ActionOutcome> {
        let (domain, service, data) = match op {
            HaOp::Toggle { entity_id } => {
                let entity_id = entity_id.trim();
                if entity_id.is_empty() {
                    return Err(ActionError::Invalid("no entity selected".into()));
                }
                ("homeassistant", "toggle", json!({ "entity_id": entity_id }))
            }
            HaOp::CallService { domain, service, entity_id, data } => {
                let mut body = match data {
                    None | Some(Value::Null) => Map::new(),
                    Some(Value::Object(map)) => map.clone(),
                    Some(_) => return Err(ActionError::Invalid("service data must be a JSON object".into())),
                };
                if let Some(entity_id) = entity_id.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
                    body.insert("entity_id".into(), entity_id.into());
                }
                (domain.trim(), service.trim(), Value::Object(body))
            }
        };
        if ![domain, service].iter().all(|part| is_identifier(part)) {
            return Err(ActionError::Invalid(format!("\"{domain}.{service}\" is not a service name")));
        }
        // Home Assistant answers with the changed states (JSON); a login page means nothing happened.
        self.send::<Value>(Method::POST, &format!("api/services/{domain}/{service}"), Some(data)).await?;
        Ok(ActionOutcome { message: None })
    }

    /// Authenticated request returning the JSON answer. Non-2xx answers become
    /// errors, and so do successful answers that are not the expected JSON (a
    /// login page, another website). The status follows definite outcomes: an
    /// expected answer, a rejected token, an unreachable or non-HA server.
    async fn send<T: DeserializeOwned>(&self, method: Method, path: &str, body: Option<Value>) -> Result<T> {
        let endpoint = {
            let config = self.config.lock().unwrap_or_else(PoisonError::into_inner);
            match (&config.endpoint, &config.settings) {
                (Some(endpoint), _) => endpoint.clone(),
                (None, Some(settings)) if settings.enabled => {
                    return Err(ActionError::Failed(
                        Endpoint::new(settings).err().unwrap_or_else(|| "Home Assistant is not configured".into()),
                    ));
                }
                _ => return Err(ActionError::NotConnected("Home Assistant")),
            }
        };
        let mut request =
            self.http.request(method, endpoint.url(path)).bearer_auth(&endpoint.token).timeout(REQUEST_TIMEOUT);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let mut response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                let error = request_error(error);
                set_status(&self.status, ConnState::Error, Some(format!("unreachable: {error}")));
                return Err(error);
            }
        };

        let status = response.status();
        if is_rejected(status) {
            set_status(&self.status, ConnState::Error, Some("invalid token".into()));
            return Err(ActionError::Failed("Home Assistant rejected the access token".into()));
        }
        if !status.is_success() {
            // Any web server can answer 404 or 500, so the status stays as it is.
            // Proxies and debug pages can echo the request, token included.
            let detail = preview(&mut response, &endpoint.token).await;
            return Err(ActionError::Failed(if detail.is_empty() {
                format!("Home Assistant returned {status}")
            } else {
                format!("Home Assistant returned {status}: {detail}")
            }));
        }
        match read_json(response).await? {
            Some(answer) => {
                set_status(&self.status, ConnState::Connected, None);
                Ok(answer)
            }
            None => {
                set_status(&self.status, ConnState::Error, Some(NOT_HOME_ASSISTANT.into()));
                Err(ActionError::Failed(NOT_HOME_ASSISTANT.into()))
            }
        }
    }
}

/// The JSON answer, read up to [`MAX_BODY`] bytes. `None` when the body is not
/// JSON of the expected shape, i.e. not from Home Assistant.
async fn read_json<T: DeserializeOwned>(mut response: Response) -> Result<Option<T>> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(request_error)? {
        if body.len() + chunk.len() > MAX_BODY {
            return Err(ActionError::Failed(format!("Home Assistant's answer is larger than {} MiB", MAX_BODY >> 20)));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&body).ok())
}

fn is_rejected(status: StatusCode) -> bool {
    matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
}

/// Service and domain names are path segments: lowercase letters, digits, `_`.
fn is_identifier(part: &str) -> bool {
    !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn set_status(status: &watch::Sender<ConnectionStatus>, state: ConnState, message: Option<String>) {
    status.send_if_modified(|current| {
        let next = ConnectionStatus { state, message };
        let changed = *current != next;
        *current = next;
        changed
    });
}

/// `GET /api/` until Home Assistant answers definitively (connected, token
/// rejected or not Home Assistant); retries while it is unreachable.
async fn check_until_settled(http: Client, endpoint: Endpoint, status: Arc<watch::Sender<ConnectionStatus>>) {
    let mut delay = RETRY_MIN;
    loop {
        let result = http.get(endpoint.url("api/")).bearer_auth(&endpoint.token).timeout(REQUEST_TIMEOUT).send().await;
        match result {
            Ok(response) if response.status().is_success() => match read_json::<Value>(response).await {
                // `{"message": "API running."}`
                Ok(Some(answer)) if answer["message"].is_string() => {
                    return set_status(&status, ConnState::Connected, None);
                }
                Ok(_) => return set_status(&status, ConnState::Error, Some(NOT_HOME_ASSISTANT.into())),
                Err(error) => set_status(&status, ConnState::Error, Some(format!("unreachable: {error}"))),
            },
            Ok(response) if is_rejected(response.status()) => {
                return set_status(&status, ConnState::Error, Some("invalid token".into()));
            }
            Ok(response) if response.status() == StatusCode::NOT_FOUND => {
                return set_status(
                    &status,
                    ConnState::Error,
                    Some("not a Home Assistant URL (/api/ not found)".into()),
                );
            }
            Ok(response) => {
                set_status(&status, ConnState::Error, Some(format!("unreachable: HTTP {}", response.status())))
            }
            Err(error) => set_status(&status, ConnState::Error, Some(format!("unreachable: {}", request_error(error)))),
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(RETRY_MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{MockHttp, Request};

    const TOKEN: &str = "secret-token";

    fn settings(url: String, token: &str) -> HomeAssistantSettings {
        HomeAssistantSettings { enabled: true, url, token: token.into() }
    }

    fn authorized(request: &Request) -> bool {
        request.headers.get("authorization").map(String::as_str) == Some(&format!("Bearer {TOKEN}"))
    }

    /// Minimal Home Assistant: `/api/`, `/api/states` and service calls.
    async fn home_assistant() -> MockHttp {
        MockHttp::start(|request| {
            if !authorized(request) {
                return (401, "401: Unauthorized".into());
            }
            match (request.method.as_str(), request.path.as_str()) {
                ("GET", "/api/") => (200, r#"{"message":"API running."}"#.into()),
                ("GET", "/api/states") => (
                    200,
                    json!([
                        {"entity_id": "switch.fan", "state": "off", "attributes": {}},
                        {"entity_id": "light.desk", "state": "on", "attributes": {"friendly_name": "Desk Lamp", "brightness": 200}},
                        {"entity_id": "sensor.temp", "state": "21.5", "attributes": {"friendly_name": 5}},
                    ])
                    .to_string(),
                ),
                ("POST", path) if path.starts_with("/api/services/") => (200, "[]".into()),
                _ => (404, "404: Not Found".into()),
            }
        })
        .await
    }

    async fn settle(client: &HaClient, wanted: ConnState) -> ConnectionStatus {
        let mut status = client.subscribe();
        let reached = tokio::time::timeout(Duration::from_secs(5), status.wait_for(|s| s.state == wanted)).await;
        reached.unwrap_or_else(|_| panic!("never reached {wanted:?}, stuck at {:?}", client.status())).unwrap().clone()
    }

    #[tokio::test]
    async fn connects_and_lists_entities() {
        let server = home_assistant().await;
        let client = HaClient::new(Client::new());
        assert_eq!(client.status().state, ConnState::Disabled);
        client.configure(&settings(server.url(), TOKEN));
        settle(&client, ConnState::Connected).await;

        let entities = client.entities().await.unwrap();
        let entities: Vec<_> =
            entities.iter().map(|e| (e.entity_id.as_str(), e.name.as_str(), e.state.as_str())).collect();
        assert_eq!(
            entities,
            [
                ("light.desk", "Desk Lamp", "on"),
                ("sensor.temp", "sensor.temp", "21.5"),
                ("switch.fan", "switch.fan", "off")
            ]
        );

        // Same settings again must not re-validate.
        let checks = server.requests().len();
        client.configure(&settings(server.url(), TOKEN));
        assert_eq!(server.requests().len(), checks);
    }

    #[tokio::test]
    async fn wrong_token_is_reported() {
        let server = home_assistant().await;
        let client = HaClient::new(Client::new());
        client.configure(&settings(server.url(), "wrong"));
        let status = settle(&client, ConnState::Error).await;
        assert_eq!(status.message.as_deref(), Some("invalid token"));
        assert!(matches!(client.entities().await, Err(ActionError::Failed(m)) if m.contains("rejected")));
    }

    #[tokio::test]
    async fn unreachable_server_and_bad_settings() {
        let client = HaClient::new(Client::new());
        client.configure(&settings("http://127.0.0.1:1".into(), TOKEN));
        let status = settle(&client, ConnState::Error).await;
        assert!(status.message.unwrap().starts_with("unreachable"));

        client.configure(&settings("  ".into(), TOKEN));
        assert_eq!(client.status().message.as_deref(), Some("Home Assistant URL is empty"));
        client.configure(&settings("http://ha.local:8123".into(), ""));
        assert_eq!(client.status().message.as_deref(), Some("access token is empty"));

        client.configure(&HomeAssistantSettings { enabled: false, ..settings("http://x".into(), TOKEN) });
        assert_eq!(client.status(), ConnectionStatus::default());
        assert!(matches!(client.entities().await, Err(ActionError::NotConnected(_))));
    }

    #[tokio::test]
    async fn toggle_and_service_calls() {
        let server = home_assistant().await;
        let client = HaClient::new(Client::new());
        client.configure(&settings(format!("{}/", server.url()), TOKEN));
        settle(&client, ConnState::Connected).await;

        client.run(&HaOp::Toggle { entity_id: "light.desk".into() }).await.unwrap();
        client
            .run(&HaOp::CallService {
                domain: "light".into(),
                service: "turn_on".into(),
                entity_id: Some("light.desk".into()),
                data: Some(json!({"brightness": 128})),
            })
            .await
            .unwrap();
        client
            .run(&HaOp::CallService { domain: "scene".into(), service: "turn_on".into(), entity_id: None, data: None })
            .await
            .unwrap();

        let calls: Vec<_> = server.requests().into_iter().filter(|r| r.method == "POST").collect();
        assert_eq!(calls[0].path, "/api/services/homeassistant/toggle");
        assert_eq!(serde_json::from_str::<Value>(&calls[0].body).unwrap(), json!({"entity_id": "light.desk"}));
        assert_eq!(calls[1].path, "/api/services/light/turn_on");
        assert_eq!(
            serde_json::from_str::<Value>(&calls[1].body).unwrap(),
            json!({"entity_id": "light.desk", "brightness": 128})
        );
        assert_eq!(calls[2].body, "{}");
    }

    #[tokio::test]
    async fn other_websites_are_not_home_assistant() {
        let server = MockHttp::start(|_| (200, "<html>Please log in</html>".into())).await;
        let client = HaClient::new(Client::new());
        client.configure(&settings(server.url(), TOKEN));
        let status = settle(&client, ConnState::Error).await;
        assert_eq!(status.message.as_deref(), Some(NOT_HOME_ASSISTANT));
        // Nothing was switched, so the action must fail.
        let toggle = client.run(&HaOp::Toggle { entity_id: "light.desk".into() }).await;
        assert!(matches!(toggle, Err(ActionError::Failed(m)) if m == NOT_HOME_ASSISTANT));
        assert!(client.entities().await.is_err());
    }

    #[tokio::test]
    async fn answers_are_bounded_and_errors_never_show_the_token() {
        let server = MockHttp::start(|request| match request.path.as_str() {
            "/api/" => (200, r#"{"message":"API running."}"#.into()),
            "/api/states" => (200, format!("[{}]", " ".repeat(MAX_BODY))),
            // A proxy debug page echoing the request.
            _ => (500, format!("debug: Authorization: Bearer {TOKEN} {}", "x".repeat(10_000))),
        })
        .await;
        let client = HaClient::new(Client::new());
        client.configure(&settings(server.url(), TOKEN));
        settle(&client, ConnState::Connected).await;

        let error = client.run(&HaOp::Toggle { entity_id: "light.desk".into() }).await.unwrap_err().to_string();
        assert!(error.starts_with("Home Assistant returned 500") && error.len() < 300, "{error}");
        assert!(error.contains("[redacted]") && !error.contains(TOKEN), "{error}");
        assert!(matches!(client.entities().await, Err(ActionError::Failed(m)) if m.contains("larger than")));
    }

    #[tokio::test]
    async fn rejects_malformed_service_calls() {
        let client = HaClient::new(Client::new());
        client.configure(&settings("http://127.0.0.1:1".into(), TOKEN));
        let call = |domain: &str, service: &str, data: Option<Value>| HaOp::CallService {
            domain: domain.into(),
            service: service.into(),
            entity_id: None,
            data,
        };
        for op in
            [call("light", "../states", None), call("", "turn_on", None), call("light", "turn_on", Some(json!([1])))]
        {
            assert!(matches!(client.run(&op).await, Err(ActionError::Invalid(_))), "{op:?}");
        }
        assert!(matches!(client.run(&HaOp::Toggle { entity_id: " ".into() }).await, Err(ActionError::Invalid(_))));
    }

    #[test]
    fn endpoint_keeps_sub_paths() {
        let endpoint = Endpoint::new(&settings("https://example.com/ha".into(), "t")).unwrap();
        assert_eq!(endpoint.url("api/states").as_str(), "https://example.com/ha/api/states");
        let endpoint = Endpoint::new(&settings("homeassistant.local:8123".into(), "t")).unwrap();
        assert_eq!(endpoint.url("api/").as_str(), "http://homeassistant.local:8123/api/");
    }
}
