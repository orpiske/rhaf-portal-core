use serde::de::DeserializeOwned;
use serde_yaml::Value;
use wanaku_feature_evaluator::config::{
    EvaluatorDef, EvaluatorsConfig, LlmConnection, OpaConnection, SystemOneConnection,
};

pub fn redact_malformed_sections(root: &mut Value) {
    redact_connections::<LlmConnection>(root, "llm_connections");
    redact_opa_connections(root);
    redact_connections::<SystemOneConnection>(root, "typesafe_system_one_connections");
    if let Some(value) = root.get_mut("evaluators")
        && serde_yaml::from_value::<EvaluatorsConfig>(value.clone()).is_err()
        && serde_yaml::from_value::<Vec<EvaluatorDef>>(value.clone()).is_err()
    {
        tracing::warn!(
            section = "evaluators",
            "invalid bootstrap section; authored values omitted"
        );
        *value = Value::String("redacted invalid configuration".to_owned());
    }
}

fn redact_opa_connections(root: &mut Value) {
    let invalid = root.get("opa_connections").and_then(|value| {
        serde_yaml::from_value::<Vec<OpaConnection>>(value.clone())
            .ok()
            .and_then(|entries| {
                entries
                    .iter()
                    .position(|connection| connection.client().is_err())
            })
    });
    match invalid {
        Some(index) => {
            tracing::warn!(
                section = "opa_connections",
                index,
                "invalid bootstrap section; authored values omitted"
            );
            root["opa_connections"] = Value::String("redacted invalid configuration".to_owned());
        }
        None => redact_connections::<OpaConnection>(root, "opa_connections"),
    }
}

fn redact_connections<T: DeserializeOwned>(root: &mut Value, section: &'static str) {
    if let Some(value) = root.get_mut(section)
        && serde_yaml::from_value::<Vec<T>>(value.clone()).is_err()
    {
        let index = value.as_sequence().and_then(|entries| {
            entries
                .iter()
                .position(|entry| serde_yaml::from_value::<T>(entry.clone()).is_err())
        });
        tracing::warn!(
            section,
            index,
            "invalid bootstrap section; authored values omitted"
        );
        // Preserve upstream invalid-state marking and evaluator startup deny/abort behavior.
        *value = Value::String("redacted invalid configuration".to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::redact_malformed_sections;
    use anyhow::{Context, Result};
    use wanaku_feature_evaluator::config::OpaConnection;

    #[test]
    fn redaction_keeps_malformed_connection_invalid_and_preserves_startup_policy() -> Result<()> {
        let mut config = serde_yaml::from_str(
            "opa_connections: [HIDDEN_VALUE]\nevaluator_startup_failure: deny",
        )?;
        redact_malformed_sections(&mut config);
        let connections = config
            .get("opa_connections")
            .context("OPA configuration missing")?;
        assert!(serde_yaml::from_value::<Vec<OpaConnection>>(connections.clone()).is_err());
        assert_eq!(
            config
                .get("evaluator_startup_failure")
                .and_then(serde_yaml::Value::as_str),
            Some("deny")
        );
        assert!(!serde_yaml::to_string(&config)?.contains("HIDDEN_VALUE"));
        Ok(())
    }

    #[test]
    fn redaction_keeps_semantically_invalid_opa_connection_invalid() -> Result<()> {
        let mut config = serde_yaml::from_str(
            "opa_connections: [{name: HIDDEN_NAME, url: 'http://localhost:8181', timeout_ms: 0}]\nevaluator_startup_failure: deny",
        )?;
        redact_malformed_sections(&mut config);
        let serialized = serde_yaml::to_string(&config)?;
        assert!(!serialized.contains("HIDDEN_NAME"));
        assert_eq!(config["evaluator_startup_failure"].as_str(), Some("deny"));
        assert!(
            serde_yaml::from_value::<Vec<OpaConnection>>(config["opa_connections"].clone())
                .is_err()
        );
        Ok(())
    }
}
