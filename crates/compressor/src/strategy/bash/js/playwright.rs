use super::BashCompressor;
use regex::Regex;
use serde::Deserialize;
use std::sync::OnceLock;

pub struct PlaywrightCompressor;

impl BashCompressor for PlaywrightCompressor {
    fn compress(&self, _command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        // Try JSON parsing first
        if let Some(filtered) = filter_playwright_json(output)
            && filtered != output.trim()
        {
            return Some(filtered);
        }

        // Fallback to regex extraction
        if let Some(filtered) = filter_playwright_regex(output)
            && filtered != output.trim()
        {
            return Some(filtered);
        }

        None
    }
}

#[derive(Debug, Deserialize)]
struct PlaywrightJsonOutput {
    stats: PlaywrightStats,
    #[serde(default)]
    suites: Vec<PlaywrightSuite>,
}

#[derive(Debug, Deserialize)]
struct PlaywrightStats {
    expected: usize,
    unexpected: usize,
    skipped: usize,
    #[serde(default)]
    duration: f64,
}

#[derive(Debug, Deserialize)]
struct PlaywrightSuite {
    title: String,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    specs: Vec<PlaywrightSpec>,
    #[serde(default)]
    suites: Vec<PlaywrightSuite>,
}

#[derive(Debug, Deserialize)]
struct PlaywrightSpec {
    title: String,
    ok: bool,
    #[serde(default)]
    tests: Vec<PlaywrightExecution>,
}

#[derive(Debug, Deserialize)]
struct PlaywrightExecution {
    status: String,
    #[serde(default)]
    results: Vec<PlaywrightAttempt>,
}

#[derive(Debug, Deserialize)]
struct PlaywrightAttempt {
    status: String,
    #[serde(default)]
    errors: Vec<PlaywrightError>,
}

#[derive(Debug, Deserialize)]
struct PlaywrightError {
    #[serde(default)]
    message: String,
}

fn filter_playwright_json(output: &str) -> Option<String> {
    let json: PlaywrightJsonOutput = serde_json::from_str(output).ok()?;

    let mut failures = Vec::new();
    let mut total = 0;
    collect_test_results(&json.suites, &mut total, &mut failures);

    let mut result = String::new();
    result.push_str(&format!(
        "Playwright: {} passed, {} failed, {} skipped",
        json.stats.expected, json.stats.unexpected, json.stats.skipped
    ));

    if json.stats.duration > 0.0 {
        result.push_str(&format!(" | {:.0}ms", json.stats.duration));
    }
    result.push('\n');

    if !failures.is_empty() {
        result.push_str("\nFailures:\n");
        const MAX_FAIL: usize = 10;
        for (i, failure) in failures.iter().take(MAX_FAIL).enumerate() {
            result.push_str(&format!("{}. {}\n", i + 1, failure.test_name));
            if !failure.error_message.is_empty() {
                let short = if failure.error_message.len() > 120 {
                    format!("{}...", &failure.error_message[..120])
                } else {
                    failure.error_message.clone()
                };
                result.push_str(&format!("   {}\n", short));
            }
        }
        if failures.len() > MAX_FAIL {
            result.push_str(&format!(
                "... +{} more failures\n",
                failures.len() - MAX_FAIL
            ));
        }
    }

    Some(result.trim().to_string())
}

fn collect_test_results(
    suites: &[PlaywrightSuite],
    total: &mut usize,
    failures: &mut Vec<TestFailure>,
) {
    for suite in suites {
        let file_path = suite.file.as_deref().unwrap_or(&suite.title);

        for spec in &suite.specs {
            *total += 1;

            if !spec.ok {
                let error_msg = spec
                    .tests
                    .iter()
                    .find(|t| t.status == "unexpected")
                    .and_then(|t| {
                        t.results
                            .iter()
                            .find(|r| r.status == "failed" || r.status == "timedOut")
                    })
                    .and_then(|r| r.errors.first())
                    .map(|e| e.message.clone())
                    .unwrap_or_else(|| "Test failed".to_string());

                failures.push(TestFailure {
                    test_name: spec.title.clone(),
                    file_path: file_path.to_string(),
                    error_message: error_msg,
                });
            }
        }

        collect_test_results(&suite.suites, total, failures);
    }
}

