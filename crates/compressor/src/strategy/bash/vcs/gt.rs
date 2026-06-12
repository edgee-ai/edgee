use super::BashCompressor;
use regex::Regex;
use std::sync::OnceLock;

pub struct GtCompressor;

impl BashCompressor for GtCompressor {
    fn compress(&self, command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        let subcommand = parse_gt_subcommand(command);
        let filtered = match subcommand {
            "log" => filter_gt_log(output),
            "submit" => filter_gt_submit(output),
            "sync" => filter_gt_sync(output),
            "restack" => filter_gt_restack(output),
            "create" => filter_gt_create(output),
            _ => return None,
        };

        if filtered == output.trim() {
            return None;
        }

        Some(filtered)
    }
}

fn parse_gt_subcommand(command: &str) -> &str {
    let mut found_gt = false;
    for arg in command.split_whitespace() {
        if arg == "gt" {
            found_gt = true;
            continue;
        }
        if found_gt && !arg.starts_with('-') {
            return arg;
        }
    }
    ""
}

fn filter_gt_log(input: &str) -> String {
    static EMAIL_RE: OnceLock<Regex> = OnceLock::new();
    let email_re = EMAIL_RE
        .get_or_init(|| Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b").unwrap());

    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let lines: Vec<&str> = trimmed.lines().collect();
    let mut result = Vec::new();
    let mut entry_count = 0;
    const MAX_LOG_ENTRIES: usize = 8;

    for (i, line) in lines.iter().enumerate() {
        if is_graph_node(line) {
            entry_count += 1;
        }

        let replaced = email_re.replace_all(line, "");
        let processed = if replaced.len() > 120 {
            format!("{}...", &replaced[..120].trim_end())
        } else {
            replaced.trim().to_string()
        };
        result.push(processed);

        if entry_count >= MAX_LOG_ENTRIES {
            let remaining = lines[i + 1..].iter().filter(|l| is_graph_node(l)).count();
            if remaining > 0 {
                result.push(format!("... +{} more entries", remaining));
            }
            break;
        }
    }

    result.join("\n")
}

fn filter_gt_submit(input: &str) -> String {
    static PR_LINE_RE: OnceLock<Regex> = OnceLock::new();
    let pr_line_re = PR_LINE_RE.get_or_init(|| {
        Regex::new(r"(Created|Updated)\s+pull\s+request\s+#(\d+)\s+for\s+([^\s:]+)(?::\s*(\S+))?")
            .unwrap()
    });

    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut pushed = Vec::new();
    let mut prs = Vec::new();

    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if line.contains("pushed") || line.contains("Pushed") {
            pushed.push(extract_branch_name(line));
        } else if let Some(caps) = pr_line_re.captures(line) {
            let action = caps[1].to_lowercase();
            let num = &caps[2];
            let branch = &caps[3];
            if let Some(url) = caps.get(4) {
                prs.push(format!(
                    "{} PR #{} {} {}",
                    action,
                    num,
                    branch,
                    url.as_str()
                ));
            } else {
                prs.push(format!("{} PR #{} {}", action, num, branch));
            }
        }
    }

    let mut summary = Vec::new();

    if !pushed.is_empty() {
        let branch_names: Vec<&str> = pushed
            .iter()
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        if !branch_names.is_empty() {
            summary.push(format!("pushed {}", branch_names.join(", ")));
        } else {
            summary.push(format!("pushed {} branches", pushed.len()));
        }
    }

    summary.extend(prs);

    if summary.is_empty() {
        let first = trimmed.lines().next().unwrap_or("");
        return if first.len() > 200 {
            format!("{}...", &first[..200])
        } else {
            first.to_string()
        };
    }

    summary.join("\n")
}

fn filter_gt_sync(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut synced = 0;
    let mut deleted = 0;
    let mut deleted_names = Vec::new();

    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if (line.contains("Synced") && line.contains("branch"))
            || line.starts_with("Synced with remote")
        {
            synced += 1;
        }
        if line.contains("deleted") || line.contains("Deleted") {
            deleted += 1;
            let name = extract_branch_name(line);
            if !name.is_empty() {
                deleted_names.push(name);
            }
        }
    }

    let mut parts = Vec::new();

    if synced > 0 {
        parts.push(format!("{} synced", synced));
    }

    if deleted > 0 {
        if deleted_names.is_empty() {
            parts.push(format!("{} deleted", deleted));
        } else {
            parts.push(format!(
                "{} deleted ({})",
                deleted,
                deleted_names.join(", ")
            ));
        }
    }

    if parts.is_empty() {
        return "ok synced".to_string();
    }

    format!("ok sync: {}", parts.join(", "))
}

