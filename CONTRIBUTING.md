# Contributing to Edgee

Thank you for considering a contribution. Edgee is Apache 2.0 licensed and we welcome bug reports, feature requests, and pull requests.

## Prerequisites

- **Rust** stable toolchain (1.85 or later). Install via [rustup](https://rustup.rs).
- **cargo** is bundled with Rust.
- On Linux you may need `pkg-config` and `libssl-dev` (or equivalent) for the TLS backend.

## Clone and build

```bash
git clone https://github.com/edgee-ai/edgee
cd edgee
cargo build
```

For a release (optimized) build:

```bash
cargo build --release
```

Install the CLI locally:

```bash
cargo install --path crates/cli
edgee --version
```

## Run the CLI in development

```bash
# Run directly with cargo (no install step required)
cargo run -- launch claude
cargo run -- stats
```

## Tests

```bash
# All tests across the workspace
cargo test --all

# A single test by name
cargo test my_test_name

# With stdout visible
cargo test --all -- --nocapture
```

## Lint and format

```bash
# Format all code in place
cargo fmt --all

# Lint (must be clean before opening a PR)
cargo clippy --all-targets
```

## Pre-commit gate

All three checks must pass before committing:

```bash
cargo fmt --all && cargo clippy --all-targets && cargo test --all
```

## Pull request process

1. Fork the repo and create a branch from `main`. Use the naming scheme `feat/<topic>`, `fix/<topic>`, or `chore/<topic>`.
2. Make your changes and ensure the pre-commit gate passes locally.
3. Open a PR against `main` with a concise, imperative title (e.g. `Add OpenAI streaming support`).
4. Reference the relevant GitHub issue in the PR description (e.g. `Closes #42`).
5. A maintainer will review within a few business days. Small, focused PRs get reviewed fastest.

For significant new features or architectural changes, open an issue first so we can discuss the approach before you invest time building.

## Tool-result trimming strategies

Tool-output trimmers live in the gateway repo under
[`tool-result-trimming/`](https://github.com/edgee-ai/gateway/tree/develop/tool-result-trimming).
See [`tool-result-trimming/README.md`](https://github.com/edgee-ai/gateway/blob/develop/tool-result-trimming/README.md)
for how to add a new strategy.

## Repository layout

See the [Repository layout](../README.md#repository-layout) section in the README for the crate tree and purpose table, and [`doc/architecture.md`](doc/architecture.md) for how the CLI relates to the hosted gateway.

## License

By contributing you agree that your work will be licensed under the Apache License 2.0.
