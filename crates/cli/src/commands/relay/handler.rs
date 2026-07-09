//! `hudsucker` handler that logs LLM API traffic and reroutes inference requests
//! (`/v1/messages`, `/v1/responses`, `/v1/chat/completions`) to the Edgee gateway,
//! injecting Edgee auth headers. All HTTPS traffic is decrypted; non-inference
//! traffic is forwarded untouched after optional logging.

use std::fmt::Write as _;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use console::{strip_ansi_codes, style};
use http::uri::{Authority, Scheme};
use http::{Request, Response, Uri};
use http_body_util::BodyExt;
use hudsucker::{Body, HttpContext, HttpHandler, RequestOrResponse};

/// Inference paths rerouted to the Edgee gateway (host-agnostic; gated by domain).
/// Maps the client's request path to the gateway path it should hit. Most are
/// identity; Codex's ChatGPT backend (`/backend-api/codex/responses`) is remapped
/// to the gateway's `/v1/responses`, and VS Code Copilot's `/chat/completions` to
/// the gateway's `/v1/chat/completions`.
const REROUTE_MAP: &[(&str, &str)] = &[
    ("/v1/messages", "/v1/messages"),
    ("/v1/responses", "/v1/responses"),
    ("/v1/chat/completions", "/v1/chat/completions"),
    ("/backend-api/codex/responses", "/v1/responses"),
    // VS Code Copilot: chat uses the bare Responses API path, title generation
    // uses the bare chat-completions path. Both map to the gateway's /v1 routes.
    ("/responses", "/v1/responses"),
    ("/chat/completions", "/v1/chat/completions"),
];

/// Gateway path for a request path, or `None` if it isn't an inference path.
fn gateway_path_for(path: &str) -> Option<&'static str> {
    REROUTE_MAP
        .iter()
        .find(|(from, _)| *from == path)
        .map(|(_, to)| *to)
}

/// The Anthropic path that Claude Code uses. When a VS Code relay sees this, the
/// request is Claude Code running inside the editor — not Copilot — so it routes
/// through the claude pipeline with the claude key instead of Copilot passthrough.
const CLAUDE_PATH: &str = "/v1/messages";

/// Where rerouted inference requests are sent, plus the Edgee auth to inject.
/// The key is the active relay's provider key (claude when the agent is claude /
/// proxy-only, codex when the agent is codex).
#[derive(Clone)]
pub struct GatewayTarget {
    pub scheme: Scheme,
    pub authority: Authority,
    /// Optional base path prefix (empty for the default `https://edgee.io`).
    pub base_path: String,
    pub api_key: String,
    pub session_id: String,
    pub repo: Option<String>,
    /// When true, record the original upstream in `x-edgee-upstream-url` so the
    /// gateway forwards there (preserving the caller's own auth) instead of running
    /// its standard Edgee provider pipeline. Set for passthrough-only providers like
    /// VS Code Copilot; left false for claude/codex, which the gateway routes itself.
    pub passthrough_to_upstream: bool,
    /// Edgee claude key, set only for the VS Code relay where Claude Code may run
    /// alongside Copilot. When present, `/v1/messages` traffic uses this key and the
    /// gateway's claude pipeline instead of the default (Copilot) key + passthrough.
    pub claude_api_key: Option<String>,
}

impl GatewayTarget {
    /// Resolve the `(api_key, passthrough_to_upstream)` to use for a request whose
    /// original path is `orig_path`. Claude Code traffic (`/v1/messages`) in a VS
    /// Code relay uses the claude key and the gateway pipeline; everything else uses
    /// the relay's default key and passthrough setting.
    fn auth_for(&self, orig_path: &str) -> (&str, bool) {
        if orig_path == CLAUDE_PATH {
            if let Some(key) = &self.claude_api_key {
                return (key, false);
            }
        }
        (&self.api_key, self.passthrough_to_upstream)
    }
}

/// Where formatted log blocks are written. Each block is emitted atomically so
/// concurrent connections don't interleave. File output is stripped of ANSI
/// color codes.
#[derive(Clone)]
pub struct Sink {
    inner: Arc<Mutex<Box<dyn Write + Send>>>,
    file: bool,
}

