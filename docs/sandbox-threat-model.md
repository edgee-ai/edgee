# Local sandbox threat model

The adversary is an agent, repository content, shell command or newly spawned
local MCP server trying to read unrelated workstation data, write outside its
workspace/session or contact disallowed destinations. The trusted computing
base is Edgee, the administrator policy, Node, the pinned Anthropic Sandbox
Runtime and dependencies, and the OS/toolchain. Host authentication/setup runs
outside isolation. Machine administrators and compromised kernels are out of
scope; the promise covers sessions launched through Edgee.

## Enforcement

Anthropic Sandbox Runtime 0.0.82 supplies macOS Seatbelt and Linux bubblewrap,
network namespaces and seccomp. Read access starts denied at `/`, with explicit
literal canonical allow paths. Writes are confined to workspace/session with
protected configuration denies. The runtime network allowlist is backed by OS
restrictions; changing proxy variables or opening direct sockets does not grant
network access. Unix sockets, host loopback and private resolved addresses are
blocked. Descendants inherit confinement. No existing Edgee MITM relay port is
exempted.

The pinned macOS profile is narrowed by exact checked substitutions removing
Keychain/SecurityServer, host preferences and POSIX shared-memory/semaphore
grants. Unexpected profile text fails closed. This is version-coupled and must
be reviewed and tested before changing the pin. Runtime's shared `/tmp/claude`
write allowance is denied; each session uses private temp space instead.

The policy is read from a fixed administrator location, not the workspace.
Mandatory mode requires trusted Node/runtime paths and helpers; invalid policy
is an error even if the user did not ask for a sandbox. Environment sanitization
also applies to the trusted runtime supervisor, removing proxy and Node loader
injection. Secrets are not intentionally logged or placed in shell command
arguments. The agent's own scoped provider/OAuth credentials remain readable
by the agent; console credentials are not supplied.

## Residual risks and scope limits

- An allowed destination can receive exfiltrated workspace data or supplied agent
  credentials. A hostname allowlist is not a DLP system. A remote MCP may act on
  remote systems with the permissions of its own credentials.
- Workspace secrets are in scope for reads. Existing hardlinks can give an
  allowed path another name for an outside file, particularly on Linux. Use
  trusted workspaces; this does not promise safe isolation of a deliberately
  preconstructed hardlink tree or concurrent hostile host filesystem mutation.
- Tools/code in readable toolchain directories run inside confinement but may
  access all data granted to that session. Shared Git metadata can disclose
  configured remotes. Selected worktree administration and shared objects/refs/logs are writable;
  other worktree administration, hooks/configuration and object alternates are
  protected. Some shared-root maintenance operations are blocked.
- The OS runtime constrains filesystem/network operations, not CPU, memory or
  disk consumption. Resource exhaustion and kernel vulnerabilities remain.
- macOS process-group cleanup reaps ordinary descendants. A hostile process that
  creates another session/process group can survive the launcher; it remains
  Seatbelt-confined but is not guaranteed to be reaped. Linux PID namespaces
  provide a stronger descendant lifetime boundary. SIGKILL/host crash can leave
  private session directories; remove stale `edgee-sandbox-*` directories after
  verifying no session is running.
- Persistent security files and hooks are protected, but workspace code itself
  remains attacker-modifiable. Running that code later outside a sandbox grants
  it the host's permissions. Review changes before doing so.
- This mode does not inherit all host plugins, caches, settings, persistent
  session history or credential refresh state. It rejects unsupported launch
  targets and the legacy relay instead of silently weakening its boundary.

## Runtime distribution review

The upstream package is Apache-2.0, distributed through npm with an exact version
and checked-in integrity lockfile. Node >=22.12 and OS-specific helpers are
external dependencies; optional packages and install scripts are omitted.
`npm audit` reported upstream Zod 3 advisory GHSA-mrhx-c7xc-m3w5 (unbounded
collection validation causing memory exhaustion). The runtime consumes generated
trusted configuration here, but this is a dependency risk to track. Do not
silently substitute a different runtime version or incompatible Zod major.
