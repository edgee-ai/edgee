//! Adversarial protocol tests for the trusted native network boundary.
use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use base64::Engine;

use super::{allowed_name, constant_equal, public_ip, Proxy};

fn authentication(proxy: &Proxy) -> String {
    format!(
        "Proxy-Authorization: Basic {}\r\n",
        base64::engine::general_purpose::STANDARD.encode(format!("edgee:{}", proxy.token))
    )
}

fn request(proxy: &Proxy, data: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", proxy.port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream.write_all(data.as_bytes()).unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn exact_destinations_do_not_accept_suffixes_or_wrong_ports() {
    let domains = vec![
        "api.example.test".to_owned(),
        "registry.example.test:443".to_owned(),
    ];
    assert!(allowed_name("api.example.test", 443, &domains));
    assert!(allowed_name("API.EXAMPLE.TEST", 443, &domains));
    assert!(allowed_name("registry.example.test", 443, &domains));
    for (host, port) in [
        ("api.example.test.evil.test", 443),
        ("evilapi.example.test", 443),
        ("registry.example.test", 80),
        ("example.test", 443),
        ("127.0.0.1", 443),
    ] {
        assert!(!allowed_name(host, port, &domains), "{host}:{port}");
    }
}

#[test]
fn private_special_and_transition_addresses_are_not_public() {
    for value in [
        "0.0.0.0",
        "0.1.2.3",
        "127.0.0.1",
        "10.2.3.4",
        "172.31.1.1",
        "192.168.1.1",
        "169.254.169.254",
        "100.64.0.1",
        "100.100.100.200",
        "168.63.129.16",
        "192.0.0.192",
        "198.18.0.1",
        "224.0.0.1",
        "255.255.255.255",
        "::",
        "::1",
        "fc00::1",
        "fe80::1",
        "ff02::1",
        "::ffff:127.0.0.1",
        "::127.0.0.1",
        "64:ff9b::7f00:1",
        "2002:7f00:1::",
        "2001:0::1",
    ] {
        assert!(
            !public_ip(value.parse::<IpAddr>().unwrap()),
            "Must block {value}"
        );
    }
    for value in [
        "1.1.1.1",
        "8.8.8.8",
        "2606:4700:4700::1111",
        "2001:4860:4860::8888",
    ] {
        assert!(
            public_ip(value.parse::<IpAddr>().unwrap()),
            "Public fixture {value}"
        );
    }
}

#[test]
fn authentication_is_exact_and_required_before_destination_access() {
    assert!(constant_equal(b"same", b"same"));
    assert!(!constant_equal(b"same", b"samE"));
    assert!(!constant_equal(b"same", b"same-more"));
    let proxy = Proxy::start(&[]).unwrap();
    for header in [
        String::new(),
        "Proxy-Authorization: Basic incorrect\r\n".to_owned(),
    ] {
        let response = request(
            &proxy,
            &format!(
                "GET http://example.invalid/ HTTP/1.1\r\nHost: example.invalid\r\n{header}\r\n"
            ),
        );
        assert!(response.starts_with("HTTP/1.1 407"), "{response}");
    }
    let response = request(
        &proxy,
        &format!(
            "GET http://example.invalid/ HTTP/1.1\r\nHost: example.invalid\r\n{}\r\n",
            authentication(&proxy)
        ),
    );
    assert!(response.starts_with("HTTP/1.1 403"), "{response}");
}

#[test]
fn ambiguous_http_framing_and_obsolete_header_folding_are_rejected() {
    let proxy = Proxy::start(&[]).unwrap();
    let auth = authentication(&proxy);
    for extra in [
        "Content-Length: 0\r\nContent-Length: 0\r\n",
        "Content-Length: 1\r\nTransfer-Encoding: chunked\r\n",
        "Content-Length: +1\r\n",
        "Transfer-Encoding: chunked\r\n",
        "Host: second.example.invalid\r\n",
        " Folded: value\r\n",
        "Proxy-Authorization: Basic incorrect\r\n",
    ] {
        let response = request(&proxy, &format!("POST http://example.invalid/ HTTP/1.1\r\nHost: example.invalid\r\n{auth}{extra}\r\n"));
        assert!(
            response.starts_with("HTTP/1.1 400"),
            "Header {extra:?}: {response}"
        );
    }
}

