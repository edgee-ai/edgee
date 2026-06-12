use super::BashCompressor;
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

pub struct GlabCompressor;

impl BashCompressor for GlabCompressor {
    fn compress(&self, command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        // Try JSON parsing first (glab with -F json)
        if let Ok(json) = serde_json::from_str::<Value>(output) {
            let filtered = format_glab_json(command, &json);
            if filtered != output.trim() {
                return Some(filtered);
            }
        }

        // Plain text fallback - strip ANSI and collapse blank lines
        let filtered = strip_ansi(output);
        let collapsed = collapse_blank_lines(&filtered);
        if collapsed != output.trim() {
            return Some(collapsed);
        }

        None
    }
}

fn parse_glab_subcommand(command: &str) -> (&str, &str) {
    let parts: Vec<&str> = command.split_whitespace().collect();
    let mut found_glab = false;
    let mut main = "";
    let mut sub = "";

    for arg in parts.iter() {
        if *arg == "glab" {
            found_glab = true;
            continue;
        }
        if found_glab && !arg.starts_with('-') {
            if main.is_empty() {
                main = arg;
            } else if sub.is_empty() {
                sub = arg;
                break;
            }
        }
    }

    (main, sub)
}

fn format_glab_json(command: &str, json: &Value) -> String {
    let (main, sub) = parse_glab_subcommand(command);

    match main {
        "mr" => match sub {
            "list" => format_mr_list(json),
            "view" => format_mr_view(json),
            _ => json.to_string(),
        },
        "issue" => match sub {
            "list" => format_issue_list(json),
            "view" => format_issue_view(json),
            _ => json.to_string(),
        },
        "ci" | "pipeline" => match sub {
            "list" => format_ci_list(json),
            "status" => format_ci_status(json),
            _ => json.to_string(),
        },
        _ => json.to_string(),
    }
}

fn format_mr_list(json: &Value) -> String {
    let mrs = match json.as_array() {
        Some(arr) => arr,
        None => return String::new(),
    };

    if mrs.is_empty() {
        return "No Merge Requests".to_string();
    }

    let mut result = String::from("Merge Requests\n");
    const MAX_LIST: usize = 10;

    for mr in mrs.iter().take(MAX_LIST) {
        let iid = mr["iid"].as_i64().unwrap_or(0);
        let title = mr["title"].as_str().unwrap_or("???");
        let state = mr["state"].as_str().unwrap_or("???");
        let author = mr["author"]["username"].as_str().unwrap_or("???");
        let icon = state_icon(state);
        result.push_str(&format!("  {} !{} {} ({}\n", icon, iid, title, author));
    }

    if mrs.len() > MAX_LIST {
        result.push_str(&format!("  ... +{} more\n", mrs.len() - MAX_LIST));
    }

    result.trim().to_string()
}

fn format_mr_view(json: &Value) -> String {
    let iid = json["iid"].as_i64().unwrap_or(0);
    let title = json["title"].as_str().unwrap_or("???");
    let state = json["state"].as_str().unwrap_or("???");
    let author = json["author"]["username"].as_str().unwrap_or("???");
    let web_url = json["web_url"].as_str().unwrap_or("");
    let merge_status = json["merge_status"].as_str().unwrap_or("unknown");
    let source_branch = json["source_branch"].as_str().unwrap_or("???");
    let target_branch = json["target_branch"].as_str().unwrap_or("???");

    let icon = state_icon(state);
    let mergeable = match merge_status {
        "can_be_merged" => "[ok]",
        "cannot_be_merged" => "[conflict]",
        _ => "[?]",
    };

    let mut result = String::new();
    result.push_str(&format!("{} MR !{}: {}\n", icon, iid, title));
    result.push_str(&format!("  Author: {}\n", author));
    result.push_str(&format!("  State: {} | {}\n", state, mergeable));
    result.push_str(&format!(
        "  Branch: {} -> {}\n",
        source_branch, target_branch
    ));

    if let Some(labels) = json["labels"].as_array() {
        let joined: Vec<&str> = labels.iter().filter_map(|v| v.as_str()).collect();
        if !joined.is_empty() {
            result.push_str(&format!("  Labels: {}\n", joined.join(", ")));
        }
    }

    if let Some(pipeline) = json.get("head_pipeline").filter(|p| !p.is_null()) {
        let status = pipeline["status"].as_str().unwrap_or("unknown");
        result.push_str(&format!("  Pipeline: {}\n", status));
    }

    if !web_url.is_empty() {
        result.push_str(&format!("  URL: {}\n", web_url));
    }

    result.trim().to_string()
}

