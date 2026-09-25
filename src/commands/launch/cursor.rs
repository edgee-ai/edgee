//! `edgee launch cursor` — configure Cursor's OpenAI-compatible BYOK provider.
//!
//! Cursor stores this configuration in its VS Code-style `state.vscdb`, not in
//! `settings.json`. We snapshot the relevant rows before the first write so
//! `edgee relay cursor` can restore the user's previous provider and use their
//! Cursor plan through the relay.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use console::style;
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::util;

const APPLICATION_USER_KEY: &str =
    "src.vs.platform.reactivestorage.browser.reactiveStorageServiceImpl.persistentStorage.applicationUser";
const OPENAI_KEY: &str = "cursorAuth/openAIKey";
const OPENAI_SECRET_KEY: &str = "secret://cursorAuth/openAIKey";

#[derive(Debug, clap::Parser)]
pub struct Options {}

#[derive(Debug, Serialize, Deserialize)]
struct Backup {
    db_path: PathBuf,
    application_user: Option<String>,
    openai_key: Option<String>,
    openai_secret_key: Option<String>,
}

pub async fn run(_opts: Options) -> Result<()> {
    ensure_cursor_stopped()?;

    let mut creds = crate::config::read()?;
    if creds.user_token.as_deref().unwrap_or("").is_empty() {
        crate::commands::auth::login::perform_login().await?;
    }
    crate::commands::auth::login::ensure_org_selected().await?;

    let reprovisioned = crate::commands::auth::login::ensure_valid_provider_key("cursor")
        .await?
        .created;
    if reprovisioned {
        crate::commands::auth::login::ensure_onboarded("cursor").await?;
    }
    creds = crate::config::read()?;

    let api_key = creds
        .provider_api_key("cursor")
        .context("no Edgee API key for 'cursor'; run `edgee auth login`")?;
    let gateway_url = super::resolve_gateway_base_url(&creds).await;
    let (models, catalog) = tokio::join!(
        util::fetch_gateway_models(&gateway_url, api_key),
        util::fetch_model_catalog(&creds)
    );
    let models = util::without_app_subscription_models(models, &catalog);

    let db_path = cursor_state_db_path()?;
    configure_provider(&db_path, &gateway_url, api_key, &models)?;
    if let Some(cursor) = creds.cursor.as_mut() {
        cursor.connection = Some("api".into());
        crate::config::write(&creds)?;
    }

    println!(
        "  {} {}",
        style("Cursor configured for Edgee:").green().bold(),
        style(format!("{}/v1", gateway_url.trim_end_matches('/'))).dim()
    );
    println!(
        "  {}",
        style("Use `edgee relay cursor` to switch back to your Cursor plan.").dim()
    );

    // Provider settings persist in Cursor's database, so this launcher can
    // hand off immediately. Only `edgee relay cursor` needs `--wait` to keep
    // its foreground proxy alive for the editor session.
    let status = Command::new(util::resolve_binary("cursor"))
        .status()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!(
                    "Cursor is not on PATH. In Cursor, run: Command Palette → Install 'cursor' command"
                )
            } else {
                anyhow::anyhow!(e)
            }
        })?;

    if status.code().is_some_and(|code| code != 0) {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

/// Restore provider state saved by [`run`] before starting the Plan relay.
pub(crate) fn restore_for_plan() -> Result<()> {
    let path = backup_path();
    if !path.exists() {
        set_connection_mode("plan")?;
        return Ok(());
    }
    ensure_cursor_stopped()?;

    let body = std::fs::read_to_string(&path)
        .with_context(|| format!("reading Cursor provider backup at {}", path.display()))?;
    let backup: Backup = serde_json::from_str(&body).context("parsing Cursor provider backup")?;
    restore_backup(&backup)?;
    std::fs::remove_file(&path)
        .with_context(|| format!("removing Cursor provider backup at {}", path.display()))?;
    set_connection_mode("plan")?;
    eprintln!("{}", style("Restored Cursor plan settings.").dim());
    Ok(())
}

fn set_connection_mode(mode: &str) -> Result<()> {
    let mut creds = crate::config::read()?;
    if let Some(cursor) = creds.cursor.as_mut() {
        cursor.connection = Some(mode.into());
        crate::config::write(&creds)?;
    }
    Ok(())
}

fn configure_provider(
    db_path: &Path,
    gateway_url: &str,
    api_key: &str,
    models: &[String],
) -> Result<()> {
    if !db_path.is_file() {
        anyhow::bail!(
            "Cursor settings database not found at {}. Open Cursor once, quit it, then retry.",
            db_path.display()
        );
    }

    let mut connection = Connection::open(db_path)
        .with_context(|| format!("opening Cursor settings at {}", db_path.display()))?;
    let transaction = connection.transaction()?;

    let application_user = read_value(&transaction, APPLICATION_USER_KEY)?;
    let openai_key = read_value(&transaction, OPENAI_KEY)?;
    let openai_secret_key = read_value(&transaction, OPENAI_SECRET_KEY)?;

    let backup_file = backup_path();
    if !backup_file.exists() {
        write_backup(
            &backup_file,
            &Backup {
                db_path: db_path.to_path_buf(),
                application_user: application_user.clone(),
                openai_key,
                openai_secret_key,
            },
        )?;
    }

    let mut blob = application_user
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .context("parsing Cursor application settings")?
        .unwrap_or_else(|| serde_json::json!({}));
    configure_blob(
        &mut blob,
        &format!("{}/v1", gateway_url.trim_end_matches('/')),
        models,
    );

    set_value(
        &transaction,
        APPLICATION_USER_KEY,
        &serde_json::to_string(&blob)?,
    )?;
    // Modern Cursor prefers the encrypted secret row. Removing it makes Cursor
    // import this legacy cell at startup, encrypt it with Electron safeStorage,
    // then delete the plaintext fallback itself.
    transaction.execute("DELETE FROM ItemTable WHERE key = ?1", [OPENAI_SECRET_KEY])?;
    set_value(&transaction, OPENAI_KEY, api_key)?;
    transaction.commit()?;
    Ok(())
}

fn configure_blob(blob: &mut Value, base_url: &str, models: &[String]) {
    if !blob.is_object() {
        *blob = serde_json::json!({});
    }
    let Some(root) = blob.as_object_mut() else {
        return;
    };
    root.insert("openAIBaseUrl".into(), Value::String(base_url.into()));
    root.insert("useOpenAIKey".into(), Value::Bool(true));

    let ai = root
        .entry("aiSettings")
        .or_insert_with(|| serde_json::json!({}));
    if !ai.is_object() {
        *ai = serde_json::json!({});
    }
    let Some(ai) = ai.as_object_mut() else {
        return;
    };
    merge_models(ai, "userAddedModels", models);
    merge_models(ai, "modelOverrideEnabled", models);
}

fn merge_models(ai: &mut serde_json::Map<String, Value>, key: &str, models: &[String]) {
    let list = ai.entry(key).or_insert_with(|| Value::Array(Vec::new()));
    if !list.is_array() {
        *list = Value::Array(Vec::new());
    }
    let Some(list) = list.as_array_mut() else {
        return;
    };
    for model in models {
        if !list.iter().any(|existing| existing.as_str() == Some(model)) {
            list.push(Value::String(model.clone()));
        }
    }
}

fn restore_backup(backup: &Backup) -> Result<()> {
    let mut connection = Connection::open(&backup.db_path).with_context(|| {
        format!(
            "opening Cursor settings at {}",
            backup.db_path.display()
        )
    })?;
    let transaction = connection.transaction()?;
    restore_value(
        &transaction,
        APPLICATION_USER_KEY,
        backup.application_user.as_deref(),
    )?;
    restore_value(&transaction, OPENAI_KEY, backup.openai_key.as_deref())?;
    restore_value(
        &transaction,
        OPENAI_SECRET_KEY,
        backup.openai_secret_key.as_deref(),
    )?;
    transaction.commit()?;
    Ok(())
}

fn read_value(transaction: &Transaction<'_>, key: &str) -> Result<Option<String>> {
    transaction
        .query_row("SELECT value FROM ItemTable WHERE key = ?1", [key], |row| row.get(0))
        .optional()
        .map_err(Into::into)
}

fn set_value(transaction: &Transaction<'_>, key: &str, value: &str) -> Result<()> {
    transaction.execute(
        "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?1, ?2)",
        (key, value),
    )?;
    Ok(())
}

fn restore_value(transaction: &Transaction<'_>, key: &str, value: Option<&str>) -> Result<()> {
    match value {
        Some(value) => set_value(transaction, key, value),
        None => {
            transaction.execute("DELETE FROM ItemTable WHERE key = ?1", [key])?;
            Ok(())
        }
    }
}

fn write_backup(path: &Path, backup: &Backup) -> Result<()> {
    let parent = path.parent().context("Cursor backup path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let body = serde_json::to_string(backup)?;
    std::fs::write(path, body)
        .with_context(|| format!("writing Cursor provider backup at {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn backup_path() -> PathBuf {
    crate::config::global_data_dir()
        .join("cursor")
        .join("provider-backup.json")
}

fn cursor_state_db_path() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        home_dir().map(|home| {
            home.join("Library/Application Support/Cursor/User/globalStorage/state.vscdb")
        })
    }
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|dir| dir.join("Cursor/User/globalStorage/state.vscdb"))
            .context("APPDATA is not set")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home_dir().ok().map(|home| home.join(".config")))
            .context("HOME and XDG_CONFIG_HOME are not set")?;
        Ok(config.join("Cursor/User/globalStorage/state.vscdb"))
    }
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("HOME is not set")
}

