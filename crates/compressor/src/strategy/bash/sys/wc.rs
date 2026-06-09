use super::BashCompressor;

pub struct WcCompressor;

impl BashCompressor for WcCompressor {
    fn compress(&self, command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        let mode = detect_mode(command);
        let filtered = filter_wc_output(output, &mode);

        if filtered == output {
            return None;
        }

        Some(filtered)
    }
}

#[derive(Debug, PartialEq)]
enum WcMode {
    /// Default: lines, words, bytes (3 columns)
    Full,
    /// Lines only (-l)
    Lines,
    /// Words only (-w)
    Words,
    /// Bytes only (-c)
    Bytes,
    /// Chars only (-m)
    Chars,
    /// Multiple flags combined — keep compact format
    Mixed,
}

fn detect_mode(command: &str) -> WcMode {
    let mut has_l = false;
    let mut has_w = false;
    let mut has_c = false;
    let mut has_m = false;
    let mut flag_count = 0;

    for arg in command.split_whitespace().skip(1) {
        if arg.starts_with('-') && !arg.starts_with("--") {
            for ch in arg.chars().skip(1) {
                match ch {
                    'l' => {
                        has_l = true;
                        flag_count += 1;
                    }
                    'w' => {
                        has_w = true;
                        flag_count += 1;
                    }
                    'c' => {
                        has_c = true;
                        flag_count += 1;
                    }
                    'm' => {
                        has_m = true;
                        flag_count += 1;
                    }
                    _ => {}
                }
            }
        }
    }

    if flag_count == 0 {
        return WcMode::Full;
    }
    if flag_count > 1 {
        return WcMode::Mixed;
    }

    if has_l {
        WcMode::Lines
    } else if has_w {
        WcMode::Words
    } else if has_c {
        WcMode::Bytes
    } else if has_m {
        WcMode::Chars
    } else {
        WcMode::Full
    }
}

fn filter_wc_output(raw: &str, mode: &WcMode) -> String {
    let lines: Vec<&str> = raw.trim().lines().collect();

    if lines.is_empty() {
        return String::new();
    }

    // Single file (one output line, no "total")
    if lines.len() == 1 {
        return format_single_line(lines[0], mode);
    }

    // Multiple files — compact table
    format_multi_line(&lines, mode)
}

fn format_single_line(line: &str, mode: &WcMode) -> String {
    let parts: Vec<&str> = line.split_whitespace().collect();

    match mode {
        WcMode::Lines | WcMode::Words | WcMode::Bytes | WcMode::Chars => {
            parts.first().map(|s| s.to_string()).unwrap_or_default()
        }
        WcMode::Full => {
            if parts.len() >= 3 {
                format!("{}L {}W {}B", parts[0], parts[1], parts[2])
            } else {
                line.trim().to_string()
            }
        }
        WcMode::Mixed => {
            if parts.len() >= 2 {
                let last_is_path = parts.last().is_some_and(|p| p.parse::<u64>().is_err());
                if last_is_path {
                    parts[..parts.len() - 1].join(" ")
                } else {
                    parts.join(" ")
                }
            } else {
                line.trim().to_string()
            }
        }
    }
}

fn format_multi_line(lines: &[&str], mode: &WcMode) -> String {
    let mut result = Vec::new();

    let paths: Vec<&str> = lines
        .iter()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            parts.last().copied()
        })
        .filter(|p| *p != "total")
        .collect();

    let common_prefix = find_common_prefix(&paths);

    for line in lines {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        let is_total = parts.last().is_some_and(|p| *p == "total");

        match mode {
            WcMode::Lines | WcMode::Words | WcMode::Bytes | WcMode::Chars => {
                if is_total {
                    result.push(format!("Σ {}", parts.first().unwrap_or(&"0")));
                } else {
                    let name = strip_prefix(parts.last().unwrap_or(&""), &common_prefix);
                    result.push(format!("{} {}", parts.first().unwrap_or(&"0"), name));
                }
            }
            WcMode::Full => {
                if is_total {
                    result.push(format!(
                        "Σ {}L {}W {}B",
                        parts.first().unwrap_or(&"0"),
                        parts.get(1).unwrap_or(&"0"),
                        parts.get(2).unwrap_or(&"0"),
                    ));
                } else if parts.len() >= 4 {
                    let name = strip_prefix(parts[3], &common_prefix);
                    result.push(format!(
                        "{}L {}W {}B {}",
                        parts[0], parts[1], parts[2], name
                    ));
                } else {
                    result.push(line.trim().to_string());
                }
            }
            WcMode::Mixed => {
                if is_total {
                    let nums: Vec<&str> = parts[..parts.len() - 1].to_vec();
                    result.push(format!("Σ {}", nums.join(" ")));
                } else if parts.len() >= 2 {
                    let last_is_path = parts.last().is_some_and(|p| p.parse::<u64>().is_err());
                    if last_is_path {
                        let name = strip_prefix(parts.last().unwrap_or(&""), &common_prefix);
                        let nums: Vec<&str> = parts[..parts.len() - 1].to_vec();
                        result.push(format!("{} {}", nums.join(" "), name));
                    } else {
                        result.push(parts.join(" "));
                    }
                } else {
                    result.push(line.trim().to_string());
                }
            }
        }
    }

    result.join("\n")
}

