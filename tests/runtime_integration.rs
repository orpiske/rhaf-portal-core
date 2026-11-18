//! Real MCP forwarding and process lifecycle evidence.

mod support;

use anyhow::Result;
use serde_json::json;
use support::Harness;

#[tokio::test]
#[ignore = "requires RHAF_MCP_FIXTURE_JAR and RHAF_MCP_FIXTURE_REVISION"]
async fn conventional_runtime_against_controlled_mcp_fixture() -> Result<()> {
    let harness = Harness::from_env()?;
    harness.verify_startup().await?;
    harness.verify_opa_redaction().await?;
    harness.verify_audit_shutdown_failure().await?;
    let fixture = harness.start_fixture().await?;
    harness.verify_rejected_bootstrap().await?;
    let first = harness.start_core("core-first.log").await?;
    harness.verify_exclusive_storage().await?;
    let expected = harness.exercise().await?;
    first.stop().await?;
    anyhow::ensure!(
        harness.data_dir().join("registry.json").is_file(),
        "registry shutdown flush missing"
    );
    let second = harness.start_core("core-restart.log").await?;
    harness.verify_restart(&expected).await?;
    second.stop().await?;
    fixture.stop().await?;
    harness.record_success()
}

#[test]
fn nested_json_boolean_is_distinct_from_number() {
    let expected = json!({"metadata": {"samples": [1, false]}});
    assert_eq!(expected, json!({"metadata": {"samples": [1, false]}}));
    assert_ne!(expected, json!({"metadata": {"samples": [1, 0]}}));
}
