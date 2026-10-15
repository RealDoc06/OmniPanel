//! AI providers that turn a description into a validated OmniPanel control.

mod prompt;
mod provider;
mod safety;

use omni_core::{AiContext, AiProvider, AiSettings, ChatMessage, ChatRole, GenerateRequest, GeneratedControl};

/// Configuration, transport, provider, or generated-control failure.
#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("AI provider is not configured: {0}")]
    NotConfigured(String),
    #[error("request failed: {0}")]
    Http(String),
    #[error("provider error ({status}): {message}")]
    Provider { status: u16, message: String },
    #[error("model returned an invalid control: {0}")]
    InvalidOutput(String),
}

/// Result of an AI operation.
pub type Result<T> = std::result::Result<T, AiError>;

/// Default model when settings omit one; custom endpoints require an explicit model.
pub fn default_model(provider: AiProvider) -> &'static str {
    match provider {
        AiProvider::Openai => "gpt-6-luna",
        AiProvider::Anthropic => "claude-sonnet-5-5",
        AiProvider::Ollama => "llama3.2",
        AiProvider::Openrouter => "openai/gpt-6-luna",
        AiProvider::Custom => "",
    }
}

/// Human-readable provider name for messages.
pub fn provider_name(provider: AiProvider) -> &'static str {
    match provider {
        AiProvider::Openai => "OpenAI",
        AiProvider::Anthropic => "Anthropic",
        AiProvider::Ollama => "Ollama",
        AiProvider::Openrouter => "OpenRouter",
        AiProvider::Custom => "the custom endpoint",
    }
}

/// API root, overridden by settings; custom endpoints have no default.
pub fn default_base_url(provider: AiProvider) -> Option<&'static str> {
    match provider {
        AiProvider::Openai => Some("https://api.openai.com/v1"),
        AiProvider::Anthropic => Some("https://api.anthropic.com"),
        AiProvider::Ollama => Some("http://localhost:11434"),
        AiProvider::Openrouter => Some("https://openrouter.ai/api/v1"),
        AiProvider::Custom => None,
    }
}

/// Shared HTTP client for all providers. Generation never executes actions.
pub struct AiClient {
    http: reqwest::Client,
}

impl AiClient {
    /// Reuse the host's HTTP client; requests have their own bounded timeouts.
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// Generate or refine a control, retrying malformed control JSON once.
    pub async fn generate_control(
        &self,
        settings: &AiSettings,
        context: &AiContext,
        request: &GenerateRequest,
    ) -> Result<GeneratedControl> {
        let config = provider::Config::new(settings)?;
        let system = prompt::system(context);
        let mut messages = request.history.clone();
        messages.push(ChatMessage {
            role: ChatRole::User,
            content: format!(
                "Current button action (JSON, null means a new button): {}\nRequest: {}",
                serde_json::to_string(&request.current).map_err(|e| AiError::InvalidOutput(e.to_string()))?,
                request.prompt
            ),
        });
        for attempt in 0..2 {
            let text = self.complete(&config, &system, &messages, true).await?;
            match serde_json::from_str::<GeneratedControl>(extract_json(&text)) {
                Ok(mut control) => {
                    safety::validate(&control.action)?;
                    control.label = control.label.trim().chars().take(14).collect();
                    // Unknown icons render poorly. Use the documented Lucide subset or no icon.
                    control.icon = control.icon.filter(|name| prompt::ICONS.contains(&name.as_str()));
                    return Ok(control);
                }
                Err(error) if attempt == 0 => {
                    messages.push(ChatMessage { role: ChatRole::Assistant, content: text });
                    messages.push(ChatMessage {
                        role: ChatRole::User,
                        content: format!(
                            "The control could not be deserialized: {error}. Correct it to match the schema. Return only the complete JSON object."
                        ),
                    });
                }
                Err(error) => return Err(AiError::InvalidOutput(error.to_string())),
            }
        }
        unreachable!("the final attempt returns a control or an error")
    }

    /// Model ids offered by the provider, sorted and deduplicated.
    pub async fn list_models(&self, settings: &AiSettings) -> Result<Vec<String>> {
        let config = provider::Config::new(settings)?;
        self.models(&config).await
    }

    /// Small generation request proving the key, endpoint, and selected model work.
    pub async fn test(&self, settings: &AiSettings) -> Result<String> {
        let config = provider::Config::new(settings)?;
        let messages = [ChatMessage { role: ChatRole::User, content: "Reply with OK.".into() }];
        self.complete(&config, "Reply briefly.", &messages, false).await?;
        Ok(format!("Connected. Model {} is responding.", config.model()?))
    }
}

/// Locate the outer JSON object, ignoring fences and braces inside JSON strings.
fn extract_json(text: &str) -> &str {
    let text = text.trim();
    let Some(start) = text.find('{') else {
        return text;
    };
    let mut depth = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else {
            match ch {
                '"' => in_string = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &text[start..=start + offset];
                    }
                }
                _ => {}
            }
        }
    }
    &text[start..]
}

#[cfg(test)]
mod tests;
