# Sandbox validation record

Validation performed on 2026-11-12. This record distinguishes OS enforcement,
agent-shaped test commands, and actual installed agent sessions.

| Check | macOS ARM64 | Linux |
|---|---|---|
| Real OS isolation harness (`test.mjs`) | Passed: Seatbelt | CI configured; result pending |
| Rust supervisor environment/lifecycle test | Passed: real Seatbelt, inherited fake secret removed, exit 42, ordinary descendant cleanup | CI configured; result pending |
| `cargo fmt --all`, `cargo clippy --all-targets`, `cargo test --all` | Passed: 456 unit tests + 3 integration tests; zero clippy warnings | CI configured |
| Claude Code 2.1.322 `--version` | Passed | Not run |
| Claude Code noninteractive gateway response | Passed, exact expected response | Not run |
| Claude Code interactive TUI gateway response and Ctrl-C exit | Passed | Not run |
| Codex CLI 0.164.0 `--version` | Passed | Not run |
| Codex authenticated model response | Passed, exact expected response | Not run |
| Codex interactive TUI gateway response and Ctrl-C exit | Passed | Not run |
| Installed mandatory root policy end to end | Not run; policy ownership/ACL/parser unit tests passed | Disposable CI fixture added; result pending |

The JS suite uses fake secrets, a controlled HTTP endpoint and Unix socket,
and an isolated fake macOS Keychain with an outside positive control. It covers
workspace read/write, outside file denial, symlink and relative escapes, child
process/local MCP confinement, direct socket denial, authenticated allowed
proxy traffic, forbidden proxy destinations, Keychain denial, complex argv,
exit status, runtime absence and SIGINT. The controlled local endpoint is
allowlisted only in the test specification; production policy rejects it.

The feature-gated Rust test is explicitly run in CI, not ignored or skipped:

```sh
npm ci --ignore-scripts --omit=optional --prefix scripts/sandbox-runtime
node scripts/sandbox-runtime/test.mjs
EDGEE_TEST_NODE="$(command -v node)" \
EDGEE_TEST_RUNTIME_ROOT="$PWD/scripts/sandbox-runtime" \
FAKE_HOST_SECRET=host-only-fake-secret \
cargo test --features sandbox-runtime-tests real_runtime_lifecycle
```

Native Claude tests used an empty temporary workspace, a trivial fixed-response
prompt, and the public Edgee gateway via an invocation-local override. The
workstation's preconfigured alternate gateway was correctly rejected by the
default allowlist. No real secret was printed. Codex exposed compatibility issues with optional `/etc/codex` metadata probing,
certificate loading, and libc resolution of the runtime's localhost proxy.
Targeted metadata access, a public CA bundle and numeric loopback proxy URLs
resolved them without granting `/etc/hosts` or Keychain access. Its interactive
account discovery requires the explicitly allowlisted `chatgpt.com`; both the
noninteractive and TUI model responses then passed.

The real linked-worktree suite passed status/add/commit and verified that common
configuration/hooks, current worktree control files, object alternates and other
worktrees remain protected. Five Rust regression fixtures cover writable-root
symlink and backlink substitutions.

Linux is not available locally: the Docker CLI exists but its daemon socket is
absent. The dedicated GitHub Actions matrix runs actual bubblewrap and Seatbelt
on Ubuntu 22.04 and the supported `macos-latest` image; missing/degraded isolation fails the job. There are
no skipped isolation tests. Two preexisting macro documentation tests are ignored
by the repository's ordinary Rust suite.

The Linux-only administrator fixture installs root-owned Node/runtime files and
an `/etc/edgee/sandbox.toml` policy on a disposable CI runner. It checks mandatory
selection through ordinary launches and aliases, rejected repository/environment
overrides, writable or symlinked policy rejection, writable or escaping runtime
files, and missing-runtime refusal. It refuses to replace existing system paths
and removes only its own fixtures. This fixture has not been run on the developer
workstation; its CI result remains pending.

Separate sub-agents examined architecture, security bypasses and test validity.
Their findings drove trusted runtime/helper/ACL checks, narrower macOS IPC,
secret-free supervisor command strings, realistic Keychain/socket tests and a
real Rust environment-cleaning test. See the threat model for residual cleanup,
resource-exhaustion and dependency limitations; a passing test is not a claim
that every native-agent workflow has been validated.
