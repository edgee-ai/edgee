use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};

use super::message::Message;
use crate::error::Error;

// ── Usage ─────────────────────────────────────────────────────────────────

/// Token-level usage details for a completion.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens_details: Option<CompletionTokensDetails>,
}

/// Breakdown of prompt token counts.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
pub struct PromptTokensDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u32>,
    /// For Anthropic: tokens written into the prompt cache.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u32>,
}

/// Breakdown of completion token counts.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
pub struct CompletionTokensDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u32>,
}

// ── Finish reason ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ContentFilter,
    ToolCalls,
}

// ── Non-streaming response ────────────────────────────────────────────────

/// A complete (non-streaming) LLM response in OpenAI Chat Completions format.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CompletionResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<Choice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// A single choice within a [`CompletionResponse`].
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Choice {
    pub index: u32,
    pub message: Message,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
}

// ── Streaming response ────────────────────────────────────────────────────

/// A single streaming chunk in OpenAI SSE format (`object: "chat.completion.chunk"`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CompletionChunk {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
    /// Only present in the final chunk when `stream_options.include_usage` is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// A single choice within a [`CompletionChunk`].
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChunkChoice {
    pub index: u32,
    pub delta: Delta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
}

/// The incremental content delta for a streaming chunk.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Delta {
    /// Present only in the first chunk for a given choice.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<DeltaToolCall>>,
}

/// An incremental tool call in a streaming delta.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeltaToolCall {
    pub index: u32,
    pub id: String,
    #[serde(rename = "type")]
    pub tool_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<DeltaFunction>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeltaFunction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

// ── Unified service response ──────────────────────────────────────────────

/// The response type produced by [`crate::service::ProviderDispatchService`].
///
/// Callers match on this enum to handle streaming and non-streaming responses
/// uniformly through the same Tower service interface.
pub enum GatewayResponse {
    /// A complete, buffered response.
    Complete(CompletionResponse),
    /// A lazy stream of chunks. The HTTP request to the provider is not made
    /// until the stream is first polled.
    Stream(BoxStream<'static, Result<CompletionChunk, Error>>),
}
