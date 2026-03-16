use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, clap::Parser)]
pub struct Options {}

pub async fn run(_opts: Options) -> Result<()> {
    let api_key = perform_login().await?;
    println!("Logged in. API key: {}", mask_key(&api_key));
    Ok(())
}

pub async fn perform_login() -> Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();

    let callback = format!("http://127.0.0.1:{port}");
    let url = format!(
        "{}/authorize/apikey?callback={}&name=Edgee+CLI&compression=claude",
        crate::config::console_base_url(),
        percent_encode(&callback)
    );

    println!("Opening browser for authentication...");
    println!("If the browser does not open, visit:\n  {url}");

    if let Err(e) = open::that(&url) {
        eprintln!("Could not open browser automatically: {e}");
    }

    println!("Waiting for authentication callback...");
    let (mut stream, _) = listener.accept().await?;

    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await?;
    let request = String::from_utf8_lossy(&buf[..n]);

    let api_key = extract_api_key(&request)
        .ok_or_else(|| anyhow::anyhow!("No api_key found in callback URL"))?;

    let html = "<!DOCTYPE html><html><body>\
        <h1>Authentication successful!</h1>\
        <p>You can close this tab and return to the terminal.</p>\
        </body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    );
    stream.write_all(response.as_bytes()).await?;

    let mut creds = crate::config::read()?;
    creds.api_key = api_key.clone();
    crate::config::write(&creds)?;

    Ok(api_key)
}

fn percent_encode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            _ => c.to_string(),
        })
        .collect()
}

fn extract_api_key(request: &str) -> Option<String> {
    let first_line = request.lines().next()?;
    let path = first_line.split_whitespace().nth(1)?;
    let (_, query) = path.split_once('?')?;
    for param in query.split('&') {
        if let Some((key, value)) = param.split_once('=') {
            if key == "api_key" {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn mask_key(key: &str) -> String {
    if key.len() <= 12 {
        return "***".to_string();
    }
    format!("{}…{}", &key[..8], &key[key.len() - 4..])
}