impl Sink {
    pub fn stdout() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(std::io::stdout()))),
            file: false,
        }
    }

    pub fn file(f: File) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(f))),
            file: true,
        }
    }

    fn emit(&self, block: &str) {
        let data = if self.file {
            strip_ansi_codes(block).into_owned()
        } else {
            block.to_string()
        };
        if let Ok(mut w) = self.inner.lock() {
            let _ = w.write_all(data.as_bytes());
            let _ = w.flush();
        }
    }
}

#[derive(Clone)]
pub struct RelayHandler {
    /// Output target for log blocks.
    sink: Sink,
    /// Whether to emit log blocks at all (reroute still applies when false).
    log_enabled: bool,
    /// Print a concise one-line summary (with response status) to stdout for every
    /// request. On when the terminal is free (GUI client / `--no-launch`).
    announce: bool,
    /// Gateway to reroute inference requests to (with auth to inject).
    gateway: Arc<GatewayTarget>,
    /// Shared monotonic counter allocating one id per logged request.
    /// hudsucker clones the handler per request, so this `Arc` is shared while
    /// the per-request fields below stay private to each request's clone.
    counter: Arc<AtomicU64>,
    /// Sequence id of the in-flight request (0 until assigned).
    seq: u64,
    /// `METHOD url` of the in-flight request, echoed on its response.
    desc: String,
    /// Concise `METHOD host/path` of the in-flight request, for the one-liner.
    summary: String,
    /// Whether the in-flight request on this clone matched the filter.
    matched: bool,
    /// Whether the in-flight request was rerouted to the gateway.
    rerouted: bool,
}

impl RelayHandler {
    pub fn new(
        sink: Sink,
        gateway: Arc<GatewayTarget>,
        log_enabled: bool,
        announce: bool,
    ) -> Self {
        Self {
            sink,
            log_enabled,
            announce,
            gateway,
            counter: Arc::new(AtomicU64::new(1)),
            seq: 0,
            desc: String::new(),
            summary: String::new(),
            matched: false,
            rerouted: false,
        }
    }
}

impl HttpHandler for RelayHandler {
    async fn should_intercept(&mut self, _ctx: &HttpContext, _req: &Request<Body>) -> bool {
        true
    }

    async fn handle_request(
        &mut self,
        _ctx: &HttpContext,
        req: Request<Body>,
    ) -> RequestOrResponse {
        // CONNECT is the tunnel-establishment request; the real request follows
        // after TLS termination. We don't log/reroute it, but its authority is
        // exactly the host hudsucker will mint a leaf cert for, so surface it.
        let host = request_host(&req);
        if req.method() == http::Method::CONNECT {
            if self.announce {
                if let Some(h) = host.as_deref() {
                    println!("{} mint cert for {h}", style("⚿").dim());
                }
            }
            return RequestOrResponse::Request(req);
        }
        self.matched = true;

        let reroute = gateway_path_for(req.uri().path());
        self.rerouted = reroute.is_some();
        // One id per matched request, used by both the one-liner and the file log.
        self.seq = self.counter.fetch_add(1, Ordering::Relaxed);
        if self.announce {
            let h = host.as_deref().unwrap_or("");
            self.summary = format!("{} {h}{}", req.method(), req.uri().path());
        }

        // Logging disabled: reroute if needed, forward the body stream untouched
        // (no buffering).
        if !self.log_enabled {
            if let Some(gw_path) = reroute {
                let (mut parts, body) = req.into_parts();
                apply_reroute(&mut parts, &self.gateway, gw_path);
                return RequestOrResponse::Request(Request::from_parts(parts, body));
            }
            return RequestOrResponse::Request(req);
        }

        let host = host.unwrap_or_default();
        let method = req.method().clone();
        let url = absolute_url(&req, &host);
        self.desc = format!("{method} {url}");
        let headers = collect_headers(req.headers());
        let json_body = is_json(req.headers());

        let title_suffix = match reroute {
            Some(gw_path) => format!("  →relay→ {}{gw_path}", self.gateway.authority),
            None => String::new(),
        };

        let (mut parts, body) = req.into_parts();
        let bytes = match body.collect().await {
            Ok(c) => c.to_bytes(),
            Err(_) => {
                self.sink.emit(&format!(
                    "{}\n",
                    style(format!("━━━ #{} {}  (failed to read body) ━━━", self.seq, self.desc))
                        .red()
                ));
                let req = Request::from_parts(parts, Body::from(bytes::Bytes::new()));
                return RequestOrResponse::Request(req);
            }
        };

        if let Some(gw_path) = reroute {
            apply_reroute(&mut parts, &self.gateway, gw_path);
        }

        let mut buf = String::new();
        let _ = writeln!(
            buf,
            "{}",
            style(format!("━━━ #{} {}{title_suffix} ━━━", self.seq, self.desc))
                .bold()
                .green()
        );
        fmt_headers(&mut buf, &headers);
        fmt_body(&mut buf, &bytes, json_body);
        self.sink.emit(&buf);

        let req = Request::from_parts(parts, Body::from(bytes));
        RequestOrResponse::Request(req)
    }