fn format_issue_list(json: &Value) -> String {
    let issues = match json.as_array() {
        Some(arr) => arr,
        None => return String::new(),
    };

    if issues.is_empty() {
        return "No Issues".to_string();
    }

    let mut result = String::from("Issues\n");
    const MAX_LIST: usize = 10;

    for issue in issues.iter().take(MAX_LIST) {
        let iid = issue["iid"].as_i64().unwrap_or(0);
        let title = issue["title"].as_str().unwrap_or("???");
        let state = issue["state"].as_str().unwrap_or("???");
        let icon = if state == "opened" {
            "[open]"
        } else {
            "[closed]"
        };
        result.push_str(&format!("  {} #{} {}\n", icon, iid, title));
    }

    if issues.len() > MAX_LIST {
        result.push_str(&format!("  ... +{} more\n", issues.len() - MAX_LIST));
    }

    result.trim().to_string()
}

fn format_issue_view(json: &Value) -> String {
    let iid = json["iid"].as_i64().unwrap_or(0);
    let title = json["title"].as_str().unwrap_or("???");
    let state = json["state"].as_str().unwrap_or("???");
    let author = json["author"]["username"].as_str().unwrap_or("???");
    let web_url = json["web_url"].as_str().unwrap_or("");

    let icon = if state == "opened" {
        "[open]"
    } else {
        "[closed]"
    };

    let mut result = String::new();
    result.push_str(&format!("{} Issue #{}: {}\n", icon, iid, title));
    result.push_str(&format!("  Author: {}\n", author));
    result.push_str(&format!("  Status: {}\n", state));

    if !web_url.is_empty() {
        result.push_str(&format!("  URL: {}\n", web_url));
    }

    result.trim().to_string()
}

fn format_ci_list(json: &Value) -> String {
    let pipelines = match json.as_array() {
        Some(arr) => arr,
        None => return String::new(),
    };

    if pipelines.is_empty() {
        return "No Pipelines".to_string();
    }

    let mut result = String::from("Pipelines\n");
    const MAX_LIST: usize = 10;

    for pipeline in pipelines.iter().take(MAX_LIST) {
        let id = pipeline["id"].as_i64().unwrap_or(0);
        let status = pipeline["status"].as_str().unwrap_or("???");
        let ref_name = pipeline["ref"].as_str().unwrap_or("???");
        let icon = pipeline_icon(status);
        result.push_str(&format!("  {} #{} {} ({}\n", icon, id, status, ref_name));
    }

    if pipelines.len() > MAX_LIST {
        result.push_str(&format!("  ... +{} more\n", pipelines.len() - MAX_LIST));
    }

    result.trim().to_string()
}

fn format_ci_status(json: &Value) -> String {
    let status = json["status"].as_str().unwrap_or("unknown");
    let icon = pipeline_icon(status);

    let mut result = format!("Pipeline: {} {}\n", icon, status);

    if let Some(stages) = json["stages"].as_array() {
        for stage in stages {
            let name = stage["name"].as_str().unwrap_or("???");
            let stage_status = stage["status"].as_str().unwrap_or("unknown");
            let s_icon = pipeline_icon(stage_status);
            result.push_str(&format!("  {} {}: {}\n", s_icon, name, stage_status));
        }
    }

    result.trim().to_string()
}

fn state_icon(state: &str) -> &'static str {
    match state {
        "opened" => "[open]",
        "merged" => "[merged]",
        "closed" => "[closed]",
        _ => "[?]",
    }
}

fn pipeline_icon(status: &str) -> &'static str {
    match status {
        "success" => "[ok]",
        "failed" => "[fail]",
        "canceled" | "cancelled" => "[cancel]",
        "running" => "[run]",
        "pending" => "[pend]",
        "skipped" => "[skip]",
        _ => "[?]",
    }
}

