# Local agent sandbox

`edgee launch --sandbox claude` and `edgee launch --sandbox codex` run the
agent, its commands and newly spawned local MCP servers under OS restrictions.
This is local execution, not a remote workspace. Existing launches are unchanged
unless a machine administrator requires sandboxing.

## Install

The sandbox supervisor, policy enforcement and authenticated network proxy are
compiled into the Edgee Rust binary. No Node.js, npm package or downloaded
sandbox runtime is required.

macOS uses the system Seatbelt facility and needs Git/Xcode Command Line Tools
for Git operations. Linux uses bubblewrap and a Rust-generated seccomp filter;
install bubblewrap with your distribution package manager. Working unprivileged
user namespaces are required. Missing or degraded isolation fails closed.
Windows and nested environments that cannot supply these restrictions are
unsupported. No dependency is downloaded during launch.

## Launch

All Edgee options go **before** the agent name; everything after it remains an
agent argument, including `--sandbox` if the agent itself uses that spelling.

```sh
edgee launch --sandbox claude
edgee launch --sandbox codex exec 'Describe this project'
edgee launch --sandbox --workspace "$HOME/projects/my app" claude -p 'Run the tests'
edgee launch --sandbox --repo https://github.com/acme/my-app.git codex
```

From a Git repository, Edgee selects its root and preserves the current
subdirectory. An explicit workspace must exist. Without either, an interactive
terminal prompts for a directory; unattended execution requires `--workspace`.
System root, the account home and their ancestors are rejected, even through
symlinks or a changed `HOME`. Workspaces overlapping policy directories
are rejected.

**Existing workspace edits are real and persist. A sandbox does not make a
copy or provide rollback.** `--repo` reserves a fresh persistent directory under
`~/.edgee/workspaces`, then clones with the same OS and network restrictions.
The repository host must be in administrator `allowed_domains`. Clone does not
use host Git credentials, SSH agents, helpers or templates; private repositories
should be cloned beforehand. Failed clones are removed.

Before launch, dependency checks and a real OS read/write denial probe must
succeed. Only then is `sandbox active` printed, alongside the workspace and
network allowlist. A refusal never falls back to an unprotected agent.

## Machine policy

Only `/etc/edgee/sandbox.toml` is consulted (the canonical
`/private/etc/edgee/sandbox.toml` on macOS). Repository files and environment
variables cannot replace this policy. Unknown fields or invalid/untrusted files
cause refusal. Remove the obsolete `node_binary` and `runtime_root` keys when
migrating from the earlier runtime-based implementation. The file and its parent directories must be root-owned and not
writable by group/others. Symlinked policy files are rejected.

Example (paths must exist):

```toml
mandatory = true
allowed_domains = ["api.edgee.ai", "chatgpt.com", "github.com", "registry.npmjs.org"]
read_paths = ["/opt/edgee/toolchains"]
```

The defaults are optional sandboxing, network access only to `api.edgee.ai` and `chatgpt.com` (Codex account/workspace discovery), and
no additional readable toolchains. Exact DNS names only: no wildcards, localhost,
IP addresses, URLs or private-network names. DNS-resolved local/private addresses
are rejected by the runtime proxy. The gateway must be HTTPS on port 443 and
match an allowed domain. HTTPS redirects do not grant new allowed destinations.

For mandatory policy, install Edgee and its OS helpers in administrator-controlled
locations. Their ownership and permissions are checked before execution.
The trust boundary includes the system toolchain; this does not prevent a
workstation administrator from running a different binary.

`read_paths` grants recursive read/execute access to explicit toolchain roots.
It does not grant writes or pass through environment variables. Avoid placing
credentials in these roots. Host package-manager caches and arbitrary home
files are not automatically shared. Dependencies installed **inside the
workspace** work when their registry domains and required tooling are allowed.

Mandatory policy applies through existing aliases without reinstalling them.
Unsupported launch targets and Claude's legacy `--relay` path are refused;
they cannot bypass mandatory sandboxing.

## Files, authentication and MCP

