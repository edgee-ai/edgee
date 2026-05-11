//! Compression technique: inject `cache_control: ephemeral` on large stable
//! system prompts.
//!
//! Coding agents like Claude Code send a long, stable system prompt
//! (CLAUDE.md, agent rules, tool descriptions) on every request. Anthropic's
//! prompt cache can avoid re-encoding it — but only if the request marks the
//! block with a `cache_control` hint. Many clients don't.
//!
//! This technique scans every system / developer message and, for those large
//! enough to be worth caching but not yet hinted, injects
//! `{"type": "ephemeral"}`. Anthropic accepts up to 4 cache breakpoints per
//! request; we cap our own injections at 2 so a client that *also* adds hints
//! still has headroom.

use edgee_ai_gateway_core::{CompletionRequest, types::Message};
use serde_json::json;

use crate::technique::CompressionTechnique;

/// Below this many bytes of system content, don't bother — the prompt-cache
/// minimums for Anthropic models are around 1 KB and the round-trip
/// bookkeeping is not worth it for tiny system blocks.
pub const DEFAULT_MIN_CACHEABLE_BYTES: usize = 1024;

/// Maximum number of hints this technique adds per request. Leaves room for
/// upstream callers to set their own without blowing past Anthropic's
/// 4-breakpoint limit.
pub const DEFAULT_MAX_INJECTIONS: usize = 2;

/// Inject `cache_control: {"type": "ephemeral"}` on large, un-hinted system
/// prompts so the upstream provider can serve them from prompt cache.
#[derive(Debug, Clone)]
pub struct SystemPromptCacheTechnique {
    min_bytes: usize,
    max_injections: usize,
}

impl Default for SystemPromptCacheTechnique {
    fn default() -> Self {
        Self {
            min_bytes: DEFAULT_MIN_CACHEABLE_BYTES,
            max_injections: DEFAULT_MAX_INJECTIONS,
        }
    }
}

impl SystemPromptCacheTechnique {
    /// Build with default thresholds.
    pub fn new() -> Self {
        Self::default()
    }

    /// Minimum text size to consider hinting.
    pub fn with_min_bytes(mut self, min_bytes: usize) -> Self {
        self.min_bytes = min_bytes;
        self
    }

    /// Maximum number of cache_control hints this technique will add per
    /// request.
    pub fn with_max_injections(mut self, max_injections: usize) -> Self {
        self.max_injections = max_injections;
        self
    }
}

impl CompressionTechnique for SystemPromptCacheTechnique {
    fn name(&self) -> &'static str {
        "system-prompt-cache"
    }

    fn apply(&self, mut req: CompletionRequest) -> CompletionRequest {
        // Count hints already present so we stay idempotent across passes —
        // running apply() twice must not silently exceed `max_injections`.
        // Counts include hints set by upstream callers, since those still
        // consume Anthropic's 4-breakpoint budget.
        let mut total_hinted = req
            .messages
            .iter()
            .filter(|m| match m {
                Message::System(s) => s.cache_control.is_some(),
                Message::User(u) => u.cache_control.is_some(),
                Message::Assistant(a) => a.cache_control.is_some(),
                _ => false,
            })
            .count();

        for msg in &mut req.messages {
            if total_hinted >= self.max_injections {
                break;
            }

            if let Message::System(s) = msg
                && s.cache_control.is_none()
                && s.content.as_text().len() >= self.min_bytes
            {
                s.cache_control = Some(json!({"type": "ephemeral"}));
                total_hinted += 1;
            }
        }

        req
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgee_ai_gateway_core::types::{Message, MessageContent, SystemMessage, UserMessage};

    fn req_with_systems(blocks: Vec<usize>) -> CompletionRequest {
        let messages = blocks
            .into_iter()
            .map(|len| {
                Message::System(SystemMessage {
                    name: None,
                    content: MessageContent::Text("x".repeat(len)),
                    cache_control: None,
                })
            })
            .chain(std::iter::once(Message::User(UserMessage {
                name: None,
                content: MessageContent::Text("hello".into()),
                cache_control: None,
            })))
            .collect();
        CompletionRequest::new("test".to_string(), messages)
    }

    #[test]
    fn injects_on_large_system_prompt() {
        let req = req_with_systems(vec![2048]);
        let out = SystemPromptCacheTechnique::new().apply(req);
        let Message::System(s) = &out.messages[0] else {
            panic!("expected system message");
        };
        assert!(s.cache_control.is_some(), "large system must get hint");
    }

    #[test]
    fn skips_small_system_prompt() {
        let req = req_with_systems(vec![100]);
        let out = SystemPromptCacheTechnique::new().apply(req);
        let Message::System(s) = &out.messages[0] else {
            panic!("expected system message");
        };
        assert!(s.cache_control.is_none(), "small system must not get hint");
    }

    #[test]
    fn respects_existing_cache_control() {
        let mut req = req_with_systems(vec![2048]);
        if let Message::System(s) = &mut req.messages[0] {
            s.cache_control = Some(json!({"type": "ephemeral"}));
        }
        // Apply again — should not overwrite.
        let out = SystemPromptCacheTechnique::new().apply(req);
        let Message::System(s) = &out.messages[0] else {
            panic!()
        };
        assert_eq!(s.cache_control, Some(json!({"type": "ephemeral"})));
    }

    #[test]
    fn caps_injections_at_max() {
        // Three large system blocks; default cap = 2.
        let req = req_with_systems(vec![2048, 2048, 2048]);
        let out = SystemPromptCacheTechnique::new().apply(req);
        let hinted = out
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::System(s) => Some(s.cache_control.is_some()),
                _ => None,
            })
            .filter(|h| *h)
            .count();
        assert_eq!(hinted, 2, "must stop at the configured max");
    }

    #[test]
    fn idempotent_on_second_apply() {
        let req = req_with_systems(vec![2048, 2048, 2048]);
        let tech = SystemPromptCacheTechnique::new();
        let once = tech.apply(req);
        let twice = tech.apply(once);
        let hinted = twice
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::System(s) => Some(s.cache_control.is_some()),
                _ => None,
            })
            .filter(|h| *h)
            .count();
        assert_eq!(hinted, 2, "second apply must not add more hints");
    }

    #[test]
    fn custom_thresholds() {
        let req = req_with_systems(vec![500, 500, 500]);
        let out = SystemPromptCacheTechnique::new()
            .with_min_bytes(100)
            .with_max_injections(1)
            .apply(req);
        let hinted = out
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::System(s) => Some(s.cache_control.is_some()),
                _ => None,
            })
            .filter(|h| *h)
            .count();
        assert_eq!(hinted, 1);
    }
}
