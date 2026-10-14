//! Keep saved Codex threads resolvable when Edgee's launch overrides are absent.

use std::path::Path;

use anyhow::{Context, Result};

pub(super) const PROVIDER_ID: &str = "edgee-cli";
const BEGIN: &str = "# >>> edgee saved-thread compatibility";
const END: &str = "# <<< edgee saved-thread compatibility";

/// No base URL: Codex chooses its normal endpoint from the OpenAI auth mode.
/// No Edgee headers or selector: bare Codex must not keep routing through Edgee.
pub(super) fn with_fallback(text: &str) -> Result<String> {
    let config: toml::Table = text
        .parse()
        .context("Invalid Codex config.toml; fix it and retry")?;
    if config
        .get("model_providers")
        .and_then(|v| v.get(PROVIDER_ID))
        .is_some()
    {
        return Ok(text.to_owned());
    }
    Ok(format!(
        "{text}\n{BEGIN}\n\
         # Saved Edgee threads use normal OpenAI authentication outside Edgee.\n\
         [model_providers.{PROVIDER_ID}]\n\
         name = \"OpenAI\"\n\
         wire_api = \"responses\"\n\
         requires_openai_auth = true\n\
         {END}\n"
    ))
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
}
