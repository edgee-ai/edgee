//! Compressor for `ls` command output.
//!
//! Strips permissions, owner, group, date columns and noise directories,
//! producing a compact listing: dirs with trailing `/`, files with human-readable sizes.

use std::collections::HashMap;

use super::BashCompressor;

/// Directories that are noise for LLM context.
const NOISE_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "__pycache__",
    ".next",
    "dist",
    "build",
    ".cache",
    ".turbo",
    ".vercel",
    ".pytest_cache",
    ".mypy_cache",
    ".tox",
    ".venv",
    "venv",
    "coverage",
    ".nyc_output",
    ".DS_Store",
    "Thumbs.db",
    ".idea",
    ".vscode",
    ".vs",
    "*.egg-info",
    ".eggs",
];

pub struct LsCompressor;

impl BashCompressor for LsCompressor {
    fn compress(&self, command: &str, output: &str) -> Option<String> {
        // Only compress long-format output (ls -l / ls -la / ls -al …)
        if !is_long_format(command) {
            return None;
        }

        let show_all = has_flag(command, 'a');
        compact_ls(output, show_all)
    }
}

/// Check whether the command uses long-format output (`-l`).
fn is_long_format(command: &str) -> bool {
    for arg in command.split_whitespace().skip(1) {
        if arg == "--" {
            break;
        }
        if arg.starts_with("--") {
            continue;
        }
        if arg.starts_with('-') && arg.contains('l') {
            return true;
        }
    }
    false
}

/// Check whether a short flag character is present in the command.
fn has_flag(command: &str, flag: char) -> bool {
    for arg in command.split_whitespace().skip(1) {
        if arg == "--" {
            break;
        }
        if arg == "--all" && flag == 'a' {
            return true;
        }
        if arg.starts_with('-') && !arg.starts_with("--") && arg.contains(flag) {
            return true;
        }
    }
    false
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1}M", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{}B", bytes)
    }
}

