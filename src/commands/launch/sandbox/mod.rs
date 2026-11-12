//! Local OS isolation. This path deliberately prepares a fresh agent home rather
//! than loading host hooks, plugins, MCP credentials or symlinked agent homes.
pub(super) mod policy;
mod runtime;
#[cfg(test)]
mod worktree_tests;

use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use anyhow::{bail, Context, Result};

use super::{util, Command};

pub(super) async fn run(
    prepared: policy::Prepared,
    target: Command,
    reroute: &super::reroute::Options,
) -> Result<()> {
    let (agent, args) = match target {
        Command::Claude(options) if !options.relay => ("claude", options.args),
        Command::Codex(options) => ("codex", options.args),
        Command::Claude(_) => bail!("Sandbox mode cannot use --relay. Remove --relay to use the sandbox's restricted network transport."),
        _ => bail!("Local sandbox supports Claude Code and Codex CLI only. Choose `claude` or `codex`; mandatory sandbox policy cannot be bypassed."),
    };
    let host_home = PathBuf::from(
        std::env::var_os("HOME")
            .context("HOME is missing; set your home directory before launching a sandbox.")?,
    );
    let runtime_root = prepared
        .policy
        .runtime_root
        .clone()
        .unwrap_or_else(|| host_home.join(".local/share/edgee/sandbox-runtime"));
    let node_binary = prepared.policy.node_binary.clone().or_else(|| util::find_binary("node"))
        .context("Sandbox requires Node.js >=22.12. Install Node.js and the pinned sandbox runtime; see docs/sandbox.md.")?.canonicalize()?;
    runtime::preflight(&node_binary, &runtime_root, &prepared.policy.tool_path)?;
    let binary = util::find_binary(agent)
        .with_context(|| {
            format!("{agent} is not installed. Install the agent before launching its sandbox.")
        })?
        .canonicalize()?;
    reject_workspace_credentials(&prepared.workspace)?;
    let session = tempfile::Builder::new()
        .prefix("edgee-sandbox-")
        .tempdir()?;
    let session_dir = session.path().canonicalize()?;
    let home = session_dir.join("home");
    let tmp = session_dir.join("tmp");
    std::fs::create_dir(&home)?;
    std::fs::create_dir(&tmp)?;
    let mut read_paths = system_read_paths();
    let git_binary = native_git()?;
    let tool_path = if let Some(bin) = git_binary.parent() {
        if cfg!(target_os = "macos") {
            if let Some(usr) = bin.parent() {
                read_paths.push(usr.to_path_buf());
            }
        }
        format!(
            "{}:/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin:/opt/homebrew/bin",
            bin.display()
        )
    } else {
        "/usr/bin:/bin".to_owned()
    };
    read_paths.extend([
        PathBuf::from("/etc/codex"),
        PathBuf::from("/private/etc/codex"),
    ]);
    read_paths.extend(prepared.policy.read_paths.clone());
    read_paths.extend(prepared.git_metadata.clone());
    read_paths.push(binary.clone());
    if agent == "codex" {
        if let Some(helper) = binary
            .parent()
            .map(|dir| dir.join("codex-code-mode-host"))
            .filter(|p| p.is_file())
        {
            read_paths.push(helper.canonicalize()?);
        }
    }
    // Interpreted agent installations need their explicit package/tool root in
    // the trusted policy. Never grant the executable's entire parent implicitly.
    let mut cmd = ProcessCommand::new(&binary);
    cmd.env_clear().current_dir(&prepared.cwd);
    for name in ["TERM", "COLORTERM", "LANG", "LC_ALL", "LC_CTYPE", "TZ"] {
        if let Some(value) = std::env::var_os(name) {
            cmd.env(name, value);
        }
    }
    cmd.env("HOME", &home)
        .env("TMPDIR", &tmp)
        .env("TMP", &tmp)
        .env("TEMP", &tmp)
        .env("CLAUDE_CODE_TMPDIR", &tmp)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("PATH", &tool_path)
        .env("SHELL", "/bin/sh")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");

    if let Some(url) = &prepared.clone_url {
        validate_gateway(url, &prepared.policy.allowed_domains)
            .context("Repository host is not allowed by sandbox network policy")?;
        let template = session_dir.join("git-template");
        std::fs::create_dir(&template)?;
        let mut clone = ProcessCommand::new(&git_binary);
        clone.env_clear();
        for (key, value) in cmd.get_envs() {
            if let Some(value) = value {
                clone.env(key, value);
            }
        }
        clone
            .env("GIT_TERMINAL_PROMPT", "0")
            .args([
                "-c",
                "credential.helper=",
                "-c",
                "http.followRedirects=false",
                "-c",
                "core.hooksPath=/dev/null",
                "clone",
                "--template",
            ])
            .arg(&template)
            .arg("--")
            .arg(url)
            .arg(&prepared.workspace);
        let clone_spec = runtime::RuntimeSpec {
            tool_path: prepared.policy.tool_path.clone(),
            workspace: prepared.workspace.clone(),
            cwd: prepared.cwd.clone(),
            session_dir: session_dir.clone(),
            read_paths: read_paths.clone(),
            write_paths: vec![],
            deny_write_paths: vec![],
            allowed_domains: prepared.policy.allowed_domains.clone(),
            runtime_root: runtime_root.clone(),
            node_binary: node_binary.clone(),
        };
        match runtime::execute(&clone, &clone_spec) {
            Ok(status) if status.success() => {}
            result => {
                let _ = std::fs::remove_dir_all(&prepared.workspace);
                result.context("Sandboxed repository clone failed")?;
                bail!("Sandboxed repository clone failed. Allow the HTTPS repository host in administrator policy and retry; private repositories must be cloned separately.");
            }
        }
        reject_workspace_credentials(&prepared.workspace)?;
    }

    // Host-side login never runs inside the sandbox. No console token is passed
    // to the child, and automatic Edgee MCP injection is intentionally absent.
    let mut creds = crate::config::read()?;
    if creds.user_token.as_deref().unwrap_or("").is_empty() {
        crate::commands::auth::login::perform_login().await?;
    }
    crate::commands::auth::login::ensure_org_selected().await?;
    let key = crate::commands::auth::login::ensure_valid_provider_key(agent).await?;
    if key.created {
        crate::commands::auth::login::ensure_onboarded(agent).await?;
    }
    creds = crate::config::read()?;
    let org = super::fetch_active_org(&creds).await;
    let gateway = super::gateway_base_url_with_org(org.as_ref());
    validate_gateway(&gateway, &prepared.policy.allowed_domains)?;
    let api_key = creds
        .provider_api_key(agent)
        .context("No agent key is configured. Run `edgee auth login` first.")?;
    let reroute = reroute.resolve().await?;
    let session_id = reroute.create_session(&creds, agent).await?;
    cmd.env("EDGEE_SESSION_ID", &session_id);
    if agent == "claude" {
        let claude_home = home.join(".claude");
        std::fs::create_dir(&claude_home)?;
        // Authentication was handled above; avoid first-run connectivity probes
        // to vendor hosts outside the administrator's model gateway allowlist.
        let onboarding = br#"{"hasCompletedOnboarding":true,"theme":"dark"}"#;
        std::fs::write(home.join(".claude.json"), onboarding)?;
        std::fs::write(claude_home.join(".claude.json"), onboarding)?;
        cmd.env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
        cmd.env("CLAUDE_CONFIG_DIR", home.join(".claude"))
            .env("ANTHROPIC_BASE_URL", &gateway)
            .env(
                "ANTHROPIC_CUSTOM_HEADERS",
                format!("x-edgee-api-key: {api_key}\nx-edgee-session-id: {session_id}"),
            );
        if creds.claude.as_ref().and_then(|p| p.connection.as_deref()) == Some("api") {
            cmd.env("ANTHROPIC_API_KEY", api_key);
        } else {
            let token = claude_token(&host_home)?;
            cmd.env("CLAUDE_CODE_OAUTH_TOKEN", token);
        }
    } else {
        // Use the public CA bundle without granting host Keychain broker access.
        if let Some(ca) = [
            "/etc/ssl/cert.pem",
            "/etc/ssl/certs/ca-certificates.crt",
            "/etc/pki/tls/certs/ca-bundle.crt",
        ]
        .iter()
        .map(Path::new)
        .find(|p| p.is_file())
        {
            cmd.env("SSL_CERT_FILE", ca).env("CODEX_CA_CERTIFICATE", ca);
        }
        let codex_home = home.join(".codex");
        std::fs::create_dir(&codex_home)?;
        let source = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| host_home.join(".codex"));
        let auth = source.join("auth.json");
        if auth.exists() {
            copy_auth(&auth, &codex_home.join("auth.json"))?;
        }
        cmd.env("CODEX_HOME", &codex_home)
            .env("EDGEE_SANDBOX_API_KEY", api_key);
        for setting in [
            "model_provider=\"edgee-cli\"".to_owned(),
            "features.apps=false".to_owned(),
            "model_providers.edgee-cli.name=\"EDGEE\"".to_owned(),
            format!("model_providers.edgee-cli.base_url={}", toml_string(&format!("{}/v1", gateway.trim_end_matches('/')))),
            "model_providers.edgee-cli.wire_api=\"responses\"".to_owned(),
            "model_providers.edgee-cli.env_http_headers={\"x-edgee-api-key\"=\"EDGEE_SANDBOX_API_KEY\",\"x-edgee-session-id\"=\"EDGEE_SESSION_ID\"}".to_owned(),
            "model_providers.edgee-cli.requires_openai_auth=true".to_owned(),
        ] { cmd.args(["-c", &setting]); }
    }
    cmd.args(&args);
    let (write_paths, mut deny_write_paths) = worktree_access(&prepared)?;
    deny_write_paths.extend(protected_paths(&prepared.workspace));
    deny_write_paths.push(binary.clone());
    deny_write_paths.push(node_binary.clone());
    deny_write_paths.push(runtime_root.clone());
    let spec = runtime::RuntimeSpec {
        tool_path: prepared.policy.tool_path.clone(),
        workspace: prepared.workspace.clone(),
        cwd: prepared.cwd.clone(),
        session_dir,
        read_paths,
        write_paths,
        allowed_domains: prepared.policy.allowed_domains.clone(),
        runtime_root,
        node_binary,
        deny_write_paths,
    };
    eprintln!(
        "Edgee sandbox requested: workspace {} (changes persist); network: {}",
        prepared.workspace.display(),
        prepared.policy.allowed_domains.join(", ")
    );
    let status = runtime::execute(&cmd, &spec)?;
    // Explicit cleanup precedes process::exit, which otherwise skips destructors.
    session.close().context("Sandbox ended but private session cleanup failed; remove the session directory before retrying.")?;
    super::print_session_stats(&creds, &session_id, agent).await;
    std::process::exit(exit_code(status));
}

