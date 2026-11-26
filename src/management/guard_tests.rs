use http::{HeaderMap, HeaderValue, StatusCode};
use wanaku_types::feature::HttpContext;
use wanaku_types::registry::{ToolEntry, ToolRegistry};

use super::tests::service;

#[tokio::test]
async fn raw_capability_writes_respect_both_managed_namespaces() -> anyhow::Result<()> {
    let service = service();
    let lifecycle = &service.forwards.lifecycle;
    lifecycle.reserve("managed", "owned").await?;
    let mut headers = HeaderMap::new();
    headers.insert("x-rhaf-internal", HeaderValue::from_static("true"));

    for query in [
        "namespace=owned-all",
        "name%73pace=owned-progressive&namespace=default",
    ] {
        for kind in ["tools", "resources", "prompts"] {
            for (method, suffix) in [("DELETE", ""), ("PUT", "/enable"), ("PUT", "/disable")] {
                let path = format!("/api/v1/{kind}/example{suffix}");
                let response = service
                    .dispatch(&HttpContext::new(
                        method,
                        &path,
                        Some(query),
                        None,
                        &headers,
                    ))
                    .await;
                assert_eq!(
                    response.status(),
                    StatusCode::CONFLICT,
                    "{method} {path} {query}"
                );
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn progressive_tool_stays_readable_and_unmanaged_tool_can_change() -> anyhow::Result<()> {
    let service = service();
    let lifecycle = &service.forwards.lifecycle;
    lifecycle.reserve("managed", "owned").await?;
    let registry = lifecycle.registry();
    register_tool(registry, "owned-progressive", "test")?;
    register_tool(registry, "default", "test")?;
    let headers = HeaderMap::new();
    let query = Some("namespace=owned-progressive");
    let item = "/api/v1/tools/example";
    let disable = "/api/v1/tools/example/disable";
    for (method, path, query, expected) in [
        ("PUT", disable, query, StatusCode::CONFLICT),
        ("DELETE", item, query, StatusCode::CONFLICT),
        ("GET", item, query, StatusCode::OK),
        ("PUT", disable, None, StatusCode::OK),
        ("DELETE", item, None, StatusCode::OK),
    ] {
        let response = service
            .dispatch(&HttpContext::new(method, path, query, None, &headers))
            .await;
        assert_eq!(response.status(), expected, "{method} {path}");
    }
    assert!(
        registry
            .get_tool("owned-progressive", "example")
            .is_some_and(|tool| tool.enabled)
    );
    assert!(registry.get_tool("default", "example").is_none());
    Ok(())
}

#[tokio::test]
async fn raw_handler_snapshot_rejects_managed_targets_on_both_paths() -> anyhow::Result<()> {
    let service = service();
    service
        .forwards
        .lifecycle
        .reserve("managed", "owned")
        .await?;
    let body = r#"[{"tool":{"name":"example","description":"","uri":"test://example","type":"wasm","inputSchema":{"type":"object"},"namespace":"owned-progressive"},"path":"example.wasm"}]"#;
    let headers = HeaderMap::new();
    for path in ["/api/v1/tool-handlers", "/api/v1/tool-handlers/"] {
        let response = service
            .dispatch(&HttpContext::new("PUT", path, None, Some(body), &headers))
            .await;
        assert_eq!(response.status(), StatusCode::CONFLICT, "{path}");
    }
    Ok(())
}

#[tokio::test]
async fn raw_handler_snapshot_preserves_managed_progressive_tool() -> anyhow::Result<()> {
    let service = service();
    let lifecycle = &service.forwards.lifecycle;
    lifecycle.reserve("managed", "owned").await?;
    let registry = lifecycle.registry();
    register_tool(registry, "owned-progressive", "wasm")?;
    let headers = HeaderMap::new();
    for (method, path, body) in [
        ("PUT", "/api/v1/tool-handlers", Some("[]")),
        ("PUT", "/api/v1/tool-handlers/", Some("[]")),
        (
            "DELETE",
            "/api/v1/tool-handlers/owned-progressive/example",
            None,
        ),
    ] {
        let response = service
            .dispatch(&HttpContext::new(method, path, None, body, &headers))
            .await;
        assert_eq!(response.status(), StatusCode::CONFLICT, "{method} {path}");
    }
    assert!(registry.get_tool("owned-progressive", "example").is_some());
    Ok(())
}

fn register_tool(registry: &impl ToolRegistry, namespace: &str, kind: &str) -> anyhow::Result<()> {
    let tool: ToolEntry = serde_json::from_value(serde_json::json!({
        "name": "example",
        "description": "",
        "uri": "test://example",
        "type": kind,
        "inputSchema": {"type": "object"},
        "namespace": namespace
    }))?;
    registry.register_tool(tool);
    Ok(())
}
