use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use std::os::unix::process::ExitStatusExt;
use tokio::time::{Instant, sleep};

use super::http::Http;

#[derive(Clone, Copy)]
pub enum ExitPolicy {
    Core,
    Fixture,
}

pub struct Running {
    child: Child,
    log: PathBuf,
    policy: ExitPolicy,
    stopped: bool,
}

impl Running {
    pub fn spawn(mut command: Command, log: &Path, policy: ExitPolicy) -> Result<Self> {
        let output = std::fs::File::create(log)?;
        for (name, _) in std::env::vars_os() {
            if ["WANAKU_", "PRAXIS_", "QUARKUS_", "FIXTURE_"]
                .iter()
                .any(|prefix| name.to_string_lossy().starts_with(prefix))
            {
                command.env_remove(name);
            }
        }
        let child = command
            .stdout(Stdio::from(output.try_clone()?))
            .stderr(Stdio::from(output))
            .spawn()
            .with_context(|| format!("cannot start process; inspect {}", log.display()))?;
        Ok(Self {
            child,
            log: log.to_owned(),
            policy,
            stopped: false,
        })
    }

    pub async fn wait_ready(&mut self, http: &Http, url: &str) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            ensure!(
                self.child.try_wait()?.is_none(),
                "process exited; inspect {}",
                self.log.display()
            );
            if http.ready(url).await {
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "readiness timed out; inspect {}",
                self.log.display()
            );
            sleep(Duration::from_millis(100)).await;
        }
    }

    async fn wait_exit(&mut self, timeout: Duration) -> Result<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            ensure!(
                Instant::now() < deadline,
                "process timeout; inspect {}",
                self.log.display()
            );
            sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn finish(mut self, timeout: Duration) -> Result<ExitStatus> {
        let status = self.wait_exit(timeout).await?;
        self.stopped = true;
        Ok(status)
    }

    pub async fn stop(mut self) -> Result<()> {
        ensure!(
            self.child.try_wait()?.is_none(),
            "process exited before SIGTERM; inspect {}",
            self.log.display()
        );
        kill(
            Pid::from_raw(i32::try_from(self.child.id())?),
            Signal::SIGTERM,
        )
        .context("SIGTERM failed")?;
        let status = self.wait_exit(Duration::from_secs(25)).await?;
        self.stopped = true;
        let accepted = status.success()
            || matches!(self.policy, ExitPolicy::Fixture)
                && (status.code() == Some(143) || status.signal() == Some(Signal::SIGTERM as i32));
        ensure!(
            accepted,
            "unclean exit {status}; inspect {}",
            self.log.display()
        );
        Ok(())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if !self.stopped {
            // Cleanup must reap failed test children even when normal shutdown checks fail.
            if matches!(self.child.try_wait(), Ok(None)) {
                let _ = self.child.kill();
            }
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ExitPolicy, Running};
    use anyhow::{Result, bail, ensure};
    use std::process::Command;

    #[tokio::test]
    async fn clean_early_exit_does_not_prove_sigterm_shutdown() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut command = Command::new("sh");
        command.args(["-c", "exit 0"]);
        let mut process = Running::spawn(
            command,
            &directory.path().join("early-exit.log"),
            ExitPolicy::Core,
        )?;
        ensure!(
            process.child.wait()?.success(),
            "fixture did not exit cleanly"
        );
        let Err(error) = process.stop().await else {
            bail!("early exit accepted as SIGTERM shutdown")
        };
        ensure!(
            error.to_string().contains("exited before SIGTERM"),
            "wrong shutdown failure: {error}"
        );
        Ok(())
    }
}
