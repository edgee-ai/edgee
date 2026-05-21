/// Runtime configuration for a single LLM provider, used by the dispatch path.
///
/// API keys are provided by the caller at construction time; the core crate
/// never resolves, rotates, or validates credentials.
///
/// **Note**: this type is consumed by the [`crate::Provider`] dispatch trait.
/// Passthrough services do not use it because they forward the client's own
/// credentials verbatim — see [`AnthropicPassthroughConfig`] and
/// [`OpenAIPassthroughConfig`] instead.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Provider API key (e.g. Anthropic `x-api-key` or OpenAI `Bearer` token).
    pub api_key: String,
    /// Override the provider's default base URL (e.g. for proxies or local stubs).
    /// `None` means use the provider's production endpoint.
    pub base_url: Option<String>,
}

impl ProviderConfig {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: None,
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }
}

/// Configuration for the Anthropic passthrough service.
///
/// Holds only routing data: the gateway never injects its own credentials on
/// the passthrough path, so there is no `api_key` field.
#[derive(Debug, Clone)]
pub struct AnthropicPassthroughConfig {
    /// Base URL of the Anthropic Messages API. The path `/v1/messages` is
    /// appended at request time.
    pub base_url: String,
}

impl AnthropicPassthroughConfig {
    pub const DEFAULT_BASE_URL: &'static str = "https://api.anthropic.com";

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }
}

impl Default for AnthropicPassthroughConfig {
    fn default() -> Self {
        Self {
            base_url: Self::DEFAULT_BASE_URL.to_owned(),
        }
    }
}

/// Configuration for the OpenAI passthrough service.
///
/// OpenAI's "Responses" API has two production endpoints depending on the
/// type of credential the caller presents. Both endpoints are configurable:
///
/// - [`Self::api_url`] is used when the request bears an OpenAI Platform
///   project key (`Authorization: Bearer sk-proj-…`).
/// - [`Self::chatgpt_url`] is used in every other case (Codex CLI default).
///
/// To force all traffic to one endpoint, set both fields to the same URL.
#[derive(Debug, Clone)]
pub struct OpenAIPassthroughConfig {
    /// Endpoint used when the request bears an OpenAI Platform project key.
    pub api_url: String,
    /// Endpoint used by default for non-`sk-proj-` requests.
    pub chatgpt_url: String,
}

impl OpenAIPassthroughConfig {
    pub const DEFAULT_API_URL: &'static str = "https://api.openai.com/v1/responses";
    pub const DEFAULT_CHATGPT_URL: &'static str = "https://chatgpt.com/backend-api/codex/responses";

    pub fn with_api_url(mut self, api_url: impl Into<String>) -> Self {
        self.api_url = api_url.into();
        self
    }

    pub fn with_chatgpt_url(mut self, chatgpt_url: impl Into<String>) -> Self {
        self.chatgpt_url = chatgpt_url.into();
        self
    }
}

impl Default for OpenAIPassthroughConfig {
    fn default() -> Self {
        Self {
            api_url: Self::DEFAULT_API_URL.to_owned(),
            chatgpt_url: Self::DEFAULT_CHATGPT_URL.to_owned(),
        }
    }
}
