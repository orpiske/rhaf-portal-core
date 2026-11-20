use rhaf_api::models::{Catalog, Forward, ForwardRequest, Response};
use serde_json::{Value, json};

#[test]
fn build_output_matches_rust_declarations() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(rhaf_api::OPENAPI_JSON, rhaf_api::openapi::openapi_json()?);
    Ok(())
}

#[test]
fn requests_reject_runtime_state() -> Result<(), serde_json::Error> {
    let request =
        json!({"name": "sample", "address": "https://example.test/mcp", "namespaceBase": "sample"});
    let decoded: ForwardRequest = serde_json::from_value(request.clone())?;
    assert!(decoded.labels.is_empty());
    assert!(decoded.credential_bindings.discovery.is_none());
    for field in ["registrationId", "state", "available", "namespace"] {
        let mut invalid = request.clone();
        invalid[field] = json!("client-controlled");
        assert!(serde_json::from_value::<ForwardRequest>(invalid).is_err());
    }
    Ok(())
}

#[test]
fn schema_covers_routes_and_statuses() -> Result<(), serde_json::Error> {
    let document: Value = serde_json::from_slice(rhaf_api::OPENAPI_JSON)?;
    for (path, method, statuses) in [
        (
            "/api/rhaf/v1/forwards",
            "post",
            &["201", "202", "400", "409", "500", "504"][..],
        ),
        ("/api/rhaf/v1/forwards", "get", &["200"][..]),
        ("/api/rhaf/v1/forwards/{name}", "get", &["200", "404"][..]),
        (
            "/api/rhaf/v1/forwards/{name}",
            "put",
            &["200", "400", "404", "409", "502", "500", "504"][..],
        ),
        (
            "/api/rhaf/v1/forwards/{name}",
            "delete",
            &["200", "202", "404", "409", "500", "504"][..],
        ),
        ("/api/rhaf/v1/catalog", "get", &["200"][..]),
    ] {
        for status in statuses {
            assert!(
                document["paths"][path][method]["responses"][status].is_object(),
                "missing {method} {path} {status}"
            );
        }
    }
    Ok(())
}

#[test]
fn schema_covers_constraints() -> Result<(), serde_json::Error> {
    let document: Value = serde_json::from_slice(rhaf_api::OPENAPI_JSON)?;
    let schemas = &document["components"]["schemas"];
    let request = &schemas["ForwardRequest"];
    assert_eq!(request["additionalProperties"], false);
    assert_eq!(request["properties"]["namespaceBase"]["minLength"], 1);
    assert_eq!(request["properties"]["namespaceBase"]["maxLength"], 51);
    assert_eq!(
        request["properties"]["namespaceBase"]["pattern"],
        "^[a-z0-9]([a-z0-9-]{0,49}[a-z0-9])?$"
    );
    assert_eq!(
        schemas["Forward"]["properties"]["registrationId"]["readOnly"],
        true
    );
    Ok(())
}

#[test]
fn examples_match_wire_types_and_unknown_availability() -> Result<(), serde_json::Error> {
    let document: Value = serde_json::from_slice(rhaf_api::OPENAPI_JSON)?;
    for (status, example) in [("202", "pendingCleanup"), ("200", "completed")] {
        let value = document["paths"]["/api/rhaf/v1/forwards/{name}"]["delete"]["responses"]
            [status]["content"]["application/json"]["examples"][example]["value"]
            .clone();
        let response: Response<Forward> = serde_json::from_value(value)?;
        assert!(response.data.available.is_none());
    }
    for example in ["loaded", "absent", "invalid"] {
        let value = document["paths"]["/api/rhaf/v1/catalog"]["get"]["responses"]["200"]["content"]
            ["application/json"]["examples"][example]["value"]
            .clone();
        let response: Response<Catalog> = serde_json::from_value(value)?;
        assert!(!response.data.products.is_empty() || example != "loaded");
    }
    Ok(())
}

#[test]
fn creation_examples_match_wire_types() -> Result<(), serde_json::Error> {
    let document: Value = serde_json::from_slice(rhaf_api::OPENAPI_JSON)?;
    for (status, example) in [("201", "success"), ("202", "failedRegistrationCleanup")] {
        let value = document["paths"]["/api/rhaf/v1/forwards"]["post"]["responses"][status]
            ["content"]["application/json"]["examples"][example]["value"].clone();
        let response: Response<Forward> = serde_json::from_value(value)?;
        assert_eq!(response.data.name, "jolokia");
    }
    Ok(())
}

#[test]
fn all_local_schema_references_resolve() -> Result<(), serde_json::Error> {
    fn check(value: &Value, document: &Value) {
        match value {
            Value::Object(object) => {
                if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                    assert!(
                        reference
                            .strip_prefix('#')
                            .is_some_and(|path| document.pointer(path).is_some()),
                        "unresolved reference: {reference}"
                    );
                }
                for child in object.values() {
                    check(child, document);
                }
            }
            Value::Array(array) => {
                for child in array {
                    check(child, document);
                }
            }
            _ => {}
        }
    }
    let document: Value = serde_json::from_slice(rhaf_api::OPENAPI_JSON)?;
    check(&document, &document);
    Ok(())
}

#[test]
fn exporter_detects_drift_without_changing_the_snapshot() -> Result<(), Box<dyn std::error::Error>>
{
    use std::process::Command;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("openapi.json");
    let executable = env!("CARGO_BIN_EXE_rhaf-openapi");
    assert!(Command::new(executable).arg(&path).status()?.success());
    assert_eq!(std::fs::read(&path)?, rhaf_api::OPENAPI_JSON);
    assert!(
        Command::new(executable)
            .arg("--check")
            .arg(&path)
            .status()?
            .success()
    );
    assert_eq!(
        Command::new(executable).output()?.stdout,
        rhaf_api::OPENAPI_JSON
    );
    std::fs::write(&path, b"outdated")?;
    assert!(
        !Command::new(executable)
            .arg("--check")
            .arg(&path)
            .output()?
            .status
            .success()
    );
    assert_eq!(std::fs::read(&path)?, b"outdated");
    std::fs::remove_file(path)?;
    Ok(())
}
