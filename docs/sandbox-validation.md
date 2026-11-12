# Native Rust sandbox validation record

The sandbox implementation now lives in the Edgee Rust binary. The previous
JavaScript/Anthropic-runtime validation is historical and does not establish
correctness of this replacement. This record lists results for the native
implementation only.

## Reproduce

Install bubblewrap on Linux. No Node.js/npm installation is needed for these
checks. Use the repository's mandatory quality gate, then the explicit real-OS
suite (missing isolation support is a failure, never a skip):

```sh
cargo fmt --all
cargo clippy --all-targets
cargo test --all
cargo build --bin edgee
EDGEE_TEST_BINARY="$PWD/target/debug/edgee" \
FAKE_HOST_SECRET=host-only-fake-secret \
cargo test --features sandbox-runtime-tests native_sandbox -- --nocapture --test-threads=1
```

The dedicated GitHub Actions job runs on Ubuntu 22.04 and macos-latest. Native
Claude/Codex authentication is checked separately from synthetic workloads;
production credentials are never installed on CI runners.

## Results (2026-11-12)

| Check | macOS ARM64 | Linux |
|---|---|---|
| Native OS filesystem, network, fake-secret, local MCP, Keychain/socket boundary | Passed locally | Passed in CI |
| Inherited non-CLOEXEC descriptor closure | Passed locally | Included in follow-up CI |
| Authenticated HTTP and binary CONNECT tunnel; malformed/pipelined requests denied | Passed | Passed in CI |
| Linked-worktree commit and protected configuration | Passed | Passed in CI |
| HTTPS clone through authenticated proxy | Passed | Passed in CI |
| Argv/exit status, descendant cleanup, direct-parent termination | Passed | Passed in CI |
| Root-owned mandatory machine policy, relay/argument/environment bypass refusals | Unit checks passed; no system policy installed locally | Passed on disposable CI runner |
| Claude Code 2.1.322 and Codex 0.164.0 `--version` | Passed | Not run |
| Authenticated model response, file creation and shell command | Passed for both agents | Not run |
| Interactive TUI response and Ctrl-C exit | Passed for both agents | Not run |

The local quality gate passes with 469 unit tests and 3 integration tests,
including strict Clippy (`-D warnings`). The real native macOS suite also passes
without skipped isolation tests. Linux enforcement and administrator fixtures
passed in [the native implementation CI run](https://github.com/edgee-ai/edgee/actions/runs/43229509876).
Final follow-up CI status is tracked on the PR.

Actual-agent tests used empty temporary workspaces and an invocation-local
`EDGEE_API_URL=https://api.edgee.ai`, because the workstation's alternate gateway
is outside the default allowlist. Both agents returned the requested fixed
responses; Codex's tool transcript showed a successful `/bin/sh test.sh`.
Claude was given explicit native Bash/Write/Read permission for its tool test.
No user project files or credentials were printed or uploaded as test material.

## Scope

Real-OS tests exercise filesystem permissions, fake secrets, symlinks, protected
Git metadata, descendant commands, scrubbed environments, direct network and
Unix-socket denial, authenticated proxy access, forbidden destinations, process
exit and cancellation, worktrees and HTTPS cloning. The Linux administrator
fixture installs root-owned files only on a disposable runner, refuses to replace
existing system paths, and cleans up its own policy/install files.

The repository's two preexisting ignored macro doctests are unrelated to
sandbox isolation. See the [threat model](sandbox-threat-model.md) for residual
limits. Passing an isolation test does not validate every native-agent workflow.
