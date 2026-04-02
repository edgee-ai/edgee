use std::collections::{HashMap, HashSet};

use super::loader::{Session, Turn};

const DEP_MIN_LEN: usize = 30;
const BURST_THRESHOLD: usize = 3;
const FREQ_MIN_COUNT: usize = 3;    // absolute: must appear in at least N sessions
const FREQ_MIN_RATIO: f64 = 0.15;   // relative: must be >= 15% of project sessions
const READ_EDIT_WINDOW: usize = 2;


const RETRY_LOOP_THRESHOLD: usize = 3; // same tool+target erroring this many times = loop
const COREAD_WINDOW: usize = 5;        // max turn distance between two reads to count as co-read
const COREAD_MIN_SESSIONS: usize = 3;  // min sessions a pair must appear in to be flagged

#[derive(Debug)]
pub struct Suggestion {
    pub severity: String,
    pub title: String,
    pub example_after: Option<String>,
}

pub struct ProjectFile {
    pub fname: String,
    pub session_count: usize,
}

pub struct SessionResult {
    pub suggestions: Vec<Suggestion>,
    pub start_date: String,  // "YYYY-MM-DD" from first turn timestamp
    pub max_severity: u8,    // 2=high, 1=medium, 0=low
}

#[allow(dead_code)]
pub struct SuggestResult {
    pub sessions: HashMap<String, SessionResult>,
    pub project_files: HashMap<String, Vec<ProjectFile>>,  // CLAUDE.md candidates per project
    pub coread_pairs: HashMap<String, Vec<(String, String, usize)>>, // file pairs read close together
    pub total_sessions: usize,
    pub opening_burst_count: usize,
    pub read_then_edit_count: usize,
    pub redundant_read_count: usize,
}

// ── Cross-session repeat detection ───────────────────────────────────────────

pub fn fingerprint_sessions(sessions: &mut Vec<Session>) {
    let mut all_refs: Vec<(usize, usize, String)> = sessions
        .iter()
        .enumerate()
        .flat_map(|(si, session)| {
            session
                .turns
                .iter()
                .enumerate()
                .map(move |(ti, turn)| (si, ti, turn.timestamp.clone()))
        })
        .collect();
    all_refs.sort_by(|a, b| a.2.cmp(&b.2));

    let mut fp_map: HashMap<String, (String, usize)> = HashMap::new();
    for &(si, ti, _) in &all_refs {
        let turn = &sessions[si].turns[ti];
        for block in &turn.content {
            if block.block_type != "text" && block.block_type != "tool_result" {
                continue;
            }
            if let Some(fp) = &block.fingerprint {
                let entry = fp_map
                    .entry(fp.clone())
                    .or_insert_with(|| (turn.uuid.clone(), 0));
                entry.1 += 1;
            }
        }
    }

    for session in sessions.iter_mut() {
        for turn in session.turns.iter_mut() {
            let uuid = turn.uuid.clone();
            for block in turn.content.iter_mut() {
                if block.block_type != "text" && block.block_type != "tool_result" {
                    continue;
                }
                if let Some(fp) = &block.fingerprint {
                    if let Some((first_uuid, count)) = fp_map.get(fp) {
                        if *count > 1 && first_uuid != &uuid {
                            block.is_repeat = true;
                        }
                    }
                }
            }
        }
    }
}

// ── Detection helpers ─────────────────────────────────────────────────────────

fn read_file_paths(turns: &[Turn]) -> Vec<String> {
    let mut paths = Vec::new();
    for turn in turns {
        for block in &turn.content {
            if block.block_type == "tool_use" && block.tool_name.as_deref() == Some("Read") {
                if let Some(input) = &block.tool_input {
                    if let Some(fp) = input["file_path"].as_str() {
                        if !fp.is_empty() {
                            paths.push(fp.to_string());
                        }
                    }
                }
            }
        }
    }
    paths
}