The workspace is readable/writable, with Git configuration/hooks and Edgee,
Claude and Codex security configuration protected against writes. Ordinary Git
operations in a regular repository can update objects, refs and the index.
Linked worktrees receive writes only to their own validated administration directory
and the shared objects, refs and logs directories. Status, add and commit are
supported; shared Git configuration, hooks, object alternates and other worktree
administration remain protected. Shared refs and objects are real repository
changes. Operations needing shared-root writes, such as some maintenance or
packed-ref operations, may be refused.
Do not store credentials in Git URLs/configuration: readable Git metadata is
visible to the agent.

Only explicit OS tool/library roots, the selected executable, workspace,
validated Git metadata, configured toolchains and a private session
home/temp directory are exposed. Symlinks do not extend that access. An
interpreted/npm-installed agent may need its package and interpreter roots in
`read_paths`; Edgee does not automatically expose the executable's whole parent.

The child environment starts empty. Terminal/locale values and the necessary
agent settings are added explicitly. Host `AWS_*`, cloud credentials,
`SSH_AUTH_SOCK`, Docker sockets, loader options and arbitrary proxy variables
are not inherited. Codex reads system-managed `/etc/codex` configuration and a public CA bundle
without granting access to other `/etc` contents. macOS Git uses its resolved
Xcode toolchain binary and read-only `Developer/usr` files.

Keychain/security broker access is removed from the macOS
sandbox profile. Only the specific Claude access token is retrieved by the
trusted host launcher; other Keychain items remain outside the sandbox.

Run native agent login and `edgee auth login` before unattended use. Claude
subscription sessions receive only their existing access token; refresh/login
must happen outside the sandbox. Codex receives a private copy of supported
fields from `auth.json`; refresh changes are not written back. API-connected
Claude receives its scoped Edgee key. The agent can read its own supplied
credentials: this boundary protects **unrelated** workstation secrets.

No Edgee console user token or host agent configuration is injected. Codex
ChatGPT app connectors default to disabled; native agent options can enable
them within the same network allowlist. Automatic
Edgee MCP, mods and organization plugin delivery are not enabled in this path.
Configure a local stdio MCP through the agent's native options and a configuration
inside the workspace. It inherits the same OS restrictions. Local HTTP listeners
and Unix-socket MCP transports are blocked by the default policy. Remote MCP hosts
must be explicitly allowed and have their own credentials. Preexisting services
are not retroactively sandboxed; host localhost and Unix sockets are blocked.

The Rust sandbox supervisor's restricted local network transport is separate from the
existing Edgee MITM relay. `--relay` is rejected because that relay's broad
passthrough is not a suitable security boundary. Model HTTPS/streaming traffic
uses the allowed Edgee gateway through the authenticated Rust proxy.

## Diagnostics and limits

- Dependency/isolation probe failure: fix user namespaces, seccomp, missing
  helpers or platform restrictions; do not disable isolation.
- Agent cannot find a library/tool: add its specific installation root to
  administrator `read_paths`, then retry.
- Network denied: add only the necessary exact registry/model/MCP hostname.
  Local/private services intentionally remain unavailable.
- Workspace contains `.edgee/credentials.toml`: move it to home configuration
  before launching. Workspace contents, including `.env` files and other secrets
  deliberately placed in the project, are otherwise visible to the agent.
- Ctrl-C and normal exit clean up the private session and ordinary descendants.
  See the threat model for hostile detached processes on macOS.

Native argv, stdin/stdout/stderr and exit codes are preserved by the supervisor;
terminal ownership is transferred for interactive sessions. Agent sessions do
not persist their private home after exit. See [threat model](sandbox-threat-model.md)
and [validation record](sandbox-validation.md) for guarantees and testing limits.

Codex's inner OS sandbox defaults to `danger-full-access` in this mode because
macOS prohibits applying Seatbelt twice. This is announced at startup: Edgee's
outer OS sandbox still confines every command, and native approval settings
remain in force. Explicit agent arguments can change native settings but cannot
relax the outer policy. For unattended Claude tool use, pass the native tool
approvals you intend (for example `--allowedTools=Bash`); Edgee does not bypass
those prompts.
