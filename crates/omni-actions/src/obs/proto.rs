//! obs-websocket v5 wire format: handshake, authentication, requests.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::ActionError;

/// WebSocket subprotocol obs-websocket requires.
pub(super) const SUBPROTOCOL: &str = "obswebsocket.json";

/// Event categories OmniPanel listens to: config (scene collection
/// changes), scenes, inputs, outputs, scene items and UI (studio mode).
/// The high-volume ones (input volume meters, transforms...) are left out.
const EVENT_SUBSCRIPTIONS: u32 = 1 << 1 | 1 << 2 | 1 << 3 | 1 << 6 | 1 << 7 | 1 << 10;

/// Close code obs-websocket sends when authentication failed.
pub(super) const CLOSE_AUTH_FAILED: u16 = 4009;

/// `base64(sha256(base64(sha256(password + salt)) + challenge))`
pub(super) fn auth_response(password: &str, salt: &str, challenge: &str) -> String {
    let secret = STANDARD.encode(Sha256::digest(format!("{password}{salt}")));
    STANDARD.encode(Sha256::digest(format!("{secret}{challenge}")))
}

/// Identify message (op 1).
pub(super) fn identify(authentication: Option<String>) -> String {
    let mut data = json!({ "rpcVersion": 1, "eventSubscriptions": EVENT_SUBSCRIPTIONS });
    if let Some(authentication) = authentication {
        data["authentication"] = authentication.into();
    }
    json!({ "op": 1, "d": data }).to_string()
}

/// Request message (op 6).
pub(super) fn request(id: &str, kind: &str, data: &Value) -> String {
    let mut body = json!({ "requestType": kind, "requestId": id });
    if !data.is_null() {
        body["requestData"] = data.clone();
    }
    json!({ "op": 6, "d": body }).to_string()
}

#[derive(Debug, Deserialize)]
pub(super) struct Challenge {
    pub challenge: String,
    pub salt: String,
}

#[derive(Debug)]
pub(super) enum Incoming {
    /// Op 0; `Some` when the server wants a password.
    Hello(Option<Challenge>),
    /// Op 2.
    Identified,
    /// Op 5.
    Event { kind: String, data: Value },
    /// Op 7.
    Response(Response),
    /// Anything else (batch responses, unknown ops).
    Other,
}

#[derive(Debug)]
pub(super) struct Response {
    pub id: String,
    pub ok: bool,
    pub code: u64,
    pub comment: Option<String>,
    pub data: Value,
}

impl Response {
    pub fn into_result(self) -> crate::Result<Value> {
        if self.ok {
            return Ok(self.data);
        }
        Err(ActionError::Failed(match self.comment.filter(|c| !c.is_empty()) {
            Some(comment) => comment,
            None => format!("OBS refused the request (code {})", self.code),
        }))
    }
}

#[derive(Deserialize)]
struct Frame {
    op: u8,
    #[serde(default)]
    d: Value,
}

pub(super) fn parse(text: &str) -> Result<Incoming, serde_json::Error> {
    let Frame { op, d } = serde_json::from_str(text)?;
    Ok(match op {
        0 => Incoming::Hello(serde_json::from_value(d["authentication"].clone()).ok()),
        2 => Incoming::Identified,
        5 => Incoming::Event {
            kind: d["eventType"].as_str().unwrap_or_default().to_owned(),
            data: d["eventData"].clone(),
        },
        7 => Incoming::Response(Response {
            id: d["requestId"].as_str().unwrap_or_default().to_owned(),
            ok: d["requestStatus"]["result"].as_bool().unwrap_or(false),
            code: d["requestStatus"]["code"].as_u64().unwrap_or(0),
            comment: d["requestStatus"]["comment"].as_str().map(str::to_owned),
            data: d["responseData"].clone(),
        }),
        _ => Incoming::Other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_matches_the_protocol_example() {
        assert_eq!(
            auth_response(
                "supersecretpassword",
                "lM1GncleQOaCu9lT1yeUZhFYnqhsLLP1G5lAGo3ixaI=",
                "+IxH4CnCiqpX1rM9scsNynZzbOe4KhDeYcTNS3PDaeY="
            ),
            "1Ct943GAT+6YQUUX47Ia/ncufilbe6+oD6lY+5kaCu4="
        );
    }

    #[test]
    fn identify_subscribes_without_high_volume_events() {
        let frame: Value = serde_json::from_str(&identify(Some("auth".into()))).unwrap();
        assert_eq!(frame["op"], 1);
        assert_eq!(frame["d"]["authentication"], "auth");
        let mask = frame["d"]["eventSubscriptions"].as_u64().unwrap();
        // Outputs (64) and Inputs (8) in, InputVolumeMeters (1 << 16) out.
        assert!(mask & 64 != 0 && mask & 8 != 0 && mask & (1 << 16) == 0);
        assert!(serde_json::from_str::<Value>(&identify(None)).unwrap()["d"].get("authentication").is_none());
    }

    #[test]
    fn parses_server_messages() {
        let hello = parse(r#"{"op":0,"d":{"rpcVersion":1,"authentication":{"challenge":"c","salt":"s"}}}"#).unwrap();
        assert!(
            matches!(hello, Incoming::Hello(Some(Challenge { challenge, salt })) if challenge == "c" && salt == "s")
        );
        assert!(matches!(parse(r#"{"op":0,"d":{"rpcVersion":1}}"#).unwrap(), Incoming::Hello(None)));

        let event =
            parse(r#"{"op":5,"d":{"eventType":"RecordStateChanged","eventData":{"outputActive":true}}}"#).unwrap();
        assert!(
            matches!(event, Incoming::Event { kind, data } if kind == "RecordStateChanged" && data["outputActive"] == true)
        );

        let failed = parse(
            r#"{"op":7,"d":{"requestId":"7","requestStatus":{"result":false,"code":600,"comment":"No scene found"}}}"#,
        )
        .unwrap();
        let Incoming::Response(response) = failed else { panic!("not a response") };
        assert_eq!(response.id, "7");
        assert!(matches!(response.into_result(), Err(ActionError::Failed(m)) if m == "No scene found"));

        assert!(parse("not json").is_err());
    }
}
