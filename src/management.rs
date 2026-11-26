mod forwards;

#[cfg(test)]
mod guard_tests;

use std::sync::Arc;

use http::header::{ALLOW, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, HeaderValue};
use http::{Response, StatusCode};
use pingora_core::apps::http_app::ServeHttp;
use pingora_core::protocols::http::ServerSession;
use rhaf_portal_core::lifecycle::{
    Lifecycle, is_lifecycle_label, protects_forward, protects_namespace,
};
use wanaku_infra::credentials::{CredentialAuditSink, CredentialBroker};
use wanaku_infra::registry::InMemoryRegistry;
use wanaku_server::management::WanakuManagementService;
use wanaku_types::agents::AgentEntry;
use wanaku_types::feature::HttpContext;
use wanaku_types::http_response::json_err;
use wanaku_types::registry::{
    DEFAULT_NAMESPACE, ForwardEntry, ForwardRegistry, NamespaceEntry, ToolEntry, ToolRegistry,
};

const MAX_BODY_BYTES: usize = 1_048_576;

pub struct ManagementService {
    wanaku: WanakuManagementService,
    forwards: forwards::ForwardService,
}

#[derive(Debug, PartialEq, Eq)]
enum Route {
    Schema,
    Lifecycle,
    Catalog,
    MethodNotAllowed(&'static str),
    NotFound,
    Wanaku,
}

fn resolve_route(method: &str, path: &str) -> Route {
    let allowed = match path {
        "/api/rhaf/v1/openapi.json" => "GET, HEAD",
        "/api/rhaf/v1/forwards" => "GET, POST",
        "/api/rhaf/v1/catalog" => "GET",
        _ => {
            return match path.strip_prefix("/api/rhaf/v1/forwards/") {
                Some(name) if !name.is_empty() && !name.contains('/') => match method {
                    "GET" | "PUT" | "DELETE" => Route::Lifecycle,
                    _ => Route::MethodNotAllowed("GET, PUT, DELETE"),
                },
                _ if path == "/api/rhaf" || path.starts_with("/api/rhaf/") => Route::NotFound,
                _ => Route::Wanaku,
            };
        }
    };
    match (method, path) {
        ("GET" | "HEAD", "/api/rhaf/v1/openapi.json") => Route::Schema,
        ("GET" | "POST", "/api/rhaf/v1/forwards") => Route::Lifecycle,
        ("GET", "/api/rhaf/v1/catalog") => Route::Catalog,
        _ => Route::MethodNotAllowed(allowed),
    }
}

impl ManagementService {
    pub const fn new(
        wanaku: WanakuManagementService,
        lifecycle: Lifecycle,
        broker: Arc<CredentialBroker>,
        audit_sink: Option<Arc<dyn CredentialAuditSink>>,
    ) -> Self {
        Self {
            wanaku,
            forwards: forwards::ForwardService {
                lifecycle,
                broker,
                audit_sink,
            },
        }
    }

    pub async fn dispatch(&self, ctx: &HttpContext<'_>) -> Response<Vec<u8>> {
        let mut response = match resolve_route(ctx.method, ctx.path) {
            Route::Schema => schema_response(),
            Route::Lifecycle => self.forwards.dispatch(ctx.method, ctx.path, ctx.body).await,
            Route::Catalog => json_err(
                StatusCode::NOT_IMPLEMENTED,
                "RHAF catalog operations are not available in this build",
            ),
            Route::MethodNotAllowed(allowed) => method_not_allowed(allowed),
            Route::NotFound => json_err(StatusCode::NOT_FOUND, "not found"),
            Route::Wanaku => self.dispatch_wanaku(ctx).await,
        };
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response.headers_mut().insert(
            "x-content-type-options",
            HeaderValue::from_static("nosniff"),
        );
        if ctx.method == "HEAD" {
            response.body_mut().clear();
        }
        response
    }

