//! Keep saved Codex threads resolvable when Edgee's launch overrides are absent.

use std::path::Path;

use anyhow::{Context, Result};

pub(super) const PROVIDER_ID: &str = "edgee-cli";
const BEGIN: &str = "# >>> edgee saved-thread compatibility";
const END: &str = "# <<< edgee saved-thread compatibility";

/// No base URL: Codex chooses its normal endpoint from the OpenAI auth mode.
/// No Edgee headers or selector: bare Codex must not keep routing through Edgee.
pub(super) fn with_fallback(text: &str) -> Result<String> {
    let mut config: toml::Table = text
        .parse()
        .context("Invalid Codex config.toml; fix it and retry")?;
    if config
        .get("model_providers")
        .and_then(|v| v.get(PROVIDER_ID))
        .is_some()
    {
        return Ok(text.to_owned());
    }
    let appended = format!(
        "{text}\n{BEGIN}\n\
         # Saved Edgee threads use normal OpenAI authentication outside Edgee.\n\
         [model_providers.{PROVIDER_ID}]\n\
         name = \"OpenAI\"\n\
         wire_api = \"responses\"\n\
         requires_openai_auth = true\n\
         {END}\n"
    );
    if appended.parse::<toml::Table>().is_ok() {
        return Ok(appended);
    }

    // Inline tables cannot be extended by appending a table header. Rebuild
    // those configurations structurally, preserving their existing values.
    let providers = config
        .entry("model_providers")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .context("Invalid Codex model_providers; use a TOML table and retry")?;
    let provider: toml::Table =
        "name = 'OpenAI'\nwire_api = 'responses'\nrequires_openai_auth = true\n".parse()?;
    providers.insert(PROVIDER_ID.to_owned(), toml::Value::Table(provider));
    let updated = toml::to_string(&config).context("Serialize Codex compatibility config")?;
    updated
        .parse::<toml::Table>()
        .context("Validate Codex compatibility config before saving")?;
    Ok(updated)
}

pub(super) fn strip_fallback(text: &str) -> String {
    let mut inside = false;
    text.lines()
        .filter(|line| {
            if line.trim() == BEGIN {
                inside = true;
                return false;
            }
            if line.trim() == END {
                inside = false;
                return false;
            }
            !inside
        })
        .map(|line| format!("{line}\n"))
        .collect()
}

pub(super) fn ensure_fallback(path: &Path) -> Result<()> {
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Read {}; check its permissions", path.display()))
        }
    };
    let updated = with_fallback(&existing)?;
    if updated != existing {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, updated)
            .with_context(|| format!("Update {}; check its permissions", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_provider_survives_without_routing_or_credentials() {
        let text = with_fallback("model_provider = \"openai\"\n").unwrap();
        let config: toml::Table = text.parse().unwrap();
        assert_eq!(config["model_provider"].as_str(), Some("openai"));
        let provider = config["model_providers"][PROVIDER_ID].as_table().unwrap();
        assert_eq!(provider.len(), 3);
        assert_eq!(provider["requires_openai_auth"].as_bool(), Some(true));
        assert_eq!(with_fallback(&text).unwrap(), text);
    }

    #[test]
    fn preserves_existing_provider_and_rejects_invalid_config() {
        let text = "[model_providers.edgee-cli]\nname = 'Custom'\n";
        assert_eq!(with_fallback(text).unwrap(), text);
        assert!(with_fallback("model = [").is_err());
    }

    #[test]
    fn inline_provider_tables_preserve_settings_and_accept_fallback() {
        for providers in ["{}", "{ custom = { name = 'Custom', base_url = 'https://example.com' } }"] {
            let original = format!("model = 'example'\nmodel_providers = {providers}\n");
            let before: toml::Table = original.parse().unwrap();
            let updated = with_fallback(&original).unwrap();
            let mut after: toml::Table = updated.parse().unwrap();
            let provider = after["model_providers"].as_table_mut().unwrap()
                .remove(PROVIDER_ID).unwrap();
            assert_eq!(provider["requires_openai_auth"].as_bool(), Some(true));
            assert_eq!(after, before);
            assert_eq!(with_fallback(&updated).unwrap(), updated);
        }
    }

    #[test]
    fn invalid_config_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for original in ["model = [", "model_providers = 'invalid'"] {
            std::fs::write(&path, original).unwrap();
            assert!(ensure_fallback(&path).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        }
    }
}
