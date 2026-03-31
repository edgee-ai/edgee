//! Compressor for the Claude Code `Glob` tool output.
//!
//! Glob returns file paths, one per line, sorted by modification time.
//! This compressor groups paths by parent directory and adds an extension
//! summary — the same approach used by the bash `find` compressor.

use std::collections::HashMap;
use std::path::Path;

use super::ClaudeToolCompressor;

/// Below this threshold, leave output as-is.
const SMALL_THRESHOLD: usize = 30;
/// Maximum paths to show before truncating.
const MAX_RESULTS: usize = 50;

pub struct GlobCompressor;

impl ClaudeToolCompressor for GlobCompressor {
    fn compress(&self, _arguments: &str, output: &str) -> Option<String> {
        let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();

        if lines.len() < SMALL_THRESHOLD {
            return None;
        }

        let compressed = compact_glob(&lines);
        Some(compressed)
    }
}

fn compact_glob(paths: &[&str]) -> String {
    let mut by_dir: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut by_ext: HashMap<String, usize> = HashMap::new();

    for path in paths {
        let p = Path::new(path);
        let dir = p.parent().map(|d| d.to_str().unwrap_or(".")).unwrap_or(".");
        let dir = if dir.is_empty() { "." } else { dir };
        let filename = p
            .file_name()
            .map(|f| f.to_str().unwrap_or(""))
            .unwrap_or("");

        by_dir.entry(dir).or_default().push(filename);

        let ext = p
            .extension()
            .map(|e| format!(".{}", e.to_str().unwrap_or("")))
            .unwrap_or_else(|| "no ext".to_string());
        *by_ext.entry(ext).or_default() += 1;
    }

    let mut dirs: Vec<_> = by_dir.keys().copied().collect();
    dirs.sort();

    let total = paths.len();
    let mut out = format!("{}F {}D:\n\n", total, dirs.len());

    let mut shown = 0;

    for dir in &dirs {
        if shown >= MAX_RESULTS {
            break;
        }

        let files_in_dir = &by_dir[dir];
        let dir_display = compact_path(dir);
        let remaining = MAX_RESULTS - shown;

        if files_in_dir.len() <= remaining {
            out.push_str(&format!("{}/ {}\n", dir_display, files_in_dir.join(" ")));
            shown += files_in_dir.len();
        } else {
            let partial: Vec<&str> = files_in_dir.iter().take(remaining).copied().collect();
            out.push_str(&format!("{}/ {}\n", dir_display, partial.join(" ")));
            shown += partial.len();
            break;
        }
    }

    if shown < total {
        out.push_str(&format!("+{} more\n", total - shown));
    }

    if by_ext.len() > 1 {
        let mut exts: Vec<_> = by_ext.iter().collect();
        exts.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let ext_parts: Vec<String> = exts
            .iter()
            .take(5)
            .map(|(e, c)| format!("{}({})", e, c))
            .collect();
        out.push_str(&format!("\next: {}\n", ext_parts.join(" ")));
    }

    out
}

fn compact_path(path: &str) -> String {
    if path.len() <= 50 {
        return path.to_string();
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 3 {
        return path.to_string();
    }
    format!(
        "{}/.../{}/{}",
        parts[0],
        parts[parts.len() - 2],
        parts[parts.len() - 1]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_small_output_not_compressed() {
        let output = "src/main.rs\nsrc/lib.rs\n";
        let compressor = GlobCompressor;
        assert!(compressor.compress("{}", output).is_none());
    }

    #[test]
    fn test_large_output_compressed() {
        let paths: Vec<String> = (0..50)
            .map(|i| format!("src/components/file{}.tsx", i))
            .collect();
        let output = paths.join("\n");
        let compressor = GlobCompressor;
        let result = compressor.compress("{}", &output);
        assert!(result.is_some());
        let compressed = result.unwrap();
        assert!(compressed.contains("50F 1D:"));
        assert!(compressed.contains("src/components/"));
    }

    #[test]
    fn test_groups_by_directory() {
        let mut paths = Vec::new();
        for i in 0..15 {
            paths.push(format!("src/file{}.rs", i));
        }
        for i in 0..15 {
            paths.push(format!("tests/test{}.rs", i));
        }
        let output = paths.join("\n");
        let compressor = GlobCompressor;
        let result = compressor.compress("{}", &output).unwrap();
        assert!(result.contains("30F 2D:"));
        assert!(result.contains("src/"));
        assert!(result.contains("tests/"));
    }

    #[test]
    fn test_extension_summary() {
        let mut paths = Vec::new();
        for i in 0..20 {
            paths.push(format!("src/file{}.rs", i));
        }
        for i in 0..15 {
            paths.push(format!("src/file{}.ts", i));
        }
        let output = paths.join("\n");
        let compressor = GlobCompressor;
        let result = compressor.compress("{}", &output).unwrap();
        assert!(result.contains("ext:"));
        assert!(result.contains(".rs(20)"));
        assert!(result.contains(".ts(15)"));
    }

    #[test]
    fn test_truncates_many_results() {
        let paths: Vec<String> = (0..100).map(|i| format!("src/file{}.rs", i)).collect();
        let output = paths.join("\n");
        let compressor = GlobCompressor;
        let result = compressor.compress("{}", &output).unwrap();
        assert!(result.contains("100F"));
        assert!(result.contains("+50 more"));
    }

    #[test]
    fn test_empty_output() {
        let compressor = GlobCompressor;
        assert!(compressor.compress("{}", "").is_none());
    }

    #[test]
    fn test_compact_path_short() {
        assert_eq!(compact_path("src/main.rs"), "src/main.rs");
    }

    #[test]
    fn test_compact_path_long() {
        let long = "very/long/deeply/nested/path/to/some/directory/here";
        let result = compact_path(long);
        assert!(result.contains("..."));
        assert!(result.len() <= long.len());
    }
}