fn ensure_cursor_stopped() -> Result<()> {
    if cursor_is_running() {
        anyhow::bail!(
            "Cursor is running. Quit it completely, then rerun this command so its in-memory settings do not overwrite the provider change."
        );
    }
    Ok(())
}

#[cfg(unix)]
fn cursor_is_running() -> bool {
    let Ok(output) = Command::new("ps").args(["-ax", "-o", "command="]).output() else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        if line.contains(" --type=") {
            return false;
        }
        #[cfg(target_os = "macos")]
        {
            line.contains("Cursor.app/Contents/MacOS/Cursor")
        }
        #[cfg(not(target_os = "macos"))]
        {
            line.split_whitespace()
                .next()
                .and_then(|path| Path::new(path).file_name())
                .is_some_and(|name| name == "cursor")
        }
    })
}

#[cfg(windows)]
fn cursor_is_running() -> bool {
    Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq Cursor.exe", "/NH"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains("Cursor.exe"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_blob_preserves_settings_and_merges_models() {
        let mut blob = serde_json::json!({
            "theme": "dark",
            "aiSettings": {
                "userAddedModels": ["mine"],
                "modelOverrideEnabled": ["mine"]
            }
        });
        configure_blob(
            &mut blob,
            "https://api.edgee.ai/v1",
            &["anthropic/claude-sonnet-5".into(), "mine".into()],
        );

        assert_eq!(blob["theme"], "dark");
        assert_eq!(blob["openAIBaseUrl"], "https://api.edgee.ai/v1");
        assert_eq!(blob["useOpenAIKey"], true);
        assert_eq!(
            blob["aiSettings"]["userAddedModels"],
            serde_json::json!(["mine", "anthropic/claude-sonnet-5"])
        );
    }

    #[test]
    fn provider_state_round_trips_through_backup() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("state.vscdb");
        let connection = Connection::open(&db_path).unwrap();
        connection
            .execute(
                "CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                (APPLICATION_USER_KEY, r#"{"openAIBaseUrl":"https://old.example/v1"}"#),
            )
            .unwrap();
        drop(connection);

        let backup = Backup {
            db_path: db_path.clone(),
            application_user: Some(r#"{"openAIBaseUrl":"https://old.example/v1"}"#.into()),
            openai_key: None,
            openai_secret_key: Some("encrypted-old-key".into()),
        };
        restore_backup(&backup).unwrap();

        let connection = Connection::open(db_path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT value FROM ItemTable WHERE key = ?1",
                    [OPENAI_SECRET_KEY],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "encrypted-old-key"
        );
    }
}
