# Architecture

This repository contains the **Edgee CLI** (`edgee-cli`) only.

The production AI Gateway that sits between coding agents and LLM providers
(AWS / Fastly / on-prem) lives in a separate repository:
[edgee-ai/gateway](https://github.com/edgee-ai/gateway). That gateway owns
tool-result trimming via its internal `tool-result-trimming` crate.

## Crate boundaries

| Crate | Crate name | Responsibility |
|---|---|---|
| `crates/cli` | `edgee-cli` | `edgee` binary — launch agents, auth, relay for GUI apps, stats, aliases |

## How the CLI relates to the gateway

```text
coding agent  ──HTTP──►  Edgee gateway (hosted / on-prem)
                              │
                              ├── tool-result-trimming (gateway workspace)
                              └── provider adapters, routing, billing, …
```

`edgee launch <agent>` points the agent at the gateway (`ANTHROPIC_BASE_URL` /
custom headers, or a local MITM relay for apps that cannot be redirected).
Compression of tool results happens **inside the gateway** — not inside the
CLI process.

See the gateway repo's [`tool-result-trimming/README.md`](https://github.com/edgee-ai/gateway/blob/develop/tool-result-trimming/README.md)
for strategy details and how to add a trimmer.