/// Parse `ls -l` output into a compact format:
///   name/        (dirs)
///   name  size   (files)
///
/// Returns `None` if the output could not be parsed (e.g. unrecognised format).
fn compact_ls(raw: &str, show_all: bool) -> Option<String> {
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<(String, String)> = Vec::new();
    let mut by_ext: HashMap<String, usize> = HashMap::new();
    let mut parseable_lines = 0usize;

    for line in raw.lines() {
        if line.starts_with("total ") || line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 9 {
            continue;
        }

        parseable_lines += 1;

        // Filename is everything from column 9 onward (handles spaces)
        let name = parts[8..].join(" ");

        if name == "." || name == ".." {
            continue;
        }

        if !show_all && NOISE_DIRS.iter().any(|noise| name == *noise) {
            continue;
        }

        let is_dir = parts[0].starts_with('d');

        if is_dir {
            dirs.push(name);
        } else if parts[0].starts_with('-') || parts[0].starts_with('l') {
            let size: u64 = parts[4].parse().unwrap_or(0);
            let ext = if let Some(pos) = name.rfind('.') {
                name[pos..].to_string()
            } else {
                "no ext".to_string()
            };
            *by_ext.entry(ext).or_insert(0) += 1;
            files.push((name, human_size(size)));
        }
    }

    // If nothing was parseable and the input had content, the format is
    // unrecognised — return None so the raw output passes through unchanged.
    if dirs.is_empty() && files.is_empty() {
        if parseable_lines == 0 {
            return None;
        }
        return Some("(empty)\n".to_string());
    }

    let mut out = String::new();

    for d in &dirs {
        out.push_str(d);
        out.push_str("/\n");
    }

    for (name, size) in &files {
        out.push_str(name);
        out.push_str("  ");
        out.push_str(size);
        out.push('\n');
    }

    out.push('\n');
    let mut summary = format!("{} files, {} dirs", files.len(), dirs.len());
    if !by_ext.is_empty() {
        let mut ext_counts: Vec<_> = by_ext.iter().collect();
        ext_counts.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let ext_parts: Vec<String> = ext_counts
            .iter()
            .take(5)
            .map(|(ext, count)| format!("{} {}", count, ext))
            .collect();
        summary.push_str(" (");
        summary.push_str(&ext_parts.join(", "));
        if ext_counts.len() > 5 {
            summary.push_str(&format!(", +{} more", ext_counts.len() - 5));
        }
        summary.push(')');
    }
    out.push_str(&summary);
    out.push('\n');

    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compact_basic() {
        let input = "total 48\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 .\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 ..\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 Cargo.toml\n\
                     -rw-r--r--  1 user  staff  5678 Jan  1 12:00 README.md\n";
        let output = compact_ls(input, false).expect("should compress");
        assert!(output.contains("src/"));
        assert!(output.contains("Cargo.toml"));
        assert!(output.contains("README.md"));
        assert!(output.contains("1.2K"));
        assert!(output.contains("5.5K"));
        assert!(!output.contains("drwx"));
        assert!(!output.contains("staff"));
        assert!(!output.contains("total"));
    }

    #[test]
    fn test_compact_filters_noise() {
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 node_modules\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 .git\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 target\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  100 Jan  1 12:00 main.rs\n";
        let output = compact_ls(input, false).expect("should compress");
        assert!(!output.contains("node_modules"));
        assert!(!output.contains(".git"));
        assert!(!output.contains("target"));
        assert!(output.contains("src/"));
        assert!(output.contains("main.rs"));
    }

    #[test]
    fn test_compact_show_all() {
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 .git\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n";
        let output = compact_ls(input, true).expect("should compress");
        assert!(output.contains(".git/"));
        assert!(output.contains("src/"));
    }

    #[test]
    fn test_compact_empty() {
        // A truly empty directory (only "total 0") has no parseable lines → None (pass-through)
        let input = "total 0\n";
        assert!(compact_ls(input, false).is_none());
    }

    #[test]
    fn test_compact_summary() {
        let input = "total 48\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 main.rs\n\
                     -rw-r--r--  1 user  staff  5678 Jan  1 12:00 lib.rs\n\
                     -rw-r--r--  1 user  staff   100 Jan  1 12:00 Cargo.toml\n";
        let output = compact_ls(input, false).expect("should compress");
        assert!(output.contains("3 files, 1 dirs"));
        assert!(output.contains(".rs"));
        assert!(output.contains(".toml"));
    }

    #[test]
    fn test_compact_symlinks() {
        let input = "total 8\n\
                     lrwxr-xr-x  1 user  staff  10 Jan  1 12:00 link -> target\n";
        let output = compact_ls(input, false).expect("should compress");
        assert!(output.contains("link -> target"));
    }

    #[test]
    fn test_compact_filenames_with_spaces() {
        let input = "total 8\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 my file.txt\n";
        let output = compact_ls(input, false).expect("should compress");
        assert!(output.contains("my file.txt"));
    }

    #[test]
    fn test_human_size() {
        assert_eq!(human_size(0), "0B");
        assert_eq!(human_size(500), "500B");
        assert_eq!(human_size(1024), "1.0K");
        assert_eq!(human_size(1234), "1.2K");
        assert_eq!(human_size(1_048_576), "1.0M");
        assert_eq!(human_size(2_500_000), "2.4M");
    }

    #[test]
    fn test_is_long_format() {
        assert!(is_long_format("ls -l"));
        assert!(is_long_format("ls -la"));
        assert!(is_long_format("ls -al"));
        assert!(is_long_format("ls -la /tmp"));
        assert!(!is_long_format("ls"));
        assert!(!is_long_format("ls -a"));
        assert!(!is_long_format("ls /tmp"));
    }

    #[test]
    fn test_has_flag() {
        assert!(has_flag("ls -a", 'a'));
        assert!(has_flag("ls -la", 'a'));
        assert!(has_flag("ls --all", 'a'));
        assert!(!has_flag("ls -l", 'a'));
        assert!(!has_flag("ls", 'a'));
    }

    #[test]
    fn test_compressor_skips_non_long_format() {
        let compressor = LsCompressor;
        assert!(compressor.compress("ls", "file1\nfile2\n").is_none());
        assert!(compressor.compress("ls -a", "file1\nfile2\n").is_none());
    }

    #[test]
    fn test_compressor_compresses_long_format() {
        let compressor = LsCompressor;
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 main.rs\n";
        let result = compressor.compress("ls -la", input);
        assert!(result.is_some());
        let output = result.unwrap();
        assert!(output.contains("src/"));
        assert!(output.contains("main.rs"));
        assert!(!output.contains("drwx"));
    }

    #[test]
    fn test_compressor_passthrough_exa_format() {
        // exa/eza output has fewer columns than POSIX ls -l; the compressor
        // should return None (pass-through) rather than a misleading "(empty)".
        let compressor = LsCompressor;
        let input = ".rw-r--r--  35k clement  4 Mar 21:12 game-ui.js\n\
                     .rw-r--r-- 3,6k clement  2 Jan 17:52 landing-client.js\n\
                     drwxr-xr-x    - clement  4 Mar 21:12 pages\n";
        let result = compressor.compress("ls -l", input);
        assert!(result.is_none(), "exa format should pass through unchanged");
    }
}
