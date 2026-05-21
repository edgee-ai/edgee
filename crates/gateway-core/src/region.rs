//! Data residency region selection for the Edgee hosted gateway.
//!
//! Enterprises can route prompts through a specific geography to satisfy
//! GDPR, FedRAMP-adjacent, and other data sovereignty requirements.
//!
//! # Region resolution order
//!
//! 1. `EDGEE_REGION` environment variable
//! 2. `region` field in the active profile (`credentials.toml`)
//! 3. Hardcoded default: [`Region::Us`]
//!
//! # Fallback behaviour
//!
//! If the requested region is unavailable (e.g. Fastly POP down), the
//! gateway falls back to the default region and logs a warning. The
//! fallback is visible in the `request.region` audit event.

use serde::{Deserialize, Serialize};

/// Supported data residency regions.
///
/// Each variant corresponds to a Fastly POP geography. The `Display`
/// implementation produces the subdomain prefix used in gateway URLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Region {
    /// United States — Fastly North American POPs.
    /// Default region. Suitable for FedRAMP-adjacent workloads.
    #[default]
    Us,
    /// European Union — Fastly European POPs (Amsterdam, Frankfurt, London, Paris).
    /// Required for GDPR data residency.
    Eu,
    /// Asia-Pacific — Fastly APAC POPs (Tokyo, Singapore, Sydney, etc.).
    Apac,
}

impl Region {
    /// All supported regions, in declaration order.
    pub const ALL: &[Region] = &[Region::Us, Region::Eu, Region::Apac];

    /// Parse a region from a string (case-insensitive).
    ///
    /// Returns `None` for unrecognised values.
    /// Prefer `<Region as std::str::FromStr>::from_str` for `Result`-based parsing.
    ///
    /// # Examples
    ///
    /// ```
    /// use edgee_gateway_core::Region;
    /// assert_eq!(Region::parse("eu"), Some(Region::Eu));
    /// assert_eq!(Region::parse("APAC"), Some(Region::Apac));
    /// assert_eq!(Region::parse("us-east-1"), None);
    /// ```
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "us" | "usa" | "united states" => Some(Region::Us),
            "eu" | "europe" | "european union" => Some(Region::Eu),
            "apac" | "asia-pacific" | "asia" => Some(Region::Apac),
            _ => None,
        }
    }

    /// Subdomain prefix for the gateway URL, including trailing dot.
    ///
    /// Returns an empty string for the default (US) region so
    /// `api.edgee.ai` continues to work for the common case.
    ///
    /// # Examples
    ///
    /// ```
    /// use edgee_gateway_core::Region;
    /// assert_eq!(Region::Us.gateway_subdomain(), "");
    /// assert_eq!(Region::Eu.gateway_subdomain(), "eu.");
    /// assert_eq!(Region::Apac.gateway_subdomain(), "apac.");
    /// ```
    pub fn gateway_subdomain(&self) -> &'static str {
        match self {
            Region::Us => "",
            Region::Eu => "eu.",
            Region::Apac => "apac.",
        }
    }

    /// Human-readable region name for audit events and logs.
    pub fn display_name(&self) -> &'static str {
        match self {
            Region::Us => "us",
            Region::Eu => "eu",
            Region::Apac => "apac",
        }
    }

    /// Data residency guarantees for this region.
    pub fn guarantees(&self) -> &'static str {
        match self {
            Region::Us => {
                "Data processed exclusively in US-based Fastly POPs. Suitable for FedRAMP-adjacent workloads."
            }
            Region::Eu => {
                "Data processed exclusively in EU-based Fastly POPs (Amsterdam, Frankfurt, London, Paris). Satisfies GDPR data residency requirements."
            }
            Region::Apac => {
                "Data processed exclusively in APAC-based Fastly POPs (Tokyo, Singapore, Sydney). Satisfies data sovereignty requirements for APAC jurisdictions."
            }
        }
    }

    /// Return the canonical short code for this region (e.g. "us", "eu", "apac").
    pub fn short_code(&self) -> &'static str {
        self.display_name()
    }
}

impl std::fmt::Display for Region {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

impl std::str::FromStr for Region {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Region::parse(s).ok_or_else(|| {
            format!(
                "invalid region '{}'. Supported: {}",
                s,
                Region::ALL
                    .iter()
                    .map(|r| r.short_code())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lowercase() {
        assert_eq!(Region::parse("us"), Some(Region::Us));
        assert_eq!(Region::parse("eu"), Some(Region::Eu));
        assert_eq!(Region::parse("apac"), Some(Region::Apac));
    }

    #[test]
    fn parse_mixed_case() {
        assert_eq!(Region::parse("Us"), Some(Region::Us));
        assert_eq!(Region::parse("EU"), Some(Region::Eu));
        assert_eq!(Region::parse("APAC"), Some(Region::Apac));
    }

    #[test]
    fn parse_aliases() {
        assert_eq!(Region::parse("europe"), Some(Region::Eu));
        assert_eq!(Region::parse("asia-pacific"), Some(Region::Apac));
        assert_eq!(Region::parse("united states"), Some(Region::Us));
    }

    #[test]
    fn parse_invalid() {
        assert_eq!(Region::parse("mars"), None);
        assert_eq!(Region::parse("us-east-1"), None);
        assert_eq!(Region::parse(""), None);
    }

    #[test]
    fn from_str_trait_valid() {
        assert_eq!("us".parse::<Region>().unwrap(), Region::Us);
        assert_eq!("EU".parse::<Region>().unwrap(), Region::Eu);
        assert_eq!("apac".parse::<Region>().unwrap(), Region::Apac);
        assert_eq!("europe".parse::<Region>().unwrap(), Region::Eu);
    }

    #[test]
    fn from_str_trait_invalid() {
        assert!("mars".parse::<Region>().is_err());
        assert!("".parse::<Region>().is_err());
    }

    #[test]
    fn subdomain_for_us_is_empty() {
        assert_eq!(Region::Us.gateway_subdomain(), "");
    }

    #[test]
    fn subdomain_for_eu_is_eu() {
        assert_eq!(Region::Eu.gateway_subdomain(), "eu.");
    }

    #[test]
    fn subdomain_for_apac_is_apac() {
        assert_eq!(Region::Apac.gateway_subdomain(), "apac.");
    }

    #[test]
    fn default_is_us() {
        assert_eq!(Region::default(), Region::Us);
    }

    #[test]
    fn display_produces_short_code() {
        assert_eq!(Region::Us.to_string(), "us");
        assert_eq!(Region::Eu.to_string(), "eu");
        assert_eq!(Region::Apac.to_string(), "apac");
    }

    #[test]
    fn all_contains_three_regions() {
        assert_eq!(Region::ALL.len(), 3);
        assert!(Region::ALL.contains(&Region::Us));
        assert!(Region::ALL.contains(&Region::Eu));
        assert!(Region::ALL.contains(&Region::Apac));
    }

    #[test]
    fn guarantees_are_non_empty() {
        for region in Region::ALL {
            assert!(!region.guarantees().is_empty());
        }
    }

    #[test]
    fn serde_roundtrip() {
        for region in Region::ALL {
            let json = serde_json::to_string(&region).unwrap();
            let parsed: Region = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, *region);
        }
    }

    #[test]
    fn serde_lowercase() {
        assert_eq!(serde_json::to_string(&Region::Us).unwrap(), "\"us\"");
        assert_eq!(serde_json::to_string(&Region::Eu).unwrap(), "\"eu\"");
        assert_eq!(serde_json::to_string(&Region::Apac).unwrap(), "\"apac\"");
    }
}
