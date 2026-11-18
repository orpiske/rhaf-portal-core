use std::net::TcpListener;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use reqwest::Method;

use super::{
    Harness,
    process::{ExitPolicy, Running},
    reserve_addresses,
};

const ORIGINAL_SNAPSHOTS: [(&str, &[u8]); 2] =
    [("registry.json", b"{}\n"), ("audit-events.json", b"[]\n")];

impl Harness {
    pub async fn verify_audit_shutdown_failure(&self) -> Result<()> {
        let directory = self.evidence.join("audit-failure-data");
        let process = self
            .start(
                self.core_command(&self.config(), &directory),
                "audit-failure.log",
                &self.management_url("/api/v1/tools"),
                ExitPolicy::Core,
            )
            .await?;
        std::fs::create_dir(directory.join("audit-events.json.tmp"))?;
        self.record_failed_tool_attempt().await?;
        let Err(error) = process.stop().await else {
            anyhow::bail!("audit persistence failure exited successfully")
        };
        ensure!(
            error.to_string().contains("unclean exit"),
            "unexpected shutdown failure: {error}"
        );
        let diagnostic = std::fs::read_to_string(self.evidence.join("audit-failure.log"))?;
        ensure!(
            diagnostic.contains("audit shutdown flush failed"),
            "missing audit flush error"
        );
        Ok(())
    }

    async fn record_failed_tool_attempt(&self) -> Result<()> {
        let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "unavailable-audit-probe", "arguments": {}}});
        let reply = self
            .http
            .request(Method::POST, &self.mcp_url(), Some(&body))
            .await?;
        ensure!(reply.status == 200, "audit probe request failed");
        let audit = self
            .http
            .request(
                Method::GET,
                &self.management_url("/api/v1/audit/events"),
                None,
            )
            .await?;
        let events = audit.body["data"]["events"]
            .as_array()
            .context("audit probe events missing")?;
        ensure!(
            events
                .iter()
                .any(|event| event["operation"] == "tools/call"),
            "audit probe did not record an event"
        );
        Ok(())
    }

    async fn expect_failure(&self, command: Command, log: &str, timeout: u64) -> Result<String> {
        let path = self.evidence.join(log);
        let process = Running::spawn(command, &path, ExitPolicy::Core)?;
        let status = process.finish(Duration::from_secs(timeout)).await?;
        ensure!(
            !status.success(),
            "invalid startup accepted; inspect {}",
            path.display()
        );
        Ok(std::fs::read_to_string(path)?)
    }

    pub async fn verify_startup(&self) -> Result<()> {
        let invalid = self.evidence.join("invalid.yaml");
        let marker = "synthetic-value-must-not-appear-in-diagnostics";
        std::fs::write(&invalid, format!("governance: [{marker}\n"))?;
        let diagnostic = self
            .expect_failure(
                self.core_command(&invalid, &self.data_dir()),
                "invalid-startup.log",
                15,
            )
            .await?;
        ensure!(
            diagnostic.contains("invalid Wanaku configuration YAML at line ")
                && diagnostic.contains(", column "),
            "missing redacted YAML parse location"
        );
        ensure!(!diagnostic.contains(marker), "configuration value leaked");
        let occupied = TcpListener::bind(self.management)?;
        self.expect_failure(
            self.core_command(&self.config(), &self.data_dir()),
            "occupied-listener.log",
            15,
        )
        .await?;
        drop(occupied);
        Ok(())
    }

    pub async fn verify_exclusive_storage(&self) -> Result<()> {
        let [management, mcp, _] = reserve_addresses()?;
        let mut command = Command::new(super::CORE);
        command
            .args([
                "--mcp-listen",
                &mcp.to_string(),
                "--management-listen",
                &management.to_string(),
            ])
            .arg("--data-dir")
            .arg(self.data_dir())
            .arg("--wanaku-config")
            .arg(self.config())
            .args(["--shutdown-timeout-secs", "1"]);
        let diagnostic = self
            .expect_failure(command, "competing-instance.log", 15)
            .await?;
        ensure!(
            diagnostic.to_lowercase().contains("lock"),
            "competing process failed without a lock error"
        );
        Ok(())
    }

    pub async fn verify_opa_redaction(&self) -> Result<()> {
        let marker = "synthetic-opa-value-must-not-appear";
        let config = self.evidence.join("invalid-opa.yaml");
        std::fs::write(
            &config,
            format!("opa_connections: [{marker}]\nevaluator_startup_failure: deny\n"),
        )?;
        let command = self.core_command(&config, &self.evidence.join("opa-data"));
        let process = self
            .start(
                command,
                "opa-startup.log",
                &self.management_url("/api/v1/tools"),
                ExitPolicy::Core,
            )
            .await?;
        self.verify_opa_status().await?;
        process.stop().await?;
        let diagnostic = std::fs::read_to_string(self.evidence.join("opa-startup.log"))?;
        ensure!(
            !diagnostic.contains(marker),
            "malformed OPA value leaked in startup logs"
        );
        ensure!(
            diagnostic.contains("opa_connections"),
            "missing safe OPA startup diagnostic"
        );
        Ok(())
    }

    async fn verify_opa_status(&self) -> Result<()> {
        let evaluator = self
            .http
            .request(
                Method::GET,
                &self.management_url("/api/v1/evaluators/status"),
                None,
            )
            .await?;
        ensure!(
            evaluator.status == 200,
            "cannot query malformed OPA startup status"
        );
        ensure!(
            evaluator.body["data"]["reason_code"] == "connection_configuration_invalid",
            "malformed OPA configuration lost its deny status"
        );
        Ok(())
    }

    pub async fn verify_rejected_bootstrap(&self) -> Result<()> {
        let directory = self.rejected_bootstrap_files()?;
        let config = self.evidence.join("rejected-bootstrap.yaml");
        let diagnostic = self
            .expect_failure(
                self.core_command(&config, &directory),
                "rejected-bootstrap.log",
                45,
            )
            .await?;
        ensure!(
            diagnostic.contains("Wanaku application assembly failed"),
            "rejected bootstrap did not reach application validation"
        );
        for (name, bytes) in ORIGINAL_SNAPSHOTS {
            ensure!(
                std::fs::read(directory.join(name))? == bytes,
                "rejected bootstrap changed persisted {name}"
            );
        }
        Ok(())
    }

    fn rejected_bootstrap_files(&self) -> Result<std::path::PathBuf> {
        let directory = self.evidence.join("rejected-data");
        std::fs::create_dir(&directory)?;
        for (name, bytes) in ORIGINAL_SNAPSHOTS {
            std::fs::write(directory.join(name), bytes)?;
        }
        let config = self.evidence.join("rejected-bootstrap.yaml");
        std::fs::write(
            &config,
            format!(
                "forwards:\n  - name: rejected-forward\n    address: {}\nsimulation:\n  max_actions: 0\n",
                self.fixture_url("/mcp")
            ),
        )?;
        Ok(directory)
    }
}
