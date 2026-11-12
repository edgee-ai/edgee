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
FAKE_HOST_SECRET=host-only-fake-secret \
cargo test --features sandbox-runtime-tests native_sandbox -- --nocapture --test-threads=1
```

The dedicated GitHub Actions job runs on Ubuntu 22.04 and macos-latest. Native
Claude/Codex authentication is checked separately from synthetic workloads;
production credentials are never installed on CI runners.

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