    async fn handle_response(
        &mut self,
        _ctx: &HttpContext,
        res: Response<Body>,
    ) -> Response<Body> {
        if !self.matched {
            return res;
        }

        let status = res.status();

        // One line per matched request: id, route marker, summary, response status.
        if self.announce {
            let marker = if self.rerouted {
                style("→gw  ").green()
            } else {
                style("pass ").dim()
            };
            let code = status.as_u16();
            let status_styled = if (200..300).contains(&code) {
                style(status.to_string()).green()
            } else if (400..600).contains(&code) {
                style(status.to_string()).red()
            } else {
                style(status.to_string()).yellow()
            };
            println!(
                "{} {marker} {}  → {status_styled}",
                style(format!("#{}", self.seq)).dim(),
                self.summary,
            );
        }

        if !self.log_enabled {
            return res;
        }

        let headers = collect_headers(res.headers());

        // Streaming responses (SSE) must not be buffered: log metadata, pass through.
        if is_event_stream(res.headers()) {
            let mut buf = String::new();
            let _ = writeln!(
                buf,
                "{}",
                style(format!(
                    "◀── #{} {} → {status}  (text/event-stream, not buffered)",
                    self.seq, self.desc
                ))
                .dim()
            );
            fmt_headers(&mut buf, &headers);
            self.sink.emit(&buf);
            return res;
        }

        let json_body = is_json(res.headers());
        let (parts, body) = res.into_parts();
        let raw = match body.collect().await {
            Ok(c) => c.to_bytes(),
            Err(_) => return Response::from_parts(parts, Body::from(bytes::Bytes::new())),
        };

        // Decode a copy for the log if the body is compressed; forward the
        // original response untouched so the client still gets exactly what the
        // upstream sent.
        let display = decode_for_log(&parts.headers, &raw).await;

        let mut buf = String::new();
        let _ = writeln!(
            buf,
            "{}",
            style(format!("◀── #{} {} → {status}", self.seq, self.desc))
                .bold()
                .magenta()
        );
        fmt_headers(&mut buf, &headers);
        fmt_body(&mut buf, &display, json_body);
        self.sink.emit(&buf);

        Response::from_parts(parts, Body::from(raw))
    }
}

/// If the response carries a supported `content-encoding`, return the decoded
/// bytes (for readable logging). Otherwise return the bytes unchanged. Never
/// fails: on any decode error it falls back to the raw bytes.
async fn decode_for_log(headers: &http::HeaderMap, raw: &bytes::Bytes) -> bytes::Bytes {
    let encoding = headers
        .get(http::header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase());

    match encoding {
        Some(enc) if needs_decode(&enc) => {
            // Rebuild a throwaway response carrying just the content-encoding so
            // hudsucker's decoder knows how to inflate it.
            let mut tmp = Response::new(Body::from(raw.clone()));
            if let Some(v) = headers.get(http::header::CONTENT_ENCODING) {
                tmp.headers_mut()
                    .insert(http::header::CONTENT_ENCODING, v.clone());
            }
            match hudsucker::decode_response(tmp) {
                Ok(decoded) => match decoded.into_body().collect().await {
                    Ok(c) => c.to_bytes(),
                    Err(_) => raw.clone(),
                },
                Err(_) => raw.clone(),
            }
        }
        _ => raw.clone(),
    }
}

