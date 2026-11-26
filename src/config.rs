use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RhafConfig {
    pub search: SearchConfig,
    pub catalog: CatalogConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CatalogConfig {
    pub path: Option<PathBuf>,
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
        let mut config: Self = serde_yaml::from_str(&content).map_err(|error| match error.location() {
            Some(location) => anyhow::anyhow!(
                "invalid RHAF configuration at line {}, column {}; check search.max_results is a positive integer and keys are supported",
                location.line(),
                location.column()
            ),
            None => anyhow::anyhow!("invalid RHAF configuration"),
        })?;
        if let (Some(catalog), Some(parent)) = (&mut config.catalog.path, path.parent()) {
            *catalog = parent.join(&*catalog);
        }
        Ok(config)
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
    fn resolves_catalog_path_from_the_rhaf_settings_directory() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let settings = directory.path().join("rhaf.yaml");
        std::fs::write(&settings, "catalog: {path: products.yaml}")?;
        assert_eq!(
            RhafConfig::load(Some(&settings))?.catalog.path,
            Some(directory.path().join("products.yaml"))
        );
        assert!(RhafConfig::load(None)?.catalog.path.is_none());
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
