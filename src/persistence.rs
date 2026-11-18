use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use wanaku_feature_audit::persistence::FileAuditPersistence;
use wanaku_infra::persistence::FilePersistence;
use wanaku_types::audit::{AuditEvent, AuditPersistence};
use wanaku_types::persistence::{PersistenceBackend, PersistenceError, RegistrySnapshot};

pub struct BootstrapPersistence {
    directory: PathBuf,
    staging: tempfile::TempDir,
    active: Mutex<bool>,
    failed_audit: Mutex<Option<Vec<AuditEvent>>>,
}

impl BootstrapPersistence {
    pub fn new(directory: &Path) -> Result<Self> {
        Ok(Self {
            directory: directory.to_owned(),
            staging: tempfile::tempdir_in(directory)
                .context("cannot stage bootstrap persistence")?,
            active: Mutex::new(false),
            failed_audit: Mutex::new(None),
        })
    }

    pub fn activate(&self) -> Result<()> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| anyhow::anyhow!("bootstrap persistence lock failed"))?;
        for filename in ["registry.json", "audit-events.json"] {
            let staged = self.staging.path().join(filename);
            if staged.is_file() {
                std::fs::rename(staged, self.directory.join(filename))
                    .context("cannot publish bootstrap persistence")?;
            }
        }
        *active = true;
        Ok(())
    }

    fn destination(&self, active: bool) -> &Path {
        if active {
            &self.directory
        } else {
            self.staging.path()
        }
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "bounded synchronous shutdown backoff after request runtimes stop"
    )]
    pub fn retry_failed_audit(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .context("invalid audit shutdown flush timeout")?;
        let mut pending = self
            .failed_audit
            .lock()
            .map_err(|_| anyhow::anyhow!("audit shutdown flush lock failed"))?;
        while let Some(events) = pending.as_ref() {
            match FileAuditPersistence::new(self.directory.join("audit-events.json")).save(events) {
                Ok(()) => *pending = None,
                Err(_) if Instant::now() >= deadline => {
                    anyhow::bail!("audit shutdown flush failed")
                }
                Err(_) => std::thread::sleep(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(25)),
                ),
            }
        }
        Ok(())
    }
}

impl PersistenceBackend for BootstrapPersistence {
    fn load(&self) -> Result<RegistrySnapshot, PersistenceError> {
        FilePersistence::new(self.directory.join("registry.json")).load()
    }

    fn save(&self, snapshot: &RegistrySnapshot) -> Result<(), PersistenceError> {
        let active = self.active.lock().map_err(|_| {
            PersistenceError::Coordination("bootstrap persistence lock failed".to_owned())
        })?;
        FilePersistence::new(self.destination(*active).join("registry.json")).save(snapshot)
    }
}

impl AuditPersistence for BootstrapPersistence {
    fn load(&self) -> Result<Vec<AuditEvent>, String> {
        FileAuditPersistence::new(self.directory.join("audit-events.json")).load()
    }

    fn save(&self, events: &[AuditEvent]) -> Result<(), String> {
        let active = self
            .active
            .lock()
            .map_err(|_| "bootstrap persistence lock failed".to_owned())?;
        let mut pending = self
            .failed_audit
            .lock()
            .map_err(|_| "audit persistence retry lock failed".to_owned())?;
        let result = FileAuditPersistence::new(self.destination(*active).join("audit-events.json"))
            .save(events);
        *pending = match &result {
            Ok(()) => None,
            Err(_) => Some(events.to_vec()),
        };
        result
    }
}

#[cfg(test)]
mod tests {
    use super::BootstrapPersistence;
    use anyhow::Result;
    use wanaku_types::persistence::{PersistenceBackend, RegistrySnapshot};