#[derive(Debug)]
#[allow(dead_code)]
struct TestFailure {
    test_name: String,
    file_path: String,
    error_message: String,
}

fn filter_playwright_regex(output: &str) -> Option<String> {
    static SUMMARY_RE: OnceLock<Regex> = OnceLock::new();
    static DURATION_RE: OnceLock<Regex> = OnceLock::new();

    let summary_re =
        SUMMARY_RE.get_or_init(|| Regex::new(r"(\d+)\s+(passed|failed|flaky|skipped)").unwrap());
    let duration_re =
        DURATION_RE.get_or_init(|| Regex::new(r"\((\d+(?:\.\d+)?)(ms|s|m)\)").unwrap());

    let clean = strip_ansi(output);

    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;

    for caps in summary_re.captures_iter(&clean) {
        let count: usize = caps[1].parse().unwrap_or(0);
        match &caps[2] {
            "passed" => passed = count,
            "failed" => failed = count,
            "skipped" => skipped = count,
            _ => {}
        }
    }

    let duration_ms = duration_re.captures(&clean).and_then(|caps| {
        let value: f64 = caps[1].parse().ok()?;
        let unit = &caps[2];
        Some(match unit {
            "ms" => value as u64,
            "s" => (value * 1000.0) as u64,
            "m" => (value * 60000.0) as u64,
            _ => value as u64,
        })
    });

    let total = passed + failed + skipped;
    if total == 0 {
        return None;
    }

    let mut result = format!(
        "Playwright: {} passed, {} failed, {} skipped",
        passed, failed, skipped
    );
    if let Some(ms) = duration_ms {
        result.push_str(&format!(" | {}ms", ms));
    }

    Some(result)
}

fn strip_ansi(input: &str) -> String {
    let re = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    re.replace_all(input, "").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_parsing() {
        let json = r#"{
            "stats": {
                "expected": 4,
                "unexpected": 1,
                "skipped": 0,
                "duration": 3519.7
            },
            "suites": [
                {
                    "title": "auth",
                    "specs": [
                        {
                            "title": "should login",
                            "ok": true,
                            "tests": [
                                {
                                    "status": "expected",
                                    "results": [{"status": "passed", "errors": []}]
                                }
                            ]
                        },
                        {
                            "title": "should fail on bad password",
                            "ok": false,
                            "tests": [
                                {
                                    "status": "unexpected",
                                    "results": [
                                        {
                                            "status": "failed",
                                            "errors": [{"message": "Expected error"}]
                                        }
                                    ]
                                }
                            ]
                        }
                    ],
                    "suites": []
                }
            ]
        }"#;
        let result = filter_playwright_json(json).unwrap();
        assert!(result.contains("4 passed, 1 failed"));
        assert!(result.contains("should fail on bad password"));
        assert!(result.contains("Expected error"));
    }

    #[test]
    fn test_regex_fallback() {
        let text = "3 passed (7.3s)\n";
        let result = filter_playwright_regex(text).unwrap();
        assert!(result.contains("3 passed"));
        assert!(result.contains("7300ms"));
    }

    #[test]
    fn test_no_match_returns_none() {
        let invalid = "random output";
        assert!(filter_playwright_regex(invalid).is_none());
    }

    #[test]
    fn test_compressor_empty() {
        let c = PlaywrightCompressor;
        assert!(c.compress("npx playwright test", "").is_none());
    }

    #[test]
    fn test_compressor_json_reduces_length() {
        let c = PlaywrightCompressor;
        let json = r#"{
            "stats": {"expected": 2, "unexpected": 0, "skipped": 0, "duration": 1000.0},
            "suites": [{"title": "suite", "specs": [], "suites": []}]
        }"#;
        let result = c.compress("npx playwright test", json).unwrap();
        assert!(result.len() < json.len());
    }
}
