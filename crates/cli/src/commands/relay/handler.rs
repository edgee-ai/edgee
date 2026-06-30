//! `hudsucker` handler that logs LLM API traffic and reroutes inference requests
//! (`/v1/messages`, `/v1/responses`, `/v1/chat/completions`) to the Edgee gateway,
//! injecting Edgee auth headers. Non-matching traffic is forwarded untouched and
//! never logged.

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
/// Maps the client's request path to the gateway path it should hit. Codex's
/// ChatGPT backend (`/backend-api/codex/responses`) is remapped to `/v1/responses`.
const REROUTE_MAP: &[(&str, &str)] = &[
    ("/v1/messages", "/v1/messages"),
    ("/v1/responses", "/v1/responses"),
    ("/v1/chat/completions", "/v1/chat/completions"),
    ("/backend-api/codex/responses", "/v1/responses"),
];

/// Gateway path for a request path, or `None` if it isn't an inference path.
fn gateway_path_for(path: &str) -> Option<&'static str> {
    REROUTE_MAP
        .iter()
        .find(|(from, _)| *from == path)
        .map(|(_, to)| *to)
}

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
    /// Hosts to log (exact host or any subdomain of it).
    domains: Arc<Vec<String>>,
    /// Output target for log blocks.
    sink: Sink,
    /// Whether to emit log blocks at all (reroute still applies when false).
    log_enabled: bool,
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
    /// Whether the in-flight request on this clone matched the filter.
    matched: bool,
}

impl RelayHandler {
    pub fn new(
        domains: Arc<Vec<String>>,
        sink: Sink,
        gateway: Arc<GatewayTarget>,
        log_enabled: bool,
    ) -> Self {
        Self {
            domains,
            sink,
            log_enabled,
            gateway,
            counter: Arc::new(AtomicU64::new(1)),
            seq: 0,
            desc: String::new(),
            matched: false,
        }
    }

    fn matches(&self, host: &str) -> bool {
        self.domains
            .iter()
            .any(|d| host == d || host.ends_with(&format!(".{d}")))
    }
}

impl HttpHandler for RelayHandler {
    async fn handle_request(
        &mut self,
        _ctx: &HttpContext,
        req: Request<Body>,
    ) -> RequestOrResponse {
        // CONNECT is the tunnel-establishment request; the real request follows
        // after TLS termination. Skip it to avoid noise.
        let host = request_host(&req);
        self.matched = req.method() != http::Method::CONNECT
            && host.as_deref().map(|h| self.matches(h)).unwrap_or(false);
        if !self.matched {
            return RequestOrResponse::Request(req);
        }

        let reroute = gateway_path_for(req.uri().path());

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
        self.seq = self.counter.fetch_add(1, Ordering::Relaxed);
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
        if !self.log_enabled || !self.matched {
            return res;
        }

        let status = res.status();
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
    let new_path = format!("{}{gw_path}{query}", gw.base_path);

    let mut builder = Uri::builder()
        .scheme(gw.scheme.clone())
        .authority(gw.authority.clone());
    builder = builder.path_and_query(new_path);
    if let Ok(uri) = builder.build() {
        parts.uri = uri;
    }

    let h = &mut parts.headers;
    if let Ok(v) = http::HeaderValue::from_str(&gw.api_key) {
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
        assert_eq!(gateway_path_for("/api/oauth/validate"), None);
    }
}

