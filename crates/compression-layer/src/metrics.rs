//! Per-tool compression metrics.
//!
//! Counters live in [`CompressionMetrics`], shared (`Arc`) by every
//! [`CompressionService`](crate::CompressionService) handle that points at the
//! same [`CompressionConfig`](crate::CompressionConfig). Every tool message the
//! layer processes is recorded — either as a successful compression
//! (`record_compression`) or as a skip (`record_skip`, when the compressor
//! returned `None`).
//!
//! Callers retrieve a stable, sorted view of the counters with
//! [`CompressionMetrics::snapshot`], and a single aggregate row with
//! [`CompressionMetrics::totals`]. Both methods take a short-lived lock and
//! return owned data, so they are safe to call from any thread.

use std::collections::HashMap;
use std::sync::Mutex;

/// Aggregated compression statistics for a single tool.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ToolStats {
    /// Number of tool messages where compression actually replaced the content.
    pub invocations: u64,
    /// Number of tool messages where the compressor returned `None`
    /// (output was kept verbatim — too small, oversized, already compressed,
    /// protected, etc.).
    pub skipped: u64,
    /// Total input bytes seen, regardless of outcome.
    pub bytes_in: u64,
    /// Total output bytes after compression. Equals `bytes_in` for skipped
    /// rows, so `bytes_in - bytes_out` is the raw byte savings.
    pub bytes_out: u64,
}

/// Thread-safe per-tool counters. Cheap to clone via `Arc`.
#[derive(Debug, Default)]
pub struct CompressionMetrics {
    // Hot path is "record one tool message"; a single mutex around a small
    // HashMap is faster than per-entry atomics under realistic contention
    // and keeps the public API trivial. Switch to DashMap only if profiling
    // proves the lock is contended.
    inner: Mutex<HashMap<String, ToolStats>>,
}

impl CompressionMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a successful compression: input shrunk from `bytes_in` to `bytes_out`.
    pub fn record_compression(&self, tool: &str, bytes_in: usize, bytes_out: usize) {
        let mut map = self
            .inner
            .lock()
            .expect("compression metrics mutex poisoned");
        let stats = map.entry(tool.to_string()).or_default();
        stats.invocations += 1;
        stats.bytes_in += bytes_in as u64;
        stats.bytes_out += bytes_out as u64;
    }

    /// Record a skipped tool message: compressor returned `None` so the
    /// original output is kept. We charge `bytes_in` to both counters so the
    /// "savings" delta stays accurate when summed across tools.
    pub fn record_skip(&self, tool: &str, bytes_in: usize) {
        let mut map = self
            .inner
            .lock()
            .expect("compression metrics mutex poisoned");
        let stats = map.entry(tool.to_string()).or_default();
        stats.skipped += 1;
        stats.bytes_in += bytes_in as u64;
        stats.bytes_out += bytes_in as u64;
    }

    /// Snapshot of per-tool stats, sorted by tool name for stable output.
    pub fn snapshot(&self) -> Vec<(String, ToolStats)> {
        let map = self
            .inner
            .lock()
            .expect("compression metrics mutex poisoned");
        let mut entries: Vec<(String, ToolStats)> =
            map.iter().map(|(k, v)| (k.clone(), *v)).collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries
    }

    /// Aggregate row across every tool.
    pub fn totals(&self) -> ToolStats {
        let map = self
            .inner
            .lock()
            .expect("compression metrics mutex poisoned");
        map.values().fold(ToolStats::default(), |mut acc, s| {
            acc.invocations += s.invocations;
            acc.skipped += s.skipped;
            acc.bytes_in += s.bytes_in;
            acc.bytes_out += s.bytes_out;
            acc
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_compression_accumulates() {
        let m = CompressionMetrics::new();
        m.record_compression("Bash", 1000, 200);
        m.record_compression("Bash", 500, 100);

        let snap = m.snapshot();
        assert_eq!(snap.len(), 1);
        let (name, stats) = &snap[0];
        assert_eq!(name, "Bash");
        assert_eq!(stats.invocations, 2);
        assert_eq!(stats.skipped, 0);
        assert_eq!(stats.bytes_in, 1500);
        assert_eq!(stats.bytes_out, 300);
    }

    #[test]
    fn record_skip_charges_full_size() {
        let m = CompressionMetrics::new();
        m.record_skip("Read", 800);

        let snap = m.snapshot();
        let stats = snap[0].1;
        assert_eq!(stats.skipped, 1);
        assert_eq!(stats.bytes_in, 800);
        assert_eq!(stats.bytes_out, 800);
    }

    #[test]
    fn snapshot_sorted_by_tool_name() {
        let m = CompressionMetrics::new();
        m.record_compression("Read", 100, 50);
        m.record_compression("Bash", 100, 50);
        m.record_compression("Glob", 100, 50);

        let snap = m.snapshot();
        let names: Vec<&str> = snap.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["Bash", "Glob", "Read"]);
    }

    #[test]
    fn totals_sums_across_tools() {
        let m = CompressionMetrics::new();
        m.record_compression("Bash", 1000, 200);
        m.record_compression("Read", 2000, 500);
        m.record_skip("Glob", 50);

        let t = m.totals();
        assert_eq!(t.invocations, 2);
        assert_eq!(t.skipped, 1);
        assert_eq!(t.bytes_in, 3050);
        assert_eq!(t.bytes_out, 750);
    }

    #[test]
    fn empty_metrics_returns_default() {
        let m = CompressionMetrics::new();
        assert!(m.snapshot().is_empty());
        assert_eq!(m.totals(), ToolStats::default());
    }
}
