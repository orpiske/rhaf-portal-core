use std::fs::{File, OpenOptions};
use std::io::Write;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use clap::Parser;
use praxis_core::config::Config;
use wanaku_feature_audit::persistence::FileAuditPersistence;
use wanaku_infra::credentials::CredentialBroker;
use wanaku_infra::persistence::FilePersistence;
use wanaku_server::app::{AppPersistence, AppResources};
use wanaku_types::audit::AuditPersistence;
use wanaku_types::credentials::{CredentialBinding, NormalizedOrigin, ResolverRegistry};
use wanaku_types::governance::{GovernanceConfig, GovernanceConfigError};
use wanaku_types::persistence::PersistenceBackend;
use wanaku_types::registry::ForwardEntry;

const DIAGNOSTIC_SECTIONS: [&str; 12] = [
    "forwards",
    "bindings",
    "llm_connections",
    "typesafe_system_one_connections",
    "opa_connections",
    "evaluators",
    "action_policy",
    "simulation",
    "governance",
    "audit",
    "plugins",
    "evaluator_startup_failure",
];

#[derive(Debug, Parser)]
#[command(version = crate::VERSION, about = "RHAF Portal Core embedded Wanaku runtime")]
pub struct Args {
    /// Internal MCP listener address.
    #[arg(long, default_value = "127.0.0.1:8081")]
    pub mcp_listen: SocketAddr,
    /// Internal management listener address.
    #[arg(long, default_value = "127.0.0.1:8080")]
    pub management_listen: SocketAddr,
    /// Directory for registry and audit state.
    #[arg(long, default_value = "data")]
    pub data_dir: PathBuf,
    /// Optional Wanaku bootstrap YAML file.
    #[arg(long, value_name = "PATH")]
    pub wanaku_config: Option<PathBuf>,
    /// Maximum drain time during orderly shutdown.
    #[arg(long, default_value = "10", value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub shutdown_timeout_secs: u64,
}

impl Args {
    pub fn pipeline(&self) -> Result<Config> {
        ensure!(self.mcp_listen.port() != 0, "MCP port must be nonzero");
        ensure!(
            self.management_listen.port() != 0,
            "management port must be nonzero"
        );
        ensure!(
            self.mcp_listen != self.management_listen,
            "listeners must use separate addresses"
        );
        let _mcp = TcpListener::bind(self.mcp_listen).context("MCP listener is unavailable")?;
        let _management = TcpListener::bind(self.management_listen)
            .context("management listener is unavailable")?;
        let mut pipeline = Config::from_yaml(include_str!("pipeline.yaml"))?;
        let listener = pipeline
            .listeners
            .first_mut()
            .context("MCP listener missing")?;
        listener.address = self.mcp_listen.to_string();
        pipeline.shutdown_timeout_secs = self.shutdown_timeout_secs;
        pipeline.validate()?;
        Ok(pipeline)
    }
}

pub fn bind_management(address: SocketAddr) -> Result<TcpListener> {
    TcpListener::bind(address).context("management listener is unavailable")
}

pub fn load_bootstrap(path: Option<&Path>) -> Result<Option<serde_yaml::Value>> {
    let Some(path) = path else { return Ok(None) };
    let content = std::fs::read_to_string(path).context("cannot read Wanaku configuration")?;
    let mut config: serde_yaml::Value =
        serde_yaml::from_str(&content).map_err(|error| match error.location() {
            Some(location) => anyhow::anyhow!(
                "invalid Wanaku configuration YAML at line {}, column {}",
                location.line(),
                location.column()
            ),
            None => anyhow::anyhow!("invalid Wanaku configuration YAML"),
        })?;
    ensure!(
        config.is_mapping(),
        "Wanaku configuration must be a YAML mapping"
    );
    validate_unknown_keys(&config)?;
    validate_bootstrap(&config)?;
    crate::feature_config::redact_malformed_sections(&mut config);
    Ok(Some(config))
}

fn validate_unknown_keys(config: &serde_yaml::Value) -> Result<()> {
    let entries = config
        .as_mapping()
        .context("Wanaku configuration must be a mapping")?;
    for (index, (key, value)) in entries.iter().enumerate() {
        if has_unknown_keys(key, value) {
            let location = unknown_key_location(key, value, index);
            anyhow::bail!("unknown Wanaku configuration keys in {location}");
        }
    }
    Ok(())
}

fn has_unknown_keys(key: &serde_yaml::Value, value: &serde_yaml::Value) -> bool {
    if key.as_str() == Some("evaluators")
        && let Some(mapping) = value.as_mapping()
    {
        if mapping.keys().any(|key| key.as_str() != Some("evaluators")) {
            return true;
        }
        return value
            .get("evaluators")
            .is_some_and(|entries| has_unknown_keys(key, entries));
    }
    let section = serde_yaml::Value::Mapping([(key.clone(), value.clone())].into_iter().collect());
    wanaku_server::config_keys::reject_unknown_keys(&section).is_err()
}