fn find_common_prefix(paths: &[&str]) -> String {
    if paths.len() <= 1 {
        return String::new();
    }

    let first = paths[0];
    let prefix = if let Some(pos) = first.rfind('/') {
        &first[..=pos]
    } else {
        return String::new();
    };

    if paths.iter().all(|p| p.starts_with(prefix)) {
        return prefix.to_string();
    }

    let mut candidate = prefix.to_string();
    while !candidate.is_empty() {
        if paths.iter().all(|p| p.starts_with(&candidate)) {
            return candidate;
        }
        if let Some(pos) = candidate[..candidate.len() - 1].rfind('/') {
            candidate.truncate(pos + 1);
        } else {
            return String::new();
        }
    }
    String::new()
}

fn strip_prefix<'a>(path: &'a str, prefix: &str) -> &'a str {
    if prefix.is_empty() {
        return path;
    }
    path.strip_prefix(prefix).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_file_full() {
        let raw = "      30      96     978 scripts/find_duplicate_attrs.py\n";
        let result = filter_wc_output(raw, &WcMode::Full);
        assert_eq!(result, "30L 96W 978B");
    }

    #[test]
    fn test_single_file_lines_only() {
        let raw = "      30 scripts/find_duplicate_attrs.py\n";
        let result = filter_wc_output(raw, &WcMode::Lines);
        assert_eq!(result, "30");
    }

    #[test]
    fn test_single_file_words_only() {
        let raw = "      96 scripts/find_duplicate_attrs.py\n";
        let result = filter_wc_output(raw, &WcMode::Words);
        assert_eq!(result, "96");
    }

    #[test]
    fn test_stdin_full() {
        let raw = "      30      96     978\n";
        let result = filter_wc_output(raw, &WcMode::Full);
        assert_eq!(result, "30L 96W 978B");
    }

    #[test]
    fn test_stdin_lines() {
        let raw = "      30\n";
        let result = filter_wc_output(raw, &WcMode::Lines);
        assert_eq!(result, "30");
    }

    #[test]
    fn test_multi_file_lines() {
        let raw = "      30 src/main.rs\n      50 src/lib.rs\n      80 total\n";
        let result = filter_wc_output(raw, &WcMode::Lines);
        assert_eq!(result, "30 main.rs\n50 lib.rs\nΣ 80");
    }

    #[test]
    fn test_multi_file_full() {
        let raw = "      30      96     978 src/main.rs\n      50     120    1500 src/lib.rs\n      80     216    2478 total\n";
        let result = filter_wc_output(raw, &WcMode::Full);
        assert_eq!(
            result,
            "30L 96W 978B main.rs\n50L 120W 1500B lib.rs\nΣ 80L 216W 2478B"
        );
    }

    #[test]
    fn test_detect_mode_full() {
        assert_eq!(detect_mode("wc file.py"), WcMode::Full);
    }

    #[test]
    fn test_detect_mode_lines() {
        assert_eq!(detect_mode("wc -l file.py"), WcMode::Lines);
    }

    #[test]
    fn test_detect_mode_mixed() {
        assert_eq!(detect_mode("wc -lw file.py"), WcMode::Mixed);
    }

    #[test]
    fn test_detect_mode_separate_flags() {
        assert_eq!(detect_mode("wc -l -w file.py"), WcMode::Mixed);
    }

    #[test]
    fn test_common_prefix() {
        let paths = vec!["src/main.rs", "src/lib.rs", "src/utils.rs"];
        assert_eq!(find_common_prefix(&paths), "src/");
    }

    #[test]
    fn test_no_common_prefix() {
        let paths = vec!["main.rs", "lib.rs"];
        assert_eq!(find_common_prefix(&paths), "");
    }

    #[test]
    fn test_deep_common_prefix() {
        let paths = vec!["src/cmd/wc.rs", "src/cmd/ls.rs"];
        assert_eq!(find_common_prefix(&paths), "src/cmd/");
    }

    #[test]
    fn test_empty() {
        let raw = "";
        let result = filter_wc_output(raw, &WcMode::Full);
        assert_eq!(result, "");
    }

    #[test]
    fn test_compressor_empty() {
        let c = WcCompressor;
        assert!(c.compress("wc -l file.py", "").is_none());
    }

    #[test]
    fn test_compressor_reduces_length() {
        let c = WcCompressor;
        let raw = "      30      96     978 src/main.rs\n      50     120    1500 src/lib.rs\n      80     216    2478 total\n";
        let result = c.compress("wc src/main.rs src/lib.rs", raw).unwrap();
        assert!(result.len() < raw.len());
    }
}
