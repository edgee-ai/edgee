use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use hex;
use serde_json::Value;
use sha2::{Digest, Sha256};

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ContentBlock {
    pub block_type: String,
    pub text: Option<String>,
    pub tool_use_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<Value>,
    pub fingerprint: Option<String>,
    pub is_repeat: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct Turn {
    pub uuid: String,
    pub role: String,
    pub timestamp: String,
    pub content: Vec<ContentBlock>,
    #[allow(dead_code)]
    pub usage: Option<TurnUsage>,
}

#[derive(Debug)]
pub struct Session {
    pub session_id: String,
    pub project: String,
    pub turns: Vec<Turn>,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn sha256_hex16(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..8])
}

fn parse_content(content_raw: &Value) -> Vec<ContentBlock> {
    match content_raw {
        Value::String(s) => {
            let fp = sha256_hex16(s);
            vec![ContentBlock {
                block_type: "text".to_string(),
                text: Some(s.clone()),
                tool_use_id: None,
                tool_name: None,
                tool_input: None,
                fingerprint: Some(fp),
                is_repeat: false,
            }]
        }
        Value::Array(items) => {
            let mut blocks = Vec::new();
            for item in items {
                if !item.is_object() {
                    continue;
                }
                let block_type = item["type"].as_str().unwrap_or("unknown").to_string();
                let mut text = item["text"].as_str().map(String::from);

                if block_type == "tool_result" && text.is_none() {
                    text = match &item["content"] {
                        Value::String(s) => Some(s.clone()),
                        v @ Value::Array(_) => Some(v.to_string()),
                        _ => None,
                    };
                }

                let item_str = item.to_string();
                let fp_input = text.as_deref().unwrap_or(&item_str);
                let fp = sha256_hex16(fp_input);

                let tool_use_id = item["tool_use_id"]
                    .as_str()
                    .or_else(|| item["id"].as_str())
                    .map(String::from);
                let tool_name = item["name"].as_str().map(String::from);
                let tool_input = if item["input"].is_object() {
                    Some(item["input"].clone())
                } else {
                    None
                };

                blocks.push(ContentBlock {
                    block_type,
                    text,
                    tool_use_id,
                    tool_name,
                    tool_input,
                    fingerprint: Some(fp),
                    is_repeat: false,
                });
            }
            blocks
        }
        _ => vec![],
    }
}

pub fn parse_session_file(path: &Path) -> Vec<Turn> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let mut seen_message_ids: HashSet<String> = HashSet::new();
    let mut turns: Vec<Turn> = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let rec: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let role = rec["type"].as_str().unwrap_or("");
        if role != "user" && role != "assistant" {
            continue;
        }

        let msg = &rec["message"];
        if !msg.is_object() {
            continue;
        }

        let message_id = msg["id"].as_str().map(String::from);

        if role == "assistant" {
            if let Some(ref mid) = message_id {
                if seen_message_ids.contains(mid) {
                    continue;
                }
                seen_message_ids.insert(mid.clone());
            }
        }

        let timestamp = rec["timestamp"].as_str().unwrap_or("").to_string();

        let content = parse_content(&msg["content"]);
        let uuid = rec["uuid"].as_str().unwrap_or("").to_string();

        let usage = {
            let u = &msg["usage"];
            if u["cache_creation_input_tokens"].is_number() || u["cache_read_input_tokens"].is_number() {
                Some(TurnUsage {
                    input_tokens: u["input_tokens"].as_u64().unwrap_or(0),
                    cache_creation_input_tokens: u["cache_creation_input_tokens"].as_u64().unwrap_or(0),
                    cache_read_input_tokens: u["cache_read_input_tokens"].as_u64().unwrap_or(0),
                })
            } else {
                None
            }
        };

        turns.push(Turn {
            uuid,
            role: role.to_string(),
            timestamp,
            content,
            usage,
        });
    }

    // Back-fill tool_name on tool_result blocks
    let mut tool_name_by_id: HashMap<String, String> = HashMap::new();
    for turn in &turns {
        for block in &turn.content {
            if block.block_type == "tool_use" {
                if let (Some(id), Some(name)) = (&block.tool_use_id, &block.tool_name) {
                    tool_name_by_id.insert(id.clone(), name.clone());
                }
            }
        }
    }
    for turn in &mut turns {
        for block in &mut turn.content {
            if block.block_type == "tool_result" && block.tool_name.is_none() {
                if let Some(id) = &block.tool_use_id {
                    block.tool_name = tool_name_by_id.get(id).cloned();
                }
            }
        }
    }

    turns.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    turns
}

fn is_jsonl(path: &Path) -> bool {
    path.extension().map(|e| e == "jsonl").unwrap_or(false)
}

pub fn load_path(path: &Path) -> Vec<Session> {
    if path.is_file() {
        if !is_jsonl(path) {
            return vec![];
        }
        let session_id = path.file_stem().unwrap().to_string_lossy().to_string();
        let turns = parse_session_file(path);
        return if turns.is_empty() {
            vec![]
        } else {
            vec![Session {
                session_id,
                project: "default".to_string(),
                turns,
            }]
        };
    }

    if !path.is_dir() {
        return vec![];
    }

    let mut sessions = Vec::new();

    let has_direct_jsonl = fs::read_dir(path)
        .map(|d| d.filter_map(|e| e.ok()).any(|e| is_jsonl(&e.path())))
        .unwrap_or(false);

    if has_direct_jsonl {
        let project = path.file_name().unwrap().to_string_lossy().to_string();
        let mut entries: Vec<_> = fs::read_dir(path)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let p = entry.path();
            if !is_jsonl(&p) {
                continue;
            }
            let session_id = p.file_stem().unwrap().to_string_lossy().to_string();
            let full_id = format!("{}/{}", project, session_id);
            let turns = parse_session_file(&p);
            if !turns.is_empty() {
                sessions.push(Session {
                    session_id: full_id,
                    project: project.clone(),
                    turns,
                });
            }
        }
    } else {
        let mut project_dirs: Vec<_> = fs::read_dir(path)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();
        project_dirs.sort_by_key(|e| e.path());

        for project_dir in project_dirs {
            let pd = project_dir.path();
            let project = pd.file_name().unwrap().to_string_lossy().to_string();
            let mut entries: Vec<_> = fs::read_dir(&pd)
                .unwrap()
                .filter_map(|e| e.ok())
                .collect();
            entries.sort_by_key(|e| e.path());
            for entry in entries {
                let p = entry.path();
                if !is_jsonl(&p) {
                    continue;
                }
                let session_id = p.file_stem().unwrap().to_string_lossy().to_string();
                let full_id = format!("{}/{}", project, session_id);
                let turns = parse_session_file(&p);
                if !turns.is_empty() {
                    sessions.push(Session {
                        session_id: full_id,
                        project: project.clone(),
                        turns,
                    });
                }
            }
        }
    }

    sessions
}