fn unknown_key_location(
    key: &serde_yaml::Value,
    value: &serde_yaml::Value,
    index: usize,
) -> String {
    let Some(section) = key
        .as_str()
        .filter(|section| DIAGNOSTIC_SECTIONS.contains(section))
    else {
        return format!("root entry[{index}]");
    };
    if let Some(entries) = value.as_sequence() {
        for (index, entry) in entries.iter().enumerate() {
            let entry = serde_yaml::Value::Sequence(vec![entry.clone()]);
            if has_unknown_keys(key, &entry) {
                return format!("{section}[{index}]");
            }
        }
    }
    section.to_owned()
}

fn validate_bootstrap(config: &serde_yaml::Value) -> Result<()> {
    if let Some(forwards) = config.get("forwards") {
        let forwards = forwards
            .as_sequence()
            .context("forwards must be a sequence")?;
        for (index, forward) in forwards.iter().enumerate() {
            let forward = serde_yaml::from_value::<ForwardEntry>(forward.clone())
                .map_err(|_| anyhow::anyhow!("invalid forwards[{index}] configuration"))?;
            validate_forward(&forward).with_context(|| format!("forwards[{index}]"))?;
        }
    }
    if let Some(audit) = config.get("audit") {
        serde_yaml::from_value::<wanaku_feature_audit::AuditConfig>(audit.clone())
            .map_err(|_| anyhow::anyhow!("invalid audit configuration"))?;
    }
    if let Some(bindings) = config.get("bindings") {
        let bindings = bindings
            .as_sequence()
            .context("bindings must be a sequence")?;
        for (index, binding) in bindings.iter().enumerate() {
            validate_binding(binding).with_context(|| format!("bindings[{index}]"))?;
        }
    }
    Ok(())
}

fn validate_binding(value: &serde_yaml::Value) -> Result<()> {
    let binding = serde_yaml::from_value::<CredentialBinding>(value.clone())
        .map_err(|_| anyhow::anyhow!("invalid credential binding configuration"))?;
    binding.validate().map_err(|_| {
        anyhow::anyhow!("invalid credential binding mechanism or secret reference count")
    })?;
    let origin = NormalizedOrigin::from_address(binding.origin.as_str())
        .map_err(|_| anyhow::anyhow!("invalid credential binding origin"))?;
    ensure!(
        origin.as_str().starts_with("https://"),
        "credential binding origin must use HTTPS"
    );
    Ok(())
}

fn validate_forward(forward: &ForwardEntry) -> Result<()> {
    NormalizedOrigin::from_address(&forward.address)
        .map_err(|_| anyhow::anyhow!("invalid forward address"))?;
    if let Some(namespace) = &forward.namespace {
        wanaku_types::registry::validate_namespace_name(namespace)
            .map_err(|_| anyhow::anyhow!("invalid forward namespace"))?;
    }
    Ok(())
}

pub fn governance(root: Option<&serde_yaml::Value>) -> Result<GovernanceConfig> {
    let governance = root
        .and_then(|value| value.get("governance"))
        .map(|value| serde_yaml::from_value::<GovernanceConfig>(value.clone()))
        .transpose()
        .map_err(|_| {
            anyhow::anyhow!(
                "invalid governance configuration; check default and namespaces posture field types"
            )
        })?
        .unwrap_or_default();
    governance.validate().map_err(|error| match error {
        GovernanceConfigError::DisabledReasonRequired { .. } => anyhow::anyhow!(
            "invalid governance configuration: disabled_reason must be nonempty for disabled scopes"
        ),
        GovernanceConfigError::EmptyNamespace => anyhow::anyhow!(
            "invalid governance.namespaces configuration: namespace must not be empty"
        ),
    })?;
    Ok(governance)
}

pub fn lock_data_directory(directory: &Path) -> Result<File> {
    std::fs::create_dir_all(directory).context("cannot create data directory")?;
    // Keep this inode in place. Removing it permits two processes to lock different files.
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(".lock"))
        .context("cannot open data directory lock")?;
    lock.try_lock()
        .context("data directory is already in use or cannot be locked")?;
    Ok(lock)
}

