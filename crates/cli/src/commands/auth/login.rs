use anyhow::{Context, Result};
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, clap::Parser)]
pub struct Options {}

pub async fn run(_opts: Options) -> Result<()> {
    let email = perform_login().await?;

    println!();
    println!(
        "  {}",
        style(format!("Logged in as {email}")).bold()
    );
    println!();
    Ok(())
}

pub async fn perform_login() -> Result<String> {
    // Authenticate via browser — do NOT clear credentials before we have a new token,
    // so an aborted re-login doesn't wipe an existing valid token.
    let mut creds = crate::config::Credentials::default();

    let (user_token, email, user_id) = {
        let (token, email, user_id) = browser_auth().await?;
        println!();
        println!("  {}", style("Authenticated!").green().bold());
        (token, email, user_id)
    };

    // Store new credentials immediately so the token is not lost if the user aborts org selection
    creds.user_token = Some(user_token.clone());
    creds.email = email.clone();
    creds.user_id = user_id.clone();
    crate::config::write(&creds)?;

    // --- Select organization ---
    let client = crate::api::ApiClient::new(&user_token)?;
    let orgs = client.list_organizations().await?;

    if orgs.is_empty() {
        anyhow::bail!("No organizations found. Please create one at {} first.", crate::config::console_base_url());
    }

    let (org_id, org_slug) = if orgs.len() == 1 {
        let org = &orgs[0];
        println!(
            "  {} {}",
            style("Organization:").dim(),
            style(&org.name).bold()
        );
        (org.id.clone(), org.slug.clone())
    } else {
        use dialoguer::Select;
        let items: Vec<String> = orgs.iter().map(|o| o.name.clone()).collect();
        let selection = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Select organization")
            .items(&items)
            .default(0)
            .interact()?;
        let org = &orgs[selection];
        (org.id.clone(), org.slug.clone())
    };

    // --- Store credentials ---
    creds.user_token = Some(user_token);
    creds.email = email.clone();
    creds.user_id = user_id;
    creds.org_slug = Some(org_slug);
    creds.org_id = Some(org_id);
    crate::config::write(&creds)?;

    Ok(email
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "(unknown)".to_string()))
}

/// Ensures an organization is selected. If the user already has a token but no org,
/// prompts for selection without re-doing browser auth.
pub async fn ensure_org_selected() -> Result<()> {
    let mut creds = crate::config::read()?;

    if creds.org_id.as_deref().filter(|s| !s.is_empty()).is_some() {
        return Ok(());
    }

    let user_token = creds.user_token.as_deref().filter(|t| !t.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Not authenticated. Run `edgee auth login` first."))?
        .to_string();

    let client = crate::api::ApiClient::new(&user_token)?;
    let orgs = client.list_organizations().await?;

    if orgs.is_empty() {
        anyhow::bail!("No organizations found. Please create one at {} first.", crate::config::console_base_url());
    }

    let (org_id, org_slug) = if orgs.len() == 1 {
        let org = &orgs[0];
        println!(
            "  {} {}",
            style("Organization:").dim(),
            style(&org.name).bold()
        );
        (org.id.clone(), org.slug.clone())
    } else {
        use dialoguer::Select;
        let items: Vec<String> = orgs.iter().map(|o| o.name.clone()).collect();
        let selection = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Select organization")
            .items(&items)
            .default(0)
            .interact()?;
        let org = &orgs[selection];
        (org.id.clone(), org.slug.clone())
    };

    creds.org_slug = Some(org_slug);
    creds.org_id = Some(org_id);
    crate::config::write(&creds)?;

    Ok(())
}

/// Creates a gateway API key for the given provider if one doesn't exist yet.
/// Requires that the user is already authenticated (user_token + org_id set).
pub async fn ensure_provider_key(provider: &str) -> Result<()> {
    let mut creds = crate::config::read()?;

    let user_token = creds.user_token.as_deref().filter(|t| !t.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Not authenticated. Run `edgee auth login` first."))?;
    let org_id = creds.org_id.as_deref().filter(|t| !t.is_empty())
        .ok_or_else(|| anyhow::anyhow!("No organization selected. Run `edgee auth login` first."))?;

    let assistant_name = coding_assistant_name(provider)?;

    let client = crate::api::ApiClient::new(user_token)?;
    let key_item = client.get_or_create_key(org_id, assistant_name).await
        .context(format!("Failed to get or create {} API key", assistant_name))?;
    let api_key = key_item.key
        .ok_or_else(|| anyhow::anyhow!("API key response did not include a key value"))?;

    let provider_config = crate::config::ProviderConfig {
        api_key,
        api_key_id: Some(key_item.id),
        connection: None,
    };
    provider_config_mut(&mut creds, provider)?.replace(provider_config);
    crate::config::write(&creds)?;

    Ok(())
}

fn coding_assistant_name(provider: &str) -> Result<&'static str> {
    match provider {
        "claude" => Ok("claude_code"),
        "codex" => Ok("codex"),
        "opencode" => Ok("opencode"),
        _ => anyhow::bail!("Unsupported provider `{provider}`"),
    }
}

fn provider_config_mut<'a>(
    creds: &'a mut crate::config::Credentials,
    provider: &str,
) -> Result<&'a mut Option<crate::config::ProviderConfig>> {
    match provider {
        "claude" => Ok(&mut creds.claude),
        "codex" => Ok(&mut creds.codex),
        "opencode" => Ok(&mut creds.opencode),
        _ => anyhow::bail!("Unsupported provider `{provider}`"),
    }
}

