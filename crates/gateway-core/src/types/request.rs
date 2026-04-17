use serde::{Deserialize, Serialize};

use super::message::{Message, Tool, ToolChoice};

/// A canonical LLM completion request in OpenAI Chat Completions format.
///
/// This is the provider-agnostic entry point for the [`crate::service::ProviderDispatchService`].
/// Provider implementations translate from this type to their native API format.
///
/// The `messages` field also accepts the Responses API `input` alias so that
/// the same type can represent requests from both Chat Completions and Responses API
/// clients before they are normalised.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CompletionRequest {
    /// Model identifier (e.g. `"claude-opus-4-5"`, `"gpt-4o"`).
    pub model: String,

    /// Conversation history.
    ///
    /// Accepts `"messages"` (Chat Completions) or `"input"` (Responses API).
    #[serde(alias = "input")]
    pub messages: Vec<Message>,

    /// Maximum tokens to generate in the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// Whether to stream the response as SSE chunks.
    #[serde(default)]
    pub stream: bool,

    /// Tools (functions) the model may call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,

    /// Controls which tool (if any) the model calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,

    /// Sampling temperature (0–2). Higher = more random.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    /// Nucleus sampling probability mass.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
}

impl CompletionRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            max_tokens: None,
            stream: false,
            tools: Vec::new(),
            tool_choice: None,
            temperature: None,
            top_p: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::message::{MessageContent, UserMessage};

    #[test]
    fn completion_request_minimal_roundtrip() {
        let req = CompletionRequest::new(
            "gpt-4o",
            vec![Message::User(UserMessage {
                name: None,
                content: MessageContent::Text("Hello".into()),
                cache_control: None,
            })],
        );
        let json = serde_json::to_string(&req).unwrap();
        let back: CompletionRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.model, "gpt-4o");
        assert!(!back.stream);
    }

    #[test]
    fn accepts_input_alias_for_messages() {
        let json = r#"{"model":"claude-opus-4-5","input":[{"role":"user","content":"Hi"}]}"#;
        let req: CompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.model, "claude-opus-4-5");
        assert_eq!(req.messages.len(), 1);
    }
}