pub fn resources(
    directory: &Path,
) -> Result<(AppResources, Arc<crate::persistence::BootstrapPersistence>)> {
    validate_directory(directory)?;
    let registry = Arc::new(FilePersistence::new(directory.join("registry.json")));
    registry
        .load()
        .map_err(|_| anyhow::anyhow!("cannot load persisted registry"))?;
    let audit = Arc::new(FileAuditPersistence::new(
        directory.join("audit-events.json"),
    ));
    audit
        .load()
        .map_err(|_| anyhow::anyhow!("cannot load persisted audit events"))?;
    let persistence = Arc::new(crate::persistence::BootstrapPersistence::new(directory)?);
    let resolvers = ResolverRegistry::new()
        .with_resolver(Arc::new(crate::credentials::HttpsEnvResolver::new()));
    Ok((
        AppResources {
            persistence: AppPersistence {
                registry: Some(persistence.clone()),
                audit: Some(persistence.clone()),
                ..AppPersistence::default()
            },
            broker: Arc::new(CredentialBroker::new(resolvers)),
        },
        persistence,
    ))
}

fn validate_directory(directory: &Path) -> Result<()> {
    std::fs::create_dir_all(directory).context("cannot create data directory")?;
    for filename in ["registry.json", "audit-events.json"] {
        validate_snapshot_path(&directory.join(filename))
            .with_context(|| format!("invalid persistence snapshot path {filename}"))?;
    }
    let mut probe =
        tempfile::NamedTempFile::new_in(directory).context("data directory is not writable")?;
    probe
        .write_all(b"RHAF storage probe")
        .context("cannot write data directory")?;
    probe
        .as_file()
        .sync_all()
        .context("cannot sync data directory")?;
    probe.close().context("cannot remove storage probe")?;
    for filename in ["registry.json.tmp", "audit-events.json.tmp"] {
        validate_temporary_file(&directory.join(filename))
            .with_context(|| format!("cannot write persistence temporary file {filename}"))?;
    }
    Ok(())
}

