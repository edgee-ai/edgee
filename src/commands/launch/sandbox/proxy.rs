//! Authenticated, destination-filtered forward proxy. TLS is never intercepted.
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine;

const MAX_HEADER: usize = 32768;
const MAX_CONNECTIONS: usize = 128;

pub(super) struct Proxy {
    pub port: u16,
    pub token: String,
    state: Arc<State>,
    acceptor: Option<JoinHandle<()>>,
}
struct State {
    stop: AtomicBool,
    active: AtomicUsize,
    sockets: Mutex<HashMap<usize, Vec<TcpStream>>>,
    next_id: AtomicUsize,
    local_ips: HashSet<IpAddr>,
    authorization: String,
    domains: Vec<String>,
}
impl Proxy {
    pub fn start(domains: &[String]) -> Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let authorization = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("edgee:{token}"))
        );
        let state = Arc::new(State {
            stop: AtomicBool::new(false),
            active: AtomicUsize::new(0),
            sockets: Mutex::new(HashMap::new()),
            next_id: AtomicUsize::new(0),
            local_ips: local_ips()?,
            authorization,
            domains: domains.to_vec(),
        });
        let worker = Arc::clone(&state);
        let acceptor = thread::spawn(move || {
            while !worker.stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if worker.active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                            worker.active.fetch_sub(1, Ordering::SeqCst);
                            continue;
                        }
                        let id = worker.next_id.fetch_add(1, Ordering::Relaxed);
                        if !register(&worker, id, &stream) {
                            worker.active.fetch_sub(1, Ordering::SeqCst);
                            continue;
                        }
                        let state = Arc::clone(&worker);
                        thread::spawn(move || {
                            let _ = handle(stream, &state, id);
                            state.sockets.lock().unwrap().remove(&id);
                            state.active.fetch_sub(1, Ordering::SeqCst);
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            port,
            token,
            state,
            acceptor: Some(acceptor),
        })
    }
    pub fn url(&self, port: u16) -> String {
        format!("http://edgee:{}@127.0.0.1:{port}", self.token)
    }
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::SeqCst);
        for sockets in self.state.sockets.lock().unwrap().values() {
            for socket in sockets {
                let _ = socket.shutdown(Shutdown::Both);
            }
        }
        if let Some(acceptor) = self.acceptor.take() {
            let _ = acceptor.join();
        }
    }
}
fn response(stream: &mut TcpStream, status: &str) -> Result<()> {
    write!(stream, "HTTP/1.1 {status}\r\n")?;
    if status.starts_with("407 ") { stream.write_all(b"Proxy-Authenticate: Basic realm=\"Edgee sandbox\"\r\n")?; }
    stream.write_all(b"Connection: close\r\nContent-Length: 0\r\n\r\n")?;
    Ok(())
}
fn handle(mut client: TcpStream, state: &State, id: usize) -> Result<()> {
    client.set_nonblocking(false)?;
    client.set_read_timeout(Some(Duration::from_secs(10)))?;
    client.set_write_timeout(Some(Duration::from_secs(10)))?;
    let mut bytes = Vec::new();
    // Read precisely through the header terminator: no body/tunnel bytes lost.
    while !bytes.ends_with(b"\r\n\r\n") {
        if bytes.len() >= MAX_HEADER {
            return response(&mut client, "431 Headers Too Large");
        }
        let mut byte = [0];
        if client.read(&mut byte)? == 0 {
            return Ok(());
        }
        bytes.push(byte[0]);
    }
    let header = std::str::from_utf8(&bytes).context("Invalid proxy request")?;
    let mut lines = header.split("\r\n");
    let request = lines
        .next()
        .unwrap_or_default()
        .split(' ')
        .collect::<Vec<_>>();
    if request.len() != 3 || !matches!(request[2], "HTTP/1.0" | "HTTP/1.1") {
        return response(&mut client, "400 Bad Request");
    }
    let (method, target) = (request[0], request[1]);
    if method.is_empty() || !method.bytes().all(|b| b.is_ascii_uppercase()) {
        return response(&mut client, "400 Bad Request");
    }
    let mut fields = Vec::new();
    let mut names = HashSet::new();
    let mut authenticated = false;
    for line in lines.take_while(|line| !line.is_empty()) {
        let Some((name, value)) = line.split_once(':') else {
            return response(&mut client, "400 Bad Request");
        };
        if name.is_empty()
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || value.bytes().any(|b| b < 32 && b != b'\t')
        {
            return response(&mut client, "400 Bad Request");
        }
        let lower = name.to_ascii_lowercase();
        if !names.insert(lower.clone())
            && matches!(
                lower.as_str(),
                "proxy-authorization" | "content-length" | "transfer-encoding" | "host"
            )
        {
            return response(&mut client, "400 Bad Request");
        }
        let value = value.trim();
        if lower == "connection" && value.split(',').any(|token| !matches!(token.trim().to_ascii_lowercase().as_str(), "close" | "keep-alive")) { return response(&mut client, "400 Bad Request"); }
        if lower == "proxy-authorization" {
            authenticated = constant_equal(value.as_bytes(), state.authorization.as_bytes());
        }
        if lower == "content-length"
            && (value.is_empty()
                || !value.bytes().all(|b| b.is_ascii_digit())
                || value.parse::<u64>().is_err())
        {
            return response(&mut client, "400 Bad Request");
        }
        fields.push((lower, value.to_owned()));
    }
    if !authenticated {
        return response(&mut client, "407 Proxy Authentication Required");
    }
    if names.contains("transfer-encoding") {
        return response(&mut client, "400 Bad Request");
    }
    if method == "CONNECT"
        && (target.contains(['/', '?', '#', '@'])
            || target.parse::<SocketAddr>().is_err()
                && target
                    .rsplit_once(':')
                    .is_none_or(|(_, p)| p.parse::<u16>().is_err()))
    {
        return response(&mut client, "400 Bad Request");
    }
    let url = if method == "CONNECT" {
        reqwest::Url::parse(&format!("https://{target}/"))
    } else {
        reqwest::Url::parse(target)
    };
    let Ok(url) = url else {
        return response(&mut client, "400 Bad Request");
    };
    if !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || (method != "CONNECT" && url.scheme() != "http")
    {
        return response(&mut client, "400 Bad Request");
    }
    let Some(host) = url.host_str() else {
        return response(&mut client, "400 Bad Request");
    };
    let host = host.trim_matches(['[', ']']);
    let port = url.port_or_known_default().unwrap_or(443);
    if let Some((_, authority)) = fields.iter().find(|(name, _)| name == "host") {
        let authority_url = reqwest::Url::parse(&format!("{}://{authority}/", url.scheme()));
        if !authority_url.is_ok_and(|u| u.host_str() == url.host_str() && u.port_or_known_default() == Some(port) && u.username().is_empty() && u.password().is_none() && u.path() == "/" && u.query().is_none() && u.fragment().is_none()) { return response(&mut client, "400 Bad Request"); }
    }
    if !allowed_name(host, port, &state.domains) {
        return response(&mut client, "403 Forbidden");
    }
    let addresses = (host, port)
        .to_socket_addrs()?
        .filter(|address| {
            (public_ip(address.ip()) && !state.local_ips.contains(&address.ip()))
                || explicit_ip(*address, &state.domains)
        })
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return response(&mut client, "403 Forbidden");
    }
    let mut upstream = None;
    for address in addresses {
        if state.stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        if let Ok(stream) = TcpStream::connect_timeout(&address, Duration::from_secs(10)) {
            upstream = Some(stream);
            break;
        }
    }
    let Some(mut upstream) = upstream else {
        return response(&mut client, "502 Bad Gateway");
    };
    if !register(state, id, &upstream) {
        return Ok(());
    }
    if method == "CONNECT" {
        if url.path() != "/" || url.query().is_some() {
            return response(&mut client, "400 Bad Request");
        }
        client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")?;
    } else {
        let path = if let Some(query) = url.query() {
            format!("{}?{query}", url.path())
        } else {
            url.path().to_owned()
        };
        write!(
            upstream,
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
            if host.contains(':') {
                format!("[{host}]:{port}")
            } else {
                format!("{host}:{port}")
            }
        )?;
        for (name, value) in &fields {
            if !matches!(
                name.as_str(),
                "proxy-authorization" | "proxy-connection" | "connection" | "host"
            ) {
                write!(upstream, "{name}: {value}\r\n")?;
            }
        }
        upstream.write_all(b"\r\n")?;
    }
    if method == "CONNECT" { client.set_read_timeout(None)?; }
    client.set_write_timeout(None)?;
    let mut left = client.try_clone()?;
    let mut right = upstream.try_clone()?;
    let tunnel = method == "CONNECT";
    let body_length = fields
        .iter()
        .find(|(name, _)| name == "content-length")
        .map(|(_, v)| v.parse::<u64>().unwrap())
        .unwrap_or(0);
    let send = thread::spawn(move || {
        if tunnel {
            let _ = std::io::copy(&mut left, &mut right);
        } else {
            let _ = std::io::copy(&mut left.take(body_length), &mut right);
        }
        let _ = right.shutdown(Shutdown::Write);
    });
    let _ = std::io::copy(&mut upstream, &mut client);
    let _ = upstream.shutdown(Shutdown::Both);
    if tunnel { let _ = client.shutdown(Shutdown::Both); }
    let _ = send.join();
    if !tunnel {
        // Discard already-buffered pipelining, never forward it. Avoid a TCP RST
        // obscuring the first response when closing with unread client bytes.
        client.set_nonblocking(true)?;
        let mut discarded = [0u8; 4096];
        for _ in 0..8 { if !matches!(client.read(&mut discarded), Ok(n) if n > 0) { break; } }
    }
    let _ = client.shutdown(Shutdown::Both);
    Ok(())
}
fn constant_equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |v, (a, b)| v | (a ^ b)) == 0
}
fn allowed_name(host: &str, port: u16, domains: &[String]) -> bool {
    domains.iter().any(|entry| {
        entry.eq_ignore_ascii_case(host)
            || entry.eq_ignore_ascii_case(&format!("{host}:{port}"))
            || entry.eq_ignore_ascii_case(&format!("[{host}]:{port}"))
    })
}
fn explicit_ip(address: SocketAddr, domains: &[String]) -> bool {
    domains
        .iter()
        .any(|entry| entry.parse::<SocketAddr>().ok() == Some(address))
}
fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, _, _] = ip.octets();
            !ip.is_loopback()
                && !ip.is_private()
                && !ip.is_link_local()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_broadcast()
                && !(a == 100 && (64..=127).contains(&b))
                && a != 0
                && a < 224
                && !(a == 192 && b == 0)
                && !(a == 198 && (18..=19).contains(&b))
                && ip.octets() != [168, 63, 129, 16]
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(v4));
            }
            let first = ip.segments()[0];
            first & 0xe000 == 0x2000
                && first != 0x2002
                && !(first == 0x2001 && ip.segments()[1] == 0)
        }
    }
}

