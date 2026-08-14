//! Turns the org's active plugins into the files an assistant will read.
//!
//! Pure: no filesystem, no clock, no environment. That is what lets the whole
//! layout be asserted from literal JSON in tests, the same way `catalog_model`
//! covers the model catalog.
//!
//! Only `plugin.active` decides whether a plugin is materialized. The server
//! already folded targeting, `enforced`/`optional` and self-install state into
//! that one flag, so there is no mode logic here to drift from the API's.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::api::{Plugin, PluginHook, PluginMcpServer, PluginSkill, PluginSubagent};

use super::delivery::{delivery, Delivery, Kind, Layout, Target};

/// A file to write, relative to the target's root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    pub path: PathBuf,
    pub contents: String,
}

/// What happened to one (plugin, kind) pair. Drives the launch summary and
/// `edgee plugins list --verbose`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub plugin: String,
    pub kind: Kind,
    pub count: usize,
    pub delivered: bool,
    /// Empty when delivered.
    pub reason: &'static str,
}

/// Everything one agent gets, plus the honest account of what it did not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Sorted by path and deduplicated, so the same input always plans the same
    /// bytes and a rebuild is comparable.
    pub files: Vec<PlannedFile>,
    pub report: Vec<Outcome>,
    /// Directory names, one per materialized plugin. `claude` turns these into
    /// `--plugin-dir` arguments.
    pub plugin_dirs: Vec<String>,
}

impl Plan {
    /// Kinds that had components but could not be delivered, deduplicated and
    /// in a stable order — for the single line the launch path prints.
    pub fn undelivered_kinds(&self) -> Vec<Kind> {
        let mut kinds: Vec<Kind> = self
            .report
            .iter()
            .filter(|o| !o.delivered && o.count > 0)
            .map(|o| o.kind)
            .collect();
        kinds.sort_by_key(|k| k.order());
        kinds.dedup();
        kinds
    }
}

/// Plans the whole Edgee-owned tree for one agent.
pub fn plan_tree(plugins: &[Plugin], target: Target) -> Plan {
    let mut plan = Plan::default();

    for plugin in plugins.iter().filter(|p| p.active) {
        let dir = plugin_dir_name(plugin);
        if dir.is_empty() {
            continue;
        }

        let mut wrote_anything = false;

        for (kind, count) in [
            (Kind::Skills, plugin.skills.len()),
            (Kind::Subagents, plugin.subagents.len()),
            (Kind::Hooks, plugin.hooks.len()),
            (Kind::McpServers, plugin.mcp_servers.len()),
        ] {
            if count == 0 {
                continue;
            }
            match delivery(target, kind) {
                Delivery::Delivered { .. } => {
                    let before = plan.files.len();
                    emit(&mut plan.files, plugin, &dir, kind, target);
                    wrote_anything |= plan.files.len() > before;
                    plan.report.push(Outcome {
                        plugin: plugin.name.clone(),
                        kind,
                        count,
                        delivered: true,
                        reason: "",
                    });
                }
                Delivery::Unsupported { reason } => plan.report.push(Outcome {
                    plugin: plugin.name.clone(),
                    kind,
                    count,
                    delivered: false,
                    reason,
                }),
            }
        }

        // A bundle is only a *plugin* because of its manifest. Emit it only when
        // the directory actually has content — a bare manifest would register an
        // empty plugin.
        if target.layout() == Layout::Bundle && wrote_anything {
            plan.files.push(PlannedFile {
                path: PathBuf::from(&dir)
                    .join(".claude-plugin")
                    .join("plugin.json"),
                contents: manifest_json(plugin),
            });
        }

        if wrote_anything {
            plan.plugin_dirs.push(dir);
        }
    }

    plan.files.sort_by(|a, b| a.path.cmp(&b.path));
    plan.files.dedup_by(|a, b| a.path == b.path);
    plan.plugin_dirs.sort();
    plan.plugin_dirs.dedup();
    plan
}