fn opening_burst(turns: &[Turn]) -> usize {
    let mut count = 0;
    for turn in turns {
        if turn.role != "assistant" {
            continue;
        }
        let tool_blocks: Vec<_> = turn
            .content
            .iter()
            .filter(|b| b.block_type == "tool_use")
            .collect();
        let has_text = turn
            .content
            .iter()
            .any(|b| b.block_type == "text" && b.text.is_some());

        if has_text {
            break;
        }
        if !tool_blocks.is_empty()
            && tool_blocks
                .iter()
                .all(|b| b.tool_name.as_deref() == Some("Read"))
        {
            count += tool_blocks.len();
        } else {
            break;
        }
    }
    count
}

fn read_then_edit_pairs(turns: &[Turn]) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for (i, turn) in turns.iter().enumerate() {
        if turn.role != "assistant" {
            continue;
        }
        for block in &turn.content {
            if block.block_type != "tool_use" || block.tool_name.as_deref() != Some("Read") {
                continue;
            }
            let read_path = match block
                .tool_input
                .as_ref()
                .and_then(|inp| inp["file_path"].as_str())
            {
                Some(p) if !p.is_empty() => p.to_string(),
                _ => continue,
            };

            let result_text: Option<String> = (i + 1 < turns.len())
                .then(|| {
                    turns[i + 1].content.iter().find(|rb| {
                        rb.block_type == "tool_result" && rb.tool_use_id == block.tool_use_id
                    })
                })
                .flatten()
                .and_then(|rb| rb.text.clone());

            let window_end = (i + 1 + READ_EDIT_WINDOW * 2 + 1).min(turns.len());
            'outer: for j in (i + 1)..window_end {
                let edit_turn = &turns[j];
                if edit_turn.role != "assistant" {
                    continue;
                }
                for eb in &edit_turn.content {
                    if eb.block_type != "tool_use" || eb.tool_name.as_deref() != Some("Edit") {
                        continue;
                    }
                    let edit_path = eb
                        .tool_input
                        .as_ref()
                        .and_then(|inp| inp["file_path"].as_str())
                        .unwrap_or("");
                    if edit_path != read_path {
                        continue;
                    }
                    let old_string = eb
                        .tool_input
                        .as_ref()
                        .and_then(|inp| inp["old_string"].as_str())
                        .unwrap_or("");
                    if old_string.len() >= DEP_MIN_LEN {
                        if let Some(ref rt) = result_text {
                            let prefix = &old_string[..DEP_MIN_LEN];
                            if rt.contains(prefix) && !seen.contains(&read_path) {
                                files.push(read_path.clone());
                                seen.insert(read_path.clone());
                                break 'outer;
                            }
                        }
                    }
                }
            }
        }
    }
    files
}