async fn browser_auth() -> Result<(String, Option<String>, Option<String>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();

    let callback = format!("http://127.0.0.1:{port}");
    let url = format!(
        "{}/authorize/oauth/apikey?callback={}&name=Edgee+CLI",
        crate::config::console_base_url(),
        percent_encode(&callback),
    );

    println!();
    println!(
        "  {} {}",
        style("Login required.").bold(),
        style("Your browser will open to authenticate with Edgee.").dim()
    );
    println!(
        "  {}",
        style("Once you sign in or create an account, you'll get access to usage analytics,").dim()
    );
    println!(
        "  {}",
        style("token consumption insights, and session history for your Claude Code usage.").dim()
    );
    println!();
    println!(
        "  {}",
        style("Your Edgee API key will be automatically created and saved in the CLI.").dim()
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

    let user_token = extract_param(&request, "api_key")
        .ok_or_else(|| anyhow::anyhow!("No api_key found in callback URL"))?;
    let email = extract_param(&request, "email");
    let user_id = extract_param(&request, "user_id");

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
    <svg width="120" height="28" viewBox="0 0 120 28" fill="none" xmlns="http://www.w3.org/2000/svg">
      <path d="M114.209 21.176C110.203 21.176 107.65 18.6644 107.65 14.0276C107.65 9.9704 110.34 6.7688 114.072 6.7688C117.503 6.7688 119.89 8.9768 120 12.9788V13.586H110.559V13.9448C110.559 17.4776 112.371 19.382 115.28 19.382C116.789 19.382 117.914 18.9128 119.012 18.14L119.671 18.9956C118.244 20.3756 116.405 21.176 114.209 21.176ZM110.642 12.3164L117.256 12.2336C117.173 9.722 115.993 8.066 114.127 8.066C112.233 8.066 110.998 9.6392 110.642 12.3164Z" fill="white"/>
      <path d="M98.4044 21.176C94.3976 21.176 91.8453 18.6644 91.8453 14.0276C91.8453 9.9704 94.5348 6.7688 98.2672 6.7688C101.698 6.7688 104.085 8.9768 104.195 12.9788V13.586H94.7544V13.9448C94.7544 17.4776 96.5657 19.382 99.4747 19.382C100.984 19.382 102.109 18.9128 103.207 18.14L103.866 18.9956C102.439 20.3756 100.6 21.176 98.4044 21.176ZM94.8367 12.3164L101.451 12.2336C101.368 9.722 100.188 8.066 98.3221 8.066C96.4284 8.066 95.1935 9.6392 94.8367 12.3164Z" fill="white"/>
      <path d="M81.5879 27.8C78.0202 27.8 75.8521 26.5028 75.8521 24.2672C75.8521 23.0804 76.5657 22.004 78.1849 20.9276C76.7029 20.5688 76.1266 19.8512 76.1266 18.6092C76.1266 18.416 76.1266 18.278 76.154 18.14L78.2672 15.2972C76.8401 14.4692 76.0168 13.1168 76.0168 11.516C76.0168 8.9492 78.4593 6.7688 81.8898 6.7688C83.015 6.7688 84.003 6.962 84.8263 7.3208L88.5861 6.7688H89.2997L89.1899 8.7008L86.3906 8.3972C87.2414 9.1976 87.7079 10.274 87.7079 11.5712C87.7079 14.2208 85.2928 16.1804 81.8898 16.1804C80.9292 16.1804 80.1059 16.0424 79.3649 15.7664L78.5416 17.1188C78.5142 17.2292 78.5142 17.3948 78.5142 17.5052C78.5142 18.1676 79.3375 18.692 81.039 18.692H84.7989C87.8726 18.692 89.3271 19.796 89.3271 22.2248C89.3271 25.5092 86.0887 27.8 81.5879 27.8ZM81.8898 14.9108C83.7011 14.9108 84.8537 13.6136 84.8537 11.516C84.8537 9.3908 83.6736 8.0384 81.8898 8.0384C79.9962 8.0384 78.871 9.6116 78.871 11.516C78.871 13.5584 80.0236 14.9108 81.8898 14.9108ZM82.5759 26.0888C85.3752 26.0888 87.4884 24.902 87.4884 23.246C87.4884 21.8936 86.6925 21.2036 84.497 21.2036H81.1488C80.545 21.2036 80.051 21.2036 79.4473 21.1484C78.5691 21.9488 78.13 22.8044 78.13 23.6324C78.13 25.2608 79.6943 26.0888 82.5759 26.0888Z" fill="white"/>
      <path d="M63.8278 21.176C60.3424 21.176 58.0646 18.3056 58.0646 14.5244C58.0646 9.722 60.8639 6.7688 65.2823 6.7688C66.3527 6.7688 67.3955 7.0724 68.2188 7.5968V4.34C68.2188 3.236 67.9444 2.9048 67.176 2.7116L66.1606 2.4908V1.8008L70.2771 0.199998L70.9084 0.586398V17.5328C70.9084 18.4988 71.1554 18.83 72.061 18.9404L73.0764 19.0508L73.1862 19.7684L69.6185 21.176H69.0422L68.2737 18.9128C66.929 20.3756 65.3647 21.176 63.8278 21.176ZM64.953 19.1612C66.2429 19.1612 67.423 18.4712 68.2188 17.45V10.8536C68.1365 9.1148 66.9015 8.066 65.1177 8.066C62.6752 8.066 60.9736 10.3568 60.9736 13.724C60.9736 17.036 62.4556 19.1612 64.953 19.1612Z" fill="white"/>
      <path d="M48.8188 21.176C44.8119 21.176 42.2597 18.6644 42.2597 14.0276C42.2597 9.9704 44.9492 6.7688 48.6815 6.7688C52.112 6.7688 54.4997 8.9768 54.6094 12.9788V13.586H45.1687V13.9448C45.1687 17.4776 46.98 19.382 49.8891 19.382C51.3985 19.382 52.5237 18.9128 53.6215 18.14L54.2801 18.9956C52.853 20.3756 51.0143 21.176 48.8188 21.176ZM45.251 12.3164L51.865 12.2336C51.7827 9.722 50.6026 8.066 48.7364 8.066C46.8428 8.066 45.6078 9.6392 45.251 12.3164Z" fill="white"/>
      <path d="M11.9146 0.628193C12.0839 0.246001 12.4596 0 12.8739 0H23.4087C24.1705 0 24.6793 0.793671 24.3679 1.49647L23.0569 4.45582C22.8876 4.83801 22.5119 5.08401 22.0976 5.08401H11.5629C10.801 5.08401 10.2922 4.29034 10.6036 3.58754L11.9146 0.628193Z" fill="white"/>
      <path d="M1.404 12.1621C1.57331 11.7799 1.94897 11.5339 2.36328 11.5339H15.0752C15.8371 11.5339 16.3459 12.3275 16.0345 13.0303L14.7235 15.9897C14.5542 16.3719 14.1785 16.6179 13.7642 16.6179H1.05226C0.290392 16.6179 -0.218379 15.8242 0.0929685 15.1214L1.404 12.1621Z" fill="white"/>
      <path d="M1.404 23.5442C1.57331 23.162 1.94897 22.916 2.36328 22.916H19.8801C20.6419 22.916 21.1507 23.7097 20.8394 24.4125L19.5283 27.3718C19.359 27.754 18.9834 28 18.5691 28H1.05226C0.290392 28 -0.218379 27.2063 0.0929685 26.5035L1.404 23.5442Z" fill="white"/>
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

    Ok((user_token, email, user_id))
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

fn extract_param(request: &str, name: &str) -> Option<String> {
    let first_line = request.lines().next()?;
    let path = first_line.split_whitespace().nth(1)?;
    let (_, query) = path.split_once('?')?;
    for param in query.split('&') {
        if let Some((key, value)) = param.split_once('=') {
            if key == name {
                return Some(percent_decode(value));
            }
        }
    }
    None
}

fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let h1 = chars.next();
            let h2 = chars.next();
            if let (Some(h1), Some(h2)) = (h1, h2) {
                if let Ok(byte) = u8::from_str_radix(&format!("{h1}{h2}"), 16) {
                    out.push(byte as char);
                    continue;
                }
            }
        }
        out.push(if c == '+' { ' ' } else { c });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_provider_to_coding_assistant_name() {
        assert_eq!(coding_assistant_name("claude").unwrap(), "claude_code");
        assert_eq!(coding_assistant_name("codex").unwrap(), "codex");
        assert_eq!(coding_assistant_name("opencode").unwrap(), "opencode");
        assert!(coding_assistant_name("unknown").is_err());
    }

    #[test]
    fn selects_the_correct_provider_slot() {
        let mut creds = crate::config::Credentials::default();

        provider_config_mut(&mut creds, "claude")
            .unwrap()
            .replace(crate::config::ProviderConfig::default());
        provider_config_mut(&mut creds, "codex")
            .unwrap()
            .replace(crate::config::ProviderConfig::default());
        provider_config_mut(&mut creds, "opencode")
            .unwrap()
            .replace(crate::config::ProviderConfig::default());

        assert!(creds.claude.is_some());
        assert!(creds.codex.is_some());
        assert!(creds.opencode.is_some());
    }
}