fn filter_gt_restack(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut restacked = 0;
    for line in trimmed.lines() {
        let line = line.trim();
        if (line.contains("Restacked") || line.contains("Rebased")) && line.contains("branch") {
            restacked += 1;
        }
    }

    if restacked > 0 {
        format!("ok restacked {} branches", restacked)
    } else {
        "ok restacked".to_string()
    }
}

fn filter_gt_create(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let branch_name = trimmed
        .lines()
        .find_map(|line| {
            let line = line.trim();
            if line.contains("Created") || line.contains("created") {
                Some(extract_branch_name(line))
            } else {
                None
            }
        })
        .unwrap_or_default();

    if branch_name.is_empty() {
        let first_line = trimmed.lines().next().unwrap_or("");
        format!("ok created {}", first_line.trim())
    } else {
        format!("ok created {}", branch_name)
    }
}

fn is_graph_node(line: &str) -> bool {
    let stripped = line
        .trim_start_matches('│')
        .trim_start_matches('|')
        .trim_start();
    stripped.starts_with('◉')
        || stripped.starts_with('○')
        || stripped.starts_with('◯')
        || stripped.starts_with('◆')
        || stripped.starts_with('●')
        || stripped.starts_with('@')
        || stripped.starts_with('*')
}

fn extract_branch_name(line: &str) -> String {
    static BRANCH_NAME_RE: OnceLock<Regex> = OnceLock::new();
    let branch_name_re = BRANCH_NAME_RE.get_or_init(|| {
        Regex::new(
            r#"(?:Created|Pushed|pushed|Deleted|deleted)\s+branch\s+[`"']?([a-zA-Z0-9/_.\-+@]+)"#,
        )
        .unwrap()
    });

    branch_name_re
        .captures(line)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_gt_log_exact_format() {
        let input = r#"◉  abc1234 feat/add-auth 2d ago
│  feat(auth): add login endpoint
│
◉  def5678 feat/add-db 3d ago user@example.com
│  feat(db): add migration system
│
◉  ghi9012 main 5d ago admin@corp.io
│  chore: update dependencies
~
"#;
        let output = filter_gt_log(input);
        let expected = "\
◉  abc1234 feat/add-auth 2d ago
│  feat(auth): add login endpoint
│
◉  def5678 feat/add-db 3d ago
│  feat(db): add migration system
│
◉  ghi9012 main 5d ago
│  chore: update dependencies
~";
        assert_eq!(output, expected);
    }

    #[test]
    fn test_filter_gt_submit_exact_format() {
        let input = r#"Pushed branch feat/add-auth
Created pull request #42 for feat/add-auth
Pushed branch feat/add-db
Updated pull request #40 for feat/add-db
"#;
        let output = filter_gt_submit(input);
        let expected = "\
pushed feat/add-auth, feat/add-db
created PR #42 feat/add-auth
updated PR #40 feat/add-db";
        assert_eq!(output, expected);
    }

    #[test]
    fn test_filter_gt_sync_exact_format() {
        let input = r#"Synced with remote
Deleted branch feat/merged-feature
Deleted branch fix/old-hotfix
"#;
        let output = filter_gt_sync(input);
        assert_eq!(
            output,
            "ok sync: 1 synced, 2 deleted (feat/merged-feature, fix/old-hotfix)"
        );
    }

    #[test]
    fn test_filter_gt_restack_exact_format() {
        let input = r#"Restacked branch feat/add-auth on main
Restacked branch feat/add-db on feat/add-auth
Restacked branch fix/parsing on feat/add-db
"#;
        let output = filter_gt_restack(input);
        assert_eq!(output, "ok restacked 3 branches");
    }

    #[test]
    fn test_filter_gt_create_exact_format() {
        let input = "Created branch feat/new-feature\n";
        let output = filter_gt_create(input);
        assert_eq!(output, "ok created feat/new-feature");
    }

    #[test]
    fn test_filter_gt_log_truncation() {
        let mut input = String::new();
        for i in 0..20 {
            input.push_str(&format!(
                "◉  hash{:02} branch-{} 1d ago dev@example.com\n│  commit message {}\n│\n",
                i, i, i
            ));
        }
        input.push_str("~\n");

        let output = filter_gt_log(&input);
        assert!(output.contains("... +"));
    }

    #[test]
    fn test_filter_gt_log_empty() {
        assert_eq!(filter_gt_log(""), String::new());
        assert_eq!(filter_gt_log("  "), String::new());
    }

    #[test]
    fn test_filter_gt_log_long() {
        let input = r#"◉  abc1234 feat/add-auth
│  Author: Dev User <dev@example.com>
│  Date: 2026-02-25 10:30:00 -0800
│
│  feat(auth): add login endpoint with OAuth2 support
│  and session management for web clients
│
◉  def5678 feat/add-db
│  Author: Other Dev <other@example.com>
│  Date: 2026-02-24 14:00:00 -0800
│
│  feat(db): add migration system
~
"#;

        let output = filter_gt_log(input);
        assert!(output.contains("abc1234"));
        assert!(!output.contains("dev@example.com"));
        assert!(!output.contains("other@example.com"));
    }

    #[test]
    fn test_filter_gt_submit_empty() {
        assert_eq!(filter_gt_submit(""), String::new());
    }

    #[test]
    fn test_filter_gt_sync_empty() {
        assert_eq!(filter_gt_sync(""), String::new());
    }

    #[test]
    fn test_filter_gt_sync_no_deletes() {
        let input = "Synced with remote\n";
        let output = filter_gt_sync(input);
        assert!(output.contains("ok sync"));
        assert!(output.contains("synced"));
        assert!(!output.contains("deleted"));
    }

    #[test]
    fn test_filter_gt_restack_empty() {
        assert_eq!(filter_gt_restack(""), String::new());
    }

    #[test]
    fn test_filter_gt_create_empty() {
        assert_eq!(filter_gt_create(""), String::new());
    }

    #[test]
    fn test_filter_gt_create_no_branch_name() {
        let input = "Some unexpected output\n";
        let output = filter_gt_create(input);
        assert!(output.starts_with("ok created"));
    }

    #[test]
    fn test_is_graph_node() {
        assert!(is_graph_node("◉  abc1234 main"));
        assert!(is_graph_node("○  def5678 feat/x"));
        assert!(is_graph_node("@  ghi9012 (current)"));
        assert!(is_graph_node("*  jkl3456 branch"));
        assert!(is_graph_node("│ ◉  nested node"));
        assert!(!is_graph_node("│  just a message line"));
        assert!(!is_graph_node("~"));
    }

    #[test]
    fn test_extract_branch_name() {
        assert_eq!(
            extract_branch_name("Created branch feat/new-feature"),
            "feat/new-feature"
        );
        assert_eq!(
            extract_branch_name("Pushed branch fix/bug-123"),
            "fix/bug-123"
        );
        assert_eq!(
            extract_branch_name("Pushed branch feat/auth+session"),
            "feat/auth+session"
        );
        assert_eq!(extract_branch_name("Created branch user@fix"), "user@fix");
        assert_eq!(extract_branch_name("no branch here"), "");
    }

    #[test]
    fn test_compressor_empty() {
        let c = GtCompressor;
        assert!(c.compress("gt log", "").is_none());
    }

    #[test]
    fn test_compressor_reduces_length() {
        let c = GtCompressor;
        let input = r#"
  ✅ Syncing with remote...
  Pulling latest changes from main...
  Successfully pulled 5 new commits
  Synced branch feat/add-auth with remote
  Synced branch feat/add-db with remote
  Branch feat/merged-feature has been merged
  Deleted branch feat/merged-feature
  Branch fix/old-hotfix has been merged
  Deleted branch fix/old-hotfix
  All branches synced!
"#;
        let result = c.compress("gt sync", input).unwrap();
        assert!(result.len() < input.len());
    }

    #[test]
    fn test_parse_subcommand() {
        assert_eq!(parse_gt_subcommand("gt log"), "log");
        assert_eq!(parse_gt_subcommand("gt sync"), "sync");
        assert_eq!(parse_gt_subcommand("gt submit"), "submit");
    }
}