fn validate_snapshot_path(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(metadata.is_file(), "snapshot path must be a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn validate_temporary_file(path: &Path) -> Result<()> {
    match File::create_new(path) {
        Ok(file) => {
            file.sync_all()?;
            drop(file);
            std::fs::remove_file(path)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            ensure!(
                std::fs::symlink_metadata(path)?.is_file(),
                "temporary path must be a regular file"
            );
            OpenOptions::new().write(true).open(path)?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{governance, load_bootstrap, resources};
    use anyhow::Result;

    #[test]
    fn rejects_corrupt_persisted_state() -> Result<()> {
        let directory = tempfile::tempdir()?;
        for filename in ["registry.json", "audit-events.json"] {
            let path = directory.path().join(filename);
            std::fs::write(&path, "invalid JSON")?;
            assert!(resources(directory.path()).is_err());
            std::fs::remove_file(path)?;
        }
        assert!(resources(directory.path()).is_ok());
        Ok(())
    }

    #[test]
    fn rejects_invalid_bootstrap_and_unknown_keys() -> Result<()> {
        let file = tempfile::NamedTempFile::new()?;
        for invalid in [
            "governance: [",
            "[governance]",
            "unknown_setting: true",
            "forwards: [{name: missing-address}]",
            "forwards: [{name: invalid-address, address: 'ftp://host/mcp'}]",
            "forwards: [{name: invalid-namespace, address: 'http://host/mcp', namespace: Bad_NS}]",
            "bindings: false",
            "audit: {max_records: invalid}",
        ] {
            std::fs::write(file.path(), invalid)?;
            assert!(load_bootstrap(Some(file.path())).is_err());
        }
        Ok(())
    }

    #[test]
    fn rejects_invalid_governance() -> Result<()> {
        let root = serde_yaml::from_str("governance: {default: {mode: disabled}}")?;
        assert!(governance(Some(&root)).is_err());
        Ok(())
    }
}

#[cfg(test)]
mod safety_tests {
    use super::{load_bootstrap, lock_data_directory};
    use anyhow::{Result, bail};

    #[test]
    fn directory_lock_prevents_overlapping_owners_and_releases_on_drop() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let first = lock_data_directory(directory.path())?;
        assert!(lock_data_directory(directory.path()).is_err());
        drop(first);
        let second = lock_data_directory(directory.path())?;
        assert!(directory.path().join(".lock").is_file());
        assert!(lock_data_directory(directory.path()).is_err());
        drop(second);
        assert!(lock_data_directory(directory.path()).is_ok());
        Ok(())
    }

    #[test]
    fn diagnostics_locate_bad_entries_without_disclosing_values_or_unknown_keys() -> Result<()> {
        let file = tempfile::NamedTempFile::new()?;
        for (input, expected) in [
            (
                "forwards: [{name: valid, address: 'http://host/mcp'}, {name: HIDDEN, address: 'ftp://HIDDEN'}]",
                "forwards[1]",
            ),
            ("bindings: [{id: HIDDEN}]", "bindings[0]"),
            (
                "forwards: [{name: HIDDEN, address: 'http://host/mcp', HIDDEN: HIDDEN}]",
                "forwards[0]",
            ),
            ("HIDDEN: HIDDEN", "root entry[0]"),
            ("governance: [HIDDEN", "line 2, column 1"),
        ] {
            std::fs::write(file.path(), input)?;
            let Err(error) = load_bootstrap(Some(file.path())) else {
                bail!("invalid input accepted")
            };
            let diagnostic = format!("{error:#}");
            assert!(diagnostic.contains(expected), "{diagnostic}");
            assert!(!diagnostic.contains("HIDDEN"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod resource_tests {
    use super::{Args, bind_management, resources, validate_temporary_file};
    use anyhow::{Result, bail};
    use std::net::TcpListener;

    #[test]
    fn actual_management_bind_rejects_occupation_after_preflight() -> Result<()> {
        for _ in 0..16 {
            let attempt = management_bind_after_preflight();
            match attempt {
                Ok(()) => return Ok(()),
                Err(error) if is_address_in_use(&error) => continue,
                Err(error) => return Err(error),
            }
        }
        bail!("could not reserve listener test ports after bounded retries")
    }

    fn is_address_in_use(error: &anyhow::Error) -> bool {
        error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::AddrInUse)
    }

    fn management_bind_after_preflight() -> Result<()> {
        let mcp = TcpListener::bind("127.0.0.1:0")?;
        let management = TcpListener::bind("127.0.0.1:0")?;
        let args = Args {
            mcp_listen: mcp.local_addr()?,
            management_listen: management.local_addr()?,
            data_dir: "unused".into(),
            wanaku_config: None,
            shutdown_timeout_secs: 1,
        };
        drop((mcp, management));
        args.pipeline()?;
        let competing = TcpListener::bind(args.management_listen)?;
        assert!(bind_management(args.management_listen).is_err());
        drop(competing);
        let owned = bind_management(args.management_listen)?;
        assert!(TcpListener::bind(args.management_listen).is_err());
        drop(owned);
        Ok(())
    }

    #[test]
    fn rejects_directories_at_native_temporary_paths() -> Result<()> {
        for filename in ["registry.json.tmp", "audit-events.json.tmp"] {
            let directory = tempfile::tempdir()?;
            std::fs::create_dir_all(directory.path().join(filename))?;
            let Err(error) = resources(directory.path()) else {
                bail!("directory accepted as persistence temporary file")
            };
            assert!(format!("{error:#}").contains(filename));
        }
        Ok(())
    }

    #[test]
    fn temporary_path_validation_preserves_existing_contents() -> Result<()> {
        let temporary = tempfile::NamedTempFile::new()?;
        std::fs::write(temporary.path(), "existing temporary contents")?;
        validate_temporary_file(temporary.path())?;
        assert_eq!(
            std::fs::read_to_string(temporary.path())?,
            "existing temporary contents"
        );
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod snapshot_tests {
    use anyhow::Result;

    #[test]
    fn rejects_snapshot_fifos_before_loading() -> Result<()> {
        let directory = tempfile::tempdir()?;
        for filename in ["registry.json", "audit-events.json"] {
            let fifo = directory.path().join(filename);
            nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRUSR)?;
            assert!(super::validate_snapshot_path(&fifo).is_err());
        }
        Ok(())
    }
}

#[cfg(test)]
mod configuration_tests {
    use super::load_bootstrap;
    use anyhow::{Result, bail};

    #[test]
    fn evaluator_mapping_rejects_unknown_keys_and_preserves_supported_form() -> Result<()> {
        let file = tempfile::NamedTempFile::new()?;
        std::fs::write(
            file.path(),
            "evaluators: {evalutors: []}\nevaluator_startup_failure: deny",
        )?;
        assert!(load_bootstrap(Some(file.path())).is_err());
        std::fs::write(file.path(), "evaluators: {evaluators: []}")?;
        assert!(load_bootstrap(Some(file.path())).is_ok());
        Ok(())
    }

    #[test]
    fn plaintext_binding_origin_has_safe_indexed_diagnostic() -> Result<()> {
        let file = tempfile::NamedTempFile::new()?;
        let config = "bindings:\n- id: HIDDEN\n  forwardId: HIDDEN\n  origin: http://HIDDEN\n  mechanism: {type: bearer}\n  secretRefs: [env:HIDDEN]\n  allowedPurposes: [discovery]\n";
        std::fs::write(file.path(), config)?;
        let Err(error) = load_bootstrap(Some(file.path())) else {
            bail!("plaintext binding accepted")
        };
        let diagnostic = format!("{error:#}");
        assert!(diagnostic.contains("bindings[0]"));
        assert!(diagnostic.contains("HTTPS"));
        assert!(!diagnostic.contains("HIDDEN"));
        Ok(())
    }
}
