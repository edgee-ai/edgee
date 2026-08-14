# How Edgee wraps coding agents

**Status:** reference document, kept in sync with `src/commands/launch/` and `src/commands/relay/`.
**Audience:** engineers working on launch targets, and anyone answering technical questions about how interception works.

This document answers, mechanism by mechanism and with links to each vendor's own documentation:

- How does interception actually work?
- Is it consistent with the providers' documented configuration surfaces?
- Can a vendor update break it?
- Does it need a local proxy, a certificate, a wrapper, or an endpoint agent?
- Can a provider block Edgee technically or contractually?
- Are there official partnerships or integrations?

The short answer: 

**For the CLI agents (the large majority of usage) Edgee sets the configuration variables the vendors themselves publish for exactly this purpose.** There is no binary patching, no code injection, no credential extraction, no reverse-engineered API.  

For three GUI targets (Cursor, Copilot in VS Code, Claude Desktop) Edgee runs a **local, opt-in, scoped MITM relay**, which is a materially different and more sensitive posture. That difference is stated plainly in [Transport C](#transport-c-local-relay) and in [Fragility](#fragility-what-a-vendor-update-can-break).

---



## The three transports

Every launch target uses exactly one of three transports. Nothing else exists in this repository.


| Transport                             | What Edgee does                                                                     | Targets                                             | Vendor-documented?                                                                                       |
| ------------------------------------- | ----------------------------------------------------------------------------------- | --------------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| **A. Environment / config injection** | Sets documented env vars or CLI config flags on the child process only              | `claude`, `codex`, `opencode`, `codebuddy`, `crush` | **Yes**, each variable below links to the vendor's own docs                                              |
| **B. Config-file patch**              | Temporarily writes a provider block into the app's own config file, then reverts    | `codex-desktop`                                     | **Partly**, the config keys are documented; patching another app's file is our own pattern               |
| **C. Local relay (MITM)**             | Runs a loopback proxy, decrypts only known inference hosts, reroutes to the gateway | `cursor`, `copilot-vscode`, `claude-desktop`        | **No**, uses documented proxy/CA plumbing, but the interception itself is outside any published contract |


Transport A covers the products that drive most enterprise coding-agent spend. Transport C is the
compatibility path for GUI apps that expose no configuration surface at all.

**Nothing runs as a daemon, a system extension, a kernel module, or an endpoint agent.** The relay
is a foreground process started by the launch command and bound to loopback; it is never installed
or registered, and it does not survive the terminal. Edgee never modifies the agent's binary, its
installed files (except `codex-desktop`'s config, reverted ~10 s later), or its stored credentials.

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

It also documents `x-claude-code-session-id`, which Anthropic supplies specifically so a gateway can
"aggregate all requests from one session without parsing request bodies", i.e. per-session
attribution is an intended gateway capability, not something we extract.

#### The subscription compatibility, answered by Anthropic's documentation

This is an important point and Anthropic documents the distinction between the two gateway modes:

> "While a gateway credential variable or `apiKeyHelper` is active, a developer's claude.ai
> subscription isn't used: the credential replaces the subscription login for that session […]
> `ANTHROPIC_BASE_URL` is the variable that points Claude Code at the gateway. **Setting only that
> variable, without a gateway credential, doesn't replace the subscription. Requests still route
> through the gateway, but a saved claude.ai login remains the active credential, so its usage
> limits and billing apply.** Gateways that pass this traffic on to Anthropic must forward the OAuth
> capability in `anthropic-beta`."
>
> - [Other LLM gateways § Subscriptions and gateways](https://code.claude.com/docs/en/llm-gateway#subscriptions-and-gateways)

**Edgee sets** `ANTHROPIC_BASE_URL` **and** `ANTHROPIC_CUSTOM_HEADERS`**. It does not set**
`ANTHROPIC_API_KEY`**,** `ANTHROPIC_AUTH_TOKEN`**, or** `apiKeyHelper`**.** `x-edgee-api-key` is a *custom* header addressed to the Edgee gateway; it is not an Anthropic credential and Claude Code does not treat it as one. The user's own claude.ai OAuth login therefore remains the active credential, the request is billed to their own subscription, and their own usage limits apply, exactly the configuration Anthropic describes above.

So "works with consumer subscriptions" is not a workaround. It is the documented behaviour of the
variable Anthropic publishes for gateways, and Anthropic documents the gateway-side obligation that
comes with it (forward `anthropic-beta` verbatim, including the OAuth capability, or those requests
fail with `401`). Edgee's gateway meets that obligation.

Two consequences worth stating:

1. Edgee never sees, stores, or replays the user's Anthropic credential. It is forwarded in the
  `Authorization` header the client sets, and Edgee's own auth travels in a separate header.
2. Anthropic is aware of and specifies this traffic pattern. What it does *not* do is endorse
  specific vendors: "Anthropic doesn't endorse, maintain, or audit third-party gateway products,
   and doesn't support routing Claude Code to non-Claude models through any gateway." That second
   clause matters for the routing pillar (see [FAQ 2](#faq)).



### Codex CLI (`edgee launch codex`)

Implementation: [`src/commands/launch/codex.rs`](../src/commands/launch/codex.rs)

Edgee passes documented `-c` config overrides to the child process:

```
-c model_provider="edgee-cli"
-c model_providers.edgee-cli.name="EDGEE"
-c model_providers.edgee-cli.base_url="https://<gateway>/v1"
-c model_providers.edgee-cli.http_headers={"x-edgee-api-key"=…,"x-edgee-session-id"=…}
-c model_providers.edgee-cli.wire_api="responses"
```

Every key is in OpenAI's published config reference
(`model_provider`[,](https://learn.chatgpt.com/docs/config-file/config-reference) `model_providers.<id>`[,](https://learn.chatgpt.com/docs/config-file/config-reference) `name`[,](https://learn.chatgpt.com/docs/config-file/config-reference) `base_url`[,](https://learn.chatgpt.com/docs/config-file/config-reference) `wire_api`[,](https://learn.chatgpt.com/docs/config-file/config-reference)
`http_headers`), and `-c` one-off
overrides are documented at
[Advanced config § one-off overrides from the CLI](https://learn.chatgpt.com/docs/config-file/config-advanced#one-off-overrides-from-the-cli).
`http_headers` exists precisely to let a custom provider carry gateway routing headers.

Edgee does not touch `~/.codex/auth.json`, does not set `CODEX_ACCESS_TOKEN`, and does not set
`env_key`. Codex forwards its own credential to the configured `base_url`; Edgee's key rides in a
separate header. Same shape as Claude Code: **the agent keeps its identity, Edgee adds its own.**

### OpenCode (`edgee launch opencode`)

Implementation: [`src/commands/launch/opencode.rs`](../src/commands/launch/opencode.rs)

Edgee reads the user's existing `opencode.json`/`.jsonc`, merges in a `provider.edgee` block, writes
the result to a per-session temp file, and points the child at it with `OPENCODE_CONFIG`. The user's
own file is never modified.

- [`OPENCODE_CONFIG`](https://opencode.ai/docs/config/): "Specify a custom config file path using
the `OPENCODE_CONFIG` environment variable."
- [Custom / OpenAI-compatible providers](https://opencode.ai/docs/providers/) documents exactly
the block Edgee writes: `npm: "@ai-sdk/openai-compatible"`, `options.baseURL`, `options.apiKey`,
`options.headers` ("Custom headers sent with each request"), and the `models` map with
`limit.context` / `limit.output`.

Edgee populates the `models` map from the gateway's `/v1/models` listing, and the per-model
`cost` rates from the Edgee catalog so OpenCode's own cost display stays accurate.

Note that OpenCode and Crush are **new-provider** integrations: the user selects "Edgee" as a
provider and pays through their Edgee/BYOK credentials. No subscription interception is involved.

### CodeBuddy (`edgee launch codebuddy`)

Implementation: [`src/commands/launch/codebuddy.rs`](../src/commands/launch/codebuddy.rs)

```
CODEBUDDY_BASE_URL       = https://<gateway>/v1
CODEBUDDY_CUSTOM_HEADERS = x-edgee-api-key: …\nx-edgee-session-id: …
```

Both are documented in CodeBuddy's
[Environment Variables Reference](https://www.codebuddy.ai/docs/cli/env-vars). `CODEBUDDY_BASE_URL`
is the documented way to point the CLI at "any third-party model service compatible with the
Anthropic protocol", and `CODEBUDDY_CUSTOM_HEADERS` is documented as being "useful for
gateways/proxies that require auth or routing headers", a verbatim description of this use case.

> ⚠️ **Known defect.** `codebuddy.rs:60-62` builds the repo header in TOML syntax
> (`,"x-edgee-repo"="…"`) copied from `codex.rs`, then appends it to a newline-delimited header
> string. Repo attribution is therefore malformed for CodeBuddy sessions. Small fix, not yet made.



### Crush (`edgee launch crush`)

Implementation: [`src/commands/launch/crush.rs`](../src/commands/launch/crush.rs)

Edgee reads the user's global `crush.json`, merges in a `providers.edgee` entry
(`type: "openai-compat"`, `base_url`, `api_key`, `extra_headers`, `models[]`), writes it to a
per-session temp directory, and points the child there with `CRUSH_GLOBAL_CONFIG`. Project-level
config still wins, as Crush intends.

`CRUSH_GLOBAL_CONFIG` and the `openai-compat` provider type are documented in the [Crush README](https://github.com/charmbracelet/crush). 
See [Fragility](#fragility-what-a-vendor-update-can-break): Crush has since introduced a `crushrc`
format and now describes the JSON config as deprecated, which makes this the least future-proof of
the Transport A integrations.

---



## Transport B: config-file patch (`codex-desktop`)

Implementation: [`src/commands/launch/codex_desktop.rs`](../src/commands/launch/codex_desktop.rs);
full rationale in [`src/commands/launch/README.md`](../src/commands/launch/README.md#codex-desktop--config-patch-not-relay).

The ChatGPT desktop app runs a bundled `codex app-server` that reads `$CODEX_HOME/config.toml` and
honours the same `model_providers` keys as the CLI. The app supplies its own argv, so `-c` injection
is impossible; `--profile` is argv-only, with no env-var equivalent; and a private `CODEX_HOME` is
unsafe because the ChatGPT OAuth refresh token is single-use and rotating, so two copies of
`auth.json` invalidate each other.

So the lifecycle is: **patch → spawn detached → wait ~10 s → revert → exit.** The app parses the
config once at startup and caches it, so the patch only has to survive the handoff. Specifics that
matter:

- The insert is bracketed by marker comments and removed surgically from the *current* file, so
concurrent writes by codex itself (trust levels, plugin state) survive the revert.
- A backup is taken and recovered on the next run if a crash strands the patch.
- `auth.json` is **never** read or modified.
- Worst case if the grace period is ever too short: the app reads the reverted config and talks to
OpenAI directly. No compression, no metering, nothing broken, but also no warning.

The config keys are documented. Writing them into another application's config file is our own
pattern, and it is the most invasive thing Edgee does short of the relay. It is time-boxed to
seconds and fully reverted.

---



## Transport C: local relay

Implementation: [`src/commands/relay/mod.rs`](../src/commands/relay/mod.rs), [`src/commands/relay/handler.rs`](../src/commands/relay/handler.rs)

Used only for GUI targets with no configuration surface: `cursor`, `copilot-vscode`, `claude-desktop`. `edgee relay` is a hidden subcommand, transport is an implementation detail, not public UX.

**This is the honest sensitive point of the product.** It is a compatibility bridge for three surfaces, not the core mechanism.  

We chose this approach for these three surfaces in order to allow for onboarding that is just as simple and efficient as with the other harnesses. However, each of these three GUI targets also allows for the installation of a gateway in a more standard way, which can be used as a fallback in case of future incompatibility.

### What it is

A `hudsucker`-based HTTP proxy bound to **127.0.0.1** on a fixed per-agent port (41100–41400). It is
started by the launch command and runs in the foreground for that command's lifetime. For the GUI
editors it keeps serving after the editor window is handed off, until the user presses Ctrl-C. It is
never installed, never registered as a service, and never survives the terminal. The child app is
pointed at it using conventional, documented plumbing:


| Mechanism                                          | Documented at                                                                                                                                                             |
| -------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `HTTPS_PROXY` / `HTTP_PROXY` / `NO_PROXY` env vars | de-facto standard; honoured by Node, curl, Electron                                                                                                                       |
| `--proxy-server=<uri>` (Cursor, Claude Desktop)    | Chromium switch, [surfaced by VS Code](https://code.visualstudio.com/docs/setup/network) and [Electron](https://www.electronjs.org/docs/latest/api/command-line-switches) |
| `NODE_EXTRA_CA_CERTS`                              | [Node.js CLI docs](https://nodejs.org/api/cli.html#node_extra_ca_certsfile), the supported way to add a CA to a Node process                                              |
| `CODEX_CA_CERTIFICATE`                             | Codex's own CA override                                                                                                                                                   |
| `cursor.general.disableHttp2: true`                | Cursor's HTTP/1.1 compatibility mode, merged into the user's `settings.json` (relay can only MITM HTTP/1.1)                                                               |


`NO_PROXY` always includes loopback, so local MCP servers and other localhost services bypass the
relay entirely.

### What it decrypts, and what it does not

TLS termination is decided **per host, at CONNECT time**, before any bytes are decrypted
(`should_intercept` in `handler.rs`). Only these hosts are ever decrypted:

```
api.anthropic.com    api.openai.com    chatgpt.com    cursor.sh
```

plus, **only under the Copilot-in-VS-Code relay**, `githubcopilot.com` and `api.github.com`. Every other host (telemetry, updates, auth, extension marketplaces, the user's own MCP servers) is **blind-tunnelled**: bytes pass through opaquely and the app validates the real certificate. This is a deliberate, tested boundary, not a best effort.

Within those hosts, only inference paths are rerouted (`/v1/messages`, `/v1/responses`,
`/v1/chat/completions`, and the Cursor/Codex equivalents); everything else on the same host is
forwarded untouched.

### The certificate question

The relay generates a local CA, stored `0600` in the user's Edgee config directory. For Cursor and VS Code the CA is handed to the child process only, through `NODE_EXTRA_CA_CERTS,` **nothing is installed in any system trust store**, and the trust disappears when the process exits.

Claude Desktop is the exception, and the one place Edgee touches the OS. Its Chromium net stack
consults only the macOS **System** keychain, so `edgee launch claude-desktop` asks for `sudo` **once**
to trust a CA. Mitigations, all in code:

- It is a **separate, dedicated CA** (`Edgee Claude Desktop CA`), never the shared relay CA.
- It carries an **X.509 name constraint permitting only** `anthropic.com` (RFC 5280), with all IPv4
and IPv6 address space excluded. Chromium enforces name constraints on locally-trusted roots, so
even if the key leaked it could not vouch for any other domain.
- Trust is matched by SHA-1 fingerprint, so a regenerated CA is detected rather than silently
shadowed, and stale duplicates are purged.
- `edgee relay claude-desktop --untrust` removes it, and fails loudly rather than silently if
removal is denied.

This is still a persistent system trust root installed by a third-party tool. It is defensible, scoped, reversible, and documented in the README, but expect it to be the single most scrutinised item in a security review, and do not let a one-pager imply it doesn't exist.

### If the relay breaks: fall back to Transport A

The relay is a convenience layer, not a dependency. **All three GUI surfaces expose a documented,
vendor-supported way to point them at a custom gateway.** We do not use it today because it is
manual GUI configuration that cannot be driven from a launch command, which would cost us the
one-command onboarding that makes the product adopt itself across a 500-developer org. But if a
vendor update breaks interception, the surface is migrated to Transport A rather than lost.

| Surface | Documented Transport A path | What it costs |
|---|---|---|
| `claude-desktop` | **Anthropic's own third-party inference configuration** — Developer → Configure Third-Party Inference, or [distributed by an administrator through managed settings](https://code.claude.com/docs/en/llm-gateway-connect#configure-each-surface) | Enabling Developer Mode per device (or an admin rollout). The app then runs local sessions only: no SSH or Anthropic-hosted cloud environments, no Remote Control |
| `copilot-vscode` | **Custom Endpoint provider** in `chatLanguageModels.json` — `url`, `apiKey`, `apiType`, `requestHeaders`, [documented for "self-hosted models, enterprise gateways"](https://code.visualstudio.com/docs/copilot/customization/language-models) | This is BYOK: it *replaces* the Copilot subscription rather than preserving it, and Copilot Business/Enterprise admins control whether the policy is allowed at all |
| `cursor` | **Settings → Models → OpenAI API Key + Override OpenAI Base URL** ([Bring your own API key](https://cursor.com/help/models-and-usage/api-keys)) | Chat models only (tab completion stays on Cursor's own models), the override disables Cursor's built-in Pro models, Cursor's Zero Data Retention policy no longer applies, and there is no per-model base URL |

Two honest observations about that table. First, the Claude Desktop fallback is the strongest of the
three: Anthropic documents it as a first-class gateway surface, and an admin-distributed
configuration needs nothing from the developer at all — for an enterprise rollout it is arguably
*better* than the relay. Second, the Copilot and Cursor fallbacks change the commercial shape of the
integration, because both are BYOK paths that consume API credits instead of the seat the customer
already pays for. For those two, "we have a fallback" means the surface stays supported, not that
the value proposition is unchanged.

So the failure mode across all three is **degraded onboarding, not a dead surface** — and the
migration is a configuration-writing change in the CLI, not a rebuild.

---



## FAQ

**1. How does interception work, exactly?**
For CLI agents: by setting the environment variables and config keys the vendor publishes for pointing their agent at a gateway. 

For the ChatGPT desktop app: by a time-boxed, reverted patch of its own config file. 

For three GUI apps: by a local loopback proxy that decrypts four known inference hosts and blind-tunnels everything else. No binary patching, no code injection, no credential extraction, no endpoint agent, no daemon.

**2. Is it consistent with the providers' terms?**
The *technical* mechanisms for Transport A are the vendors' own documented extension points, and Anthropic explicitly documents subscription traffic routed through a third-party gateway, including what such a gateway must forward. That is a strong position and it is verifiable from public docs.

It is not, by itself, a legal opinion. Two things need counsel, not engineering, before the
roadshow:

- **Consumer-plan terms.** Anthropic's and OpenAI's consumer subscription terms govern what a
subscription may be used for. The documented gateway path shows the *client* supports it; whether
routing that traffic through a commercial third party at scale is within the plan's terms is a
contract question. Get it answered in writing rather than inferred from developer docs.
- **The routing pillar specifically.** Anthropic states it "doesn't support routing Claude Code to
non-Claude models through any gateway." That does not make it prohibited, but it means
cross-vendor rerouting is explicitly outside the supported envelope, and any diligence process
will find that sentence. Have the answer ready.

**3. Can a vendor update break the product?**
Yes, at very different severities per transport. See the next section.

**4. Does it need a local proxy, a certificate, a wrapper, or an endpoint agent?**

- Wrapper: yes — `edgee launch <agent>`, or a PATH shim installed by `edgee alias`.
- Local proxy: only for `cursor`, `copilot-vscode`, `claude-desktop`. Loopback only, foreground, dies with the command.
- Certificate: only for those same three. Process-scoped for two of them; one persistent, name-constrained, removable system root on macOS for Claude Desktop.
- Endpoint agent / daemon / kernel module: **never**.

**5. Can providers block Edgee, technically or contractually?**
Technically, per transport:

- **Transport A** would require removing gateway support from their own product, a feature their enterprise customers depend on, and which Anthropic documents as an org rollout path. Removing it is possible but costly for them; it would not be a lever targeted at a single Gateway, but at all of them.
- Fine-grained blocking is possible in principle, **whatever the transport**: attestation of the client, refusing subscription credentials on requests with a non-Anthropic TLS peer, or terms enforcement against known gateway IPs. Nothing in the client today does this, and Anthropic's documented `401`-on-stripped-OAuth behaviour implies they expect and accommodate this traffic. But it is a product decision on their side, not a technical impossibility on ours.
- **Transport B** is the cheapest to block, because it is one vendor, one surface, and the lever is entirely inside their own application. The ChatGPT desktop app could ignore `model_providers` when it spawns its bundled `app-server`, pin it to the first-party provider, or move the setting out of `config.toml` — none of which would touch the `codex` CLI, so OpenAI could close this surface without any cost to the CLI users who depend on custom providers. It could also break by accident: a config schema change or a slower cold start than the ~10 s grace window degrades it silently. The fallback is not another transport on the same surface, since the desktop app has no second configuration path; it is the `codex` CLI, which is a different product for the user even though it reaches the same models.
- **Transport C** could be broken cheaply and possibly unintentionally (certificate pinning, an HTTP/2-only transport, or a protocol change). Unlike Transport B, all three of these surfaces have a documented vendor-supported gateway configuration to fall back on, so a break costs onboarding simplicity rather than the surface itself. See [If the relay breaks: fall back to Transport A](#if-the-relay-breaks-fall-back-to-transport-a).


Contractually: a provider can change subscription terms at any time. BYOK and API-credit routing work identically and are unaffected, so the business does not depend on any single credential mode.

**6. Are there official partnerships or integrations?**
**No.** There is no partnership, endorsement, certification, or private API with Anthropic, OpenAI, GitHub, Cursor... 
What is true, and strong enough on its own: Edgee uses the vendors' own published gateway and provider configuration surfaces, and Anthropic publishes a formal protocol contract for exactly the class of product Edgee is.

---



## Fragility: what a vendor update can break


| Transport                   | Blast radius | Failure mode                                                                                                     | Recovery                                                   |
| --------------------------- | ------------ | ---------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------- |
| **A - CLI env/config**      | Low          | A renamed env var or config key breaks one target; requests fall back to the vendor's own endpoint               | One-line change, ship in a CLI release                     |
| **B - codex-desktop patch** | Medium       | A schema change, or a cold start exceeding the 10 s grace, silently sends traffic direct to OpenAI               | Against accidental drift: config keys are shared with the CLI, so they move together. Against a deliberate close: no fallback on this surface — see [FAQ 5](#faq) |
| **C - relay**               | **High**     | Certificate pinning, HTTP/2-only transport, or a protocol change breaks the relay outright, with no relay-side workaround | [Migrate the surface to Transport A](#if-the-relay-breaks-fall-back-to-transport-a): all three have a documented vendor gateway path, at the cost of manual setup |


Fragility is highest exactly where usage is lowest, which is the right shape.

Structural mitigations already in the codebase: every target degrades to the vendor's own endpoint
rather than failing closed; the launch catalogue is deliberately wide, so no single vendor decision
removes the product; and `src/commands/launch/README.md` treats target names as long-lived public
API so integrations can be swapped underneath without breaking user aliases.

