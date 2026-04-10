/// Runtime configuration for a single LLM provider.
///
/// API keys are provided by the caller at construction time; the core crate
/// never resolves, rotates, or validates credentials.
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