fn strip_ansi(input: &str) -> String {
    static ANSI_RE: OnceLock<Regex> = OnceLock::new();
    let re = ANSI_RE.get_or_init(|| Regex::new(r"\x1b\[[0-9;]*m").unwrap());
    re.replace_all(input, "").to_string()
}

fn collapse_blank_lines(input: &str) -> String {
    let mut result = Vec::new();
    let mut prev_blank = false;

    for line in input.lines() {
        let is_blank = line.trim().is_empty();
        if is_blank && prev_blank {
            continue;
        }
        result.push(line);
        prev_blank = is_blank;
    }

    result.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_mr_list() {
        let json = serde_json::json!([
            {"iid": 42, "title": "Add auth", "state": "opened", "author": {"username": "alice"}},
            {"iid": 41, "title": "Fix bug", "state": "merged", "author": {"username": "bob"}}
        ]);
        let result = format_mr_list(&json);
        assert!(result.contains("Merge Requests"));
        assert!(result.contains("!42"));
        assert!(result.contains("Add auth"));
        assert!(result.contains("alice"));
    }

    #[test]
    fn test_format_mr_view() {
        let json = serde_json::json!({
            "iid": 42,
            "title": "Add auth",
            "state": "opened",
            "author": {"username": "alice"},
            "web_url": "https://gitlab.com/org/repo/-/merge_requests/42",
            "merge_status": "can_be_merged",
            "source_branch": "feat/auth",
            "target_branch": "main"
        });
        let result = format_mr_view(&json);
        assert!(result.contains("MR !42"));
        assert!(result.contains("[ok]"));
        assert!(result.contains("feat/auth -> main"));
    }

    #[test]
    fn test_format_issue_list() {
        let json = serde_json::json!([
            {"iid": 1, "title": "Bug report", "state": "opened"},
            {"iid": 2, "title": "Feature request", "state": "closed"}
        ]);
        let result = format_issue_list(&json);
        assert!(result.contains("Issues"));
        assert!(result.contains("#1"));
        assert!(result.contains("[open]"));
        assert!(result.contains("[closed]"));
    }

    #[test]
    fn test_format_ci_list() {
        let json = serde_json::json!([
            {"id": 123, "status": "success", "ref": "main"},
            {"id": 124, "status": "failed", "ref": "feat/x"}
        ]);
        let result = format_ci_list(&json);
        assert!(result.contains("Pipelines"));
        assert!(result.contains("#123"));
        assert!(result.contains("success"));
        assert!(result.contains("failed"));
    }

    #[test]
    fn test_format_ci_status() {
        let json = serde_json::json!({
            "status": "success",
            "stages": [
                {"name": "build", "status": "success"},
                {"name": "test", "status": "success"}
            ]
        });
        let result = format_ci_status(&json);
        assert!(result.contains("Pipeline:"));
        assert!(result.contains("success"));
        assert!(result.contains("build"));
        assert!(result.contains("test"));
    }

    #[test]
    fn test_parse_subcommand() {
        assert_eq!(parse_glab_subcommand("glab mr list"), ("mr", "list"));
        assert_eq!(parse_glab_subcommand("glab mr view 42"), ("mr", "view"));
        assert_eq!(parse_glab_subcommand("glab issue list"), ("issue", "list"));
        assert_eq!(parse_glab_subcommand("glab ci status"), ("ci", "status"));
    }

    #[test]
    fn test_compressor_empty() {
        let c = GlabCompressor;
        assert!(c.compress("glab mr list", "").is_none());
    }

    #[test]
    fn test_compressor_json_reduces_length() {
        let c = GlabCompressor;
        let output = r#"[
            {"iid": 42, "title": "Add auth", "state": "opened", "author": {"username": "alice"}}
        ]"#;
        let result = c.compress("glab mr list", output).unwrap();
        assert!(result.contains("!42"));
        assert!(result.len() < output.len());
    }

    #[test]
    fn test_strip_ansi() {
        let input = "\x1b[32mSuccess\x1b[0m";
        assert_eq!(strip_ansi(input), "Success");
    }

    #[test]
    fn test_collapse_blank_lines() {
        let input = "Line 1\n\n\n\nLine 2\n";
        assert_eq!(collapse_blank_lines(input), "Line 1\n\nLine 2");
    }
}
