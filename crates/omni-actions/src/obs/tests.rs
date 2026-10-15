//! Tests against an in-process obs-websocket v5 server.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use omni_core::{ConnState, ConnectionStatus, ObsOp, ObsSettings};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use super::proto::{CLOSE_AUTH_FAILED, SUBPROTOCOL, auth_response};
use super::{ObsClient, ObsState};
use crate::ActionError;

const PASSWORD: &str = "hunter2";

struct MockObs {
    port: u16,
    connections: Arc<AtomicUsize>,
    /// (request type, request data) in arrival order.
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    inputs: Arc<Mutex<Vec<&'static str>>>,
    /// `Some(event)` is sent to every open connection, `None` drops them.
    broadcast: broadcast::Sender<Option<Value>>,
}

impl MockObs {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mock = Self {
            port: listener.local_addr().unwrap().port(),
            connections: Arc::default(),
            requests: Arc::default(),
            inputs: Arc::new(Mutex::new(vec!["Mic", "Desktop"])),
            broadcast: broadcast::channel(64).0,
        };
        let (connections, requests, inputs, tx) =
            (mock.connections.clone(), mock.requests.clone(), mock.inputs.clone(), mock.broadcast.clone());
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                connections.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(serve(stream, requests.clone(), inputs.clone(), tx.subscribe()));
            }
        });
        mock
    }

    fn settings(&self, password: &str) -> ObsSettings {
        ObsSettings { enabled: true, host: "127.0.0.1".into(), port: self.port, password: password.into() }
    }

    fn event(&self, kind: &str, data: Value) {
        self.broadcast.send(Some(json!({ "eventType": kind, "eventData": data }))).unwrap();
    }

    fn drop_connections(&self) {
        self.broadcast.send(None).unwrap();
    }

    fn requests_of(&self, kind: &str) -> Vec<Value> {
        self.requests.lock().unwrap().iter().filter(|(k, _)| k == kind).map(|(_, data)| data.clone()).collect()
    }
}

fn text(value: Value) -> Message {
    Message::text(value.to_string())
}

async fn serve(
    stream: tokio::net::TcpStream,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    inputs: Arc<Mutex<Vec<&'static str>>>,
    mut broadcast: broadcast::Receiver<Option<Value>>,
) {
    #[allow(clippy::result_large_err)]
    let accept = |request: &Request, mut response: Response| {
        assert_eq!(request.headers()["sec-websocket-protocol"], SUBPROTOCOL);
        response.headers_mut().insert("Sec-WebSocket-Protocol", HeaderValue::from_static(SUBPROTOCOL));
        Ok(response)
    };
    let Ok(mut socket) = accept_hdr_async(stream, accept).await else { return };

    let _ = socket
        .send(text(json!({"op": 0, "d": {"obsWebSocketVersion": "5.5.0", "rpcVersion": 1, "authentication": {"challenge": "chal", "salt": "salt"}}})))
        .await;
    let Some(Ok(Message::Text(identify))) = socket.next().await else { return };
    let identify: Value = serde_json::from_str(&identify).unwrap();
    if identify["d"]["authentication"] != auth_response(PASSWORD, "salt", "chal") {
        let reason = CloseFrame { code: CloseCode::from(CLOSE_AUTH_FAILED), reason: "Authentication failed.".into() };
        let _ = socket.close(Some(reason)).await;
        return;
    }
    let _ = socket.send(text(json!({"op": 2, "d": {"negotiatedRpcVersion": 1}}))).await;

    loop {
        tokio::select! {
            message = socket.next() => {
                let Some(Ok(Message::Text(message))) = message else { return };
                let frame: Value = serde_json::from_str(&message).unwrap();
                let (kind, id, data) = (frame["d"]["requestType"].as_str().unwrap(), frame["d"]["requestId"].clone(), frame["d"]["requestData"].clone());
                requests.lock().unwrap().push((kind.to_owned(), data.clone()));
                let (code, comment, response) = answer(kind, &data, &inputs.lock().unwrap());
                let status = json!({"result": code == 100, "code": code, "comment": comment});
                let reply = json!({"op": 7, "d": {"requestType": kind, "requestId": id, "requestStatus": status, "responseData": response}});
                if socket.send(text(reply)).await.is_err() {
                    return;
                }
            }
            event = broadcast.recv() => match event {
                Ok(Some(event)) => {
                    if socket.send(text(json!({"op": 5, "d": event}))).await.is_err() {
                        return;
                    }
                }
                _ => return,
            }
        }
    }
}