    async fn dispatch_wanaku(&self, ctx: &HttpContext<'_>) -> Response<Vec<u8>> {
        if !matches!(ctx.method, "POST" | "PUT" | "DELETE")
            || ![
                "/api/v1/forwards",
                "/api/v1/namespaces",
                "/api/v1/tools",
                "/api/v1/resources",
                "/api/v1/prompts",
                "/api/v1/tool-handlers",
                "/api/v1/agents",
            ]
            .iter()
            .any(|prefix| {
                ctx.path
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
            })
        {
            return self.wanaku.dispatch(ctx).await;
        }

        // Wanaku's raw registration endpoint performs its own discovery and
        // publication. Keep its entire delegation serialized with reservations.
        let _commit = self.forwards.lifecycle.commit_guard().await;
        if raw_lifecycle_conflict(self.forwards.lifecycle.registry(), ctx) {
            return json_err(
                StatusCode::CONFLICT,
                "record is reserved by the RHAF lifecycle",
            );
        }
        self.wanaku.dispatch(ctx).await
    }
}

fn raw_lifecycle_conflict(registry: &InMemoryRegistry, ctx: &HttpContext<'_>) -> bool {
    let body = ctx.body.unwrap_or_default();
    if capability_write_conflicts(registry, ctx, body) {
        return true;
    }
    match (ctx.method, ctx.path) {
        ("POST", "/api/v1/forwards" | "/api/v1/forwards/") => {
            forward_write_conflicts(registry, body)
        }
        ("POST", "/api/v1/namespaces" | "/api/v1/namespaces/") => {
            namespace_write_conflicts(registry, body, None)
        }
        ("PUT", path) => path
            .strip_prefix("/api/v1/namespaces/")
            .filter(|name| !name.is_empty())
            .is_some_and(|name| namespace_write_conflicts(registry, body, Some(name))),
        ("DELETE", path) if path.starts_with("/api/v1/namespaces/") => {
            protects_namespace(registry, &path["/api/v1/namespaces/".len()..])
        }
        ("POST", path) => path
            .strip_suffix("/refreshes")
            .is_some_and(|path| forward_path_conflicts(registry, path)),
        ("DELETE", path) => forward_path_conflicts(registry, path),
        _ => false,
    }
}

fn forward_write_conflicts(registry: &InMemoryRegistry, body: &str) -> bool {
    serde_json::from_str::<ForwardEntry>(body).is_ok_and(|forward| {
        forward.labels.keys().any(|key| is_lifecycle_label(key))
            || forward_target_conflicts(registry, &forward.name)
            || protects_namespace(
                registry,
                forward.namespace.as_deref().unwrap_or(DEFAULT_NAMESPACE),
            )
    })
}

fn forward_path_conflicts(registry: &InMemoryRegistry, path: &str) -> bool {
    path.strip_prefix("/api/v1/forwards/").is_some_and(|name| {
        !name.is_empty() && !name.contains('/') && forward_target_conflicts(registry, name)
    })
}

fn namespace_write_conflicts(
    registry: &InMemoryRegistry,
    body: &str,
    path_name: Option<&str>,
) -> bool {
    serde_json::from_str::<NamespaceEntry>(body).is_ok_and(|namespace| {
        namespace.labels.keys().any(|key| is_lifecycle_label(key))
            || protects_namespace(registry, path_name.unwrap_or(&namespace.name))
            || namespace
                .members
                .iter()
                .flatten()
                .any(|member| protects_namespace(registry, member))
    })
}

fn forward_target_conflicts(registry: &InMemoryRegistry, name: &str) -> bool {
    protects_forward(registry, name)
        || registry.get_forward(name).is_some_and(|forward| {
            protects_namespace(
                registry,
                forward.namespace.as_deref().unwrap_or(DEFAULT_NAMESPACE),
            )
        })
}

fn capability_write_conflicts(
    registry: &InMemoryRegistry,
    ctx: &HttpContext<'_>,
    body: &str,
) -> bool {
    let Some((collection, child)) = ctx
        .path
        .strip_prefix("/api/v1/")
        .map(|path| path.split_once('/').unwrap_or((path, "")))
    else {
        return false;
    };
    match (collection, ctx.method, child) {
        ("tools" | "resources" | "prompts", "PUT" | "DELETE", child)
            if capability_item_mutation(ctx.method, child) =>
        {
            let namespace = form_urlencoded::parse(ctx.query.unwrap_or_default().as_bytes())
                .find(|(key, _)| key == "namespace")
                .map_or_else(|| DEFAULT_NAMESPACE.into(), |(_, value)| value);
            protects_namespace(registry, &namespace)
        }
        ("agents", "POST", "") if ctx.path == "/api/v1/agents" => {
            serde_json::from_str::<AgentEntry>(body)
                .is_ok_and(|agent| protects_namespace(registry, &agent.namespace))
        }
        ("agents", "PUT" | "DELETE", child) | ("tool-handlers", "DELETE", child) => child
            .split_once('/')
            .filter(|(ns, name)| !ns.is_empty() && !name.is_empty() && !name.contains('/'))
            .is_some_and(|(ns, _)| protects_namespace(registry, ns)),
        ("tool-handlers", "PUT", "") if ctx.path == "/api/v1/tool-handlers" => {
            handler_snapshot_conflicts(registry, body)
        }
        _ => false,
    }
}

fn capability_item_mutation(method: &str, child: &str) -> bool {
    match method {
        "DELETE" => !child.is_empty(),
        "PUT" => child
            .strip_suffix("/enable")
            .or_else(|| child.strip_suffix("/disable"))
            .is_some_and(|name| !name.is_empty()),
        _ => false,
    }
}

fn handler_snapshot_conflicts(registry: &InMemoryRegistry, body: &str) -> bool {
    #[derive(serde::Deserialize)]
    struct HandlerTarget {
        tool: ToolEntry,
    }

    serde_json::from_str::<Vec<HandlerTarget>>(body).is_ok_and(|targets| {
        let is_managed = |tool: &ToolEntry| {
            protects_namespace(
                registry,
                tool.namespace.as_deref().unwrap_or(DEFAULT_NAMESPACE),
            )
        };
        targets.iter().any(|target| is_managed(&target.tool))
            || registry
                .list_tools()
                .iter()
                .any(|tool| tool.type_ == "wasm" && is_managed(tool))
    })
}

fn method_not_allowed(allowed: &'static str) -> Response<Vec<u8>> {
    let mut response = json_err(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
    response
        .headers_mut()
        .insert(ALLOW, HeaderValue::from_static(allowed));
    response
}

fn schema_response() -> Response<Vec<u8>> {
    let body = rhaf_api::OPENAPI_JSON;
    let mut response = Response::new(body.to_vec());
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
        .headers_mut()
        .insert(CONTENT_LENGTH, HeaderValue::from(body.len()));
    response
}

#[async_trait::async_trait]
impl ServeHttp for ManagementService {
    async fn response(&self, session: &mut ServerSession) -> Response<Vec<u8>> {
        let request = session.req_header();
        let path = request.uri.path().to_owned();
        let query = request.uri.query().map(str::to_owned);
        let method = request.method.as_str().to_owned();
        let headers = request.headers.clone();
        let body = match method.as_str() {
            "POST" | "PUT" | "PATCH" if path.starts_with("/api/") => match read_body(session).await
            {
                Ok(body) => Some(body),
                Err(mut response) => {
                    response
                        .headers_mut()
                        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
                    return response;
                }
            },
            _ => None,
        };
        self.dispatch(&HttpContext::new(
            &method,
            &path,
            query.as_deref(),
            body.as_deref(),
            &headers,
        ))
        .await
    }
}

async fn read_body(session: &mut ServerSession) -> Result<String, Response<Vec<u8>>> {
    let mut body = Vec::new();
    loop {
        match session.read_request_body().await {
            Ok(Some(chunk)) if chunk.len() <= MAX_BODY_BYTES - body.len() => {
                body.extend_from_slice(&chunk);
            }
            Ok(Some(_)) => {
                return Err(json_err(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request body too large",
                ));
            }
            Ok(None) => break,
            Err(_) => {
                return Err(json_err(
                    StatusCode::BAD_REQUEST,
                    "cannot read request body",
                ));
            }
        }
    }
    String::from_utf8(body)
        .map_err(|_| json_err(StatusCode::BAD_REQUEST, "request body is not valid UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::{ManagementService, Route, resolve_route};
    use http::{HeaderMap, HeaderValue, StatusCode};
    use std::sync::Arc;
    use wanaku_infra::credentials::CredentialBroker;
    use wanaku_infra::registry::InMemoryRegistry;
    use wanaku_server::management::WanakuManagementService;
    use wanaku_types::credentials::ResolverRegistry;
    use wanaku_types::feature::HttpContext;

    pub(super) fn service() -> ManagementService {
        let registry = InMemoryRegistry::new();
        let broker = Arc::new(CredentialBroker::new(ResolverRegistry::new()));
        ManagementService::new(
            WanakuManagementService::new(registry.clone(), Arc::clone(&broker), None, Vec::new()),
            super::Lifecycle::new(registry),
            broker,
            None,
        )
    }

    #[test]
    fn routing_keeps_rhaf_namespace_and_methods_separate() {
        for path in [
            "/api/rhaf",
            "/api/rhaf/v2/forwards",
            "/api/rhaf/v1/forwards/a/b",
            "/api/rhaf/v1/forwards/",
        ] {
            assert_eq!(resolve_route("GET", path), Route::NotFound);
        }
        assert_eq!(
            resolve_route("PATCH", "/api/rhaf/v1/forwards/a"),
            Route::MethodNotAllowed("GET, PUT, DELETE")
        );
        assert_eq!(resolve_route("GET", "/api/v1/forwards"), Route::Wanaku);
        assert_eq!(resolve_route("GET", "/api/rhaf-extra"), Route::Wanaku);
    }

    #[tokio::test]
    async fn headers_cannot_turn_lifecycle_request_into_raw_mutation() -> anyhow::Result<()> {
        let service = service();
        let mut headers = HeaderMap::new();
        headers.insert("x-rhaf-internal", HeaderValue::from_static("true"));
        headers.insert("x-rhaf-bypass-guard", HeaderValue::from_static("true"));
        let response = service
            .dispatch(&HttpContext::new(
                "POST",
                "/api/rhaf/v1/forwards",
                None,
                Some("{}"),
                &headers,
            ))
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let body: rhaf_api::models::ErrorResponse = serde_json::from_slice(response.body())?;
        assert!(!body.error.is_empty());
        let response = service
            .dispatch(&HttpContext::new(
                "GET",
                "/api/v1/forwards",
                None,
                None,
                &headers,
            ))
            .await;
        let body: serde_json::Value = serde_json::from_slice(response.body())?;
        assert_eq!(body["data"], serde_json::json!([]));
        Ok(())
    }

    #[tokio::test]
    async fn schema_head_preserves_get_headers_without_body() {
        let service = service();
        let headers = HeaderMap::new();
        let path = "/api/rhaf/v1/openapi.json";
        let get = service
            .dispatch(&HttpContext::new("GET", path, None, None, &headers))
            .await;
        let head = service
            .dispatch(&HttpContext::new("HEAD", path, None, None, &headers))
            .await;
        assert_eq!(get.status(), StatusCode::OK);
        assert_eq!(head.headers(), get.headers());
        assert!(head.body().is_empty());
        assert!(!get.body().is_empty());
    }

    #[tokio::test]
    async fn reserved_routes_return_safe_errors_and_method_allow_headers() -> anyhow::Result<()> {
        let service = service();
        let headers = HeaderMap::new();
        for (method, path, status, allowed) in [
            ("POST", "/api/rhaf/v1/forwards", 400, None),
            ("GET", "/api/rhaf/v1/forwards/registered", 404, None),
            ("PUT", "/api/rhaf/v1/forwards/registered", 400, None),
            ("DELETE", "/api/rhaf/v1/forwards/registered", 501, None),
            ("GET", "/api/rhaf/v1/catalog", 501, None),
            ("GET", "/api/rhaf/v1/unknown", 404, None),
            ("DELETE", "/api/rhaf/v1/forwards", 405, Some("GET, POST")),
            (
                "POST",
                "/api/rhaf/v1/forwards/registered",
                405,
                Some("GET, PUT, DELETE"),
            ),
            ("POST", "/api/rhaf/v1/catalog", 405, Some("GET")),
            ("POST", "/api/rhaf/v1/openapi.json", 405, Some("GET, HEAD")),
        ] {
            let response = service
                .dispatch(&HttpContext::new(method, path, None, None, &headers))
                .await;
            assert_eq!(response.status().as_u16(), status, "{method} {path}");
            assert_error_response(&response, allowed)?;
        }
        Ok(())
    }

    fn assert_error_response(
        response: &http::Response<Vec<u8>>,
        allowed: Option<&str>,
    ) -> anyhow::Result<()> {
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["content-type"], "application/json");
        assert_eq!(
            response
                .headers()
                .get("allow")
                .map(HeaderValue::to_str)
                .transpose()?,
            allowed
        );
        let body: serde_json::Value = serde_json::from_slice(response.body())?;
        assert_eq!(body.get("data"), Some(&serde_json::Value::Null));
        assert!(
            body.get("error")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|error| !error.is_empty())
        );
        Ok(())
    }

    #[tokio::test]
    async fn rhaf_and_raw_openapi_documents_stay_separate() -> anyhow::Result<()> {
        let service = service();
        let headers = HeaderMap::new();
        for (path, own_route, other_route) in [
            (
                "/api/rhaf/v1/openapi.json",
                "/api/rhaf/v1/forwards",
                "/api/v1/forwards",
            ),
            ("/openapi.json", "/api/v1/forwards", "/api/rhaf/v1/forwards"),
        ] {
            let response = service
                .dispatch(&HttpContext::new("GET", path, None, None, &headers))
                .await;
            assert_eq!(response.status(), StatusCode::OK);
            let schema: serde_json::Value = serde_json::from_slice(response.body())?;
            assert!(schema["paths"].get(own_route).is_some());
            assert!(schema["paths"].get(other_route).is_none());
        }
        Ok(())
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use http::{HeaderMap, StatusCode};
    use wanaku_types::feature::HttpContext;

    use super::tests::service;

    #[tokio::test]
    async fn raw_publication_and_namespace_mutations_respect_reservations() -> anyhow::Result<()> {
        let service = service();
        let lifecycle = &service.forwards.lifecycle;
        lifecycle.reserve("managed", "owned").await?;
        let headers = HeaderMap::new();
        for (method, path, body) in [
            (
                "POST",
                "/api/v1/forwards",
                Some(r#"{"name":"managed","address":"http://127.0.0.1"}"#),
            ),
            (
                "POST",
                "/api/v1/forwards/",
                Some(r#"{"name":"other","address":"http://127.0.0.1","namespace":"owned-all"}"#),
            ),
            ("POST", "/api/v1/forwards/managed/refreshes", None),
            ("DELETE", "/api/v1/forwards/managed", None),
            (
                "PUT",
                "/api/v1/namespaces/owned-progressive",
                Some(r#"{"name":"owned-progressive"}"#),
            ),
            ("DELETE", "/api/v1/namespaces/owned-all", None),
        ] {
            let response = service
                .dispatch(&HttpContext::new(method, path, None, body, &headers))
                .await;
            assert_eq!(response.status(), StatusCode::CONFLICT, "{method} {path}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn raw_namespace_creation_still_accepts_unmanaged_records() {
        let service = service();
        let headers = HeaderMap::new();
        let response = service
            .dispatch(&HttpContext::new(
                "POST",
                "/api/v1/namespaces",
                None,
                Some(r#"{"name":"unmanaged"}"#),
                &headers,
            ))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
}
