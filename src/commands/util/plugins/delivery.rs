//! Which component kinds each coding agent can actually be given.
//!
//! Encoded as data rather than scattered `if` statements so that adding a target
//! forces a decision for every kind, and so the reasons a kind is *not*
//! delivered can be shown to the user verbatim.
//!
//! The rule this table exists to enforce: Edgee materializes into a directory it
//! owns and points the agent at it. It never writes into a user-owned config
//! file, and never into a git working tree. A kind that cannot be redirected is
//! reported as not delivered — it is not worked around.

/// A coding agent that plugins can be delivered to. Cursor and Copilot are
/// absent on purpose: they are GUI apps reached through the relay, never spawned
/// by the CLI, so there is no launch to attach a plugin directory to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Claude,
    Codex,
    Opencode,
    Crush,
    Codebuddy,
    Pi,
    Omp,
}

impl Target {
    pub fn label(self) -> &'static str {
        match self {
            Target::Claude => "claude",
            Target::Codex => "codex",
            Target::Opencode => "opencode",
            Target::Crush => "crush",
            Target::Codebuddy => "codebuddy",
            Target::Pi => "pi",
            Target::Omp => "omp",
        }
    }

    /// Directory under the plugin root holding this target's materialized tree.
    pub fn dir(self) -> &'static str {
        self.label()
    }

    /// How this target wants its tree shaped. CodeBuddy implements Claude Code's
    /// plugin format down to the `${CLAUDE_PLUGIN_ROOT}` aliases, so the two
    /// share a bundle byte for byte; everything else reads a flat skills root.
    pub fn layout(self) -> Layout {
        match self {
            Target::Claude | Target::Codebuddy | Target::Omp => Layout::Bundle,
            Target::Codex | Target::Opencode | Target::Crush | Target::Pi => Layout::Flat,
        }
    }

    pub const ALL: [Target; 7] = [
        Target::Claude,
        Target::Codex,
        Target::Opencode,
        Target::Crush,
        Target::Codebuddy,
        Target::Pi,
        Target::Omp,
    ];
}

/// The two tree shapes an agent can want.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// A self-contained plugin directory per plugin, with a manifest — what
    /// `--plugin-dir` loads.
    Bundle,
    /// One shared skills root, so skill directory names must be namespaced.
    Flat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Skills,
    Subagents,
    Hooks,
    McpServers,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Skills => "skills",
            Kind::Subagents => "subagents",
            Kind::Hooks => "hooks",
            Kind::McpServers => "MCP servers",
        }
    }

    /// Stable display order, independent of enum declaration order.
    pub fn order(self) -> u8 {
        match self {
            Kind::Skills => 0,
            Kind::Subagents => 1,
            Kind::Hooks => 2,
            Kind::McpServers => 3,
        }
    }

    pub const ALL: [Kind; 4] = [Kind::Skills, Kind::Subagents, Kind::Hooks, Kind::McpServers];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Delivered {
        /// How the agent is pointed at it — shown by `edgee plugins list --verbose`.
        mechanism: &'static str,
    },
    Unsupported {
        reason: &'static str,
    },
}

impl Delivery {
    pub fn is_delivered(self) -> bool {
        matches!(self, Delivery::Delivered { .. })
    }
}

const PLUGIN_DIR: Delivery = Delivery::Delivered {
    mechanism: "claude --plugin-dir",
};

const PLUGIN_DIRS_ENV: Delivery = Delivery::Delivered {
    mechanism: "CODEBUDDY_PLUGIN_DIRS",
};

const OMP_PLUGIN_DIR: Delivery = Delivery::Delivered {
    mechanism: "omp --plugin-dir",
};

const fn config_key(mechanism: &'static str) -> Delivery {
    Delivery::Delivered { mechanism }
}

/// Not yet verified against the vendor's own configuration surface. Reported as
/// undelivered rather than guessed at, because injecting config an agent
/// silently ignores is worse than saying nothing was delivered.
const UNVERIFIED: Delivery = Delivery::Unsupported {
    reason: "not yet supported by the Edgee CLI for this assistant",
};

