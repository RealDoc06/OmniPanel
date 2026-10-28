//! `http` action: webhooks and local APIs.

use std::error::Error;
use std::time::Duration;

use omni_core::{ActionOutcome, HttpHeader, HttpMethod};
use reqwest::header::{CONTENT_TYPE, HeaderName, HeaderValue};
use reqwest::{Client, Method, Response, Url};

use crate::{ActionError, Result};

const TIMEOUT: Duration = Duration::from_secs(15);
/// Response bytes read for the preview; the rest of the body is never downloaded.
const PREVIEW_BYTES: usize = 2048;
/// Characters of the preview shown in the outcome.
const PREVIEW_CHARS: usize = 200;

pub(crate) async fn run(
    client: &Client,
    method: HttpMethod,
    url: &str,
    headers: &[HttpHeader],
    body: Option<&str>,
) -> Result<ActionOutcome> {
    let url = parse_url(url)?;
    let mut request = client.request(to_reqwest(method), url).timeout(TIMEOUT);
    let mut has_content_type = false;
    for HttpHeader { name, value } in headers {
        let name = HeaderName::from_bytes(name.trim().as_bytes())
            .map_err(|_| ActionError::Invalid(format!("bad header name \"{name}\"")))?;
        let value =
            HeaderValue::from_str(value).map_err(|_| ActionError::Invalid(format!("bad value for header {name}")))?;
        has_content_type |= name == CONTENT_TYPE;
        request = request.header(name, value);
    }
    if let Some(body) = body.filter(|b| !b.is_empty()) {
        // Webhook bodies are almost always JSON; servers reject them without the header.
        if !has_content_type
            && serde_json::from_str::<serde_json::Value>(body).is_ok_and(|v| v.is_object() || v.is_array())
        {
            request = request.header(CONTENT_TYPE, "application/json");
        }
        request = request.body(body.to_owned());
    }

    let mut response = request.send().await.map_err(request_error)?;
    let status = response.status();
    let preview = preview(&mut response, "").await;
    let message = if preview.is_empty() { status.to_string() } else { format!("{status}: {preview}") };
    if status.is_success() { Ok(ActionOutcome { message: Some(message) }) } else { Err(ActionError::Failed(message)) }
}

/// A missing scheme means a local address typed without `http://`. Errors
/// never repeat the URL: it may carry credentials or secret query values.
fn parse_url(text: &str) -> Result<Url> {
    let text = text.trim();
    let with_scheme = if text.contains("://") { text.to_owned() } else { format!("http://{text}") };
    let url = Url::parse(&with_scheme).map_err(|e| ActionError::Invalid(format!("the URL is not valid ({e})")))?;
    match url.scheme() {
        "http" | "https" if url.host_str().is_some() => Ok(url),
        _ => Err(ActionError::Invalid("only http:// and https:// URLs are supported".into())),
    }
}

fn to_reqwest(method: HttpMethod) -> Method {
    match method {
        HttpMethod::Get => Method::GET,
        HttpMethod::Post => Method::POST,
        HttpMethod::Put => Method::PUT,
        HttpMethod::Patch => Method::PATCH,
        HttpMethod::Delete => Method::DELETE,
    }
}

/// Start of the response body for messages: at most [`PREVIEW_BYTES`] are
/// downloaded and `secret` (when not empty) is replaced by `[redacted]`.
pub(crate) async fn preview(response: &mut Response, secret: &str) -> String {
    let mut bytes = Vec::new();
    let mut complete = false;
    while bytes.len() < PREVIEW_BYTES {
        match response.chunk().await {
            Ok(Some(chunk)) => bytes.extend_from_slice(&chunk),
            Ok(None) => {
                complete = true;
                break;
            }
            Err(_) => break,
        }
    }
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if !secret.is_empty() {
        text = text.replace(secret, "[redacted]");
        if !complete {
            // A secret cut off by the read limit escapes the replace: drop the tail it could be in.
            text.truncate(text.floor_char_boundary(text.len().saturating_sub(secret.len())));
        }
    }
    preview_text(&text)
}