#[test]
fn invalid_connect_authority_is_refused_without_dialing() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = listener.local_addr().unwrap();
    let proxy = Proxy::start(&[endpoint.to_string()]).unwrap();
    for suffix in ["/path", "?query", "#fragment"] {
        let response = request(
            &proxy,
            &format!(
                "CONNECT {endpoint}{suffix} HTTP/1.1\r\nHost: {endpoint}\r\n{}\r\n",
                authentication(&proxy)
            ),
        );
        assert!(response.starts_with("HTTP/1.1 400"), "{response}");
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn plaintext_pipeline_is_not_forwarded_as_an_unchecked_second_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut connection, _) = listener.accept().unwrap();
        connection
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            match connection.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    bytes.extend_from_slice(&chunk[..count]);
                    assert!(bytes.len() < 32768);
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    break
                }
                Err(error) => panic!("Read controlled upstream: {error}"),
            }
        }
        connection
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .unwrap();
        String::from_utf8(bytes).unwrap()
    });
    let proxy = Proxy::start(&[endpoint.to_string()]).unwrap();
    let response = request(&proxy, &format!("GET http://{endpoint}/ HTTP/1.1\r\nHost: {endpoint}\r\n{}\r\nGET http://evil.example.invalid/ HTTP/1.1\r\nHost: evil.example.invalid\r\n\r\n", authentication(&proxy)));
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let upstream = server.join().unwrap();
    assert_eq!(upstream.matches("GET ").count(), 1, "{upstream}");
    assert!(!upstream.contains("evil.example.invalid"));
    assert!(!upstream
        .to_ascii_lowercase()
        .contains("proxy-authorization"));
}

#[test]
fn authenticated_connect_preserves_bidirectional_binary_tunnel_bytes() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        thread::sleep(Duration::from_millis(100));
        stream.write_all(&[0xde, 0xad, 0x00, 0xff]).unwrap();
        let mut received = Vec::new();
        stream.read_to_end(&mut received).unwrap();
        stream.write_all(&received).unwrap();
        received
    });
    let proxy = Proxy::start(&[endpoint.to_string()]).unwrap();
    let mut client = TcpStream::connect(("127.0.0.1", proxy.port)).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let payload: Vec<u8> = (0..=255).collect();
    let header = format!(
        "CONNECT {endpoint} HTTP/1.1\r\nHost: {endpoint}\r\n{}\r\n",
        authentication(&proxy)
    );
    // Coalesce tunnel bytes with the CONNECT header to catch read-ahead loss.
    client
        .write_all(&[header.as_bytes(), payload.as_slice()].concat())
        .unwrap();
    let mut response_header = Vec::new();
    while !response_header.ends_with(b"\r\n\r\n") {
        let mut byte = [0_u8];
        client.read_exact(&mut byte).unwrap();
        response_header.push(byte[0]);
        assert!(response_header.len() < 4096);
    }
    assert!(response_header.starts_with(b"HTTP/1.1 200 "));
    thread::sleep(Duration::from_millis(100));
    client.write_all(b"delayed tunnel bytes").unwrap();
    client.shutdown(std::net::Shutdown::Write).unwrap();
    let mut payload = payload;
    payload.extend_from_slice(b"delayed tunnel bytes");
    let mut body = Vec::new();
    client.read_to_end(&mut body).unwrap();
    assert_eq!(&body[..4], &[0xde, 0xad, 0x00, 0xff]);
    assert_eq!(&body[4..], payload);
    assert_eq!(server.join().unwrap(), payload);
}