/// The scenes `Live` and `BRB`; `Live` shows `Camera` and hides `Logo`.
fn answer(kind: &str, data: &Value, inputs: &[&str]) -> (u32, &'static str, Value) {
    let ok = |value: Value| (100, "", value);
    match kind {
        "GetCurrentProgramScene" => ok(json!({"sceneName": "Live"})),
        "GetRecordStatus" => ok(json!({"outputActive": false, "outputPaused": false})),
        "GetStreamStatus" | "GetVirtualCamStatus" => ok(json!({"outputActive": false})),
        // The replay buffer is not enabled in this OBS.
        "GetReplayBufferStatus" => (604, "Replay buffer is not available", Value::Null),
        "GetStudioModeEnabled" => ok(json!({"studioModeEnabled": false})),
        "GetInputList" => ok(
            json!({"inputs": inputs.iter().map(|i| json!({"inputName": i, "inputKind": "coreaudio_input_capture"})).collect::<Vec<_>>()}),
        ),
        "GetSceneList" => {
            ok(json!({"scenes": [{"sceneName": "Live", "sceneIndex": 1}, {"sceneName": "BRB", "sceneIndex": 0}]}))
        }
        "GetInputMute" => ok(json!({"inputMuted": data["inputName"] == "Mic"})),
        "GetInputVolume" => ok(json!({"inputVolumeMul": 0.5, "inputVolumeDb": -6.0})),
        "GetSceneItemList" if data["sceneName"] == "Live" => ok(json!({"sceneItems": [
            {"sceneItemId": 1, "sourceName": "Camera", "sceneItemEnabled": true},
            {"sceneItemId": 2, "sourceName": "Logo", "sceneItemEnabled": false},
        ]})),
        "GetSceneItemList" => ok(json!({"sceneItems": []})),
        "GetSceneItemId" => ok(json!({"sceneItemId": 2})),
        "GetSceneItemEnabled" => ok(json!({"sceneItemEnabled": false})),
        "ToggleInputMute" if data["inputName"] == "Missing" => {
            (600, "No input was found by the name of Missing", Value::Null)
        }
        "SetCurrentProgramScene" | "ToggleRecord" | "SetSceneItemEnabled" | "ToggleInputMute" | "SetInputVolume" => {
            ok(Value::Null)
        }
        _ => (204, "Unknown request type", Value::Null),
    }
}

/// A client with fast reconnects.
fn client() -> ObsClient {
    ObsClient::with_backoff(Duration::from_millis(50), Duration::from_millis(200))
}

async fn wait_state(client: &ObsClient, wanted: impl Fn(&ObsState) -> bool) -> ObsState {
    let mut state = client.subscribe();
    let waited = tokio::time::timeout(Duration::from_secs(5), state.wait_for(|s| wanted(s))).await;
    match waited {
        Ok(Ok(state)) => state.clone(),
        _ => panic!("state never matched, last: {:?} / {:?}", client.state(), client.status()),
    }
}

async fn wait_status(client: &ObsClient, wanted: impl Fn(&ConnectionStatus) -> bool) -> ConnectionStatus {
    let mut state = client.subscribe();
    let waited = tokio::time::timeout(Duration::from_secs(5), state.wait_for(|_| wanted(&client.status()))).await;
    assert!(waited.is_ok(), "status never matched, last: {:?}", client.status());
    client.status()
}

async fn connected(mock: &MockObs) -> ObsClient {
    let client = client();
    client.configure(&mock.settings(PASSWORD));
    wait_state(&client, |s| s.connected).await;
    client
}

