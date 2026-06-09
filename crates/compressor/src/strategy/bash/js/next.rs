use super::BashCompressor;
use regex::Regex;
use std::sync::OnceLock;

pub struct NextCompressor;

impl BashCompressor for NextCompressor {
    fn compress(&self, _command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        let filtered = filter_next_build(output);
        if filtered == output.trim() {
            return None;
        }

        Some(filtered)
    }
}

fn filter_next_build(output: &str) -> String {
    static ROUTE_PATTERN: OnceLock<Regex> = OnceLock::new();
    static BUNDLE_PATTERN: OnceLock<Regex> = OnceLock::new();
    static TIME_RE: OnceLock<Regex> = OnceLock::new();

    let _route_pattern = ROUTE_PATTERN
        .get_or_init(|| Regex::new(r"^[○●◐λ✓]\s+(/[^\s]*)\s+(\d+(?:\.\d+)?)\s*(kB|B)").unwrap());
    let bundle_pattern = BUNDLE_PATTERN.get_or_init(|| {
        Regex::new(r"^[○●◐λ✓]\s+([\w/\-\.]+)\s+(\d+(?:\.\d+)?)\s*(kB|B)\s+(\d+(?:\.\d+)?)\s*(kB|B)")
            .unwrap()
    });
    let time_re = TIME_RE.get_or_init(|| Regex::new(r"(\d+(?:\.\d+)?)\s*(s|ms)").unwrap());

    let mut routes_static = 0;
    let mut routes_dynamic = 0;
    let mut routes_total = 0;
    let mut bundles: Vec<(String, f64, Option<f64>)> = Vec::new();
    let mut warnings = 0;
    let mut errors = 0;
    let mut build_time = String::new();

    let clean_output = strip_ansi(output);

    for line in clean_output.lines() {
        if line.starts_with('○') {
            routes_static += 1;
            routes_total += 1;
        } else if line.starts_with('●') || line.starts_with('◐') {
            routes_dynamic += 1;
            routes_total += 1;
        } else if line.starts_with('λ') {
            routes_total += 1;
        }

        if let Some(caps) = bundle_pattern.captures(line) {
            let route = caps[1].to_string();
            let size: f64 = caps[2].parse().unwrap_or(0.0);
            let total: f64 = caps[4].parse().unwrap_or(0.0);

            let pct_change = if total > 0.0 {
                Some(((total - size) / size) * 100.0)
            } else {
                None
            };

            bundles.push((route, total, pct_change));
        }

        if line.to_lowercase().contains("warning") {
            warnings += 1;
        }
        if line.to_lowercase().contains("error") && !line.contains("0 error") {
            errors += 1;
        }

        if (line.contains("Compiled") || line.contains("in"))
            && let Some(caps) = time_re.captures(line)
        {
            build_time = format!("{}{}", &caps[1], &caps[2]);
        }
    }

    let already_built = clean_output.contains("already optimized")
        || clean_output.contains("Cache")
        || (routes_total == 0 && clean_output.contains("Ready"));

    let mut result = String::new();
    result.push_str("Next.js Build\n");

    if already_built && routes_total == 0 {
        result.push_str("Already built (using cache)\n\n");
    } else if routes_total > 0 {
        result.push_str(&format!(
            "{} routes ({} static, {} dynamic)\n\n",
            routes_total, routes_static, routes_dynamic
        ));
    }

    if !bundles.is_empty() {
        result.push_str("Bundles:\n");
        bundles.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        const MAX_BUNDLES: usize = 10;
        for (route, size, pct_change) in bundles.iter().take(MAX_BUNDLES) {
            let warning_marker = if let Some(pct) = pct_change {
                if *pct > 10.0 {
                    format!(" [warn] (+{:.0}%)", pct)
                } else {
                    String::new()
                }
            } else {
                String::new()
            };
            let truncated = if route.len() > 30 {
                &route[..30]
            } else {
                route
            };
            result.push_str(&format!(
                "  {:<30} {:>6.0} kB{}\n",
                truncated, size, warning_marker
            ));
        }

        if bundles.len() > MAX_BUNDLES {
            result.push_str(&format!(
                "\n  ... +{} more routes\n",
                bundles.len() - MAX_BUNDLES
            ));
        }

        result.push('\n');
    }

    if !build_time.is_empty() {
        result.push_str(&format!("Time: {} | ", build_time));
    }

    result.push_str(&format!("Errors: {} | Warnings: {}\n", errors, warnings));

    result.trim().to_string()
}

fn strip_ansi(input: &str) -> String {
    let re = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    re.replace_all(input, "").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_next_build() {
        let output = r#"
   ▲ Next.js 15.2.0

   Creating an optimized production build ...
✓ Compiled successfully
✓ Linting and checking validity of types
✓ Collecting page data
○ /                            1.2 kB        132 kB
● /dashboard                   2.5 kB        156 kB
○ /api/auth                    0.5 kB         89 kB

Route (app)                    Size     First Load JS
┌ ○ /                          1.2 kB        132 kB
├ ● /dashboard                 2.5 kB        156 kB
└ ○ /api/auth                  0.5 kB         89 kB

○  (Static)  prerendered as static content
●  (SSG)     prerendered as static HTML
λ  (Server)  server-side renders at runtime

✓ Built in 34.2s
"#;
        let result = filter_next_build(output);
        assert!(result.contains("Next.js Build"));
        assert!(result.contains("routes"));
        assert!(!result.contains("Creating an optimized"));
    }

    #[test]
    fn test_filter_empty() {
        assert_eq!(
            filter_next_build(""),
            "Next.js Build\nErrors: 0 | Warnings: 0"
        );
    }

    #[test]
    fn test_compressor_empty() {
        let c = NextCompressor;
        assert!(c.compress("next build", "").is_none());
    }

    #[test]
    fn test_compressor_reduces_length() {
        let c = NextCompressor;
        let output = r#"
   ▲ Next.js 15.2.0
   Creating an optimized production build ...
✓ Compiled successfully
✓ Linting and checking validity of types
○ /                            1.2 kB        132 kB
✓ Built in 34.2s
"#;
        let result = c.compress("next build", output).unwrap();
        assert!(result.len() < output.len());
    }
}
