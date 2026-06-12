use super::BashCompressor;
use serde::Deserialize;
use std::collections::HashMap;

pub struct PnpmCompressor;

impl BashCompressor for PnpmCompressor {
    fn compress(&self, command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        let subcommand = parse_pnpm_subcommand(command);
        let filtered = match subcommand {
            "list" => filter_pnpm_list(output),
            "outdated" => filter_pnpm_outdated(output),
            "install" => filter_pnpm_install(output),
            _ => return None,
        };

        if filtered == output.trim() {
            return None;
        }

        Some(filtered)
    }
}

fn parse_pnpm_subcommand(command: &str) -> &str {
    let mut found_pnpm = false;
    for arg in command.split_whitespace() {
        if arg == "pnpm" || arg == "pnpx" {
            found_pnpm = true;
            continue;
        }
        if found_pnpm && !arg.starts_with('-') {
            return arg;
        }
    }
    ""
}

#[derive(Debug, Deserialize)]
struct PnpmListOutput {
    name: String,
    #[serde(flatten)]
    package: PackageJsonListItem,
}

#[derive(Debug, Deserialize)]
struct PackageJsonListItem {
    version: Option<String>,
    #[serde(rename = "dependencies", default)]
    dependencies: HashMap<String, PackageJsonListItem>,
    #[serde(rename = "devDependencies", default)]
    dev_dependencies: HashMap<String, PackageJsonListItem>,
}