#[tokio::test]
async fn authenticates_and_loads_the_initial_state() {
    let mock = MockObs::start().await;
    let client = client();
    assert_eq!(client.status().state, ConnState::Disabled);
    client.configure(&mock.settings(PASSWORD));
    let state = wait_state(&client, |s| s.connected).await;

    assert_eq!(client.status(), ConnectionStatus { state: ConnState::Connected, message: None });
    assert_eq!(state.current_scene.as_deref(), Some("Live"));
    assert!(!state.recording && !state.streaming && !state.studio_mode);
    // GetReplayBufferStatus failed: tolerated.
    assert!(!state.replay_buffer);
    assert_eq!(state.input_muted, [("Mic".to_owned(), true), ("Desktop".to_owned(), false)].into());
    assert_eq!(state.input_volume["Desktop"], 0.5);
    let visible = |scene: &str, source: &str| state.source_visible[&(scene.to_owned(), source.to_owned())];
    assert!(visible("Live", "Camera") && !visible("Live", "Logo"));
    assert_eq!(state.source_visible.len(), 2);
}

#[tokio::test]
async fn wrong_password_is_reported() {
    let mock = MockObs::start().await;
    let client = client();
    client.configure(&mock.settings("not-the-password"));
    let status = wait_status(&client, |s| s.state == ConnState::Error).await;
    assert_eq!(status.message.as_deref(), Some("authentication failed"));
    assert!(!client.state().connected);

    // A missing password is caught before trying.
    client.configure(&mock.settings(""));
    wait_status(&client, |s| s.message.as_deref().is_some_and(|m| m.contains("needs a password"))).await;

    // Fixing the password reconnects without restarting anything.
    client.configure(&mock.settings(PASSWORD));
    wait_state(&client, |s| s.connected).await;
}

#[tokio::test]
async fn unreachable_obs_is_reported() {
    let free_port = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port();
    let client = client();
    client.configure(&ObsSettings {
        enabled: true,
        host: "127.0.0.1".into(),
        port: free_port,
        password: PASSWORD.into(),
    });
    let status = wait_status(&client, |s| s.state == ConnState::Error).await;
    assert!(status.message.unwrap().contains("not running"));
    assert!(matches!(client.request("GetVersion", Value::Null).await, Err(ActionError::NotConnected(_))));
}

#[tokio::test]
async fn actions_send_requests_and_surface_obs_errors() {
    let mock = MockObs::start().await;
    let client = connected(&mock).await;

    client.run(&ObsOp::SetScene { scene: "BRB".into() }).await.unwrap();
    assert_eq!(mock.requests_of("SetCurrentProgramScene"), [json!({"sceneName": "BRB"})]);

    client.run(&ObsOp::ToggleRecord).await.unwrap();
    assert_eq!(mock.requests_of("ToggleRecord").len(), 1);

    // The mock reports the item as hidden, so toggling shows it.
    client.run(&ObsOp::ToggleSource { scene: "Live".into(), source: "Logo".into() }).await.unwrap();
    assert_eq!(mock.requests_of("GetSceneItemId"), [json!({"sceneName": "Live", "sourceName": "Logo"})]);
    assert_eq!(
        mock.requests_of("SetSceneItemEnabled"),
        [json!({"sceneName": "Live", "sceneItemId": 2, "sceneItemEnabled": true})]
    );

    let error = client.run(&ObsOp::ToggleMute { input: "Missing".into() }).await.unwrap_err();
    assert!(matches!(&error, ActionError::Failed(m) if m == "No input was found by the name of Missing"), "{error}");
}

#[tokio::test]
async fn volume_requests_for_obs_inputs() {
    use omni_core::VolumeOp;
    let mock = MockObs::start().await;
    let client = connected(&mock).await;

    // Cached volume is 0.5; steps are relative to it and everything is clamped to 1.0.
    crate::volume::obs_input(&client, "Desktop", &VolumeOp::Step { delta: 0.25 }).await.unwrap();
    crate::volume::obs_input(&client, "Desktop", &VolumeOp::Set { value: 2.0 }).await.unwrap();
    let volumes = mock.requests_of("SetInputVolume");
    assert_eq!(volumes[0], json!({"inputName": "Desktop", "inputVolumeMul": 0.75}));
    assert_eq!(volumes[1], json!({"inputName": "Desktop", "inputVolumeMul": 1.0}));
}