/// Grant only the selected worktree's administration and shared Git data,
/// never other worktrees, shared configuration, hooks or object alternates.
fn worktree_access(prepared: &policy::Prepared) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut writes = Vec::new();
    let mut denies = Vec::new();
    for gitdir in &prepared.git_metadata {
        let common_file = gitdir.join("commondir");
        if !common_file.is_file() {
            continue;
        }
        let common = gitdir
            .join(std::fs::read_to_string(&common_file)?.trim())
            .canonicalize()?;
        let expected_parent = common.join("worktrees").canonicalize()?;
        let workspace_git = prepared.workspace.join(".git");
        anyhow::ensure!(expected_parent == common.join("worktrees")
            && std::fs::symlink_metadata(&workspace_git)?.file_type().is_file(),
            "Git worktree control paths must not be symlinks; repair the repository before sandboxing.");
        let backlink =
            PathBuf::from(std::fs::read_to_string(gitdir.join("gitdir"))?.trim()).canonicalize()?;
        anyhow::ensure!(gitdir.parent() == Some(expected_parent.as_path())
            && backlink == prepared.workspace.join(".git").canonicalize()?
            && prepared.git_metadata.contains(&common),
            "Git worktree metadata does not point back to this workspace; refusing writable external Git access.");
        writes.push(gitdir.clone());
        denies.extend(git_protected_paths(gitdir));
        for name in ["objects", "refs", "logs"] {
            let candidate = common.join(name);
            if !candidate.exists() {
                continue;
            }
            let resolved = candidate.canonicalize()?;
            anyhow::ensure!(resolved == candidate && !std::fs::symlink_metadata(&candidate)?.file_type().is_symlink() && resolved.is_dir(), "Git data directory escapes its administration directory; repair the repository before sandboxing.");
            writes.push(resolved);
        }
        denies.push(common.join("objects/info"));
    }
    Ok((writes, denies))
}

