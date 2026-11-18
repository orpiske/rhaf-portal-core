//! Require every workspace package to inherit the shared lint settings.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, ensure};
use serde_json::Value;

pub fn check() -> Result<()> {
    let metadata = workspace_metadata()?;
    let members = metadata["workspace_members"]
        .as_array()
        .context("workspace members missing")?;
    let packages = metadata["packages"]
        .as_array()
        .context("workspace packages missing")?;
    let mut failures = Vec::new();
    for package in packages
        .iter()
        .filter(|package| members.contains(&package["id"]))
    {
        if let Some(failure) = check_package(package)? {
            failures.push(failure);
        }
    }
    ensure!(failures.is_empty(), "{}", failures.join("\n"));
    println!(
        "All {} workspace packages inherit workspace lints.",
        members.len()
    );
    Ok(())
}

fn workspace_metadata() -> Result<Value> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("workspace root missing")?;
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["metadata", "--locked", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .output()
        .context("cannot read Cargo workspace metadata")?;
    ensure!(
        output.status.success(),
        "Cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn check_package(package: &Value) -> Result<Option<String>> {
    let path = package["manifest_path"]
        .as_str()
        .context("manifest path missing")?;
    let source = std::fs::read_to_string(path).with_context(|| format!("cannot read {path}"))?;
    Ok((!inherits_workspace_lints(&source)?).then(|| {
        format!(
            "{path}: package {} must set [lints] workspace = true",
            package["name"]
        )
    }))
}

fn inherits_workspace_lints(source: &str) -> Result<bool> {
    let manifest: toml::Value = toml::from_str(source)?;
    Ok(manifest
        .get("lints")
        .and_then(|lints| lints.get("workspace"))
        .and_then(toml::Value::as_bool)
        == Some(true))
}

#[cfg(test)]
mod tests {
    use super::inherits_workspace_lints;
    use anyhow::Result;

    #[test]
    fn requires_explicit_boolean_inheritance() -> Result<()> {
        for manifest in [
            "",
            "[lints]\nworkspace = false",
            "[lints]\nworkspace = 'true'",
            "[workspace.lints]\nworkspace = true",
        ] {
            assert!(!inherits_workspace_lints(manifest)?);
        }
        assert!(inherits_workspace_lints("[lints]\nworkspace = true")?);
        Ok(())
    }
}
