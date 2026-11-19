#![expect(
    dead_code,
    reason = "utoipa path declarations document planned routes without installing handlers"
)]
#![expect(
    clippy::large_stack_frames,
    reason = "utoipa generates path builders with large stack frames; these run during schema generation"
)]

use serde_json::json;
use utoipa::OpenApi;

use crate::models::{Catalog, Forward, Response};

#[utoipa::path(post, path = "/api/rhaf/v1/forwards", tag = "Lifecycle",
    request_body = crate::models::ForwardRequest,
    description = "Create a registration. Duplicate names and reserved namespace bases conflict. A valid unavailable endpoint can remain registered. Successful mutations cross the persistence boundary.",
    responses(
        (status = 201, description = "Registration persisted", body = Response<Forward>),
        (status = 202, description = "Failed registration cleanup remains pending; the base stays reserved", body = Response<Forward>),
        (status = 400, description = "Invalid configuration or reserved labels", body = Response<String>),
        (status = 409, description = "Name or namespace base conflict", body = Response<String>),
        (status = 500, description = "Persistence failed; completion is not confirmed", body = Response<String>),
        (status = 504, description = "Persistence timed out; completion is not confirmed", body = Response<String>)
    )
)]
const fn create_forward() {}

#[utoipa::path(put, path = "/api/rhaf/v1/forwards/{name}", tag = "Lifecycle",
    params(("name" = String, Path, description = "Existing immutable forward name")),
    request_body = crate::models::ForwardRequest,
    description = "Explicit replacement. Name and namespaceBase cannot change. Failed discovery preserves the previous configuration and capabilities. Runtime state is not client-controlled.",
    responses(
        (status = 200, description = "Replacement persisted", body = Response<Forward>),
        (status = 400, description = "Invalid replacement", body = Response<String>),
        (status = 404, description = "Confirmed absent", body = Response<String>),
        (status = 409, description = "Immutable base mismatch or lifecycle conflict", body = Response<String>),
        (status = 502, description = "Discovery failed; previous state retained", body = Response<String>),
        (status = 500, description = "Persistence failed", body = Response<String>),
        (status = 504, description = "Persistence timed out", body = Response<String>)
    )
)]
const fn replace_forward() {}

#[utoipa::path(get, path = "/api/rhaf/v1/forwards", tag = "Lifecycle",
    description = "List lifecycle registrations, including reserved entries with incomplete cleanup. Do not report durable absence before cleanup persistence completes.",
    responses((status = 200, description = "Registrations", body = Response<Vec<Forward>>))
)]
const fn list_forwards() {}

#[utoipa::path(get, path = "/api/rhaf/v1/forwards/{name}", tag = "Lifecycle",
    params(("name" = String, Path)),
    responses(
        (status = 200, description = "Registration or readable deletion completion record", body = Response<Forward>),
        (status = 404, description = "Confirmed absent", body = Response<String>)
    )
)]
const fn read_forward() {}

#[utoipa::path(delete, path = "/api/rhaf/v1/forwards/{name}", tag = "Lifecycle",
    params(("name" = String, Path)),
    description = "Persist the deletion marker before cleanup. Remove the conventional namespace last. Repeated deletion is idempotent. Keep the namespace base reserved through durable completion. Do not delete workloads, Secrets, PVCs, or binding definitions.",
    responses(
        (status = 200, description = "Deletion durably completed", body = Response<Forward>),
        (status = 202, description = "Cleanup pending; registration remains readable", body = Response<Forward>),
        (status = 404, description = "Confirmed absent", body = Response<String>),
        (status = 409, description = "Ownership or lifecycle conflict", body = Response<String>),
        (status = 500, description = "Persistence failed; absence is not confirmed", body = Response<String>),
        (status = 504, description = "Persistence timed out; absence is not confirmed", body = Response<String>)
    )
)]
const fn delete_forward() {}

#[utoipa::path(get, path = "/api/rhaf/v1/catalog", tag = "Catalog",
    description = "Read the configured product catalog. Absent or invalid content returns HTTP 200 with an empty products list and a safe message. Catalog reads do not register products, probe URLs, or report availability.",
    responses((status = 200, description = "Loaded, absent, or invalid catalog", body = Response<Catalog>))
)]
const fn read_catalog() {}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "RHAF Portal API",
        description = "Planned lifecycle and catalog contracts. Path declarations do not establish implemented routing."
    ),
    paths(
        create_forward,
        replace_forward,
        list_forwards,
        read_forward,
        delete_forward,
        read_catalog
    )
)]
struct RhafApi;

/// Generate deterministic OpenAPI from Rust types and path declarations.
pub fn openapi_json() -> Result<Vec<u8>, serde_json::Error> {
    let mut document = serde_json::to_value(RhafApi::openapi())?;
    add_examples(&mut document);
    // Workspace dependencies can enable serde_json's preserve_order feature.
    document.sort_all_objects();
    let mut bytes = serde_json::to_vec_pretty(&document)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn forward_example(state: &str, available: Option<bool>) -> serde_json::Value {
    json!({"data": {
        "name": "jolokia", "address": "https://mcp.example.test/mcp",
        "namespaceBase": "jolokia", "conventionalNamespace": "jolokia-all",
        "progressiveNamespace": "jolokia-progressive", "registrationId": "registration-123",
        "labels": {}, "credentialBindings": {}, "state": state,
        "available": available, "message": null
    }, "error": null})
}

#[expect(
    clippy::too_many_lines,
    reason = "declarative OpenAPI response example table"
)]
fn add_examples(document: &mut serde_json::Value) {
    let success = forward_example("active", Some(true));
    let pending = forward_example("deleting", None);
    let failed = forward_example("failedRegistrationCleanup", Some(false));
    let examples = [
        (
            "/api/rhaf/v1/forwards",
            "post",
            "201",
            json!({"success": {"value": success}}),
        ),
        (
            "/api/rhaf/v1/forwards",
            "post",
            "202",
            json!({"failedRegistrationCleanup": {"value": failed}}),
        ),
        (
            "/api/rhaf/v1/forwards/{name}",
            "delete",
            "202",
            json!({"pendingCleanup": {"value": pending}}),
        ),
        (
            "/api/rhaf/v1/forwards/{name}",
            "delete",
            "200",
            json!({"completed": {"value": forward_example("deleted", None)}}),
        ),
        (
            "/api/rhaf/v1/forwards",
            "post",
            "400",
            json!({"invalid": {"value": {"data": null, "error": "namespaceBase is invalid"}}}),
        ),
        (
            "/api/rhaf/v1/forwards/{name}",
            "get",
            "404",
            json!({"missing": {"value": {"data": null, "error": "Registration not found"}}}),
        ),
        (
            "/api/rhaf/v1/catalog",
            "get",
            "200",
            json!({
                "loaded": {"value": {"data": {"status": "loaded", "products": [{"id": "jolokia", "displayName": "Jolokia", "links": [{"label": "Documentation", "url": "https://jolokia.org/"}]}], "message": null}, "error": null}},
                "absent": {"value": {"data": {"status": "absent", "products": [], "message": null}, "error": null}},
                "invalid": {"value": {"data": {"status": "invalid", "products": [], "message": "Catalog validation failed"}, "error": null}}
            }),
        ),
    ];
    for (path, method, status, examples) in examples {
        document["paths"][path][method]["responses"][status]["content"]["application/json"]["examples"] =
            examples;
    }
}