#[derive(Debug, Deserialize)]
struct PnpmOutdatedOutput {
    #[serde(flatten)]
    packages: HashMap<String, PnpmOutdatedPackage>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct PnpmOutdatedPackage {
    current: String,
    latest: String,
    wanted: Option<String>,
    #[serde(rename = "dependencyType", default)]
    dependency_type: String,
}

#[derive(Debug)]
struct Dependency {
    name: String,
    current_version: String,
    latest_version: Option<String>,
    dev_dependency: bool,
}

fn filter_pnpm_list(output: &str) -> String {
    let json: Vec<PnpmListOutput> = match serde_json::from_str(output) {
        Ok(p) => p,
        Err(_) => {
            // Try text fallback
            return extract_pnpm_list_text(output);
        }
    };

    let mut dependencies = Vec::new();
    let mut total_count = 0;

    for pkg in &json {
        collect_dependencies(
            pkg.name.as_str(),
            &pkg.package,
            false,
            &mut dependencies,
            &mut total_count,
        );
    }

    format_dependency_listing(&dependencies, total_count)
}

fn collect_dependencies(
    name: &str,
    pkg: &PackageJsonListItem,
    is_dev: bool,
    deps: &mut Vec<Dependency>,
    count: &mut usize,
) {
    if let Some(version) = &pkg.version {
        deps.push(Dependency {
            name: name.to_string(),
            current_version: version.clone(),
            latest_version: None,
            dev_dependency: is_dev,
        });
        *count += 1;
    }

    for (dep_name, dep_pkg) in &pkg.dependencies {
        collect_dependencies(dep_name, dep_pkg, is_dev, deps, count);
    }

    for (dep_name, dep_pkg) in &pkg.dev_dependencies {
        collect_dependencies(dep_name, dep_pkg, true, deps, count);
    }
}

fn extract_pnpm_list_text(output: &str) -> String {
    let mut dependencies = Vec::new();
    let mut count = 0;
    let mut is_dev = false;

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed == "devDependencies:" {
            is_dev = true;
            continue;
        }
        if trimmed == "dependencies:" {
            is_dev = false;
            continue;
        }

        if line.contains('│')
            || line.contains('├')
            || line.contains('└')
            || line.contains("Legend:")
            || trimmed.is_empty()
        {
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if !parts.is_empty() {
            let pkg_str = parts[0];
            if let Some(at_pos) = pkg_str.rfind('@') {
                let name = &pkg_str[..at_pos];
                let version = &pkg_str[at_pos + 1..];
                if !name.is_empty() && !version.is_empty() {
                    dependencies.push(Dependency {
                        name: name.to_string(),
                        current_version: version.to_string(),
                        latest_version: None,
                        dev_dependency: is_dev,
                    });
                    count += 1;
                }
            }
        }
    }

    format_dependency_listing(&dependencies, count)
}

fn format_dependency_listing(deps: &[Dependency], total_count: usize) -> String {
    let prod: Vec<_> = deps.iter().filter(|d| !d.dev_dependency).collect();
    let dev: Vec<_> = deps.iter().filter(|d| d.dev_dependency).collect();

    let mut lines = vec![format!(
        "{} packages ({} prod / {} dev)",
        total_count,
        prod.len(),
        dev.len()
    )];

    const MAX_LISTING: usize = 10;

    if !prod.is_empty() {
        lines.push("[prod]".to_string());
        for dep in prod.iter().take(MAX_LISTING) {
            lines.push(format!("  {} {}", dep.name, dep.current_version));
        }
        if prod.len() > MAX_LISTING {
            lines.push(format!("  ... +{} more", prod.len() - MAX_LISTING));
        }
    }

    if !dev.is_empty() {
        lines.push("[dev]".to_string());
        for dep in dev.iter().take(MAX_LISTING) {
            lines.push(format!("  {} {}", dep.name, dep.current_version));
        }
        if dev.len() > MAX_LISTING {
            lines.push(format!("  ... +{} more", dev.len() - MAX_LISTING));
        }
    }

    lines.join("\n")
}

fn filter_pnpm_outdated(output: &str) -> String {
    let json: PnpmOutdatedOutput = match serde_json::from_str(output) {
        Ok(p) => p,
        Err(_) => {
            return extract_pnpm_outdated_text(output);
        }
    };

    let mut dependencies = Vec::new();
    let mut outdated_count = 0;

    for (name, pkg) in &json.packages {
        if pkg.current != pkg.latest {
            outdated_count += 1;
        }

        dependencies.push(Dependency {
            name: name.clone(),
            current_version: pkg.current.clone(),
            latest_version: Some(pkg.latest.clone()),
            dev_dependency: pkg.dependency_type == "devDependencies",
        });
    }

    if dependencies.is_empty() {
        return "All packages up-to-date".to_string();
    }

    let mut result = String::new();
    result.push_str(&format!("{} outdated packages\n", outdated_count));

    const MAX_LISTING: usize = 10;
    for (i, dep) in dependencies.iter().take(MAX_LISTING).enumerate() {
        let latest = dep.latest_version.as_deref().unwrap_or("unknown");
        result.push_str(&format!(
            "{}. {} ({} → {})\n",
            i + 1,
            dep.name,
            dep.current_version,
            latest
        ));
    }

    if dependencies.len() > MAX_LISTING {
        result.push_str(&format!(
            "\n... +{} more\n",
            dependencies.len() - MAX_LISTING
        ));
    }

    result.trim().to_string()
}

fn extract_pnpm_outdated_text(output: &str) -> String {
    let mut dependencies = Vec::new();
    let mut outdated_count = 0;

    for line in output.lines() {
        if line.contains('│')
            || line.contains('├')
            || line.contains('└')
            || line.contains('─')
            || line.starts_with("Legend:")
            || line.starts_with("Package")
            || line.trim().is_empty()
        {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            let name = parts[0];
            let current = parts[1];
            let latest = parts[3];

            if current != latest {
                outdated_count += 1;
            }

            dependencies.push(Dependency {
                name: name.to_string(),
                current_version: current.to_string(),
                latest_version: Some(latest.to_string()),
                dev_dependency: false,
            });
        }
    }

    if dependencies.is_empty() {
        return "All packages up-to-date".to_string();
    }

    let mut result = String::new();
    result.push_str(&format!("{} outdated packages\n", outdated_count));

    const MAX_LISTING: usize = 10;
    for (i, dep) in dependencies.iter().take(MAX_LISTING).enumerate() {
        let latest = dep.latest_version.as_deref().unwrap_or("unknown");
        result.push_str(&format!(
            "{}. {} ({} → {})\n",
            i + 1,
            dep.name,
            dep.current_version,
            latest
        ));
    }

    if dependencies.len() > MAX_LISTING {
        result.push_str(&format!(
            "\n... +{} more\n",
            dependencies.len() - MAX_LISTING
        ));
    }

    result.trim().to_string()
}

fn filter_pnpm_install(output: &str) -> String {
    let mut result = Vec::new();
    let mut saw_progress = false;

    for line in output.lines() {
        if line.contains("Progress") || line.contains('│') || line.contains('%') {
            saw_progress = true;
            continue;
        }

        if saw_progress && line.trim().is_empty() {
            continue;
        }

        if line.contains("ERR") || line.contains("error") || line.contains("ERROR") {
            result.push(line.to_string());
            continue;
        }

        if line.contains("packages in")
            || line.contains("dependencies")
            || line.starts_with('+')
            || line.starts_with('-')
        {
            result.push(line.trim().to_string());
        }
    }

    if result.is_empty() {
        "ok".to_string()
    } else {
        result.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pnpm_list_json() {
        let json = r#"[
            {
                "name": "my-project",
                "version": "1.0.0",
                "dependencies": {
                    "express": {
                        "version": "4.18.2"
                    }
                }
            }
        ]"#;

        let result = filter_pnpm_list(json);
        assert!(result.contains("packages"));
        assert!(result.contains("express"));
    }

    #[test]
    fn test_pnpm_outdated_json() {
        let json = r#"{
            "express": {
                "current": "4.18.2",
                "latest": "4.19.0",
                "wanted": "4.18.2",
                "dependencyType": "dependencies"
            }
        }"#;

        let result = filter_pnpm_outdated(json);
        assert!(result.contains("1 outdated"));
        assert!(result.contains("express"));
        assert!(result.contains("4.18.2 → 4.19.0"));
    }

