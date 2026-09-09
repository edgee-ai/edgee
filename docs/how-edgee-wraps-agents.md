# How Edgee wraps coding agents

**Status:** reference document, kept in sync with `src/commands/launch/` and `src/commands/relay/`.
**Audience:** engineers working on launch targets, and anyone answering technical questions about how interception works.

This document answers six questions, mechanism by mechanism, with links to each vendor's own
documentation. The [FAQ](#faq) answers them in this order:

1. How does interception actually work?
2. Is it consistent with the providers' terms?
3. Can a vendor update break it?
4. Does it need a local proxy, a certificate, a wrapper, or an endpoint agent?
5. Can a provider block Edgee technically or contractually?
6. Are there official partnerships or integrations?

The short answer is in two parts.

**For the CLI agents, which are the large majority of usage, Edgee sets the configuration variables
the vendors themselves publish for exactly this purpose.** There is no binary patching, no code
injection, no credential extraction, no reverse-engineered API.

For three GUI targets (Cursor, Copilot in VS Code, Claude Desktop) Edgee runs a **local, opt-in,
scoped MITM relay**. That is a materially different and more sensitive posture, and it is stated
plainly in [Transport C](#transport-c-local-relay) and in
[Fragility](#fragility-what-a-vendor-update-can-break).

---

## The three transports

Every launch target uses exactly one of three transports. Nothing else exists in this repository.

| Transport | What Edgee does | Targets | Vendor-documented? |
| --- | --- | --- | --- |
| **A. Environment / config injection** | Sets documented env vars or CLI config flags on the child process only | `claude`, `codex`, `opencode`, `codebuddy`, `crush`, `kimi`, `kilo` | **Yes**, with one caveat. Each variable below links to the vendor's own docs; `KIMI_CODE_CUSTOM_HEADERS` is announced in Kimi's release notes but missing from its reference page |
| **B. Config-file patch** | Writes a provider block into the app's own config file — additive and persistent for `pi`/`omp`, temporary and reverted for `codex-desktop` | `pi`, `omp`, `codex-desktop` | **Partly.** The config keys are documented; patching another app's file is our own pattern |
| **C. Local relay (MITM)** | Runs a loopback proxy, decrypts only known inference hosts, reroutes to the gateway | `cursor`, `copilot-vscode`, `claude-desktop` | **No.** It uses documented proxy and CA plumbing, but the interception itself is outside any published contract |

Transport A covers the products that drive most enterprise coding-agent spend. Transport C is the
compatibility path for GUI apps that expose no configuration surface at all.

**Nothing runs as a daemon, a system extension, a kernel module, or an endpoint agent.** The relay
is a foreground process started by the launch command and bound to loopback; it is never installed
or registered, and it does not survive the terminal. Edgee never modifies the agent's binary, its
installed files, or its stored credentials. Two config files are the exceptions: `codex-desktop`'s,
which is reverted about ten seconds later, and `pi`'s `models.json`, which gains one namespaced
`edgee` provider key and keeps it.

---

## Transport A: environment and config injection

### Claude Code (`edgee launch claude`)

Implementation: [`src/commands/launch/claude.rs`](../src/commands/launch/claude.rs)

```
ANTHROPIC_BASE_URL      = https://<gateway>
ANTHROPIC_CUSTOM_HEADERS= x-edgee-api-key: …
                          x-edgee-session-id: …
                          x-edgee-repo: …            (when in a git repo)
ANTHROPIC_DEFAULT_SONNET_MODEL / ANTHROPIC_DEFAULT_OPUS_MODEL
ENABLE_TOOL_SEARCH      = true                        (only when tool-surface reduction is on)
```

Plus, when the user has opted into Edgee's MCP server, three documented CLI flags:
`--mcp-config=<file>`, `--append-system-prompt <text>`, `--allowedTools=<list>`.

**This is Anthropic's own documented gateway configuration.** Anthropic publishes a four-page
specification for third-party LLM gateways:

- [Other LLM gateways](https://code.claude.com/docs/en/llm-gateway), the overview
- [Connect Claude Code to an LLM gateway](https://code.claude.com/docs/en/llm-gateway-connect)
- [Roll out an LLM gateway for your organization](https://code.claude.com/docs/en/llm-gateway-rollout)
- [Gateway protocol reference](https://code.claude.com/docs/en/llm-gateway-protocol), the wire contract

The protocol reference is explicit that custom headers are part of the contract:

> "If your developers set `ANTHROPIC_CUSTOM_HEADERS`, those headers appear on requests as well."

It also documents `x-claude-code-session-id`, which Anthropic supplies so that a gateway can
"aggregate all requests from one session without parsing request bodies". Per-session attribution is
therefore an intended gateway capability, not something Edgee extracts.

#### Subscriptions: what Anthropic's own documentation says

Anthropic documents two ways of running Claude Code through a gateway, and they differ in exactly
the respect that matters here: whether the gateway also supplies a credential.

> "While a gateway credential variable or `apiKeyHelper` is active, a developer's claude.ai
> subscription isn't used: the credential replaces the subscription login for that session […]
> `ANTHROPIC_BASE_URL` is the variable that points Claude Code at the gateway. **Setting only that
> variable, without a gateway credential, doesn't replace the subscription. Requests still route
> through the gateway, but a saved claude.ai login remains the active credential, so its usage
> limits and billing apply.** Gateways that pass this traffic on to Anthropic must forward the OAuth
> capability in `anthropic-beta`."
>
> — [Other LLM gateways § Subscriptions and gateways](https://code.claude.com/docs/en/llm-gateway#subscriptions-and-gateways)

**Edgee sets `ANTHROPIC_BASE_URL` and `ANTHROPIC_CUSTOM_HEADERS`, and no credential variable at
all** — not `ANTHROPIC_API_KEY`, not `ANTHROPIC_AUTH_TOKEN`, not `apiKeyHelper`. The
`x-edgee-api-key` header is a *custom* header addressed to the Edgee gateway; it is not an Anthropic
credential, and Claude Code does not treat it as one. So the user's own claude.ai OAuth login stays
the active credential, the request is billed to their subscription, and their usage limits apply.
That is precisely the second configuration Anthropic describes above.

"Works with consumer subscriptions" is therefore not a workaround. It is the documented behaviour of
the variable Anthropic publishes for gateways. Anthropic also documents the obligation that comes
with it: forward `anthropic-beta` verbatim, including the OAuth capability, or those requests fail
with `401`. Edgee's gateway meets that obligation.

Two consequences worth stating:

1. Edgee never extracts, stores, or replays the user's Anthropic credential. It transits the gateway
   in the `Authorization` header the client sets, is forwarded upstream unchanged, and is not
   persisted. Edgee's own auth travels in a separate header, and the CLI never reads the user's
   credential at all.
2. Anthropic is aware of this traffic pattern and specifies it. What it does *not* do is endorse
   specific vendors: "Anthropic doesn't endorse, maintain, or audit third-party gateway products,
   and doesn't support routing Claude Code to non-Claude models through any gateway." That second
   clause matters for the routing pillar, and is answered in
   [FAQ 2](#2-is-it-consistent-with-the-providers-terms).

### Codex CLI (`edgee launch codex`)

Implementation: [`src/commands/launch/codex.rs`](../src/commands/launch/codex.rs)

Edgee passes documented `-c` config overrides to the child process:

```
-c model_provider="edgee-cli"
-c model_providers.edgee-cli.name="EDGEE"
-c model_providers.edgee-cli.base_url="https://<gateway>/v1"
-c model_providers.edgee-cli.http_headers={"x-edgee-api-key"=…,"x-edgee-session-id"=…}
-c model_providers.edgee-cli.wire_api="responses"
-c model_providers.edgee-cli.requires_openai_auth=true
```

Every key used here — `model_provider`, `model_providers.<id>`, `name`, `base_url`, `wire_api`,
`http_headers` and `requires_openai_auth` — appears in OpenAI's
[config reference](https://learn.chatgpt.com/docs/config-file/config-reference). The `-c` one-off
override syntax is documented at
[Advanced config § one-off overrides from the CLI](https://learn.chatgpt.com/docs/config-file/config-advanced#one-off-overrides-from-the-cli).
`http_headers` exists precisely so that a custom provider can carry gateway routing headers.

Edgee does not touch `~/.codex/auth.json`, does not set `CODEX_ACCESS_TOKEN`, and does not set
`env_key`. `requires_openai_auth` is what makes Codex attach its own ChatGPT credential to a custom
provider — it is the setting OpenAI documents for reaching OpenAI models through an LLM proxy, and
it changes only *which credential is sent*, never the destination: requests still go to the
configured `base_url`. Without it Codex treats the provider as unauthenticated and sends no
`Authorization` header at all, which the gateway rejects. So Codex forwards its own credential and
Edgee's key rides in a separate header. Same shape as Claude Code: **the agent keeps its identity,
Edgee adds its own.**

### OpenCode (`edgee launch opencode`)

Implementation: [`src/commands/launch/opencode.rs`](../src/commands/launch/opencode.rs)

Edgee reads the user's existing `opencode.json` or `.jsonc`, merges in a `provider.edgee` block,
writes the result to a per-session temp file, and points the child at it with `OPENCODE_CONFIG`. The
user's own file is never modified.

Both mechanisms are documented:

- [`OPENCODE_CONFIG`](https://opencode.ai/docs/config/): "Specify a custom config file path using the
  `OPENCODE_CONFIG` environment variable."
- [Custom / OpenAI-compatible providers](https://opencode.ai/docs/providers/) describes exactly the
  block Edgee writes: `npm: "@ai-sdk/openai-compatible"`, `options.baseURL`, `options.apiKey`,
  `options.headers` ("Custom headers sent with each request"), and the `models` map with
  `limit.context` and `limit.output`.

Edgee fills the `models` map from the gateway's `/v1/models` listing, and the per-model `cost` rates
from the Edgee catalog, so OpenCode's own cost display stays accurate.

OpenCode and Crush work differently from Claude Code and Codex, in a way worth being explicit about.
With Claude Code and Codex, Edgee redirects an agent the user already runs, and their own provider
credential keeps working. With OpenCode and Crush, Edgee instead appears as an **additional provider
named "Edgee"** that the user selects, billed through their Edgee or BYOK credentials. No
subscription is involved on this path.

### CodeBuddy (`edgee launch codebuddy`)

Implementation: [`src/commands/launch/codebuddy.rs`](../src/commands/launch/codebuddy.rs)

```
CODEBUDDY_BASE_URL       = https://<gateway>/v1
CODEBUDDY_CUSTOM_HEADERS = x-edgee-api-key: …\nx-edgee-session-id: …
```

Both are documented in CodeBuddy's
[Environment Variables Reference](https://www.codebuddy.ai/docs/cli/env-vars). `CODEBUDDY_BASE_URL`
is the documented way to point the CLI at "any third-party model service compatible with the
Anthropic protocol", and `CODEBUDDY_CUSTOM_HEADERS` is documented as "useful for gateways/proxies
that require auth or routing headers" — a verbatim description of this use case.

> ⚠️ **Known defect.** `codebuddy.rs:60-62` builds the repo header in TOML syntax
> (`,"x-edgee-repo"="…"`), copied from `codex.rs`, then appends it to a newline-delimited header
> string. Repo attribution is therefore malformed for CodeBuddy sessions. Small fix, not yet made.

### Crush (`edgee launch crush`)

Implementation: [`src/commands/launch/crush.rs`](../src/commands/launch/crush.rs)

Edgee reads the user's global `crush.json`, merges in a `providers.edgee` entry
(`type: "openai-compat"`, `base_url`, `api_key`, `extra_headers`, `models[]`), writes it to a
per-session temp directory, and points the child there with `CRUSH_GLOBAL_CONFIG`. Project-level
config still wins, as Crush intends.

`CRUSH_GLOBAL_CONFIG` and the `openai-compat` provider type are documented in the
[Crush README](https://github.com/charmbracelet/crush). Note, however, that Crush has since
introduced a `crushrc` format and now describes the JSON config as deprecated. That makes this the
least future-proof of the Transport A integrations, and it should be migrated.

### Kimi Code (`edgee launch kimi`)

Implementation: [`src/commands/launch/kimi.rs`](../src/commands/launch/kimi.rs)

```
KIMI_MODEL_NAME          = moonshotai/kimi-k2.7-code
KIMI_MODEL_API_KEY       = <edgee key>
KIMI_MODEL_BASE_URL      = https://<gateway>          ← no /v1; the SDK appends it
KIMI_MODEL_PROVIDER_TYPE = anthropic
KIMI_CODE_CUSTOM_HEADERS = x-edgee-api-key: …\nx-edgee-session-id: …\nx-edgee-repo: …
```

Kimi Code is the strictest of the CLI agents about credentials, and deliberately so: provider
`api_key` and `base_url` are read **only** from `config.toml`, and
[the docs state plainly](https://moonshotai.github.io/kimi-code/en/configuration/env-vars.html) that
`export KIMI_API_KEY=…` does nothing. The vendor then carves out one exception — the `KIMI_MODEL_*`
family, "an explicit channel that *does* read credentials from the shell". Setting
`KIMI_MODEL_NAME` makes the CLI synthesize a provider and a model alias **in memory**, outranking
`default_model` and evaporating when the process exits.

That makes this the cleanest integration in the catalogue after Claude Code: nothing is written to
the user's files at all, so there is no temp config to merge (OpenCode, Crush), no additive block to
maintain (`pi`), and no patch-and-revert window (`codex-desktop`). A bare `kimi` run afterwards is
byte-for-byte unaffected.

`KIMI_CODE_CUSTOM_HEADERS` carries the gateway routing headers, one `Name: Value` per line — the
same shape as `ANTHROPIC_CUSTOM_HEADERS`. It was added in Kimi Code 0.20.2 and **is not in the
vendor's environment-variables reference**; it appears only in that release's notes ("A new
`KIMI_CODE_CUSTOM_HEADERS` environment variable lets you customize headers on outbound LLM
requests"). Its behaviour was therefore confirmed on the wire against a loopback server rather than
taken from documentation. That is a weaker documentation guarantee than the other Transport A
targets have, and it is the one thing to re-check when Kimi ships a major version.

Like OpenCode and Crush — and unlike Claude Code and Codex — this path does not redirect an agent
the user already authenticated. The session runs entirely on the Edgee-supplied model, billed
through Edgee or BYOK credentials; the user's own Kimi login is not involved.

### Kilo Code (`edgee launch kilo`)

Implementation: [`src/commands/launch/kilo.rs`](../src/commands/launch/kilo.rs)

```
KILO_CONFIG_CONTENT = {"$schema":"https://app.kilo.ai/config.json",
                       "provider":{"edgee":{"npm":"@ai-sdk/openai-compatible",
                                            "name":"Edgee",
                                            "options":{"baseURL":"https://<gateway>/v1",
                                                       "apiKey":"…",
                                                       "headers":{"x-edgee-api-key":"…",
                                                                  "x-edgee-session-id":"…"}},
                                            "models":{…}}}}
```

That is the whole injection: one environment variable, carrying one `provider.edgee` key.

[`KILO_CONFIG_CONTENT`](https://kilocode.ai/docs/code-with-ai/platforms/cli#environment-variables) is
documented on Kilo's CLI reference page, which describes it — alongside `KILO_CONFIG` and the global
config — as *trusted config*, in contrast to a project-level `kilo.json` committed to a repository.
The [config reference](https://kilocode.ai/docs/code-with-ai/platforms/cli#config-reference)
documents the `$schema`, the `provider` block, and per-provider `options` including `baseURL` and
`apiKey`; the machine-readable schema at `https://app.kilo.ai/config.json` covers the `models` map
with `limit.context` / `limit.output` and `cost`. The one detail taken from the reference embedded in
the binary rather than the public page is the **exact position** of `KILO_CONFIG_CONTENT` in the
precedence chain (above project config, below MDM-managed config) — worth re-checking on a major
version, though the integration only depends on it ranking above the user's own files.

Kilo's CLI is an OpenCode fork, so this could have been `opencode.rs` with the variable renamed —
merge the user's config in `$TMPDIR`, point `KILO_CONFIG` at it. `KILO_CONFIG_CONTENT` is better on
two counts. The Edgee key **never touches disk**, where the OpenCode and Crush temp configs embed it
and rely on cleanup that a crash can skip. And because Kilo deep-merges this payload over the user's
own configuration, Edgee never has to read, parse, or re-emit their files — it contributes one
provider and nothing else.

`KILO_CONFIG_DIR` looks like the natural companion for delivering skills and agents, and is not:
Kilo's embedded documentation calls it "appended to the search list", but the binary resolves
`config: KILO_CONFIG_DIR ?? <default>`, **replacing** the user's global config root and hiding their
own commands, agents and skills. Edgee does not use it.

Edgee fills the `models` map from the gateway's `/v1/models` listing and the per-model `cost` rates
from the Edgee catalog, so Kilo's own cost display stays accurate. The base URL keeps its `/v1`
because Kilo speaks OpenAI Chat Completions (`POST /v1/chat/completions`) and does not append the
version segment itself.

Like OpenCode and Crush — and unlike Claude Code and Codex — this path **runs entirely on
Edgee-supplied credentials**. Edgee appears as an additional provider named "Edgee" that the user
selects, billed through their Edgee or BYOK credentials. It does not redirect an agent the user
already authenticated, and no subscription is involved: Kilo's own `kilo auth` login is left
untouched and unused.

---

## Transport B: config-file patch (`pi`, `omp`, `codex-desktop`)

Two targets write into a config file the user owns, for opposite reasons and with opposite
lifecycles.

### Pi (`edgee launch pi`)

Implementation: [`src/commands/launch/pi.rs`](../src/commands/launch/pi.rs), whose module docs carry
the full rationale.

Pi resolves models through `<agent dir>/models.json`, and its only directory override moves the
whole agent root — `auth.json`, `settings.json`, history — so a private copy would strand the user's
login. The block therefore goes into the real `models.json`, under a single namespaced `edgee`
provider key. Because it is **additive** rather than a hijack of an existing key, it needs no
patch-and-revert dance: nothing else in the file is touched, and the key is simply left in place.
The provider uses Pi's `openai-completions` transport with a `/v1` base URL, so Pi sends requests to
the gateway's `/v1/chat/completions` endpoint.

### Oh My Pi (`edgee launch omp`)

Implementation: [`src/commands/launch/omp.rs`](../src/commands/launch/omp.rs), backed by Pi's shared
provider builder in [`src/commands/launch/pi.rs`](../src/commands/launch/pi.rs).

OMP uses Pi's custom-provider schema. Edgee writes the same additive `providers.edgee` block to
`~/.omp/agent/models.json`, launches `omp` with credential references supplied through environment
variables, and reuses the `pi` coding-agent key. Existing OMP providers and credentials remain
untouched.

### Codex Desktop (`edgee launch codex-desktop`)

Implementation: [`src/commands/launch/codex_desktop.rs`](../src/commands/launch/codex_desktop.rs).
Full rationale in
[`src/commands/launch/README.md`](../src/commands/launch/README.md#codex-desktop--config-patch-not-relay).

The ChatGPT desktop app runs a bundled `codex app-server` that reads `$CODEX_HOME/config.toml` and
honours the same `model_providers` keys as the CLI. Three cleaner approaches were tried first and
each is closed off:

- `-c` injection is impossible, because the app supplies its own argv.
- `--profile` is argv-only, with no env-var equivalent.
- A private `CODEX_HOME` is unsafe, because the ChatGPT OAuth refresh token is single-use and
  rotating, so two copies of `auth.json` invalidate each other.

That leaves the config file, and the lifecycle is: **patch → spawn detached → wait ~10 s → revert →
exit.** The app parses the config once at startup and caches it, so the patch only has to survive
the handoff. Four specifics matter:

- The inserted block is bracketed by marker comments and removed surgically from the file *as it
  then stands*, so concurrent writes by codex itself (trust levels, plugin state) survive the revert.
- A backup is taken, and recovered on the next run if a crash strands the patch.
- `auth.json` is **never** read or modified.
- Worst case, if the grace period is ever too short, the app reads the reverted config and talks to
  OpenAI directly. No compression, no metering, nothing broken, but also no warning.

The config keys themselves are documented. Writing them into another application's config file is
our own pattern, and it is the most invasive thing Edgee does short of the relay. It is time-boxed
to seconds and fully reverted.

---

## Transport C: local relay

Implementation: [`src/commands/relay/mod.rs`](../src/commands/relay/mod.rs) and
[`src/commands/relay/handler.rs`](../src/commands/relay/handler.rs)

Used only for the three GUI targets that expose no configuration surface: `cursor`,
`copilot-vscode`, `claude-desktop`. `edgee relay` is itself a hidden subcommand, because transport
is an implementation detail rather than public UX.

**This is the part of the product that deserves the most scrutiny.** It is a compatibility bridge
for three surfaces, not the core mechanism. We use it because it preserves the same one-command
onboarding as every other target. Each of these three surfaces can also be pointed at a gateway
through documented vendor settings, which is the fallback if interception ever breaks — see
[If the relay breaks](#if-the-relay-breaks-fall-back-to-transport-a).

### What it is

A `hudsucker`-based HTTP proxy bound to **127.0.0.1** on a fixed per-agent port (41100–41400). It is
started by the launch command and runs in the foreground for that command's lifetime. For the GUI
editors it keeps serving after the editor window is handed off, until the user presses Ctrl-C. It is
never installed, never registered as a service, and never survives the terminal.

The child app is pointed at it using conventional, documented plumbing:

| Mechanism | Documented at |
| --- | --- |
| `HTTPS_PROXY` / `HTTP_PROXY` / `NO_PROXY` env vars | De-facto standard, honoured by Node, curl and Electron |
| `--proxy-server=<uri>` (Cursor, Claude Desktop) | Chromium switch, surfaced by [VS Code](https://code.visualstudio.com/docs/setup/network) and [Electron](https://www.electronjs.org/docs/latest/api/command-line-switches) |
| `NODE_EXTRA_CA_CERTS` | [Node.js CLI docs](https://nodejs.org/api/cli.html#node_extra_ca_certsfile), the supported way to add a CA to a Node process |
| `CODEX_CA_CERTIFICATE` | Codex's own CA override |
| `cursor.general.disableHttp2: true` | Cursor's HTTP/1.1 compatibility mode, merged into the user's `settings.json`. The relay can only MITM HTTP/1.1 |

`NO_PROXY` always includes loopback, so local MCP servers and other localhost services bypass the
relay entirely.

### What it decrypts, and what it does not

TLS termination is decided **per host, at CONNECT time**, before any bytes are decrypted
(`should_intercept` in `handler.rs`). Only these hosts are ever decrypted:

```
api.anthropic.com    api.openai.com    chatgpt.com    cursor.sh
```

Two more, `githubcopilot.com` and `api.github.com`, are decrypted **only under the Copilot-in-VS-Code
relay**. Every other host is **blind-tunnelled**: the bytes pass through opaquely and the app
validates the real certificate. That covers telemetry, updates, auth, extension marketplaces and the
user's own MCP servers. This is a deliberate, tested boundary, not a best effort.

Within the decrypted hosts, only inference paths are rerouted (`/v1/messages`, `/v1/responses`,
`/v1/chat/completions`, and the Cursor and Codex equivalents). Everything else on the same host is
forwarded untouched.

### The certificate question

The relay generates a local CA, stored `0600` in the user's Edgee config directory. For Cursor and
VS Code that CA is handed to the child process alone, through `NODE_EXTRA_CA_CERTS`. **Nothing is
installed in any system trust store**, and the trust disappears when the process exits.

Claude Desktop is the exception, and the one place Edgee touches the OS. Its Chromium net stack
consults only the macOS **System** keychain, so `edgee launch claude-desktop` asks for `sudo`
**once** to trust a CA. Four mitigations, all in code:

- It is a **separate, dedicated CA** (`Edgee Claude Desktop CA`), never the shared relay CA.
- It carries an **X.509 name constraint permitting only `anthropic.com`** (RFC 5280), with all IPv4
  and IPv6 address space excluded. Chromium enforces name constraints on locally-trusted roots, so
  even a leaked key could not vouch for any other domain.
- Trust is matched by SHA-1 fingerprint, so a regenerated CA reads as untrusted and gets refreshed
  rather than silently shadowed, and stale duplicates are purged.
- `edgee relay claude-desktop --untrust` removes it, and fails loudly rather than silently if
  removal is denied.

It is nonetheless a persistent system trust root installed by a third-party tool. It is defensible,
scoped, reversible and documented in the README, but expect it to be the single most scrutinised
item in any security review, and never describe the product as if it did not exist.

### If the relay breaks: fall back to Transport A

The relay is a convenience layer, not a dependency. **All three GUI surfaces expose a documented,
vendor-supported way to point them at a custom gateway.** Edgee does not use those paths today
because they are manual GUI configuration that a launch command cannot drive, which would cost the
one-command onboarding that lets the product spread across a 500-developer org on its own. But if a
vendor update breaks interception, the surface migrates to Transport A rather than being lost.

| Surface | Documented Transport A path | What it costs |
| --- | --- | --- |
| `claude-desktop` | **Anthropic's own third-party inference configuration**: Developer → Configure Third-Party Inference, or [distributed by an administrator through managed settings](https://code.claude.com/docs/en/llm-gateway-connect#configure-each-surface) | Enabling Developer Mode on each device, or an admin rollout. The app then runs local sessions only: no SSH or Anthropic-hosted cloud environments, and no Remote Control |
| `copilot-vscode` | **Custom Endpoint provider** in `chatLanguageModels.json` (`url`, `apiKey`, `apiType`, `requestHeaders`), [documented for "self-hosted models, enterprise gateways"](https://code.visualstudio.com/docs/copilot/customization/language-models) | This is BYOK, so it *replaces* the Copilot subscription rather than preserving it. Copilot Business/Enterprise admins also control whether the policy is allowed at all |
| `cursor` | **Settings → Models → OpenAI API Key + Override OpenAI Base URL** ([Bring your own API key](https://cursor.com/help/models-and-usage/api-keys)) | Chat models only, as tab completion stays on Cursor's own models. The override also disables Cursor's built-in Pro models, Cursor's Zero Data Retention policy no longer applies, and there is no per-model base URL |

Two things about that table are worth saying out loud.

First, the Claude Desktop fallback is the strongest of the three. Anthropic documents it as a
first-class gateway surface, and an admin-distributed configuration needs nothing from the developer
at all. For an enterprise rollout it is arguably *better* than the relay.

Second, the Copilot and Cursor fallbacks change the commercial shape of the integration, because
both are BYOK paths that consume API credits instead of the seat the customer already pays for. For
those two, "we have a fallback" means the surface stays supported, not that the value proposition is
unchanged.

So the failure mode across all three is **degraded onboarding, not a dead surface**, and the
migration is a configuration-writing change in the CLI rather than a rebuild.

---

## Delivering org plugins

A second axis, orthogonal to the three transports. A transport decides where the agent's **LLM
traffic** goes; plugin delivery decides whether Edgee can put an org's **skills, subagents, hooks
and MCP servers** in front of that agent. A target can score well on one and badly on the other —
Kimi Code is the cleanest Transport A integration in the catalogue and still cannot take a full
plugin bundle.

Implementation: `src/commands/util/plugins/`, with the per-agent matrix in `delivery.rs`.

**The rule the design turns on.** Edgee materializes components into a directory **it owns** and
points the agent at it. It never writes into a user-owned config file, and never into a git working
tree. A component kind that cannot be redirected for a given agent is reported as *undelivered*
rather than worked around — no silent half-delivery. That rule is what makes most of the gaps below
gaps rather than bugs.

| Target | Skills | Subagents | Hooks | MCP | How it lands |
| --- | :-: | :-: | :-: | :-: | --- |
| `claude` | ✅ | ✅ | ✅ | ✅ | `claude --plugin-dir` — session-only, all four kinds at once, nothing written to `~/.claude` |
| `codebuddy` | ✅ | ✅ | ✅ | ✅ | `CODEBUDDY_PLUGIN_DIRS` — the same bundle byte for byte, down to the `${CLAUDE_PLUGIN_ROOT}` aliases |
| `opencode` | ✅ | ✅ | ❌ | ✅ | config fragments on the redirected document; no `hooks` key exists — its extension point is JS plugins |
| `codex` | ✅¹ | ❌ | ⏳ | ✅ | skills via a `CODEX_HOME` symlink mirror; MCP via `-c mcp_servers`; no user-defined subagents exist |
| `crush` | ✅ | ✅ | ⏳ | ✅ | config fragments on the redirected document |
| `kimi` | ⏳ | ❌ | ❌ | ❌ | not yet wired — see below |
| `pi` | ⏳ | ⏳ | ⏳ | ⏳ | not yet wired |
| `omp` | ⏳ | ⏳ | ⏳ | ⏳ | not yet wired |
| `cursor`, `copilot-vscode`, `claude-desktop` | ❌ | ❌ | ❌ | ❌ | relay targets — Edgee never spawns the process, so there is no launch to attach a directory to |
| `codex-desktop` | ❌ | ❌ | ❌ | ❌ | launched, but reads the real Codex config root that Edgee patches only for the handoff and reverts; it cannot use the symlink mirror because `auth.json` holds a single-use rotating token |

¹ Unix only. The mirror is symlinks, Windows needs elevated privileges for those, and copying a
config root would duplicate the user's credentials — not a trade worth making.

⏳ marks a mechanism that is plausible but unverified. Those stay undelivered on purpose: injecting
config an agent silently ignores is worse than reporting that nothing was delivered.

### What decides the answer

Two questions, in order:

1. **Does the CLI spawn the process?** If not (`cursor`, `copilot-vscode`, `claude-desktop`), there
   is no launch to attach anything to and the whole row is `❌`. This is the same boundary Transport
   C draws, for the same reason.
2. **Is there a documented way to point the agent at a directory, without editing a file the user
   owns?** A flag or env var means delivery. Discovery driven only by the user's config file or data
   root means no delivery — the rule above forbids writing there.

That second question is why Claude Code and CodeBuddy get everything and most others get a subset:
`--plugin-dir` is a *session-scoped, additive* pointer at an arbitrary directory, and few agents
ship an equivalent.

### Kimi Code: skills yes, bundle no

Kimi has a real plugin format — a manifest at `kimi.plugin.json` or `.kimi-plugin/plugin.json`, with
`${KIMI_PLUGIN_ROOT}` substitution inside it, and a dedicated "Plugin Instructions" block in the
system prompt. It is a close analogue of Claude Code's `.claude-plugin/plugin.json` and
`${CLAUDE_PLUGIN_ROOT}`. What it lacks is the pointer: plugins are installed from a **marketplace**
catalog into the user's data root, and there is no `--plugin-dir` equivalent to load one from an
arbitrary directory for a single session. Under the rule above, that makes the bundle undeliverable
today.

Skills are a different story. `kimi --skills-dir <dir>` is repeatable and takes an arbitrary
directory — exactly the shape delivery needs. One caveat decides how it must be used:

> `--skills-dir <dir>` — Load skills from this directory **instead of** auto-discovered user and
> project directories.

It **replaces** discovery rather than adding to it, unlike Claude Code's additive `--plugin-dir`. So
delivering Edgee skills naively would hide every skill the user already has. Because the flag is
repeatable, the fix is to pass the user's own discovered directories alongside the Edgee tree — but
that means the CLI has to know Kimi's discovery rules, which is a materially larger job than
appending one flag, and it is why this row is `⏳` rather than `✅`.

Subagents have a near-miss: `--agent-file <path>` is repeatable and loads an agent definition from a
Markdown file, but it *selects* that agent for the session and cannot be combined with
`--session` / `--continue`. That is a way to start one agent, not a way to register a set. Hooks and
MCP servers have no CLI surface at all — both are `config.toml`, which Edgee will not write.

---

## FAQ

### 1. How does interception work, exactly?

- **CLI agents:** by setting the environment variables and config keys the vendor publishes for
  pointing their agent at a gateway.
- **The ChatGPT desktop app:** by a patch of its own config file, reverted about ten seconds later.
- **The three GUI apps:** by a local loopback proxy that decrypts four known inference hosts and
  blind-tunnels everything else.

None of the three involves binary patching, code injection, credential extraction, an endpoint
agent, or a daemon.

### 2. Is it consistent with the providers' terms?

The *technical* mechanisms for Transport A are the vendors' own documented extension points, and
Anthropic explicitly documents subscription traffic routed through a third-party gateway, including
what such a gateway must forward. That is a strong position, and it is verifiable from public docs.

Before the clauses themselves, one thing to settle: **which contract applies depends on how the
developer pays for the model**, and both cases occur in a real Edgee deployment.

| What the developer is on | Governing contract |
| --- | --- |
| A ChatGPT subscription (Plus, Pro) | OpenAI [Terms of Use](https://openai.com/policies/row-terms-of-use/) |
| A claude.ai subscription | Anthropic [Consumer Terms](https://www.anthropic.com/legal/consumer-terms) |
| OpenAI API credits, or their own OpenAI key under BYOK | OpenAI [Services Agreement](https://openai.com/policies/services-agreement/) |

This matters because the clauses do not carry across. OpenAI's prohibition on transferring API keys,
for instance, lives in the Services Agreement, which states that it "does not apply to OpenAI
services used by consumers or individuals". It therefore says nothing about a developer on a ChatGPT
subscription. Reading a business-tier clause onto a consumer plan, or the reverse, is the easiest
way to reach a wrong conclusion here, so the *Where* column below names the document behind each
clause.

With that settled, here are the restrictions that could plausibly be raised, and where Edgee sits
against each.

| Clause | Where | Our position |
| --- | --- | --- |
| "You may not share your Account login information, Anthropic API key, or Account credentials with anyone else" / "You may not share your account credentials or make your account available to anyone else" | Anthropic §2; OpenAI ToU, *Registration* | No credential is shared. One user authenticates with their own credential, on their own machine, for their own use. It transits the gateway inside the request their own client initiates, and Edgee neither extracts, stores nor replays it. Anthropic's [gateway protocol](https://code.claude.com/docs/en/llm-gateway-protocol) specifies what a gateway must do with exactly this traffic: a vendor that writes that spec is not treating transit as making an account available to someone else |
| "Modify, copy, lease, sell or distribute any of our Services"; no reselling the Services; "buy, sell, or transfer API keys from, to, or with a third party" | OpenAI ToU, *What you cannot do*; Anthropic §3; OpenAI Services Agreement §3.3(g) | Edgee sells no model access. Customers arrive with their own subscription or their own provider keys, and what they pay Edgee for is the gateway itself: routing, compression, metering and governance. No provider credential is minted, brokered or transferred, on either the consumer or the API path |
| "circumvent any rate limits or restrictions or bypass any protective measures or safety mitigations"; "violate or circumvent Usage Limits or otherwise configure the Services to avoid Usage Limits" | OpenAI ToU + Services Agreement §3.3(h)–(i); Anthropic §3–§4 | Nothing is circumvented. Because the user's own credential stays active, the provider enforces their quota exactly as before. Compression *reduces* consumption against that same cap, and budget rerouting means the request is not sent to that provider at all — declining to consume a service is not evading its limits. Note that both texts frame this prohibition as a species of interfering with or disrupting the Services |
| "Automatically or programmatically extract data or Output" | OpenAI ToU, *What you cannot do* | Metering counts tokens; it does not retain Output. Debug logs are opt-in and encrypted to a public key that the CLI derives from the user's own passphrase and then discards the private half of ([`src/crypto.rs`](../src/crypto.rs)). The passphrase never reaches Edgee, so the ciphertext is undecryptable by us. No Output corpus is accumulated, and none is used for training — which is the clause this one sits next to |
| "access the Services through automated or non-human means" outside an API key or where explicitly allowed | Anthropic §3(7) | The client is Claude Code, Anthropic's own product, explicitly supported on subscription plans. Edgee originates no traffic; it forwards what the user's agent sends |
| No reverse engineering, decompiling, or discovering underlying components | Anthropic §3(3); OpenAI ToU + Services Agreement §3.3(d) | Transports A and B use published configuration surfaces only. Transport C reads a proprietary wire protocol as it crosses a proxy running on the user's own machine, which is observation of one's own traffic rather than decompilation or model extraction. It remains the mechanism least anchored in a documented contract |

**Our view.** These terms govern *who* uses an account and *whether limits are evaded*, not which
network path a request takes. Edgee changes only the path: one user, one credential, one quota, one
bill, on infrastructure their employer chose. That is what every enterprise egress proxy already
does. If credential transit through infrastructure were itself the violation, TLS-inspecting
corporate proxies would be too.

One design constraint keeps that argument true, and it is worth stating because it is what a review
will probe: **Edgee never multiplexes several users onto one subscription, and never rotates
credentials to defeat a per-account cap.** That is the line, and the architecture does not permit
crossing it, because provider keys are issued per user and per agent.

On routing specifically, Anthropic states that it "doesn't support routing Claude Code to non-Claude
models through any gateway". That is a support-scope statement in developer documentation, not a
prohibition in the terms: it means Anthropic will not help debug it, not that it is disallowed. The
substantive obligations above are unaffected by which model ultimately serves a request.

Residual risk, stated plainly. Terms can change, and neither provider has a clause that addresses
proxies by name, which cuts both ways. The clause with the most textual friction is OpenAI's ban on
programmatically extracting Output, because a metering gateway necessarily sees the stream; the
answer there is architectural rather than interpretive, which is why the end-to-end encryption of
debug logs matters. The broader mitigation is structural: BYOK and API-credit routing are untouched
by any change to consumer-plan terms, so no single set of terms governs the business.

### 3. Can a vendor update break the product?

Yes, at very different severities per transport. See
[Fragility](#fragility-what-a-vendor-update-can-break) below.

### 4. Does it need a local proxy, a certificate, a wrapper, or an endpoint agent?

- **Wrapper:** yes — `edgee launch <agent>`, or a PATH shim installed by `edgee alias`.
- **Local proxy:** only for `cursor`, `copilot-vscode` and `claude-desktop`. Loopback only,
  foreground, and it dies with the command.
- **Certificate:** only for those same three. Process-scoped for two of them; for Claude Desktop, one
  persistent, name-constrained, removable system root on macOS.
- **Endpoint agent, daemon or kernel module:** **never**.

### 5. Can providers block Edgee, technically or contractually?

Technically, the answer differs per transport:

- **Transport A** would require removing gateway support from their own product — a feature their
  enterprise customers depend on, and one Anthropic documents as an org rollout path. Removing it is
  possible but costly for them, and it would hit every gateway product at once rather than Edgee
  specifically.
- **Whatever the transport**, fine-grained blocking is possible in principle: client attestation,
  refusing subscription credentials on requests with a non-Anthropic TLS peer, or terms enforcement
  against known gateway IPs. Nothing in the clients does this today, and Anthropic's documented
  `401`-on-stripped-OAuth behaviour implies they expect and accommodate this traffic. But that is a
  product decision on their side, not a technical impossibility on ours.
- **Transport B** is the cheapest to block, because it is one vendor, one surface, and the lever sits
  entirely inside their own application. The ChatGPT desktop app could ignore `model_providers` when
  it spawns its bundled `app-server`, pin it to the first-party provider, or move the setting out of
  `config.toml`. None of those would touch the `codex` CLI, so OpenAI could close this surface at no
  cost to the CLI users who depend on custom providers. It could also break by accident: a config
  schema change, or a cold start slower than the ~10 s grace window, degrades it silently. There is
  no second configuration path on this surface, so the only fallback is the `codex` CLI — a
  different product for the user, even though it reaches the same models.
- **Transport C** could be broken cheaply, and possibly unintentionally, by certificate pinning, an
  HTTP/2-only transport, or a protocol change. Unlike Transport B, though, all three of these
  surfaces have a documented vendor-supported gateway configuration to fall back on, so a break
  costs onboarding simplicity rather than the surface itself. See
  [If the relay breaks](#if-the-relay-breaks-fall-back-to-transport-a).

Contractually, a provider can change subscription terms at any time. BYOK and API-credit routing
work identically and are unaffected, so the business does not depend on any single credential mode.

### 6. Are there official partnerships or integrations?

**No.** There is no partnership, endorsement, certification or private API with Anthropic, OpenAI,
GitHub, Cursor, or any other vendor named in this document.

What is true, and strong enough on its own: Edgee uses the vendors' own published gateway and
provider configuration surfaces, and Anthropic publishes a formal protocol contract for exactly the
class of product Edgee is.

---

## Fragility: what a vendor update can break

| Transport | Blast radius | Failure mode | Recovery |
| --- | --- | --- | --- |
| **A — CLI env/config** | Low | A renamed env var or config key breaks one target, and requests fall back to the vendor's own endpoint | One-line change, shipped in a CLI release |
| **B — codex-desktop patch** | Medium | A schema change, or a cold start exceeding the 10 s grace window, silently sends traffic direct to OpenAI | Against accidental drift: the config keys are shared with the CLI, so they move together. Against a deliberate close: no fallback on this surface, see [FAQ 5](#5-can-providers-block-edgee-technically-or-contractually) |
| **C — relay** | **High** | Certificate pinning, an HTTP/2-only transport, or a protocol change breaks the relay outright, with no relay-side workaround | [Migrate the surface to Transport A](#if-the-relay-breaks-fall-back-to-transport-a): all three have a documented vendor gateway path, at the cost of manual setup |

Fragility is therefore highest exactly where usage is lowest, which is the right way round.

Three structural mitigations are already in the codebase. Every target degrades to the vendor's own
endpoint rather than failing closed. The launch catalogue is deliberately wide, so no single vendor
decision removes the product. And `src/commands/launch/README.md` treats target names as long-lived
public API, so integrations can be swapped underneath without breaking user aliases.