/// Encodings hudsucker's `decoder` feature can inflate.
fn needs_decode(enc: &str) -> bool {
    enc.split(',').any(|part| {
        matches!(
            part.trim(),
            "gzip" | "x-gzip" | "deflate" | "br" | "zstd"
        )
    })
}

fn collect_headers(map: &http::HeaderMap) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = map
        .iter()
        .map(|(n, v)| (n.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
        .collect();
    headers.sort_by(|a, b| a.0.cmp(&b.0));
    headers
}

fn is_json(map: &http::HeaderMap) -> bool {
    content_type(map).map(|ct| ct.contains("json")).unwrap_or(false)
}

fn is_event_stream(map: &http::HeaderMap) -> bool {
    content_type(map)
        .map(|ct| ct.contains("event-stream"))
        .unwrap_or(false)
}

fn content_type(map: &http::HeaderMap) -> Option<&str> {
    map.get(http::header::CONTENT_TYPE).and_then(|v| v.to_str().ok())
}

/// Rewrite the request to target the Edgee gateway at `gw_path` (the client's
/// original query is preserved) and inject Edgee auth headers. `normalize_request`
/// (hudsucker) strips the `Host` header, so hyper regenerates it from the new
/// authority — we only set scheme/authority/path on the URI.
fn apply_reroute(parts: &mut http::request::Parts, gw: &GatewayTarget, gw_path: &str) {
    let query = parts
        .uri
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();

    // Capture the original upstream (host + path) before rewriting the URI, so the
    // gateway can passthrough to it (e.g. forward to api.business.githubcopilot.com
    // with the caller's own bearer token). The Host header carries it after MITM.
    let upstream_host = parts
        .headers
        .get(http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| parts.uri.authority().map(|a| a.host().to_string()));
    let upstream_path = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| parts.uri.path().to_string());

    // Pick the key + routing mode from the original path before it's rewritten:
    // Claude Code (`/v1/messages`) in a VS Code relay uses the claude key + pipeline,
    // everything else the relay's default key + passthrough setting.
    let (api_key, passthrough_to_upstream) = gw.auth_for(parts.uri.path());

    let new_path = format!("{}{gw_path}{query}", gw.base_path);

    let mut builder = Uri::builder()
        .scheme(gw.scheme.clone())
        .authority(gw.authority.clone());
    builder = builder.path_and_query(new_path);
    if let Ok(uri) = builder.build() {
        parts.uri = uri;
    }

    let h = &mut parts.headers;
    if let Ok(v) = http::HeaderValue::from_str(api_key) {
        h.insert("x-edgee-api-key", v);
    }
    if let Ok(v) = http::HeaderValue::from_str(&gw.session_id) {
        h.insert("x-edgee-session-id", v);
    }
    if let Some(repo) = &gw.repo {
        if let Ok(v) = http::HeaderValue::from_str(repo) {
            h.insert("x-edgee-repo", v);
        }
    }
    // For passthrough providers, tell the gateway where this inference call
    // originated so it can forward to the provider's own backend, preserving the
    // caller's `Authorization`. Omitted for claude/codex, which the gateway routes
    // through its own pipeline using `x-edgee-api-key`.
    if passthrough_to_upstream {
        if let Some(host) = upstream_host {
            if let Ok(v) = http::HeaderValue::from_str(&format!("https://{host}{upstream_path}")) {
                h.insert("x-edgee-upstream-url", v);
            }
        }
    }
}

/// Best-effort host extraction: prefer the URI authority (HTTP/2 / absolute-form),
/// fall back to the `Host` header (origin-form after TLS termination).
fn request_host(req: &Request<Body>) -> Option<String> {
    if let Some(authority) = req.uri().authority() {
        return Some(authority.host().to_string());
    }
    req.headers()
        .get(http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(':').next().unwrap_or(s).to_string())
}

