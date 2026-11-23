use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Result, ensure};
use reqwest::Method;
use serde_json::json;

use super::{Expected, Harness, process::ExitPolicy};

impl Harness {
    pub async fn verify_observational_deny_and_storage_failure(
        &self,
        expected: &Expected,
    ) -> Result<()> {
        let directory = self.evidence.join("governance-data");
        let process = self
            .start(
                self.core_command(&self.config(), &directory),
                "governance.log",
                &self.management_url("/api/v1/tools"),
                ExitPolicy::Core,
            )
            .await?;
        self.register_forward().await?;
        self.http
            .initialize(&self.mcp_url(), "audit-client")
            .await?;
        self.configure_observational_deny().await?;
        self.verify_admitted_read(expected).await?;
        self.verify_deny_observation().await?;
        let blocked_path = directory.join("audit-events.json.tmp");
        block_audit_writes(&blocked_path).await?;
        self.verify_admitted_read(expected).await?;
        self.verify_degraded_audit_health().await?;
        self.verify_admitted_read(expected).await?;
        std::fs::remove_dir(blocked_path)?;
        process.stop().await
    }

    async fn configure_observational_deny(&self) -> Result<()> {
        let policy = json!({"policy": {"rules": [{
            "id": "observational-deny", "effect": "deny",
            "selectors": {"operation": "tools/call"}
        }]}});
        let reply = self
            .http
            .request(
                Method::PUT,
                &self.management_url("/api/v1/action-policies"),
                Some(&policy),
            )
            .await?;
        ensure!(reply.status == 200, "cannot configure deny policy");
        Ok(())
    }

    async fn verify_deny_observation(&self) -> Result<()> {
        ensure!(
            self.audit_events().await?.iter().any(|event| {
                event["filter"] == "wanaku_action_policy"
                    && event["decision"] == "block"
                    && event["attributes"]["enforcement_action"] == "continue"
                    && event["attributes"]["deny_rule_ids"] == json!(["observational-deny"])
            }),
            "configured deny was not observed in audit mode"
        );
        Ok(())
    }

    async fn verify_admitted_read(&self, expected: &Expected) -> Result<()> {
        let read = self
            .http
            .call(
                &self.mcp_url(),
                "read_state",
                json!({"target": expected.state["target"]}),
            )
            .await?;
        ensure!(read == expected.state, "audit governance blocked real read");
        Ok(())
    }

    async fn verify_degraded_audit_health(&self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let reply = self
                .http
                .request(
                    Method::GET,
                    &self.management_url("/api/v1/audit/health"),
                    None,
                )
                .await?;
            ensure!(reply.status == 200, "audit health unavailable");
            if reply.body["data"]["healthy"] == false {
                ensure!(
                    reply.body["data"]["last_error"].is_string(),
                    "audit storage error missing"
                );
                self.save_json("degraded-audit-health.json", &reply.body)?;
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "audit storage failure not observable"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

async fn block_audit_writes(path: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match std::fs::create_dir(path) {
            Ok(()) => return Ok(()),
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists
                    && Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
}