/// How `target` receives `kind`, if at all.
///
/// Claude Code is complete: `--plugin-dir` loads a directory for the session
/// only and carries all four kinds at once (verified against Claude Code
/// 2.1.224 — the flag enables the plugin, it does not merely register it, and
/// nothing is written to `~/.claude`).
///
/// CodeBuddy takes the same bundle via `CODEBUDDY_PLUGIN_DIRS`, documented as
/// the env-var form of `--plugin-dir`. OMP also consumes that bundle through its
/// repeatable `--plugin-dir` flag. Crush and OpenCode are configured by a
/// document instead, and the CLI already clones-and-redirects that document, so
/// the fragments in `config.rs` ride the same mechanism. Pi accepts skills by
/// path but has no declarative configuration for the other component kinds.
///
/// Remaining `UNVERIFIED` entries stay undelivered until their mechanism is
/// confirmed — injecting config an agent silently ignores is worse than
/// reporting that nothing was delivered.
pub fn delivery(target: Target, kind: Kind) -> Delivery {
    match target {
        Target::Claude => PLUGIN_DIR,

        // OpenCode's config schema has `skills.paths`, `agent` and `mcp`, but no
        // `hooks` key anywhere — its event extensibility is the JS `plugin`
        // array. That makes hooks structurally impossible here, not merely
        // unimplemented, so the reason says so rather than promising a later fix.
        Target::Opencode => match kind {
            Kind::Skills => config_key("opencode skills.paths"),
            Kind::Subagents => config_key("opencode agent config"),
            Kind::McpServers => config_key("opencode mcp config"),
            Kind::Hooks => Delivery::Unsupported {
                reason: "OpenCode has no hooks configuration; its extension point is JS plugins",
            },
        },

        // Codex has no way to define a named subagent with its own prompt and
        // tool allowlist — it has SubagentStart/Stop hook events but no subagent
        // definitions. That one is a real absence; the others await a spike.
        // Verified against Codex 0.144.6: with CODEX_HOME pointed at a symlink
        // mirror, a skill placed in the mirror's `skills/` loaded while the same
        // prompt against the real config root did not know it. MCP rides `-c`
        // overrides, which codex.rs already uses for the provider.
        Target::Codex => match kind {
            // The mirror is symlinks; Windows needs elevated privileges for
            // those, and copying a config root would duplicate the user's
            // credentials — which is not a trade worth making.
            #[cfg(unix)]
            Kind::Skills => config_key("CODEX_HOME mirror"),
            #[cfg(not(unix))]
            Kind::Skills => Delivery::Unsupported {
                reason: "the Codex config mirror needs symlink support",
            },
            Kind::McpServers => config_key("codex -c mcp_servers"),
            Kind::Subagents => Delivery::Unsupported {
                reason: "Codex has no configuration for user-defined subagents",
            },
            // Codex ships --dangerously-bypass-hook-trust, so an injected hook is
            // either refused or prompts. Left undelivered until that is tested:
            // a hook that silently vanishes is worse than one never promised.
            Kind::Hooks => UNVERIFIED,
        },

        // CodeBuddy documents CODEBUDDY_PLUGIN_DIRS as "equivalent to
        // --plugin-dir" and consumes the same bundle format, so it takes the
        // identical tree Claude gets.
        Target::Codebuddy => PLUGIN_DIRS_ENV,

        // Pi's repeatable --skill flag accepts a file or directory outside its
        // config root. Its other extensibility lives in executable JavaScript
        // extensions, not declarative subagent, hook, or MCP configuration, so
        // Edgee cannot translate those component kinds without changing their
        // semantics.
        Target::Pi => match kind {
            Kind::Skills => config_key("pi --skill"),
            Kind::Subagents => Delivery::Unsupported {
                reason: "Pi has no configuration for user-defined subagents",
            },
            Kind::Hooks => Delivery::Unsupported {
                reason:
                    "Pi hooks require JavaScript extensions; declarative hooks are not supported",
            },
            Kind::McpServers => Delivery::Unsupported {
                reason: "Pi has no built-in MCP configuration; MCP requires an extension",
            },
        },

        // OMP accepts Claude-compatible plugin bundles from arbitrary paths.
        // One repeatable flag loads every component kind for the session and
        // leaves the user's plugin installation untouched.
        Target::Omp => OMP_PLUGIN_DIR,

        // Crush's own schema (charm.land/crush.json) carries `options.skills_paths`,
        // a top-level `hooks` map and an `mcp` map — all injected into the config
        // the CLI already generates for it. Subagents are the one real absence:
        // Crush manages its own agents and exposes no way to define one.
        Target::Crush => match kind {
            Kind::Skills => config_key("crush options.skills_paths"),
            Kind::Hooks => config_key("crush hooks config"),
            Kind::McpServers => config_key("crush mcp config"),
            Kind::Subagents => Delivery::Unsupported {
                reason: "Crush manages its own agents; user-defined subagents are not configurable",
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every pair must resolve, and every refusal must be able to explain
    /// itself — so adding a target cannot silently leave holes.
    #[test]
    fn delivery_matrix_is_exhaustive_and_explains_refusals() {
        for target in Target::ALL {
            for kind in Kind::ALL {
                match delivery(target, kind) {
                    Delivery::Delivered { mechanism } => assert!(
                        !mechanism.is_empty(),
                        "{}/{} delivered with no mechanism",
                        target.label(),
                        kind.label()
                    ),
                    Delivery::Unsupported { reason } => assert!(
                        !reason.is_empty(),
                        "{}/{} unsupported with no reason",
                        target.label(),
                        kind.label()
                    ),
                }
            }
        }
    }

    /// The spike that unblocked this feature: one flag, all four kinds.
    #[test]
    fn claude_delivers_every_kind() {
        for kind in Kind::ALL {
            assert!(
                delivery(Target::Claude, kind).is_delivered(),
                "claude should deliver {}",
                kind.label()
            );
        }
    }

    #[test]
    fn omp_delivers_every_kind() {
        for kind in Kind::ALL {
            assert!(
                delivery(Target::Omp, kind).is_delivered(),
                "omp should deliver {}",
                kind.label()
            );
        }
    }

    /// A structural absence should read differently from "we haven't got to it",
    /// because one of them will never change.
    #[test]
    fn structural_gaps_say_why() {
        match delivery(Target::Opencode, Kind::Hooks) {
            Delivery::Unsupported { reason } => assert!(reason.contains("JS plugins")),
            other => panic!("expected OpenCode hooks to be unsupported, got {other:?}"),
        }
        match delivery(Target::Codex, Kind::Subagents) {
            Delivery::Unsupported { reason } => assert!(reason.contains("subagents")),
            other => panic!("expected Codex subagents to be unsupported, got {other:?}"),
        }
        match delivery(Target::Pi, Kind::McpServers) {
            Delivery::Unsupported { reason } => assert!(reason.contains("requires an extension")),
            other => panic!("expected Pi MCP servers to be unsupported, got {other:?}"),
        }
    }

    #[test]
    fn kind_order_is_stable() {
        let mut kinds = Kind::ALL;
        kinds.sort_by_key(|k| k.order());
        assert_eq!(kinds, Kind::ALL);
    }
}
