use std::num::NonZeroUsize;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RhafConfig {
    pub search: SearchConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchConfig {
    pub max_results: NonZeroUsize,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            max_results: NonZeroUsize::new(20).unwrap_or(NonZeroUsize::MIN),
        }
    }
}

impl RhafConfig {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let content = std::fs::read_to_string(path).context("cannot read RHAF configuration")?;
        serde_yaml::from_str(&content).map_err(|error| match error.location() {
            Some(location) => anyhow::anyhow!(
                "invalid RHAF configuration at line {}, column {}; check search.max_results is a positive integer and keys are supported",
                location.line(),
                location.column()
            ),
            None => anyhow::anyhow!("invalid RHAF configuration"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::RhafConfig;
    use anyhow::{Result, bail};

    #[test]
    fn loads_default_and_positive_search_limit() -> Result<()> {
        assert_eq!(RhafConfig::load(None)?.search.max_results.get(), 20);
        let file = tempfile::NamedTempFile::new()?;
        std::fs::write(file.path(), "search: {max_results: 7}")?;
        assert_eq!(
            RhafConfig::load(Some(file.path()))?
                .search
                .max_results
                .get(),
            7
        );
        Ok(())
    }

    #[test]
    fn rejects_invalid_settings_without_disclosing_values() -> Result<()> {
        let file = tempfile::NamedTempFile::new()?;
        for input in [
            "search: {max_results: 0}",
            "search: {max_results: -1}",
            "search: {max_results: HIDDEN}",
            "search: {HIDDEN: HIDDEN}",
            "bindings: [{secretRefs: [env:HIDDEN]}]",
            "search: [HIDDEN",
            "[HIDDEN]",
        ] {
            std::fs::write(file.path(), input)?;
            let Err(error) = RhafConfig::load(Some(file.path())) else {
                bail!("invalid RHAF configuration accepted");
            };
            assert!(!format!("{error:#}").contains("HIDDEN"));
        }
        Ok(())
    }
}