/// Whitespace-collapsed start of the body.
fn preview_text(body: &str) -> String {
    let collapsed = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= PREVIEW_CHARS {
        return collapsed;
    }
    let cut: String = collapsed.chars().take(PREVIEW_CHARS).collect();
    format!("{cut}\u{2026}")
}

/// Human readable transport error (without the URL, which may carry secrets).
pub(crate) fn request_error(error: reqwest::Error) -> ActionError {
    if error.is_timeout() {
        return ActionError::Timeout(TIMEOUT.as_secs() as u32);
    }
    let error = error.without_url();
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    ActionError::Failed(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::MockHttp;

    fn header(name: &str, value: &str) -> HttpHeader {
        HttpHeader { name: name.into(), value: value.into() }
    }

    #[tokio::test]
    async fn sends_method_headers_and_body_and_previews_the_response() {
        let server = MockHttp::start(|_| (200, "{\n  \"ok\": true\n}".into())).await;
        let outcome = run(
            &Client::new(),
            HttpMethod::Post,
            &format!("{}/hook?a=1", server.url()),
            &[header("X-Token", "secret")],
            Some(r#"{"scene":"live"}"#),
        )
        .await
        .unwrap();
        assert_eq!(outcome.message.as_deref(), Some(r#"200 OK: { "ok": true }"#));

        let request = &server.requests()[0];
        assert_eq!((request.method.as_str(), request.path.as_str()), ("POST", "/hook?a=1"));
        assert_eq!(request.headers["x-token"], "secret");
        assert_eq!(request.headers["content-type"], "application/json");
        assert_eq!(request.body, r#"{"scene":"live"}"#);
    }

    #[tokio::test]
    async fn explicit_content_type_wins_and_plain_bodies_are_untouched() {
        let server = MockHttp::start(|_| (204, String::new())).await;
        let outcome =
            run(&Client::new(), HttpMethod::Put, &server.url(), &[header("Content-Type", "text/plain")], Some("{}"))
                .await
                .unwrap();
        assert_eq!(outcome.message.as_deref(), Some("204 No Content"));
        assert_eq!(server.requests()[0].headers["content-type"], "text/plain");

        run(&Client::new(), HttpMethod::Post, &server.url(), &[], Some("hello")).await.unwrap();
        assert!(!server.requests()[1].headers.contains_key("content-type"));
    }

    #[tokio::test]
    async fn error_statuses_fail_with_the_body() {
        let server = MockHttp::start(|_| (404, "no such hook".into())).await;
        let err = run(&Client::new(), HttpMethod::Get, &server.url(), &[], None).await.unwrap_err();
        assert!(matches!(&err, ActionError::Failed(m) if m == "404 Not Found: no such hook"), "{err}");
    }

    #[tokio::test]
    async fn rejects_bad_input_and_unreachable_hosts() {
        let client = Client::new();
        for url in ["ftp://example.com", "http://", "not a url"] {
            let err = run(&client, HttpMethod::Get, url, &[], None).await.unwrap_err();
            assert!(matches!(err, ActionError::Invalid(_)), "{url}: {err}");
        }
        // The URL can carry secrets, so errors never repeat it.
        for url in ["http://127.0.0.1:bad/?api_key=SECRET", "ftp://user:SECRET@example.com"] {
            let err = run(&client, HttpMethod::Get, url, &[], None).await.unwrap_err();
            assert!(!err.to_string().contains("SECRET"), "{err}");
        }
        let bad_header = [header("bad name", "x")];
        assert!(matches!(
            run(&client, HttpMethod::Get, "http://127.0.0.1:1", &bad_header, None).await,
            Err(ActionError::Invalid(_))
        ));
        // Nothing listens on port 1.
        assert!(matches!(run(&client, HttpMethod::Get, "127.0.0.1:1", &[], None).await, Err(ActionError::Failed(_))));
    }

    #[test]
    fn long_previews_are_cut() {
        assert_eq!(preview_text("  a \n b  "), "a b");
        let cut = preview_text(&"x".repeat(500));
        assert_eq!(cut.chars().count(), PREVIEW_CHARS + 1);
    }
}
