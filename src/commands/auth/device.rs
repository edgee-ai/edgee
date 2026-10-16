use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Deserialize)]
struct Authorization {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Deserialize)]
struct Token {
    access_token: String,
    email: Option<String>,
    user_id: Option<String>,
}

#[derive(Deserialize)]
struct OAuthError {
    error: String,
}

pub(super) async fn authenticate() -> Result<(String, Option<String>, Option<String>)> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let base = crate::config::console_api_base_url();
    let response = client
        .post(format!("{}/auth/device/code", base.trim_end_matches('/')))
        .send()
        .await
        .context("Could not start device login. Check your connection and retry `edgee auth login --device`.")?;
    if !response.status().is_success() {
        bail!(
            "Device login is unavailable (HTTP {}). Retry or contact your Edgee administrator.",
            response.status()
        );
    }
    let auth: Authorization = response
        .json()
        .await
        .context("Invalid device login response")?;
    eprintln!("\nOpen {} in your browser.", auth.verification_uri);
    eprintln!("Enter code: {}", auth.user_code);
    eprintln!("Only approve this code if it matches your terminal.\nWaiting for authentication…");
    let token = poll(&client, &base, &auth).await?;
    Ok((token.access_token, token.email, token.user_id))
}

async fn poll(client: &reqwest::Client, base: &str, auth: &Authorization) -> Result<Token> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(auth.expires_in.min(900));
    let mut interval = auth.interval.clamp(1, 60);
    loop {
        let attempt = async {
            tokio::time::sleep(Duration::from_secs(interval)).await;
            client
                .post(format!("{}/auth/device/token", base.trim_end_matches('/')))
                .form(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("device_code", auth.device_code.as_str()),
                ])
                .send()
                .await
        };
        let response = match tokio::time::timeout_at(deadline, attempt).await {
            Err(_) => bail!("Device code expired. Run `edgee auth login --device` again."),
            Ok(Err(_)) => {
                interval = (interval * 2).min(60);
                continue;
            }
            Ok(Ok(response)) => response,
        };
        if response.status().is_success() {
            let token: Token = response
                .json()
                .await
                .context("Invalid device token response")?;
            if token.access_token.is_empty() {
                bail!("Empty device token. Run `edgee auth login --device` again.");
            }
            return Ok(token);
        }
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            interval = (interval + 5).min(60);
            continue;
        }
        let error: OAuthError = response
            .json()
            .await
            .context("Device login failed. Run `edgee auth login --device` again.")?;
        match error.error.as_str() {
            "authorization_pending" => {}
            "slow_down" => interval = (interval + 5).min(60),
            "access_denied" => {
                bail!("Device login was denied. Run `edgee auth login --device` to retry.")
            }
            "expired_token" => bail!(
                "Device code expired or was already used. Run `edgee auth login --device` again."
            ),
            _ => bail!("Device login failed. Run `edgee auth login --device` again."),
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    async fn server(replies: Vec<(u16, &'static str)>) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            for (status, body) in replies {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![];
                loop {
                    let mut buf = [0; 2048];
                    let n = socket.read(&mut buf).await.unwrap();
                    assert_ne!(n, 0);
                    request.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&request);
                    if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                        let len: usize = headers
                            .lines()
                            .find_map(|line| {
                                line.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse().unwrap())
                            })
                            .unwrap();
                        if body.len() >= len {
                            break;
                        }
                    }
                }
                let request = String::from_utf8(request).unwrap();
                assert!(request.starts_with("POST /auth/device/token "));
                assert!(request.contains("device_code=test-secret"));
                assert!(request
                    .contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"));
                let response = format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        (base, task)
    }

    fn authorization() -> Authorization {
        Authorization {
            device_code: "test-secret".into(),
            user_code: "ABCDE-FGHJK".into(),
            verification_uri: "https://example.test/authorize/device".into(),
            expires_in: 30,
            interval: 1,
        }
    }

    #[tokio::test]
    async fn waits_for_approval_then_returns_identity() {
        let (base, server) = server(vec![
            (400, r#"{"error":"authorization_pending"}"#),
            (
                200,
                r#"{"access_token":"token","email":"me@example.test","user_id":"user"}"#,
            ),
        ])
        .await;
        let token = poll(&reqwest::Client::new(), &base, &authorization())
            .await
            .unwrap();
        assert_eq!(token.access_token, "token");
        assert_eq!(token.user_id.as_deref(), Some("user"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn denial_and_expiration_stop_polling() {
        for (body, expected) in [
            (r#"{"error":"access_denied"}"#, "denied"),
            (r#"{"error":"expired_token"}"#, "expired"),
        ] {
            let (base, server) = server(vec![(400, body)]).await;
            let result = poll(&reqwest::Client::new(), &base, &authorization()).await;
            assert!(result.err().unwrap().to_string().contains(expected));
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn deadline_stops_even_without_a_server() {
        let mut auth = authorization();
        auth.expires_in = 0;
        let result = poll(&reqwest::Client::new(), "http://127.0.0.1:1", &auth).await;
        assert!(result.err().unwrap().to_string().contains("expired"));
    }
}
