use super::BashCompressor;
use serde::Deserialize;

pub struct PipCompressor;

impl BashCompressor for PipCompressor {
    fn compress(&self, command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        // Detect subcommand from the command string
        let subcommand = parse_pip_subcommand(command);
        let filtered = match subcommand {
            "list" => {
                if command.contains("outdated") {
                    filter_pip_outdated(output)
                } else {
                    filter_pip_list(output)
                }
            }
            _ => return None,
        };

        if filtered == output.trim() {
            return None;
        }

        Some(filtered)
    }
}

fn parse_pip_subcommand(command: &str) -> &str {
    let mut found_pip = false;
    for arg in command.split_whitespace() {
        if arg == "pip" || arg == "pip3" {
            found_pip = true;
            continue;
        }
        if found_pip && !arg.starts_with('-') {
            return arg;
        }
    }
    ""
}

#[derive(Debug, Deserialize)]
struct Package {
    name: String,
    version: String,
    #[serde(default)]
    latest_version: Option<String>,
}

fn filter_pip_list(output: &str) -> String {
    let packages: Vec<Package> = match serde_json::from_str(output) {
        Ok(p) => p,
        Err(_) => {
            // Not JSON - return as-is for non-list output
            return output.trim().to_string();
        }
    };

    if packages.is_empty() {
        return "pip list: No packages installed".to_string();
    }

    let mut result = String::new();
    result.push_str(&format!("pip list: {} packages\n", packages.len()));

    let mut by_letter: std::collections::HashMap<char, Vec<&Package>> =
        std::collections::HashMap::new();

    for pkg in &packages {
        let first_char = pkg.name.chars().next().unwrap_or('?').to_ascii_lowercase();
        by_letter.entry(first_char).or_default().push(pkg);
    }

    let mut letters: Vec<_> = by_letter.keys().collect();
    letters.sort();

    const MAX_PER_LETTER: usize = 50;
    for letter in letters {
        let pkgs = by_letter.get(letter).unwrap();
        result.push_str(&format!("\n[{}]\n", letter.to_uppercase()));

        for pkg in pkgs.iter().take(MAX_PER_LETTER) {
            result.push_str(&format!("  {} ({}\n", pkg.name, pkg.version));
        }

        if pkgs.len() > MAX_PER_LETTER {
            result.push_str(&format!("  ... +{} more\n", pkgs.len() - MAX_PER_LETTER));
        }
    }

    result.trim().to_string()
}

fn filter_pip_outdated(output: &str) -> String {
    let packages: Vec<Package> = match serde_json::from_str(output) {
        Ok(p) => p,
        Err(_) => {
            return output.trim().to_string();
        }
    };

    if packages.is_empty() {
        return "pip outdated: All packages up to date".to_string();
    }

    let mut result = String::new();
    result.push_str(&format!("pip outdated: {} packages\n", packages.len()));

    const MAX_PIP_PACKAGES: usize = 10;
    for (i, pkg) in packages.iter().take(MAX_PIP_PACKAGES).enumerate() {
        let latest = pkg.latest_version.as_deref().unwrap_or("unknown");
        result.push_str(&format!(
            "{}. {} ({} → {})\n",
            i + 1,
            pkg.name,
            pkg.version,
            latest
        ));
    }

    if packages.len() > MAX_PIP_PACKAGES {
        result.push_str(&format!(
            "\n... +{} more packages\n",
            packages.len() - MAX_PIP_PACKAGES
        ));
    }

    result.push_str("\n[hint] Run `pip install --upgrade <package>` to update\n");

    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_pip_list() {
        let output = r#"[
  {"name": "requests", "version": "2.31.0"},
  {"name": "pytest", "version": "7.4.0"},
  {"name": "rich", "version": "13.0.0"}
]"#;

        let result = filter_pip_list(output);
        assert!(result.contains("3 packages"));
        assert!(result.contains("requests"));
        assert!(result.contains("2.31.0"));
        assert!(result.contains("pytest"));
    }

    #[test]
    fn test_filter_pip_list_empty() {
        let output = "[]";
        let result = filter_pip_list(output);
        assert!(result.contains("No packages installed"));
    }

    #[test]
    fn test_filter_pip_outdated_none() {
        let output = "[]";
        let result = filter_pip_outdated(output);
        assert!(result.contains("All packages up to date"));
    }

    #[test]
    fn test_filter_pip_outdated_some() {
        let output = r#"[
  {"name": "requests", "version": "2.31.0", "latest_version": "2.32.0"},
  {"name": "pytest", "version": "7.4.0", "latest_version": "8.0.0"}
]"#;

        let result = filter_pip_outdated(output);
        assert!(result.contains("2 packages"));
        assert!(result.contains("requests"));
        assert!(result.contains("2.31.0 → 2.32.0"));
        assert!(result.contains("pytest"));
        assert!(result.contains("7.4.0 → 8.0.0"));
    }

    #[test]
    fn test_compressor_empty() {
        let c = PipCompressor;
        assert!(c.compress("pip list", "").is_none());
    }

    #[test]
    fn test_compressor_reduces_length() {
        let c = PipCompressor;
        let output = r#"[
  {"name": "requests", "version": "2.31.0"},
  {"name": "pytest", "version": "7.4.0"},
  {"name": "rich", "version": "13.0.0"}
]"#;
        let result = c.compress("pip list --format=json", output).unwrap();
        assert!(result.len() < output.len());
    }

    #[test]
    fn test_parse_subcommand() {
        assert_eq!(parse_pip_subcommand("pip list"), "list");
        assert_eq!(parse_pip_subcommand("pip list --outdated"), "list");
        assert_eq!(parse_pip_subcommand("pip3 list"), "list");
    }
}