fn emit(out: &mut Vec<PlannedFile>, plugin: &Plugin, dir: &str, kind: Kind, target: Target) {
    match kind {
        Kind::Skills => {
            for skill in &plugin.skills {
                if skill.name.is_empty() {
                    continue;
                }
                out.push(PlannedFile {
                    path: skill_path(dir, &skill.name, target),
                    contents: skill_md(skill),
                });
            }
        }
        Kind::Subagents => {
            for sub in &plugin.subagents {
                if sub.name.is_empty() {
                    continue;
                }
                // Flat targets that take subagents do so through their config
                // rather than a file tree, so only bundles emit these.
                if target.layout() != Layout::Bundle {
                    continue;
                }
                out.push(PlannedFile {
                    path: PathBuf::from(dir)
                        .join("agents")
                        .join(format!("{}.md", sub.name)),
                    contents: subagent_md(sub),
                });
            }
        }
        Kind::Hooks => {
            if let Some(contents) = hooks_json(&plugin.hooks) {
                out.push(PlannedFile {
                    path: PathBuf::from(dir).join("hooks").join("hooks.json"),
                    contents,
                });
            }
        }
        Kind::McpServers => {
            if let Some(contents) = mcp_json(&plugin.mcp_servers) {
                out.push(PlannedFile {
                    path: PathBuf::from(dir).join(".mcp.json"),
                    contents,
                });
            }
        }
    }
}

/// Where one skill lands for a target.
///
/// Claude nests skills inside the plugin bundle, so the plugin name already
/// separates them. Every other target shares a single flat skills root, so the
/// name is namespaced — otherwise two plugins each shipping a `review` skill
/// would silently overwrite one another.
fn skill_path(dir: &str, skill_name: &str, target: Target) -> PathBuf {
    match target.layout() {
        Layout::Bundle => PathBuf::from(dir)
            .join("skills")
            .join(skill_name)
            .join("SKILL.md"),
        Layout::Flat => PathBuf::from("skills")
            .join(format!("{dir}__{skill_name}"))
            .join("SKILL.md"),
    }
}

/// The directory a plugin materializes into. Isolated so switching to a
/// namespaced form (`edgee-<name>`) later is a one-line change — see the
/// collision risk with a user's own installed plugin of the same name.
pub fn plugin_dir_name(plugin: &Plugin) -> String {
    plugin.name.clone()
}

fn manifest_json(plugin: &Plugin) -> String {
    let value = serde_json::json!({
        "name": plugin.name,
        "description": plugin.description,
        "version": plugin.version,
        "author": { "name": "Edgee" },
    });
    pretty(&value)
}

/// SKILL.md frontmatter has no field for "when to use", so it is folded into the
/// description — which is the text the assistant matches a task against anyway.
fn skill_md(skill: &PluginSkill) -> String {
    let mut description = skill.description.clone();
    if !skill.when_to_use.trim().is_empty() {
        if !description.is_empty() && !description.ends_with(['.', '!', '?']) {
            description.push('.');
        }
        if !description.is_empty() {
            description.push(' ');
        }
        description.push_str("Use when: ");
        description.push_str(skill.when_to_use.trim());
    }

    format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}\n",
        skill.name,
        yaml_quote(&description),
        skill.body.trim_end()
    )
}

fn subagent_md(sub: &PluginSubagent) -> String {
    let mut front = format!(
        "---\nname: {}\ndescription: {}\n",
        sub.name,
        yaml_quote(&sub.description)
    );
    if !sub.model.trim().is_empty() && sub.model != "inherit" {
        front.push_str(&format!("model: {}\n", sub.model.trim()));
    }
    if !sub.allowed_tools.is_empty() {
        front.push_str(&format!("tools: {}\n", sub.allowed_tools.join(", ")));
    }
    front.push_str("---\n\n");
    front.push_str(sub.prompt.trim_end());
    front.push('\n');
    front
}

/// The API stores a flat hook list; `hooks.json` groups by event, then by
/// matcher within the event.
fn hooks_json(hooks: &[PluginHook]) -> Option<String> {
    // BTreeMap on both levels so the output is deterministic — a rebuild must
    // not churn the file just because a map iterated differently.
    let mut by_event: BTreeMap<&str, BTreeMap<&str, Vec<serde_json::Value>>> = BTreeMap::new();

    for hook in hooks {
        if hook.event.trim().is_empty() || hook.command.trim().is_empty() {
            continue;
        }
        let mut entry = serde_json::Map::new();
        entry.insert("type".into(), "command".into());
        entry.insert("command".into(), hook.command.clone().into());
        // Zero means "the assistant's own default", which is expressed by
        // leaving the key out rather than sending 0.
        if hook.timeout > 0 {
            entry.insert("timeout".into(), hook.timeout.into());
        }
        by_event
            .entry(&hook.event)
            .or_default()
            .entry(hook.matcher.trim())
            .or_default()
            .push(serde_json::Value::Object(entry));
    }

    if by_event.is_empty() {
        return None;
    }

    let mut events = serde_json::Map::new();
    for (event, by_matcher) in by_event {
        let groups: Vec<serde_json::Value> = by_matcher
            .into_iter()
            .map(|(matcher, entries)| {
                let mut group = serde_json::Map::new();
                // A matcher only means something on tool events; the server
                // already blanks it elsewhere, so an empty one is omitted rather
                // than written as a filter that matches nothing.
                if !matcher.is_empty() {
                    group.insert("matcher".into(), matcher.into());
                }
                group.insert("hooks".into(), serde_json::Value::Array(entries));
                serde_json::Value::Object(group)
            })
            .collect();
        events.insert(event.to_string(), serde_json::Value::Array(groups));
    }

    Some(pretty(&serde_json::json!({ "hooks": events })))
}