/// Reconstruct an absolute URL for display. After TLS MITM the URI is usually
/// origin-form (path only), so we synthesize the `https://host` prefix.
fn absolute_url(req: &Request<Body>, host: &str) -> String {
    let uri = req.uri();
    if uri.scheme().is_some() && uri.authority().is_some() {
        return uri.to_string();
    }
    let path = uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or_else(|| uri.path());
    format!("https://{host}{path}")
}

fn fmt_headers(buf: &mut String, headers: &[(String, String)]) {
    for (n, v) in headers {
        let _ = writeln!(buf, "    {}: {v}", style(n).cyan());
    }
}

fn fmt_body(buf: &mut String, body: &[u8], json_body: bool) {
    if body.is_empty() {
        return;
    }
    let len = body.len();
    let pretty = if json_body {
        serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
    } else {
        None
    };
    let text = pretty.unwrap_or_else(|| String::from_utf8_lossy(body).into_owned());

    let _ = writeln!(buf, "  {} ({len} bytes):", style("body").bold());
    for line in text.lines() {
        let _ = writeln!(buf, "    {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> GatewayTarget {
        GatewayTarget {
            scheme: Scheme::HTTPS,
            authority: "edgee.io".parse().unwrap(),
            base_path: String::new(),
            api_key: "sk-edgee-test".into(),
            session_id: "sess-1".into(),
            repo: Some("git@github.com:edgee-ai/edgee.git".into()),
            passthrough_to_upstream: true,
            claude_api_key: None,
        }
    }

    #[test]
    fn reroute_rewrites_uri_and_injects_headers() {
        let req = Request::builder()
            .method("POST")
            .uri("https://api.anthropic.com/v1/messages?beta=true")
            .header("authorization", "Bearer x")
            .body(Body::from(bytes::Bytes::new()))
            .unwrap();
        let (mut parts, _b) = req.into_parts();
        let gw_path = gateway_path_for("/v1/messages").unwrap();
        apply_reroute(&mut parts, &target(), gw_path);

        assert_eq!(parts.uri.scheme_str(), Some("https"));
        assert_eq!(parts.uri.authority().unwrap().as_str(), "edgee.io");
        assert_eq!(
            parts.uri.path_and_query().unwrap().as_str(),
            "/v1/messages?beta=true"
        );
        assert_eq!(parts.headers.get("x-edgee-api-key").unwrap(), "sk-edgee-test");
        assert_eq!(parts.headers.get("x-edgee-session-id").unwrap(), "sess-1");
        assert_eq!(
            parts.headers.get("x-edgee-repo").unwrap(),
            "git@github.com:edgee-ai/edgee.git"
        );
        // Original provider auth is preserved for the gateway.
        assert_eq!(parts.headers.get("authorization").unwrap(), "Bearer x");
        // The original upstream is recorded so the gateway can passthrough to it.
        assert_eq!(
            parts.headers.get("x-edgee-upstream-url").unwrap(),
            "https://api.anthropic.com/v1/messages?beta=true"
        );
    }

    #[test]
    fn reroute_omits_upstream_url_for_pipeline_providers() {
        // claude/codex route through the gateway pipeline, so no upstream-url.
        let mut t = target();
        t.passthrough_to_upstream = false;
        let req = Request::builder()
            .method("POST")
            .uri("https://api.anthropic.com/v1/messages")
            .body(Body::from(bytes::Bytes::new()))
            .unwrap();
        let (mut parts, _b) = req.into_parts();
        apply_reroute(&mut parts, &t, gateway_path_for("/v1/messages").unwrap());
        assert!(parts.headers.get("x-edgee-upstream-url").is_none());
    }

    #[test]
    fn vscode_claude_traffic_uses_claude_key_and_pipeline() {
        // VS Code relay: default (Copilot) key + passthrough, plus a claude key.
        let mut t = target();
        t.api_key = "sk-copilot".into();
        t.claude_api_key = Some("sk-claude".into());

        // Claude Code's /v1/messages → claude key, no upstream-url (pipeline routing).
        let req = Request::builder()
            .method("POST")
            .uri("https://api.anthropic.com/v1/messages")
            .body(Body::from(bytes::Bytes::new()))
            .unwrap();
        let (mut parts, _b) = req.into_parts();
        apply_reroute(&mut parts, &t, gateway_path_for("/v1/messages").unwrap());
        assert_eq!(parts.headers.get("x-edgee-api-key").unwrap(), "sk-claude");
        assert!(parts.headers.get("x-edgee-upstream-url").is_none());

        // Copilot's /responses still uses the default key + passthrough.
        let req = Request::builder()
            .method("POST")
            .uri("https://api.githubcopilot.com/responses")
            .body(Body::from(bytes::Bytes::new()))
            .unwrap();
        let (mut parts, _b) = req.into_parts();
        apply_reroute(&mut parts, &t, gateway_path_for("/responses").unwrap());
        assert_eq!(parts.headers.get("x-edgee-api-key").unwrap(), "sk-copilot");
        assert!(parts.headers.get("x-edgee-upstream-url").is_some());
    }

    #[test]
    fn claude_path_uses_default_key_without_claude_override() {
        // No claude key set (e.g. claude/codex relays): /v1/messages uses the default.
        let mut t = target();
        t.api_key = "sk-default".into();
        t.claude_api_key = None;
        let req = Request::builder()
            .method("POST")
            .uri("https://api.anthropic.com/v1/messages")
            .body(Body::from(bytes::Bytes::new()))
            .unwrap();
        let (mut parts, _b) = req.into_parts();
        apply_reroute(&mut parts, &t, gateway_path_for("/v1/messages").unwrap());
        assert_eq!(parts.headers.get("x-edgee-api-key").unwrap(), "sk-default");
    }

    #[test]
    fn reroute_prepends_base_path() {
        let mut t = target();
        t.base_path = "/proxy".into();
        let req = Request::builder()
            .uri("https://api.openai.com/v1/responses")
            .body(Body::from(bytes::Bytes::new()))
            .unwrap();
        let (mut parts, _b) = req.into_parts();
        apply_reroute(&mut parts, &t, gateway_path_for("/v1/responses").unwrap());
        assert_eq!(
            parts.uri.path_and_query().unwrap().as_str(),
            "/proxy/v1/responses"
        );
    }

    #[test]
    fn codex_chatgpt_backend_remaps_to_v1_responses() {
        let req = Request::builder()
            .method("POST")
            .uri("https://chatgpt.com/backend-api/codex/responses?foo=1")
            .body(Body::from(bytes::Bytes::new()))
            .unwrap();
        let (mut parts, _b) = req.into_parts();
        let gw_path = gateway_path_for("/backend-api/codex/responses").unwrap();
        apply_reroute(&mut parts, &target(), gw_path);
        // Path is remapped to /v1/responses; query preserved.
        assert_eq!(
            parts.uri.path_and_query().unwrap().as_str(),
            "/v1/responses?foo=1"
        );
        assert_eq!(parts.uri.authority().unwrap().as_str(), "edgee.io");
    }

    #[test]
    fn gateway_path_mapping() {
        assert_eq!(gateway_path_for("/v1/messages"), Some("/v1/messages"));
        assert_eq!(gateway_path_for("/v1/responses"), Some("/v1/responses"));
        assert_eq!(
            gateway_path_for("/v1/chat/completions"),
            Some("/v1/chat/completions")
        );
        assert_eq!(
            gateway_path_for("/backend-api/codex/responses"),
            Some("/v1/responses")
        );
        // VS Code Copilot posts to bare /responses (chat) and /chat/completions
        // (title generation); both reroute to the gateway's /v1 routes.
        assert_eq!(gateway_path_for("/responses"), Some("/v1/responses"));
        assert_eq!(
            gateway_path_for("/chat/completions"),
            Some("/v1/chat/completions")
        );
        assert_eq!(gateway_path_for("/api/oauth/validate"), None);
    }
}