#[tokio::test]
async fn events_update_the_state() {
    let mock = MockObs::start().await;
    let client = connected(&mock).await;

    mock.event("RecordStateChanged", json!({"outputActive": true, "outputState": "OBS_WEBSOCKET_OUTPUT_STARTED"}));
    wait_state(&client, |s| s.recording && !s.recording_paused).await;
    mock.event("RecordStateChanged", json!({"outputActive": true, "outputState": "OBS_WEBSOCKET_OUTPUT_PAUSED"}));
    wait_state(&client, |s| s.recording && s.recording_paused).await;

    mock.event("CurrentProgramSceneChanged", json!({"sceneName": "BRB"}));
    mock.event("InputMuteStateChanged", json!({"inputName": "Mic", "inputMuted": false}));
    mock.event("InputVolumeChanged", json!({"inputName": "Desktop", "inputVolumeMul": 0.25}));
    mock.event("SceneItemEnableStateChanged", json!({"sceneName": "Live", "sceneItemId": 2, "sceneItemEnabled": true}));
    mock.event("StreamStateChanged", json!({"outputActive": true, "outputState": "OBS_WEBSOCKET_OUTPUT_STARTED"}));
    let state = wait_state(&client, |s| s.streaming).await;
    assert_eq!(state.current_scene.as_deref(), Some("BRB"));
    assert!(!state.input_muted["Mic"]);
    assert_eq!(state.input_volume["Desktop"], 0.25);
    assert!(state.source_visible[&("Live".to_owned(), "Logo".to_owned())]);
}

#[tokio::test]
async fn structural_events_reload_the_state() {
    let mock = MockObs::start().await;
    let client = connected(&mock).await;
    assert!(!client.state().input_muted.contains_key("Webcam"));

    mock.inputs.lock().unwrap().push("Webcam");
    mock.event("InputCreated", json!({"inputName": "Webcam"}));
    let state = wait_state(&client, |s| s.input_muted.contains_key("Webcam")).await;
    assert_eq!(state.input_volume["Webcam"], 0.5);
    // A reload keeps the connection flag.
    assert!(state.connected);
}

#[tokio::test]
async fn reconnects_after_the_connection_drops() {
    let mock = MockObs::start().await;
    let client = connected(&mock).await;
    assert_eq!(mock.connections.load(Ordering::SeqCst), 1);

    mock.drop_connections();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while mock.connections.load(Ordering::SeqCst) < 2 {
        assert!(tokio::time::Instant::now() < deadline, "no reconnect");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    wait_state(&client, |s| s.connected && s.current_scene.is_some()).await;
    client.run(&ObsOp::ToggleRecord).await.unwrap();
}

#[tokio::test]
async fn settings_changes_reconnect_and_disable() {
    let mock = MockObs::start().await;
    let client = connected(&mock).await;

    // Same settings: nothing happens.
    client.configure(&mock.settings(PASSWORD));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(mock.connections.load(Ordering::SeqCst), 1);

    client.configure(&ObsSettings { enabled: false, ..mock.settings(PASSWORD) });
    wait_status(&client, |s| s.state == ConnState::Disabled).await;
    assert_eq!(client.state(), ObsState::default());
    assert!(matches!(client.run(&ObsOp::ToggleRecord).await, Err(ActionError::NotConnected(_))));

    client.configure(&mock.settings(PASSWORD));
    wait_state(&client, |s| s.connected).await;
    assert_eq!(mock.connections.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn inventory_lists_scenes_inputs_and_items() {
    let mock = MockObs::start().await;
    let client = connected(&mock).await;
    let inventory = client.inventory().await.unwrap();
    assert_eq!(inventory.scenes, ["Live", "BRB"]);
    assert_eq!(inventory.inputs, ["Mic", "Desktop"]);
    let items: Vec<_> = inventory.scene_items.iter().map(|s| (s.scene.as_str(), s.sources.clone())).collect();
    assert_eq!(items, [("Live", vec!["Camera".to_owned(), "Logo".to_owned()]), ("BRB", vec![])]);
}