fn register(state: &State, id: usize, stream: &TcpStream) -> bool {
    let mut sockets = state.sockets.lock().unwrap();
    if state.stop.load(Ordering::SeqCst) {
        let _ = stream.shutdown(Shutdown::Both);
        return false;
    }
    let Ok(copy) = stream.try_clone() else {
        return false;
    };
    sockets.entry(id).or_default().push(copy);
    true
}
fn local_ips() -> Result<HashSet<IpAddr>> {
    let mut interfaces = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut interfaces) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut ips = HashSet::new();
    let mut item = interfaces;
    while !item.is_null() {
        unsafe {
            let address = (*item).ifa_addr;
            if !address.is_null() {
                match (*address).sa_family as i32 {
                    libc::AF_INET => {
                        let a = &*(address as *const libc::sockaddr_in);
                        ips.insert(IpAddr::V4(std::net::Ipv4Addr::from(
                            a.sin_addr.s_addr.to_ne_bytes(),
                        )));
                    }
                    libc::AF_INET6 => {
                        let a = &*(address as *const libc::sockaddr_in6);
                        ips.insert(IpAddr::V6(std::net::Ipv6Addr::from(a.sin6_addr.s6_addr)));
                    }
                    _ => {}
                }
            }
            item = (*item).ifa_next;
        }
    }
    unsafe {
        libc::freeifaddrs(interfaces);
    }
    Ok(ips)
}
#[cfg(test)]
#[path = "proxy_tests.rs"]
mod tests;