fn mcp_json(servers: &[PluginMcpServer]) -> Option<String> {
    let mut map = serde_json::Map::new();

    for server in servers {
        if server.name.is_empty() || server.url.is_empty() {
            continue;
        }
        // Only http exists: a stdio server would need a local binary the plugin
        // never installs, so the API refuses to store one.
        if server.transport != "http" {
            continue;
        }
        map.insert(
            server.name.clone(),
            serde_json::json!({
                "type": "http",
                "url": server.url,
                "headers": server.headers,
            }),
        );
    }

    if map.is_empty() {
        return None;
    }
    Some(pretty(&serde_json::json!({ "mcpServers": map })))
}

/// A double-quoted YAML scalar.
///
/// Descriptions are free text up to 280 characters and routinely contain `:` and
/// `"`. An unquoted or naively quoted value produces frontmatter the assistant
/// fails to parse, and it drops the skill *silently* — so this is load-bearing.
fn yaml_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            // A literal newline would end the scalar.
            '\n' | '\r' => out.push(' '),
            '\t' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn pretty(value: &serde_json::Value) -> String {
    let mut s = serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string());
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(name: &str, active: bool) -> Plugin {
        Plugin {
            id: format!("plg_{name}"),
            name: name.to_string(),
            version: "1.0.0".to_string(),
            description: "A plugin under test".to_string(),
            active,
            ..Default::default()
        }
    }

    fn skill(name: &str) -> PluginSkill {
        PluginSkill {
            name: name.to_string(),
            description: "Does a thing".to_string(),
            body: "Do the thing.".to_string(),
            ..Default::default()
        }
    }

    fn paths(plan: &Plan) -> Vec<String> {
        plan.files
            .iter()
            .map(|f| f.path.to_string_lossy().replace('\\', "/"))
            .collect()
    }

    /// Returns owned content so callers can pass a freshly-built plan inline.
    fn find(plan: &Plan, suffix: &str) -> String {
        plan.files
            .iter()
            .find(|f| {
                f.path
                    .to_string_lossy()
                    .replace('\\', "/")
                    .ends_with(suffix)
            })
            .map(|f| f.contents.clone())
            .unwrap_or_else(|| panic!("no planned file ending in {suffix}"))
    }

    /// `active` is the server's verdict on targeting + mode + install state.
    /// Nothing else may gate materialization, or the CLI and API can disagree.
    #[test]
    fn only_active_plugins_are_materialized() {
        let mut assigned = plugin("assigned-one", true);
        assigned.skills = vec![skill("a")];
        // An admin receives plugins aimed at other people too; `active` is false
        // for those, and that is the only thing keeping them off this machine.
        let mut someone_elses = plugin("someone-elses", false);
        someone_elses.skills = vec![skill("b")];

        let plan = plan_tree(&[assigned, someone_elses], Target::Claude);

        assert_eq!(plan.plugin_dirs, vec!["assigned-one"]);
        assert!(paths(&plan).iter().all(|p| p.starts_with("assigned-one/")));
    }

    #[test]
    fn claude_tree_is_a_plugin_bundle() {
        let mut p = plugin("house", true);
        p.skills = vec![skill("commit-style")];
        p.subagents = vec![PluginSubagent {
            name: "reviewer".into(),
            description: "Reviews a diff".into(),
            prompt: "Review it.".into(),
            ..Default::default()
        }];
        p.hooks = vec![PluginHook {
            name: "fmt".into(),
            event: "PostToolUse".into(),
            matcher: "Write".into(),
            command: "./fmt.sh".into(),
            ..Default::default()
        }];
        p.mcp_servers = vec![PluginMcpServer {
            name: "docs".into(),
            transport: "http".into(),
            url: "https://example.com/mcp".into(),
            ..Default::default()
        }];

        let plan = plan_tree(&[p], Target::Claude);

        assert_eq!(
            paths(&plan),
            vec![
                "house/.claude-plugin/plugin.json",
                "house/.mcp.json",
                "house/agents/reviewer.md",
                "house/hooks/hooks.json",
                "house/skills/commit-style/SKILL.md",
            ]
        );
        assert_eq!(plan.plugin_dirs, vec!["house"]);
    }

    /// CodeBuddy consumes Claude's bundle format, so the two trees must be
    /// identical — if they ever diverge, one of them is wrong.
    #[test]
    fn codebuddy_gets_the_same_bundle_as_claude() {
        let mut p = plugin("house", true);
        p.skills = vec![skill("commit-style")];
        p.subagents = vec![PluginSubagent {
            name: "reviewer".into(),
            description: "Reviews a diff".into(),
            prompt: "Review it.".into(),
            ..Default::default()
        }];

        let claude = plan_tree(&[p.clone()], Target::Claude);
        let codebuddy = plan_tree(&[p], Target::Codebuddy);

        assert_eq!(claude.files, codebuddy.files);
        assert_eq!(claude.plugin_dirs, codebuddy.plugin_dirs);
        assert!(!claude.files.is_empty());
    }

    /// A manifest with no components would register an empty plugin.
    #[test]
    fn a_plugin_with_nothing_deliverable_produces_no_files() {
        let mut p = plugin("hooks-only", true);
        p.hooks = vec![PluginHook {
            name: "h".into(),
            event: "Stop".into(),
            command: "./x.sh".into(),
            ..Default::default()
        }];

        // OpenCode has no hooks surface at all, so this plugin delivers nothing.
        let plan = plan_tree(&[p], Target::Opencode);

        assert!(plan.files.is_empty());
        assert!(plan.plugin_dirs.is_empty());
        assert_eq!(plan.undelivered_kinds(), vec![Kind::Hooks]);
    }

    #[test]
    fn skill_frontmatter_folds_when_to_use_into_description() {
        let mut p = plugin("house", true);
        p.skills = vec![PluginSkill {
            name: "commit-style".into(),
            description: "How this team writes commits".into(),
            when_to_use: "Writing or amending a commit".into(),
            body: "Imperative mood.".into(),
        }];

        let md = find(&plan_tree(&[p], Target::Claude), "SKILL.md");

        assert!(md.contains("name: commit-style"));
        assert!(md.contains("Use when: Writing or amending a commit"));
        assert!(md.trim_end().ends_with("Imperative mood."));
    }

    /// Broken frontmatter makes the assistant drop the skill without saying so,
    /// which is why quoting is tested rather than assumed.
    #[test]
    fn skill_description_is_yaml_escaped() {
        let mut p = plugin("house", true);
        p.skills = vec![PluginSkill {
            name: "tricky".into(),
            description: "Handles \"quotes\": and\nnewlines".into(),
            body: "Body.".into(),
            ..Default::default()
        }];

        let md = find(&plan_tree(&[p], Target::Claude), "SKILL.md");
        let line = md
            .lines()
            .find(|l| l.starts_with("description:"))
            .expect("description line");

        assert!(line.contains("\\\"quotes\\\""));
        assert_eq!(md.matches('\n').count(), md.lines().count());
        assert!(!line.contains('\n'));
    }

    #[test]
    fn subagent_frontmatter_carries_model_and_tools() {
        let mut p = plugin("house", true);
        p.subagents = vec![PluginSubagent {
            name: "reviewer".into(),
            description: "Reviews".into(),
            model: "sonnet".into(),
            prompt: "Review.".into(),
            allowed_tools: vec!["Read".into(), "Grep".into()],
        }];

        let md = find(&plan_tree(&[p], Target::Claude), "agents/reviewer.md");

        assert!(md.contains("model: sonnet"));
        assert!(md.contains("tools: Read, Grep"));

        // `inherit` is the absence of a choice, not a model name.
        let mut q = plugin("other", true);
        q.subagents = vec![PluginSubagent {
            name: "plain".into(),
            description: "Plain".into(),
            model: "inherit".into(),
            prompt: "Go.".into(),
            ..Default::default()
        }];
        let md = find(&plan_tree(&[q], Target::Claude), "agents/plain.md");
        assert!(!md.contains("model:"));
    }

    #[test]
    fn hooks_are_grouped_by_event_then_matcher() {
        let mut p = plugin("house", true);
        p.hooks = vec![
            PluginHook {
                name: "a".into(),
                event: "PostToolUse".into(),
                matcher: "Write".into(),
                command: "./a.sh".into(),
                timeout: 30,
            },
            PluginHook {
                name: "b".into(),
                event: "PostToolUse".into(),
                matcher: "Write".into(),
                command: "./b.sh".into(),
                timeout: 0,
            },
            PluginHook {
                name: "c".into(),
                event: "Stop".into(),
                matcher: String::new(),
                command: "./c.sh".into(),
                timeout: 0,
            },
        ];

        let json: serde_json::Value =
            serde_json::from_str(&find(&plan_tree(&[p], Target::Claude), "hooks.json")).unwrap();
        let hooks = &json["hooks"];

        // Same event + same matcher collapse into one group with two commands.
        assert_eq!(hooks["PostToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(hooks["PostToolUse"][0]["matcher"], "Write");
        assert_eq!(
            hooks["PostToolUse"][0]["hooks"].as_array().unwrap().len(),
            2
        );

        assert_eq!(hooks["PostToolUse"][0]["hooks"][0]["timeout"], 30);
        assert!(hooks["PostToolUse"][0]["hooks"][1].get("timeout").is_none());

        // A non-tool event carries no matcher key at all.
        assert!(hooks["Stop"][0].get("matcher").is_none());
    }

    #[test]
    fn mcp_servers_emit_http_entries_and_skip_the_rest() {
        let mut p = plugin("house", true);
        p.mcp_servers = vec![
            PluginMcpServer {
                name: "remote".into(),
                transport: "http".into(),
                url: "https://example.com/mcp".into(),
                ..Default::default()
            },
            // No url — must not emit a half-written entry.
            PluginMcpServer {
                name: "broken".into(),
                transport: "http".into(),
                ..Default::default()
            },
            // A withdrawn transport, as an old cache entry could still carry.
            PluginMcpServer {
                name: "local".into(),
                transport: "stdio".into(),
                url: "https://example.com/mcp".into(),
                ..Default::default()
            },
        ];

        let json: serde_json::Value =
            serde_json::from_str(&find(&plan_tree(&[p], Target::Claude), ".mcp.json")).unwrap();
        let servers = &json["mcpServers"];

        assert_eq!(servers["remote"]["type"], "http");
        assert_eq!(servers["remote"]["url"], "https://example.com/mcp");
        assert!(servers.get("broken").is_none());
        assert!(servers.get("local").is_none());
    }

    /// Two plugins each shipping a `review` skill must not overwrite each other
    /// on targets that share one flat skills root. Asserted on the path helper
    /// directly, because no flat target delivers skills yet — the layout is
    /// settled even though the delivery is not.
    #[test]
    fn flat_targets_namespace_skill_directories() {
        let a = skill_path("plugin-a", "review", Target::Opencode);
        let b = skill_path("plugin-b", "review", Target::Opencode);

        assert_ne!(a, b);
        assert_eq!(
            a.to_string_lossy().replace('\\', "/"),
            "skills/plugin-a__review/SKILL.md"
        );

        // Claude keeps them apart via the bundle directory instead.
        assert_eq!(
            skill_path("plugin-a", "review", Target::Claude)
                .to_string_lossy()
                .replace('\\', "/"),
            "plugin-a/skills/review/SKILL.md"
        );
    }

    #[test]
    fn an_empty_plugin_set_plans_no_files() {
        let plan = plan_tree(&[], Target::Claude);
        assert!(plan.files.is_empty());
        assert!(plan.report.is_empty());
    }

    /// A rebuild compares against what is on disk, so identical input must plan
    /// identical bytes regardless of the order it arrived in.
    #[test]
    fn plan_is_deterministic_and_order_independent() {
        let mut a = plugin("alpha", true);
        a.skills = vec![skill("one")];
        let mut b = plugin("beta", true);
        b.skills = vec![skill("two")];

        let forward = plan_tree(&[a.clone(), b.clone()], Target::Claude);
        let reversed = plan_tree(&[b, a], Target::Claude);

        assert_eq!(forward.files, reversed.files);
        assert_eq!(forward.plugin_dirs, reversed.plugin_dirs);
    }

    /// One gap shared by two plugins is reported once, not twice — the launch
    /// line has room for a summary, not a per-plugin dump.
    #[test]
    fn undelivered_kinds_are_deduplicated_across_plugins() {
        let hook = PluginHook {
            name: "h".into(),
            event: "Stop".into(),
            command: "./x.sh".into(),
            ..Default::default()
        };
        let mut a = plugin("a", true);
        a.hooks = vec![hook.clone()];
        let mut b = plugin("b", true);
        b.hooks = vec![hook];
        b.skills = vec![skill("s")];

        let plan = plan_tree(&[a, b], Target::Opencode);

        // Skills land; hooks cannot, for either plugin — reported once, not twice.
        assert_eq!(paths(&plan), vec!["skills/b__s/SKILL.md"]);
        assert_eq!(plan.undelivered_kinds(), vec![Kind::Hooks]);
    }
}