    #[test]
    fn rejected_bootstrap_does_not_change_persisted_snapshot() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("registry.json");
        let audit = directory.path().join("audit-events.json");
        std::fs::write(&path, "{ }\n")?;
        std::fs::write(&audit, "[ ]\n")?;
        {
            let persistence = BootstrapPersistence::new(directory.path())?;
            PersistenceBackend::save(&persistence, &RegistrySnapshot::default())?;
            wanaku_types::audit::AuditPersistence::save(&persistence, &[])
                .map_err(anyhow::Error::msg)?;
            assert_eq!(std::fs::read_to_string(&path)?, "{ }\n");
            assert_eq!(std::fs::read_to_string(&audit)?, "[ ]\n");
        }
        assert_eq!(std::fs::read_to_string(&path)?, "{ }\n");
        assert_eq!(std::fs::read_to_string(&audit)?, "[ ]\n");
        Ok(())
    }

    #[test]
    fn successful_bootstrap_publishes_and_then_saves_to_data_directory() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let persistence = BootstrapPersistence::new(directory.path())?;
        PersistenceBackend::save(&persistence, &RegistrySnapshot::default())?;
        assert!(!directory.path().join("registry.json").exists());
        persistence.activate()?;
        assert!(directory.path().join("registry.json").is_file());
        std::fs::remove_file(directory.path().join("registry.json"))?;
        PersistenceBackend::save(&persistence, &RegistrySnapshot::default())?;
        assert!(directory.path().join("registry.json").is_file());
        Ok(())
    }
}

#[cfg(test)]
mod audit_flush_tests {
    use std::time::Duration;

    use anyhow::Result;
    use wanaku_types::audit::{AuditCategory, AuditDecision, AuditEvent, AuditPersistence};

    use super::BootstrapPersistence;

    fn failed_snapshot() -> Result<(tempfile::TempDir, BootstrapPersistence, AuditEvent)> {
        let directory = tempfile::tempdir()?;
        let persistence = BootstrapPersistence::new(directory.path())?;
        persistence.activate()?;
        AuditPersistence::save(&persistence, &[]).map_err(anyhow::Error::msg)?;
        std::fs::create_dir(directory.path().join("audit-events.json.tmp"))?;
        let event = AuditEvent::new(
            AuditCategory::Decision,
            AuditDecision::Allow,
            "tools/call",
            "allowed",
            "Allowed",
        );
        assert!(AuditPersistence::save(&persistence, std::slice::from_ref(&event)).is_err());
        Ok((directory, persistence, event))
    }

    #[test]
    fn retries_failed_final_snapshot_after_storage_recovers() -> Result<()> {
        let (directory, persistence, event) = failed_snapshot()?;
        std::fs::remove_dir(directory.path().join("audit-events.json.tmp"))?;
        persistence.retry_failed_audit(Duration::from_millis(50))?;
        assert_eq!(
            AuditPersistence::load(&persistence).map_err(anyhow::Error::msg)?,
            vec![event]
        );
        Ok(())
    }

    #[test]
    fn unrecoverable_final_write_returns_error_and_keeps_pending_snapshot() -> Result<()> {
        let (_directory, persistence, _) = failed_snapshot()?;
        assert!(persistence.retry_failed_audit(Duration::ZERO).is_err());
        assert!(persistence.retry_failed_audit(Duration::ZERO).is_err());
        assert!(
            AuditPersistence::load(&persistence)
                .map_err(anyhow::Error::msg)?
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn newer_successful_snapshot_clears_failed_snapshot() -> Result<()> {
        let (directory, persistence, _) = failed_snapshot()?;
        let temporary = directory.path().join("audit-events.json.tmp");
        std::fs::remove_dir(&temporary)?;
        AuditPersistence::save(&persistence, &[]).map_err(anyhow::Error::msg)?;
        std::fs::create_dir(temporary)?;
        persistence.retry_failed_audit(Duration::ZERO)?;
        assert!(
            AuditPersistence::load(&persistence)
                .map_err(anyhow::Error::msg)?
                .is_empty()
        );
        Ok(())
    }
}
