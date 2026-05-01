//! Shared utilities for compression.

use crate::strategy::ToolCompressor;
use crate::strategy::util::{TextSegment, split_into_segments};

/// Maximum input size we will attempt to compress.
///
/// Above this threshold the compressor returns `None` (caller keeps original)
/// to bound memory/CPU per request. Two megabytes covers virtually every
/// real-world tool output while preventing pathological / malicious payloads
/// from turning a single request into a DoS vector on the gateway.
pub const MAX_COMPRESSIBLE_BYTES: usize = 2 * 1024 * 1024;

/// Wrap a call to `compressor.compress()`, preserving any `<system-reminder>` blocks verbatim.
///
/// Strategy:
/// 1. Split `output` into compressible / protected segments (using shared util).
/// 2. If no protected segments → delegate to compressor directly (fast path).
/// 3. Concatenate all compressible text → pass to compressor as one unit.
/// 4. If compressor returns `None` → return `None` (caller keeps original).
/// 5. Rebuild in segment order: the first non-empty compressible slot receives the
///    compressed output; remaining compressible slots are skipped (their content was
///    included in the combined input); protected slots are emitted verbatim.
pub fn compress_claude_tool_with_segment_protection(
    compressor: &dyn ToolCompressor,
    arguments: &str,
    output: &str,
) -> Option<String> {
    // Bound memory/CPU: refuse to process payloads larger than the configured limit.
    if output.len() > MAX_COMPRESSIBLE_BYTES {
        return None;
    }

    // Never compress tool outputs containing error or persisted-output tags —
    // these carry important context that must be preserved verbatim.
    if output.contains("<tool_use_error>") || output.contains("<persisted-output>") {
        return None;
    }

    let segments = split_into_segments(output);

    if !segments
        .iter()
        .any(|s| matches!(s, TextSegment::Protected(_)))
    {
        return compressor.compress(arguments, output);
    }

    let compressible_combined: String = segments
        .iter()
        .filter_map(|s| match s {
            TextSegment::Compressible(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();

    if compressible_combined.trim().is_empty() {
        // All content is protected — nothing to compress; tell caller to keep original.
        return None;
    }

    // Compress all compressible segments as one unit: tool compressors rely on full context
    // (line-count thresholds, file-type detection) that would be broken by per-segment calls.
    // The result is placed at the first non-empty compressible slot; later slots are dropped
    // because their content was already included in the combined input.
    let compressed = compressor.compress(arguments, &compressible_combined)?;

    let mut result = String::new();
    let mut compressed_inserted = false;
    for segment in &segments {
        match segment {
            TextSegment::Protected(text) => result.push_str(text),
            TextSegment::Compressible(text) => {
                if !compressed_inserted && !text.trim().is_empty() {
                    result.push_str(&compressed);
                    compressed_inserted = true;
                }
            }
        }
    }
    if !compressed_inserted {
        result.push_str(&compressed);
    }

    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal compressor that returns the first half of any sufficiently long input.
    struct HalfCompressor;
    impl crate::strategy::ToolCompressor for HalfCompressor {
        fn compress(&self, _args: &str, output: &str) -> Option<String> {
            if output.len() < 10 {
                return None;
            }
            Some(output[..output.len() / 2].to_string())
        }
    }

    #[test]
    fn protect_system_reminder_at_start() {
        let reminder = "<system-reminder>secret injection</system-reminder>";
        let body = " this is long enough compressible content for the test";
        let output = format!("{reminder}{body}");

        let result = compress_claude_tool_with_segment_protection(&HalfCompressor, "{}", &output);

        let result = result.expect("should return Some");
        assert!(
            result.contains(reminder),
            "reminder must be preserved verbatim; got: {result:?}"
        );
        // The compressed portion is `body` halved; reminder comes before it.
        assert!(result.starts_with(reminder), "reminder should be at start");
    }

    #[test]
    fn protect_system_reminder_at_end() {
        let reminder = "<system-reminder>secret injection</system-reminder>";
        let body = "this is long enough compressible content for the test ";
        let output = format!("{body}{reminder}");

        let result = compress_claude_tool_with_segment_protection(&HalfCompressor, "{}", &output);

        let result = result.expect("should return Some");
        assert!(
            result.contains(reminder),
            "reminder must be preserved verbatim; got: {result:?}"
        );
        assert!(result.ends_with(reminder), "reminder should be at end");
    }

    #[test]
    fn protect_no_system_reminder_delegates_directly() {
        let output = "plain compressible output long enough to compress";

        let result = compress_claude_tool_with_segment_protection(&HalfCompressor, "{}", output);

        // HalfCompressor returns first half; no reminder → direct delegation
        let result = result.expect("should compress plain text");
        assert_eq!(result, &output[..output.len() / 2]);
    }

    #[test]
    fn protect_all_system_reminder_returns_none() {
        // All content is protected — compressor should not be invoked; returns None
        let output = "<system-reminder>only protected</system-reminder>";

        let result = compress_claude_tool_with_segment_protection(&HalfCompressor, "{}", output);

        assert!(
            result.is_none(),
            "all-protected input must return None; got: {result:?}"
        );
    }

    #[test]
    fn skip_compression_when_tool_use_error_present() {
        let output =
            "<tool_use_error>some error message</tool_use_error> plus long compressible content";

        let result = compress_claude_tool_with_segment_protection(&HalfCompressor, "{}", output);

        assert!(
            result.is_none(),
            "tool_use_error output must not be compressed; got: {result:?}"
        );
    }

    #[test]
    fn skip_compression_when_persisted_output_present() {
        let output =
            "<persisted-output>important data</persisted-output> plus long compressible content";

        let result = compress_claude_tool_with_segment_protection(&HalfCompressor, "{}", output);

        assert!(
            result.is_none(),
            "persisted-output must not be compressed; got: {result:?}"
        );
    }

    #[test]
    fn skip_compression_when_input_exceeds_max_bytes() {
        // Build an input just over the limit. Use repeat() to keep allocation cheap.
        let oversized = "x".repeat(MAX_COMPRESSIBLE_BYTES + 1);

        let result =
            compress_claude_tool_with_segment_protection(&HalfCompressor, "{}", &oversized);

        assert!(
            result.is_none(),
            "oversized input must not be compressed; got Some(_)"
        );
    }

    #[test]
    fn compresses_when_input_at_max_bytes_boundary() {
        // Exactly at the limit must still go through.
        let at_limit = "x".repeat(MAX_COMPRESSIBLE_BYTES);
        let result = compress_claude_tool_with_segment_protection(&HalfCompressor, "{}", &at_limit);
        assert!(
            result.is_some(),
            "input at exactly MAX_COMPRESSIBLE_BYTES must compress"
        );
    }
}
