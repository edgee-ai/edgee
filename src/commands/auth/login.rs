use anyhow::Result;
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm};
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

    println!();
    println!(
        "  {} {}",
        style("Login required.").bold(),
        style("Your browser will open to authenticate with Edgee.").dim()
    );
    println!(
        "  {}",
        style("Once you approve, your API key will be automatically sent back to the CLI.").dim()
    );
    println!();

    let confirmed = Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt("Open browser to continue?")
        .default(true)
        .interact()?;

    if !confirmed {
        anyhow::bail!("Login cancelled.");
    }

    println!();
    println!("  {}", style("If the browser does not open, visit:").dim());
    println!("  {}", style(&url).cyan().underlined());
    println!();

    if let Err(e) = open::that(&url) {
        eprintln!("Could not open browser automatically: {e}");
    }

    println!("  {}", style("Waiting for authentication…").dim());
    let (mut stream, _) = listener.accept().await?;

    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await?;
    let request = String::from_utf8_lossy(&buf[..n]);

    let api_key = extract_api_key(&request)
        .ok_or_else(|| anyhow::anyhow!("No api_key found in callback URL"))?;

    let html = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Edgee — Authenticated</title>
<style>
*,*::before,*::after{box-sizing:border-box;margin:0;padding:0}
body{background:#1a1622;color:hsl(209,20%,95%);font-family:system-ui,-apple-system,sans-serif;min-height:100vh;display:flex;align-items:center;justify-content:center}
.card{text-align:center;max-width:420px;padding:48px 40px}
.logo{margin-bottom:28px;display:inline-block}
.accent{height:3px;width:48px;background:linear-gradient(90deg,#9400D3 0%,#3D2EB3 100%);margin:20px auto 24px;border-radius:2px}
h1{font-family:Georgia,'Times New Roman',serif;font-size:2rem;font-weight:600;letter-spacing:-0.02em;color:hsl(209,20%,95%)}
p{color:hsl(209,15%,60%);font-size:1rem;line-height:1.6;margin-top:4px}
</style>
</head>
<body>
<div class="card">
  <div class="logo">
    <svg width="52" height="52" viewBox="0 0 52 52" fill="none" xmlns="http://www.w3.org/2000/svg">
      <defs>
        <linearGradient id="eg" x1="0" y1="0" x2="1" y2="0">
          <stop offset="0%" stop-color="#9400D3"/>
          <stop offset="100%" stop-color="#3D2EB3"/>
        </linearGradient>
      </defs>
      <polygon points="26,4 50,46 2,46" fill="url(#eg)"/>
      <polygon points="26,18 39,42 13,42" fill="#1a1622"/>
    </svg>
  </div>
  <h1>You&#39;re all set</h1>
  <div class="accent"></div>
  <p>Authentication successful. You can close this tab<br>and head back to your terminal.</p>
</div>
</body>
</html>"##;
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
