use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn validator(
    document: &Value,
    schema: &Value,
) -> Result<jsonschema::Validator, Box<dyn std::error::Error>> {
    let schema = json!({"allOf": [schema], "components": document["components"]});
    Ok(jsonschema::options()
        .should_validate_formats(true)
        .build(&schema)?)
}

fn response_media_types(document: &Value) -> impl Iterator<Item = &Value> {
    document["paths"]
        .as_object()
        .into_iter()
        .flat_map(|paths| paths.values())
        .filter_map(Value::as_object)
        .flat_map(|operations| operations.values())
        .filter_map(|operation| operation["responses"].as_object())
        .flat_map(|responses| responses.values())
        .map(|response| &response["content"]["application/json"])
}

#[test]
fn every_published_response_example_validates() -> TestResult {
    let document: Value = serde_json::from_slice(rhaf_api::OPENAPI_JSON)?;
    let mut count = 0;
    for media in response_media_types(&document) {
        let validator = validator(&document, &media["schema"])?;
        for example in media["examples"]
            .as_object()
            .into_iter()
            .flat_map(|examples| examples.values())
        {
            let value = &example["value"];
            let errors: Vec<_> = validator
                .iter_errors(value)
                .map(|error| error.to_string())
                .collect();
            assert!(errors.is_empty(), "invalid example {value}: {errors:?}");
            count += 1;
        }
    }
    assert_eq!(count, 9);
    Ok(())
}

#[test]
fn response_schemas_require_both_fields_and_separate_success_from_errors() -> TestResult {
    let document: Value = serde_json::from_slice(rhaf_api::OPENAPI_JSON)?;
    for media in response_media_types(&document) {
        let validator = validator(&document, &media["schema"])?;
        for invalid in [
            json!({}),
            json!({"data": null}),
            json!({"error": null}),
            json!({"data": null, "error": null}),
        ] {
            assert!(
                !validator.is_valid(&invalid),
                "accepted incomplete envelope {invalid}"
            );
        }
    }
    let success = &document["paths"]["/api/rhaf/v1/forwards"]["post"]["responses"]["201"]["content"]
        ["application/json"];
    let validator = validator(&document, &success["schema"])?;
    let mut value = success["examples"]["success"]["value"].clone();
    value["error"] = json!("unexpected error");
    assert!(!validator.is_valid(&value));
    Ok(())
}

#[test]
fn error_schema_requires_string_error_and_null_only_data() -> TestResult {
    let document: Value = serde_json::from_slice(rhaf_api::OPENAPI_JSON)?;
    let validator = validator(
        &document,
        &json!({"$ref": "#/components/schemas/ErrorResponse"}),
    )?;
    assert!(validator.is_valid(&json!({"data": null, "error": "Invalid namespace base"})));
    for invalid in [
        json!({"data": "unexpected", "error": "failure"}),
        json!({"data": {}, "error": "failure"}),
        json!({"data": null, "error": 4}),
    ] {
        assert!(!validator.is_valid(&invalid));
    }
    Ok(())
}

#[test]
fn list_success_requires_null_error() -> TestResult {
    let document: Value = serde_json::from_slice(rhaf_api::OPENAPI_JSON)?;
    let schema = &document["paths"]["/api/rhaf/v1/forwards"]["get"]["responses"]["200"]["content"]
        ["application/json"]["schema"];
    let validator = validator(&document, schema)?;
    assert!(validator.is_valid(&json!({"data": [], "error": null})));
    assert!(!validator.is_valid(&json!({"data": [], "error": "Registration not found"})));
    Ok(())
}

#[test]
fn http_url_schemas_accept_mixed_case_schemes_and_reject_credentials() -> TestResult {
    let document: Value = serde_json::from_slice(rhaf_api::OPENAPI_JSON)?;
    for (model, field) in [("ForwardRequest", "address"), ("ProductLink", "url")] {
        let validator = validator(
            &document,
            &document["components"]["schemas"][model]["properties"][field],
        )?;
        for address in [
            "HTTPS://example.test/mcp",
            "hTtPs://example.test/mcp",
            "HtTp://example.test/?key=value",
        ] {
            assert!(validator.is_valid(&json!(address)), "rejected {address}");
        }
        for address in [
            "https://user:secret@example.test/mcp",
            "HTTPS://user@example.test/",
            "ftp://example.test/",
            "https:///missing-host",
        ] {
            assert!(!validator.is_valid(&json!(address)), "accepted {address}");
        }
    }
    Ok(())
}