fn native_git() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let output = ProcessCommand::new("/usr/bin/xcrun")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .args(["--find", "git"])
            .output()
            .context("Cannot resolve system Git; install Xcode Command Line Tools.")?;
        anyhow::ensure!(
            output.status.success(),
            "System Git is unavailable. Install Xcode Command Line Tools before sandboxing."
        );
        let path = PathBuf::from(String::from_utf8(output.stdout)?.trim()).canonicalize()?;
        anyhow::ensure!(
            path.is_absolute() && path.is_file(),
            "Invalid system Git path; repair Xcode Command Line Tools."
        );
        Ok(path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(PathBuf::from("/usr/bin/git"))
    }
}

fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_owned()).to_string()
}

fn validate_gateway(gateway: &str, domains: &[String]) -> Result<()> {
    let url =
        reqwest::Url::parse(gateway).context("Invalid gateway URL; configure an HTTPS gateway.")?;
    anyhow::ensure!(url.scheme() == "https" && url.username().is_empty() && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && url.host_str().is_some_and(|host| domains.iter().any(|domain| domain == host)),
        "Gateway must use HTTPS port 443 and an exact domain allowed by /etc/edgee/sandbox.toml. Ask your administrator to update the sandbox policy.");
    Ok(())
}

fn copy_auth(source: &Path, destination: &Path) -> Result<()> {
    let data = std::fs::read(source)
        .context("Cannot read agent authentication. Log in with the native agent first.")?;
    let auth: serde_json::Value = serde_json::from_slice(&data)
        .context("Agent authentication is invalid JSON. Log in with the native agent again.")?;
    let mut selected = serde_json::Map::new();
    for key in ["auth_mode", "OPENAI_API_KEY", "last_refresh"] {
        if let Some(value) = auth.get(key).filter(|v| v.is_string() || v.is_null()) {
            selected.insert(key.to_owned(), value.clone());
        }
    }
    if let Some(tokens) = auth.get("tokens").and_then(|v| v.as_object()) {
        let tokens: serde_json::Map<_, _> =
            ["id_token", "access_token", "refresh_token", "account_id"]
                .iter()
                .filter_map(|key| {
                    tokens
                        .get(*key)
                        .filter(|v| v.is_string() || v.is_null())
                        .map(|value| ((*key).to_owned(), value.clone()))
                })
                .collect();
        selected.insert("tokens".into(), serde_json::Value::Object(tokens));
    }
    std::fs::write(destination, serde_json::to_vec(&selected)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(destination, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn claude_token(home: &Path) -> Result<String> {
    let file = home.join(".claude/.credentials.json");
    let data = if file.is_file() {
        std::fs::read(file)?
    } else {
        #[cfg(target_os = "macos")]
        {
            let output = ProcessCommand::new("/usr/bin/security")
                .args([
                    "find-generic-password",
                    "-s",
                    "Claude Code-credentials",
                    "-w",
                ])
                .output()?;
            anyhow::ensure!(output.status.success(), "Claude authentication is unavailable. Run `claude auth login` outside the sandbox first.");
            output.stdout
        }
        #[cfg(not(target_os = "macos"))]
        {
            bail!("Claude authentication is unavailable. Run `claude auth login` outside the sandbox first.");
        }
    };
    let value: serde_json::Value = serde_json::from_slice(&data)
        .context("Claude credentials are invalid. Run `claude auth login` again.")?;
    value
        .pointer("/claudeAiOauth/accessToken")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .context(
            "Claude OAuth token is unavailable. Run `claude auth login` outside the sandbox first.",
        )
}

fn system_read_paths() -> Vec<PathBuf> {
    // Runtime/tool code only: never /etc, /var, /tmp, /Users or /home wholesale.
    let paths = [
        "/usr/bin",
        "/bin",
        "/usr/lib",
        "/lib",
        "/lib64",
        "/usr/share",
        "/usr/libexec",
        "/System/Library",
        "/Library/Apple",
        "/dev/null",
        "/dev/zero",
        "/dev/random",
        "/dev/urandom",
        "/dev/tty",
        "/etc/ssl",
        "/private/var/select",
        "/etc/pki",
        "/etc/ld.so.cache",
        "/etc/localtime",
    ];
    paths
        .iter()
        .filter_map(|p| Path::new(p).canonicalize().ok())
        .collect()
}

fn protected_paths(workspace: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<_> = [".edgee", ".claude", ".codex"]
        .iter()
        .map(|name| workspace.join(name))
        .collect();
    if workspace.join(".git").is_file() {
        paths.push(workspace.join(".git"));
    }
    paths.extend(git_protected_paths(&workspace.join(".git")));
    paths
}

fn git_protected_paths(git: &Path) -> Vec<PathBuf> {
    [
        "config",
        "config.worktree",
        "hooks",
        "info",
        "objects/info",
        "commondir",
        "gitdir",
        "worktrees",
    ]
    .iter()
    .map(|name| git.join(name))
    .collect()
}

fn reject_workspace_credentials(workspace: &Path) -> Result<()> {
    for credentials in [
        crate::config::credentials_path(),
        crate::config::global_credentials_path(),
    ] {
        if credentials
            .canonicalize()
            .ok()
            .is_some_and(|path| path.starts_with(workspace))
        {
            bail!("Workspace contains active Edgee credentials. Move the credentials outside the workspace before sandboxing.");
        }
    }

    anyhow::ensure!(!workspace.join(".edgee/credentials.toml").exists(),
        "Workspace contains .edgee/credentials.toml. Move Edgee credentials to your home configuration before sandboxing; workspace files are readable by the agent.");
    Ok(())
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
    }
    #[cfg(not(unix))]
    {
        status.code().unwrap_or(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gateway_must_match_exact_https_origin() {
        let domains = vec!["api.edgee.ai".to_owned()];
        assert!(validate_gateway("https://api.edgee.ai", &domains).is_ok());
        for url in [
            "http://api.edgee.ai",
            "https://api.edgee.ai.evil.test",
            "https://127.0.0.1",
            "https://api.edgee.ai:8443",
            "https://user@api.edgee.ai",
        ] {
            assert!(validate_gateway(url, &domains).is_err(), "{url}");
        }
    }
    #[test]
    fn workspace_credentials_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join(".edgee")).unwrap();
        std::fs::write(temp.path().join(".edgee/credentials.toml"), "fake").unwrap();
        assert!(reject_workspace_credentials(temp.path()).is_err());
    }
}