/// Returns (file_path, total_read_count, max_turn_distance, wasted_tokens) for
/// files read 2+ times with identical content and no intervening Edit.
/// wasted_tokens = content_size * redundant_count (chars/4 token approximation).
pub fn redundant_reads(turns: &[Turn]) -> Vec<(String, usize, usize, u64)> {
    // Pre-build: tool_use_id → file_path for every Read call
    let mut read_id_to_path: HashMap<String, String> = HashMap::new();
    for turn in turns {
        for block in &turn.content {
            if block.block_type == "tool_use" && block.tool_name.as_deref() == Some("Read") {
                if let (Some(id), Some(path)) = (
                    &block.tool_use_id,
                    block.tool_input.as_ref().and_then(|i| i["file_path"].as_str()),
                ) {
                    if !path.is_empty() {
                        read_id_to_path.insert(id.clone(), path.to_string());
                    }
                }
            }
        }
    }

    struct FileState {
        last_fp: Option<String>,
        last_content_len: usize,
        edited_since: bool,
        total_reads: usize,
        redundant: usize,
        last_turn_idx: usize,
        max_distance: usize,
        wasted_tokens: u64,
    }

    let mut state: HashMap<String, FileState> = HashMap::new();

    for (turn_idx, turn) in turns.iter().enumerate() {
        for block in &turn.content {
            match block.block_type.as_str() {
                "tool_use"
                    if matches!(
                        block.tool_name.as_deref(),
                        Some("Edit") | Some("Write")
                    ) =>
                {
                    if let Some(path) =
                        block.tool_input.as_ref().and_then(|i| i["file_path"].as_str())
                    {
                        if let Some(s) = state.get_mut(path) {
                            s.edited_since = true;
                        }
                    }
                }
                "tool_result" => {
                    if let Some(id) = &block.tool_use_id {
                        if let Some(path) = read_id_to_path.get(id) {
                            let fp = block.fingerprint.clone();
                            let content_len = block.text.as_deref().map(|t| t.len()).unwrap_or(0);
                            let s = state.entry(path.clone()).or_insert(FileState {
                                last_fp: None,
                                last_content_len: 0,
                                edited_since: false,
                                total_reads: 0,
                                redundant: 0,
                                last_turn_idx: turn_idx,
                                max_distance: 0,
                                wasted_tokens: 0,
                            });
                            s.total_reads += 1;
                            if !s.edited_since {
                                if let (Some(last), Some(current)) = (&s.last_fp, &fp) {
                                    if last == current {
                                        s.redundant += 1;
                                        s.wasted_tokens += (s.last_content_len / 4) as u64;
                                        let dist = turn_idx.saturating_sub(s.last_turn_idx);
                                        if dist > s.max_distance {
                                            s.max_distance = dist;
                                        }
                                    }
                                }
                            }
                            s.last_fp = fp;
                            s.last_content_len = content_len;
                            s.last_turn_idx = turn_idx;
                            s.edited_since = false;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let mut result: Vec<(String, usize, usize, u64)> = state
        .into_iter()
        .filter(|(_, s)| s.redundant > 0)
        .map(|(path, s)| (path, s.total_reads, s.max_distance, s.wasted_tokens))
        .collect();
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

// ── Retry loop detector ───────────────────────────────────────────────────────

fn is_error_result(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with("Error") || t.starts_with("error") || t.contains("ENOENT")
        || t.contains("EACCES") || t.contains("command not found")
        || t.contains("No such file") || t.contains("Permission denied")
        || t.contains("exit code: 1") || t.contains("exit code: 2")
        || t.starts_with("Traceback") || t.starts_with("SyntaxError")
        || t.starts_with("TypeError") || t.starts_with("ValueError")
}

/// Returns (tool_name, target, error_count) for tool+target combos that errored
/// RETRY_LOOP_THRESHOLD or more times in a row without a success in between.
fn tool_error_loops(turns: &[Turn]) -> Vec<(String, String, usize)> {
    // Build tool_use_id → (tool_name, target) from all tool_use blocks
    let mut id_to_call: HashMap<String, (String, String)> = HashMap::new();
    for turn in turns {
        for block in &turn.content {
            if block.block_type != "tool_use" {
                continue;
            }
            if let Some(id) = &block.tool_use_id {
                let tool = block.tool_name.clone().unwrap_or_default();
                let target = block
                    .tool_input
                    .as_ref()
                    .and_then(|inp| {
                        inp["file_path"].as_str()
                            .or_else(|| inp["command"].as_str())
                            .or_else(|| inp["path"].as_str())
                    })
                    .unwrap_or("")
                    .to_string();
                id_to_call.insert(id.clone(), (tool, target));
            }
        }
    }

    // Count consecutive errors per (tool, target), reset on success
    let mut error_runs: HashMap<(String, String), usize> = HashMap::new();
    let mut max_errors: HashMap<(String, String), usize> = HashMap::new();

    for turn in turns {
        for block in &turn.content {
            if block.block_type != "tool_result" {
                continue;
            }
            if let Some(id) = &block.tool_use_id {
                if let Some(key) = id_to_call.get(id) {
                    let is_err = block.text.as_deref().map(is_error_result).unwrap_or(false);
                    let run = error_runs.entry(key.clone()).or_default();
                    if is_err {
                        *run += 1;
                        let max = max_errors.entry(key.clone()).or_default();
                        if *run > *max {
                            *max = *run;
                        }
                    } else {
                        *run = 0;
                    }
                }
            }
        }
    }

    let mut result: Vec<(String, String, usize)> = max_errors
        .into_iter()
        .filter(|(_, count)| *count >= RETRY_LOOP_THRESHOLD)
        .map(|((tool, target), count)| (tool, target, count))
        .collect();
    result.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    result
}

// ── Cache-based detectors ─────────────────────────────────────────────────────

// ── Co-read pair detector ─────────────────────────────────────────────────────

/// Returns the set of (fname_a, fname_b) pairs read within COREAD_WINDOW turns
/// of each other in this session.
fn session_coread_pairs(turns: &[Turn]) -> HashSet<(String, String)> {
    let reads: Vec<(String, usize)> = turns
        .iter()
        .enumerate()
        .flat_map(|(i, turn)| {
            turn.content.iter().filter_map(move |block| {
                if block.block_type == "tool_use" && block.tool_name.as_deref() == Some("Read") {
                    block
                        .tool_input
                        .as_ref()
                        .and_then(|inp| inp["file_path"].as_str())
                        .filter(|p| !p.is_empty())
                        .map(|p| {
                            let fname = p.split('/').next_back().unwrap_or(p).to_string();
                            (fname, i)
                        })
                } else {
                    None
                }
            })
        })
        .collect();

    let mut pairs: HashSet<(String, String)> = HashSet::new();
    for i in 0..reads.len() {
        for j in (i + 1)..reads.len() {
            if reads[j].1.saturating_sub(reads[i].1) > COREAD_WINDOW {
                break;
            }
            if reads[i].0 != reads[j].0 {
                let (a, b) = if reads[i].0 <= reads[j].0 {
                    (reads[i].0.clone(), reads[j].0.clone())
                } else {
                    (reads[j].0.clone(), reads[i].0.clone())
                };
                pairs.insert((a, b));
            }
        }
    }
    pairs
}

// ── Public entry point ────────────────────────────────────────────────────────

pub fn generate_suggestions(sessions: &[Session]) -> SuggestResult {
    // Total sessions per project (for relative threshold)
    let mut project_session_count: HashMap<String, usize> = HashMap::new();
    for session in sessions {
        *project_session_count.entry(session.project.clone()).or_default() += 1;
    }

    // Collect per-project file → session set (for CLAUDE.md candidates)
    let mut project_file_sessions: HashMap<String, HashMap<String, HashSet<String>>> =
        HashMap::new();
    for session in sessions {
        for fp in read_file_paths(&session.turns) {
            project_file_sessions
                .entry(session.project.clone())
                .or_default()
                .entry(fp)
                .or_default()
                .insert(session.session_id.clone());
        }
    }

    // Build project_files: CLAUDE.md candidates per project
    let mut project_files: HashMap<String, Vec<ProjectFile>> = HashMap::new();
    for (project, freq_map) in &project_file_sessions {
        let total_project_sessions = *project_session_count.get(project).unwrap_or(&1);
        let mut candidates: Vec<ProjectFile> = freq_map
            .iter()
            .filter_map(|(fp, session_set)| {
                let count = session_set.len();
                let ratio = count as f64 / total_project_sessions as f64;
                if count < FREQ_MIN_COUNT || ratio < FREQ_MIN_RATIO {
                    return None;
                }
                let fname = fp.split('/').next_back().unwrap_or(fp).to_string();
                Some(ProjectFile {
                    fname,
                    session_count: count,
                })
            })
            .collect();
        candidates.sort_by(|a, b| b.session_count.cmp(&a.session_count).then(a.fname.cmp(&b.fname)));
        if !candidates.is_empty() {
            project_files.insert(project.clone(), candidates);
        }
    }

    // Build coread_pairs: file pairs read close together across sessions
    let mut project_pair_sessions: HashMap<String, HashMap<(String, String), usize>> =
        HashMap::new();
    for session in sessions {
        for pair in session_coread_pairs(&session.turns) {
            *project_pair_sessions
                .entry(session.project.clone())
                .or_default()
                .entry(pair)
                .or_default() += 1;
        }
    }
    let mut coread_pairs: HashMap<String, Vec<(String, String, usize)>> = HashMap::new();
    for (project, pair_map) in project_pair_sessions {
        let mut pairs: Vec<(String, String, usize)> = pair_map
            .into_iter()
            .filter(|(_, count)| *count >= COREAD_MIN_SESSIONS)
            .map(|((a, b), count)| (a, b, count))
            .collect();
        pairs.sort_by(|x, y| y.2.cmp(&x.2));
        if !pairs.is_empty() {
            coread_pairs.insert(project, pairs);
        }
    }

    let mut session_results: HashMap<String, SessionResult> = HashMap::new();
    let mut opening_burst_count = 0usize;
    let mut read_then_edit_count = 0usize;
    let mut redundant_read_count = 0usize;

    for session in sessions {
        let mut suggestions: Vec<Suggestion> = Vec::new();

        let burst = opening_burst(&session.turns);
        if burst >= BURST_THRESHOLD {
            opening_burst_count += 1;
            let read_paths = read_file_paths(&session.turns);
            let examples: Vec<String> = read_paths
                .iter()
                .take(4)
                .map(|p| format!("@{}", p.split('/').next_back().unwrap_or(p)))
                .collect();
            suggestions.push(Suggestion {
                severity: if burst >= 5 { "high" } else { "medium" }.to_string(),
                title: format!("{} consecutive reads at session open", burst),
                example_after: Some(format!(
                    "\"@loader.py — explain how data flows.\"  ({})",
                    examples.join("  ")
                )),
            });
        }

        let rte_pairs = read_then_edit_pairs(&session.turns);
        read_then_edit_count += rte_pairs.len();
        for fp in rte_pairs {
            let fname = fp.split('/').next_back().unwrap_or(&fp).to_string();
            suggestions.push(Suggestion {
                severity: "medium".to_string(),
                title: format!("Read → Edit on {}", fname),
                example_after: Some(format!("\"@{} — fix the bug.\"", fname)),
            });
        }

        let rr_pairs = redundant_reads(&session.turns);
        redundant_read_count += rr_pairs.len();
        for (fp, total_reads, max_distance, wasted_tokens) in rr_pairs {
            let fname = fp.split('/').next_back().unwrap_or(&fp).to_string();
            let advice = if max_distance > 20 {
                "reads were far apart — consider splitting this into a shorter session"
            } else {
                "reads were close together — Claude may have lost track of earlier results"
            };
            let token_note = if wasted_tokens > 0 {
                format!("~{}k tokens re-read unnecessarily — {}", wasted_tokens.max(1) / 1000, advice)
            } else {
                advice.to_string()
            };
            suggestions.push(Suggestion {
                severity: "medium".to_string(),
                title: format!("{} read {}× with unchanged content ({} turns apart)", fname, total_reads, max_distance),
                example_after: Some(token_note),
            });
        }

        // frequent_read: flag files in this session that qualify as CLAUDE.md candidates
        if let Some(candidates) = project_files.get(&session.project) {
            let candidate_fnames: HashSet<&str> =
                candidates.iter().map(|pf| pf.fname.as_str()).collect();
            let session_reads = read_file_paths(&session.turns);
            let mut flagged: HashSet<String> = HashSet::new();
            for fp in &session_reads {
                let fname = fp.split('/').next_back().unwrap_or(fp);
                if candidate_fnames.contains(fname) && flagged.insert(fname.to_string()) {
                    let count = candidates
                        .iter()
                        .find(|pf| pf.fname == fname)
                        .map(|pf| pf.session_count)
                        .unwrap_or(0);
                    suggestions.push(Suggestion {
                        severity: "low".to_string(),
                        title: format!("{} read in {} sessions — consider adding to CLAUDE.md", fname, count),
                        example_after: Some(format!(
                            "Each session re-caches {} from scratch — in CLAUDE.md it is cached once and reused across all sessions",
                            fname
                        )),
                    });
                }
            }
        }

        for (tool, target, count) in tool_error_loops(&session.turns) {
            let label = if target.is_empty() {
                tool.clone()
            } else {
                format!("{} on {}", tool, target.split('/').next_back().unwrap_or(&target))
            };
            suggestions.push(Suggestion {
                severity: "high".to_string(),
                title: format!("{} failed {}× in a row — Claude was stuck", label, count),
                example_after: Some(
                    "Add expected output or constraints upfront so Claude doesn't guess"
                        .to_string(),
                ),
            });
        }

        if !suggestions.is_empty() {
            let start_date = session
                .turns
                .first()
                .map(|t| t.timestamp.get(..10).unwrap_or("unknown").to_string())
                .unwrap_or_default();
            let max_severity = suggestions
                .iter()
                .map(|s| match s.severity.as_str() {
                    "high" => 2u8,
                    "medium" => 1,
                    _ => 0,
                })
                .max()
                .unwrap_or(0);
            session_results.insert(
                session.session_id.clone(),
                SessionResult {
                    suggestions,
                    start_date,
                    max_severity,
                },
            );
        }
    }

    SuggestResult {
        total_sessions: sessions.len(),
        sessions: session_results,
        project_files,
        coread_pairs,
        opening_burst_count,
        read_then_edit_count,
        redundant_read_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::suggest::loader::{ContentBlock, Turn};

    fn make_turn(role: &str, blocks: Vec<ContentBlock>) -> Turn {
        Turn {
            uuid: uuid::Uuid::new_v4().to_string(),
            role: role.to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            content: blocks,
            usage: None,
        }
    }

    fn read_use(id: &str, path: &str) -> ContentBlock {
        ContentBlock {
            block_type: "tool_use".to_string(),
            text: None,
            tool_use_id: Some(id.to_string()),
            tool_name: Some("Read".to_string()),
            tool_input: Some(serde_json::json!({"file_path": path})),
            fingerprint: None,
            is_repeat: false,
        }
    }

    fn read_result(id: &str, content: &str) -> ContentBlock {
        ContentBlock {
            block_type: "tool_result".to_string(),
            text: Some(content.to_string()),
            tool_use_id: Some(id.to_string()),
            tool_name: None,
            tool_input: None,
            fingerprint: Some(format!("fp-{}", content)),
            is_repeat: false,
        }
    }

    fn edit_use(path: &str) -> ContentBlock {
        ContentBlock {
            block_type: "tool_use".to_string(),
            text: None,
            tool_use_id: Some("edit-1".to_string()),
            tool_name: Some("Edit".to_string()),
            tool_input: Some(serde_json::json!({"file_path": path})),
            fingerprint: None,
            is_repeat: false,
        }
    }

    #[test]
    fn detects_redundant_read() {
        let turns = vec![
            make_turn("assistant", vec![read_use("r1", "/project/foo.py")]),
            make_turn("user",      vec![read_result("r1", "content-A")]),
            make_turn("assistant", vec![read_use("r2", "/project/foo.py")]),
            make_turn("user",      vec![read_result("r2", "content-A")]), // same → redundant
        ];
        let result = redundant_reads(&turns);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "/project/foo.py");
        assert_eq!(result[0].1, 2);
        assert!(result[0].2 > 0);
        // wasted_tokens may be 0 in test (no text content in fixtures)
    }

    #[test]
    fn no_flag_after_edit() {
        let turns = vec![
            make_turn("assistant", vec![read_use("r1", "/project/foo.py")]),
            make_turn("user",      vec![read_result("r1", "content-A")]),
            make_turn("assistant", vec![edit_use("/project/foo.py")]),
            make_turn("assistant", vec![read_use("r2", "/project/foo.py")]),
            make_turn("user",      vec![read_result("r2", "content-A")]), // same content but edit happened
        ];
        let result = redundant_reads(&turns);
        assert!(result.is_empty());
    }

    #[test]
    fn no_flag_when_content_changed() {
        let turns = vec![
            make_turn("assistant", vec![read_use("r1", "/project/foo.py")]),
            make_turn("user",      vec![read_result("r1", "content-A")]),
            make_turn("assistant", vec![read_use("r2", "/project/foo.py")]),
            make_turn("user",      vec![read_result("r2", "content-B")]), // different
        ];
        let result = redundant_reads(&turns);
        assert!(result.is_empty());
    }
}
