use serde::{Deserialize, Serialize};

// ── Content ───────────────────────────────────────────────────────────────

/// A content part within a multi-part message.
///
/// The `#[serde(other)]` catch-all preserves forward compatibility with
/// provider-specific content block types (e.g. Anthropic image blocks).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    /// Standard text part. Also accepted as `input_text` (Responses API) or
    /// `output_text` (Anthropic streaming).
    #[serde(alias = "input_text", alias = "output_text")]
    Text { text: String },
    #[serde(other)]
    Unknown,
}

/// The content of a message: either a plain string or an array of content parts.
///
/// Providers accept both forms; `#[serde(untagged)]` handles the ambiguity.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl MessageContent {
    /// Extract all text, joining multi-part content with double newlines.
    pub fn as_text(&self) -> String {
        match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Parts(parts) => parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text { text } => Some(text.as_str()),
                    ContentPart::Unknown => None,
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            MessageContent::Text(s) => s.is_empty(),
            MessageContent::Parts(parts) => parts.is_empty(),
        }
    }
}

impl Default for MessageContent {
    fn default() -> Self {
        MessageContent::Text(String::new())
    }
}

impl From<String> for MessageContent {
    fn from(s: String) -> Self {
        MessageContent::Text(s)
    }
}

impl From<&str> for MessageContent {
    fn from(s: &str) -> Self {
        MessageContent::Text(s.to_owned())
    }
}

// ── Messages ──────────────────────────────────────────────────────────────

/// A conversation message. The `role` field is used as the serde tag so
/// serialized JSON matches the OpenAI Chat Completions wire format exactly.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    /// OpenAI "developer" system prompt (treated as system by most providers).
    Developer(DeveloperMessage),
    System(SystemMessage),
    User(UserMessage),
    Assistant(AssistantMessage),
    Tool(ToolMessage),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeveloperMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub content: MessageContent,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SystemMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub content: MessageContent,
    /// Preserved for passthrough; Anthropic uses this for prompt caching.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UserMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub content: MessageContent,
    /// Preserved for passthrough; Anthropic uses this for prompt caching.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AssistantMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<MessageContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Preserved for passthrough; Anthropic uses this for prompt caching.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolMessage {
    pub content: MessageContent,
    pub tool_call_id: String,
}

// ── Tools ─────────────────────────────────────────────────────────────────

/// A tool (function) the model may call.
///
/// Custom `Deserialize` handles both the OpenAI nested format
/// (`{"type":"function","function":{...}}`) and the Anthropic flat format
/// (`{"name":"...","description":"...","input_schema":{...}}`).
#[derive(Debug, Clone)]
pub enum Tool {
    Function {
        function: FunctionDefinition,
    },
    /// Unknown tool type — preserved opaquely for passthrough.
    Unknown(serde_json::Value),
}

impl<'de> Deserialize<'de> for Tool {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        if v.get("type").and_then(|t| t.as_str()) == Some("function") {
            #[derive(Deserialize)]
            struct FunctionTool {
                function: FunctionDefinition,
            }
            serde_json::from_value::<FunctionTool>(v)
                .map(|t| Tool::Function {
                    function: t.function,
                })
                .map_err(serde::de::Error::custom)
        } else {
            Ok(Tool::Unknown(v))
        }
    }
}

impl Serialize for Tool {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        match self {
            Tool::Function { function } => {
                let mut map = s.serialize_map(Some(2))?;
                map.serialize_entry("type", "function")?;
                map.serialize_entry("function", function)?;
                map.end()
            }
            Tool::Unknown(v) => v.serialize(s),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FunctionDefinition {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for the function's parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

/// Controls which tool (if any) the model calls.
///
/// `#[serde(untagged)]` handles both string shortcuts (`"auto"`, `"required"`,
/// `"none"`) and the specific-function object `{"type":"function","function":{...}}`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ToolChoice {
    /// Simple mode string: `"auto"`, `"required"`, or `"none"`.
    Mode(String),
    /// Force a specific function: `{"type":"function","function":{"name":"..."}}`
    Specific {
        #[serde(rename = "type")]
        tool_type: String,
        function: ToolChoiceFunction,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolChoiceFunction {
    pub name: String,
}

/// A tool call made by the assistant in a response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_tool_type")]
    pub tool_type: String,
    pub function: FunctionCall,
}

fn default_tool_type() -> String {
    "function".to_string()
}

/// The function invocation within a tool call.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FunctionCall {
    pub name: String,
    /// JSON-encoded arguments string (as returned by the model).
    pub arguments: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_content_text_roundtrip() {
        let content = MessageContent::Text("hello".into());
        let json = serde_json::to_string(&content).unwrap();
        assert_eq!(json, r#""hello""#);
        let back: MessageContent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.as_text(), "hello");
    }

    #[test]
    fn tool_deserializes_function_nested_format() {
        let json = r#"{"type":"function","function":{"name":"get_weather","description":"Get weather","parameters":{}}}"#;
        let tool: Tool = serde_json::from_str(json).unwrap();
        assert!(matches!(tool, Tool::Function { .. }));
    }

    #[test]
    fn tool_choice_string_mode() {
        let json = r#""auto""#;
        let tc: ToolChoice = serde_json::from_str(json).unwrap();
        assert!(matches!(tc, ToolChoice::Mode(s) if s == "auto"));
    }

    #[test]
    fn message_tagged_by_role() {
        let json = r#"{"role":"user","content":"hello"}"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, Message::User(_)));
    }
}
