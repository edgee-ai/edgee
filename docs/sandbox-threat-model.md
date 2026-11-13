# Local sandbox threat model

The adversary is an agent, repository content, shell command or newly spawned
local MCP server trying to read unrelated workstation data, write outside its
workspace/session or contact disallowed destinations. The trusted computing
base is the Edgee Rust binary, the administrator policy, bubblewrap on Linux,
and the OS/toolchain. Host authentication/setup runs
outside isolation. Machine administrators and compromised kernels are out of
scope; the promise covers sessions launched through Edgee.

## Enforcement

Edgee generates macOS Seatbelt policies and Linux bubblewrap mounts and seccomp
filters directly in Rust. Read access starts denied at `/`, with explicit
literal canonical allow paths. Writes are confined to workspace/session with
protected configuration denies. The authenticated Rust proxy checks exact
hostnames and resolved addresses before connecting. OS restrictions prevent
changing proxy variables or opening direct sockets from bypassing that policy.
Unix sockets, host loopback services and private resolved addresses are blocked.
Descendants inherit confinement. No existing Edgee MITM relay port is exempted.

On Linux, bubblewrap supplies isolated mount, network, user and PID namespaces.
The filesystem starts with an empty root; explicit read-only and writable mounts
are added, and scaffolding is sealed read-only. A private Rust bridge connects
namespace-local proxy traffic to the host proxy. The workload receives a seccomp
filter preventing Unix/vsock socket creation and namespace/process-control
operations that could attack the bridge. `/dev` and `/proc` are isolated
pseudo-filesystem exceptions, not access to host temporary files or processes.

The macOS profile is generated from a default-deny policy, without grants to
Keychain/SecurityServer, host preferences or POSIX shared memory/semaphores.
Only the authenticated proxy's exact loopback TCP endpoint is permitted.
Each session uses its own private home and temporary directory. The macOS dynamic
loader receives a read of the root directory entry itself (not its descendants)
and metadata for permitted-path ancestors; this does not expose home or `/etc`
file contents.

The policy is read from a fixed administrator location, not the workspace.
Mandatory mode requires a trusted installed Edgee executable and OS helpers;
invalid policy is an error even if the user did not ask for a sandbox. The
workload environment is explicitly constructed, without host proxy or loader
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

## Distribution

The supervisor, proxy, profiles and seccomp filter generation ship as Rust code
in Edgee under the repository's Apache-2.0 license. There is no npm dependency,
JavaScript supervisor, downloaded seccomp helper or external sandbox runtime.
Linux bubblewrap is installed through the operating system package manager;
macOS uses the system sandbox facility. Changes to these security boundaries
must pass the real macOS/Linux isolation suite, as well as ordinary Rust checks.

Existing nested security configuration is scanned before launch without following
directory symlinks. The scan refuses workspaces exceeding 250,000 entries or
4,096 protected paths. Existing `.git/modules` administration is read-only.
New security-config directories created below previously ordinary nested paths
are not retroactively discovered; review workspace changes before using new
repositories, editor configurations or executable code outside the sandbox.

Protected security trees reject symlinks and multiply-linked regular files:
read-only access through one name must not leave another writable alias to the
same security configuration. Replace those links with independent files before
launching; this does not change the residual hardlink risk for ordinary project
data described above.
