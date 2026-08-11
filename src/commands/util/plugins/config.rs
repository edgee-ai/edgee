//! Config fragments for agents configured by a document rather than a bundle.
//!
//! Claude and CodeBuddy take a plugin *directory*. Crush and OpenCode instead
//! read one config file, which the CLI already generates for them — it clones
//! the user's real config, splices Edgee's provider in, and redirects the agent
//! at the copy. These builders produce the additional fragments to splice in, so
//! plugin delivery rides the same mechanism and the user's config is never
//! touched.
//!
//! Every shape here was taken from the vendor's own JSON schema
//! (`charm.land/crush.json`, `opencode.ai/config.json`), not inferred.
//!
//! Names are namespaced `<plugin>__<name>`. These maps are flat and shared by
//! every plugin, so two plugins each shipping a `docs` MCP server would
//! otherwise silently overwrite one another.

use serde_json::{json, Map, Value};

use crate::api::{Plugin, PluginMcpServer};

/// Only plugins in force for this user contribute anything.
fn active(plugins: &[Plugin]) -> impl Iterator<Item = &Plugin> {
    plugins.iter().filter(|p| p.active)
}

fn qualified(plugin: &Plugin, name: &str) -> String {
    format!("{}__{}", plugin.name, name)
}

/// Crush `mcp`: `{ "<name>": { "type": "stdio"|"http", ... } }`.
pub fn crush_mcp(plugins: &[Plugin]) -> Option<Value> {
    let mut out = Map::new();
    for plugin in active(plugins) {
        for server in &plugin.mcp_servers {
            let Some(entry) = crush_mcp_entry(server) else {
                continue;
            };
            out.insert(qualified(plugin, &server.name), entry);
        }
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

fn crush_mcp_entry(server: &PluginMcpServer) -> Option<Value> {
    if server.name.is_empty() {
        return None;
    }
    match server.transport.as_str() {
        "stdio" if !server.command.is_empty() => Some(json!({
            "type": "stdio",
            "command": server.command,
            "args": server.args,
            "env": server.env,
        })),
        "http" if !server.url.is_empty() => Some(json!({
            "type": "http",
            "url": server.url,
            "headers": server.headers,
        })),
        _ => None,
    }
}

/// Crush `hooks`: `{ "<Event>": [ { name, matcher, command, timeout } ] }`.
///
/// Flatter than Claude's format — one array per event, no matcher grouping — so
/// hooks from different plugins simply concatenate.
pub fn crush_hooks(plugins: &[Plugin]) -> Option<Value> {
    let mut by_event: std::collections::BTreeMap<String, Vec<Value>> = Default::default();

    for plugin in active(plugins) {
        for hook in &plugin.hooks {
            if hook.event.trim().is_empty() || hook.command.trim().is_empty() {
                continue;
            }
            let mut entry = Map::new();
            entry.insert("name".into(), qualified(plugin, &hook.name).into());
            entry.insert("command".into(), hook.command.clone().into());
            // Crush matches the tool name by regex; an empty matcher already
            // means "all tools", so the key is omitted rather than sent empty.
            if !hook.matcher.trim().is_empty() {
                entry.insert("matcher".into(), hook.matcher.clone().into());
            }
            if hook.timeout > 0 {
                entry.insert("timeout".into(), hook.timeout.into());
            }
            by_event
                .entry(hook.event.clone())
                .or_default()
                .push(Value::Object(entry));
        }
    }

    (!by_event.is_empty()).then(|| {
        Value::Object(
            by_event
                .into_iter()
                .map(|(k, v)| (k, Value::Array(v)))
                .collect(),
        )
    })
}

/// OpenCode `mcp`: `{ "<name>": { "type": "local"|"remote", ... } }`.
///
/// Note the differences from every other agent, all schema-confirmed: the
/// discriminant is `local`/`remote` rather than `stdio`/`http`, `command` is a
/// single array holding the program *and* its arguments, and the environment key
/// is `environment`, not `env`.
pub fn opencode_mcp(plugins: &[Plugin]) -> Option<Value> {
    let mut out = Map::new();
    for plugin in active(plugins) {
        for server in &plugin.mcp_servers {
            if server.name.is_empty() {
                continue;
            }
            let entry = match server.transport.as_str() {
                "stdio" if !server.command.is_empty() => {
                    let mut command = vec![server.command.clone()];
                    command.extend(server.args.iter().cloned());
                    json!({
                        "type": "local",
                        "command": command,
                        "environment": server.env,
                        "enabled": true,
                    })
                }
                "http" if !server.url.is_empty() => json!({
                    "type": "remote",
                    "url": server.url,
                    "headers": server.headers,
                    "enabled": true,
                }),
                _ => continue,
            };
            out.insert(qualified(plugin, &server.name), entry);
        }
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

/// OpenCode `agent`: `{ "<name>": { description, prompt, mode: "subagent" } }`.
///
/// Subagents are configuration here, not files, so they never reach the skills
/// tree. `allowed_tools` is deliberately **not** mapped: OpenCode's `tools` field
/// is marked deprecated in its own schema, and its tool names differ from Claude's
/// (`read` vs `Read`). A wrong mapping would silently *widen* what a subagent may
/// do, which is the worst failure available here — so the restriction is dropped
/// and reported rather than guessed.
pub fn opencode_agents(plugins: &[Plugin]) -> Option<Value> {
    let mut out = Map::new();
    for plugin in active(plugins) {
        for sub in &plugin.subagents {
            if sub.name.is_empty() {
                continue;
            }
            let mut entry = Map::new();
            entry.insert("description".into(), sub.description.clone().into());
            entry.insert("prompt".into(), sub.prompt.clone().into());
            entry.insert("mode".into(), "subagent".into());
            if !sub.model.trim().is_empty() && sub.model != "inherit" {
                entry.insert("model".into(), sub.model.clone().into());
            }
            out.insert(qualified(plugin, &sub.name), Value::Object(entry));
        }
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

/// Codex `-c mcp_servers.<name>.<field>=<toml>` overrides.
///
/// Codex has no writable config in the mirror — `config.toml` is symlinked to
/// the user's own — so MCP servers arrive as command-line overrides instead.
/// That is already how `codex.rs` configures the Edgee provider, so the
/// mechanism is proven; only the keys are new.
pub fn codex_mcp_args(plugins: &[Plugin]) -> Vec<String> {
    let mut args = Vec::new();
    for plugin in active(plugins) {
        for server in &plugin.mcp_servers {
            if server.name.is_empty() {
                continue;
            }
            let key = format!("mcp_servers.{}", qualified(plugin, &server.name));
            match server.transport.as_str() {
                "stdio" if !server.command.is_empty() => {
                    args.push(format!("{key}.command={}", toml_string(&server.command)));
                    if !server.args.is_empty() {
                        args.push(format!("{key}.args={}", toml_array(&server.args)));
                    }
                    if !server.env.is_empty() {
                        args.push(format!("{key}.env={}", toml_table(&server.env)));
                    }
                }
                "http" if !server.url.is_empty() => {
                    args.push(format!("{key}.url={}", toml_string(&server.url)));
                    if !server.headers.is_empty() {
                        args.push(format!(
                            "{key}.http_headers={}",
                            toml_table(&server.headers)
                        ));
                    }
                }
                _ => continue,
            }
        }
    }
    args
}

/// A TOML basic string. Every control character has to be escaped or the value
/// silently fails to parse and Codex falls back to treating it as a literal.
fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04X}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn toml_array(values: &[String]) -> String {
    let parts: Vec<String> = values.iter().map(|v| toml_string(v)).collect();
    format!("[{}]", parts.join(", "))
}

fn toml_table(map: &std::collections::HashMap<String, String>) -> String {
    // Sorted so repeated launches produce identical arguments.
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    let parts: Vec<String> = keys
        .iter()
        .map(|k| format!("{}={}", toml_string(k), toml_string(&map[*k])))
        .collect();
    format!("{{{}}}", parts.join(", "))
}

/// Merges `value` into `config[key]`, preserving whatever the user already had
/// there. Only keys we introduce are touched, so an existing `mcp` server or
/// `hooks` event of theirs survives untouched.
pub fn merge_object(config: &mut Value, key: &str, value: Value) {
    let Some(obj) = config.as_object_mut() else {
        return;
    };
    let Value::Object(incoming) = value else {
        return;
    };
    match obj.get_mut(key).and_then(Value::as_object_mut) {
        Some(existing) => existing.extend(incoming),
        None => {
            obj.insert(key.to_string(), Value::Object(incoming));
        }
    }
}

/// Appends `path` to a nested string array such as `options.skills_paths` or
/// `skills.paths`, creating the intermediate object when absent and never
/// duplicating an entry.
pub fn push_path(config: &mut Value, parent: &str, key: &str, path: &str) {
    let Some(obj) = config.as_object_mut() else {
        return;
    };
    let parent_obj = obj
        .entry(parent.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(parent_map) = parent_obj.as_object_mut() else {
        return;
    };
    let list = parent_map
        .entry(key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(array) = list.as_array_mut() else {
        return;
    };
    if !array.iter().any(|v| v.as_str() == Some(path)) {
        array.push(Value::String(path.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{PluginHook, PluginSubagent};

    fn plugin(name: &str, active: bool) -> Plugin {
        Plugin {
            id: format!("plg_{name}"),
            name: name.to_string(),
            active,
            ..Default::default()
        }
    }

    fn stdio(name: &str) -> PluginMcpServer {
        PluginMcpServer {
            name: name.to_string(),
            transport: "stdio".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "docs".into()],
            env: [("TOKEN".to_string(), "t".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        }
    }

    fn http(name: &str) -> PluginMcpServer {
        PluginMcpServer {
            name: name.to_string(),
            transport: "http".into(),
            url: "https://example.com/mcp".into(),
            ..Default::default()
        }
    }

    #[test]
    fn inactive_plugins_contribute_nothing() {
        let mut p = plugin("off", false);
        p.mcp_servers = vec![stdio("docs")];
        p.subagents = vec![PluginSubagent {
            name: "r".into(),
            prompt: "go".into(),
            ..Default::default()
        }];

        assert!(crush_mcp(&[p.clone()]).is_none());
        assert!(opencode_mcp(&[p.clone()]).is_none());
        assert!(opencode_agents(&[p]).is_none());
    }

    #[test]
    fn crush_mcp_uses_stdio_and_http_discriminants() {
        let mut p = plugin("house", true);
        p.mcp_servers = vec![stdio("local"), http("remote")];

        let v = crush_mcp(&[p]).unwrap();

        assert_eq!(v["house__local"]["type"], "stdio");
        assert_eq!(v["house__local"]["command"], "npx");
        assert_eq!(v["house__local"]["args"][1], "docs");
        assert_eq!(v["house__remote"]["type"], "http");
        assert!(v["house__remote"].get("command").is_none());
    }

    /// OpenCode's schema differs on all three points, so they are asserted.
    #[test]
    fn opencode_mcp_folds_args_into_command_and_renames_env() {
        let mut p = plugin("house", true);
        p.mcp_servers = vec![stdio("local"), http("remote")];

        let v = opencode_mcp(&[p]).unwrap();

        assert_eq!(v["house__local"]["type"], "local");
        // command carries the program AND its arguments
        assert_eq!(v["house__local"]["command"][0], "npx");
        assert_eq!(v["house__local"]["command"][2], "docs");
        // `environment`, not `env`
        assert_eq!(v["house__local"]["environment"]["TOKEN"], "t");
        assert!(v["house__local"].get("env").is_none());
        assert_eq!(v["house__remote"]["type"], "remote");
    }

    /// Two plugins shipping the same server name must not collide in a flat map.
    #[test]
    fn names_are_namespaced_per_plugin() {
        let mut a = plugin("alpha", true);
        a.mcp_servers = vec![http("docs")];
        let mut b = plugin("beta", true);
        b.mcp_servers = vec![http("docs")];

        let v = crush_mcp(&[a, b]).unwrap();

        assert!(v.get("alpha__docs").is_some());
        assert!(v.get("beta__docs").is_some());
        assert_eq!(v.as_object().unwrap().len(), 2);
    }

    #[test]
    fn crush_hooks_group_by_event_and_omit_empty_fields() {
        let mut p = plugin("house", true);
        p.hooks = vec![
            PluginHook {
                name: "fmt".into(),
                event: "PreToolUse".into(),
                matcher: "Write".into(),
                command: "./fmt.sh".into(),
                timeout: 30,
            },
            PluginHook {
                name: "log".into(),
                event: "PreToolUse".into(),
                command: "./log.sh".into(),
                ..Default::default()
            },
        ];

        let v = crush_hooks(&[p]).unwrap();
        let events = v["PreToolUse"].as_array().unwrap();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["matcher"], "Write");
        assert_eq!(events[0]["timeout"], 30);
        // an empty matcher already means "all tools"
        assert!(events[1].get("matcher").is_none());
        assert!(events[1].get("timeout").is_none());
        assert_eq!(events[1]["name"], "house__log");
    }

    /// Tool restrictions are dropped rather than mapped onto a deprecated field
    /// with a different naming scheme — dropping is safe, widening is not.
    #[test]
    fn opencode_agents_omit_tool_restrictions() {
        let mut p = plugin("house", true);
        p.subagents = vec![PluginSubagent {
            name: "reviewer".into(),
            description: "Reviews".into(),
            prompt: "Review.".into(),
            model: "inherit".into(),
            allowed_tools: vec!["Read".into(), "Grep".into()],
        }];

        let v = opencode_agents(&[p]).unwrap();
        let agent = &v["house__reviewer"];

        assert_eq!(agent["mode"], "subagent");
        assert_eq!(agent["prompt"], "Review.");
        assert!(agent.get("tools").is_none());
        // `inherit` is the absence of a choice, not a model name
        assert!(agent.get("model").is_none());
    }

    #[test]
    fn codex_mcp_args_are_well_formed_toml_overrides() {
        let mut p = plugin("house", true);
        p.mcp_servers = vec![stdio("local"), http("remote")];

        let args = codex_mcp_args(&[p]);

        assert!(args.contains(&r#"mcp_servers.house__local.command="npx""#.to_string()));
        assert!(args.contains(&r#"mcp_servers.house__local.args=["-y", "docs"]"#.to_string()));
        assert!(args.contains(&r#"mcp_servers.house__local.env={"TOKEN"="t"}"#.to_string()));
        assert!(args
            .contains(&r#"mcp_servers.house__remote.url="https://example.com/mcp""#.to_string()));
        // stdio keys must not leak onto an http server
        assert!(!args
            .iter()
            .any(|a| a.starts_with("mcp_servers.house__remote.command")));
    }

    /// An unescaped quote or newline makes Codex treat the whole value as a
    /// literal string, so the override silently does the wrong thing.
    #[test]
    fn codex_toml_strings_escape_control_characters() {
        let mut p = plugin("house", true);
        p.mcp_servers = vec![PluginMcpServer {
            name: "odd".into(),
            transport: "stdio".into(),
            command: "say \"hi\"\nthere\u{1b}".into(),
            ..Default::default()
        }];

        let args = codex_mcp_args(&[p]);
        let command = args.iter().find(|a| a.contains(".command=")).unwrap();

        assert!(command.contains(r#"\""#));
        assert!(command.contains(r"\n"));
        assert!(command.contains(r"\u001B"));
        assert!(!command.contains('\n'));
    }

    #[test]
    fn merge_object_preserves_the_users_own_entries() {
        let mut config = json!({ "mcp": { "theirs": { "type": "http" } } });

        merge_object(&mut config, "mcp", json!({ "ours": { "type": "stdio" } }));

        assert_eq!(config["mcp"]["theirs"]["type"], "http");
        assert_eq!(config["mcp"]["ours"]["type"], "stdio");
    }

    #[test]
    fn merge_object_creates_the_key_when_absent() {
        let mut config = json!({});
        merge_object(&mut config, "hooks", json!({ "Stop": [] }));
        assert!(config["hooks"]["Stop"].is_array());
    }

    #[test]
    fn push_path_appends_without_duplicating_or_clobbering() {
        let mut config = json!({ "options": { "skills_paths": ["/theirs"] } });

        push_path(&mut config, "options", "skills_paths", "/ours");
        push_path(&mut config, "options", "skills_paths", "/ours");

        let paths = config["options"]["skills_paths"].as_array().unwrap();
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], "/theirs");
        assert_eq!(paths[1], "/ours");
    }

    #[test]
    fn push_path_creates_the_parent_when_absent() {
        let mut config = json!({});
        push_path(&mut config, "skills", "paths", "/ours");
        assert_eq!(config["skills"]["paths"][0], "/ours");
    }
}