    #[test]
    fn test_pnpm_outdated_empty() {
        let result = filter_pnpm_outdated("{}");
        assert!(result.contains("All packages up-to-date"));
    }

    #[test]
    fn test_pnpm_install_filter() {
        let output = r#"Progress: resolved 15, reused 15, downloaded 0, added 15, done

+ express@4.18.2
+ typescript@5.0.0

packages in 1s
"#;
        let result = filter_pnpm_install(output);
        assert!(result.contains("express@4.18.2"));
        assert!(!result.contains("Progress"));
    }

    #[test]
    fn test_pnpm_list_text_fallback() {
        let input = "dependencies:\nreact@18.0.0\ndevDependencies:\neslint@8.0.0\n";
        let result = extract_pnpm_list_text(input);
        assert!(result.contains("[prod]"));
        assert!(result.contains("[dev]"));
        assert!(result.contains("react"));
        assert!(result.contains("eslint"));
    }

    #[test]
    fn test_compressor_empty() {
        let c = PnpmCompressor;
        assert!(c.compress("pnpm list", "").is_none());
    }

    #[test]
    fn test_compressor_reduces_length() {
        let c = PnpmCompressor;
        let output = r#"[
            {
                "name": "my-project",
                "version": "1.0.0",
                "dependencies": {
                    "express": {
                        "version": "4.18.2"
                    }
                }
            }
        ]"#;
        let result = c.compress("pnpm list --json", output).unwrap();
        assert!(result.len() < output.len());
    }

    #[test]
    fn test_parse_subcommand() {
        assert_eq!(parse_pnpm_subcommand("pnpm list"), "list");
        assert_eq!(parse_pnpm_subcommand("pnpm outdated"), "outdated");
        assert_eq!(parse_pnpm_subcommand("pnpm install"), "install");
    }
}
