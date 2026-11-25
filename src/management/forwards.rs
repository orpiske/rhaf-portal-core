use std::sync::Arc;
use std::time::Duration;

use http::{Response, StatusCode};
use percent_encoding::percent_decode_str;
use rhaf_api::models::{Forward, ForwardRequest};
use rhaf_portal_core::lifecycle::{Lifecycle, LifecycleError, Registration, validate_discovery};
use wanaku_infra::credentials::{CredentialAuditSink, CredentialBroker};
use wanaku_infra::registry::{ForwardDiscoveryRequest, ForwardPublicationError, InMemoryRegistry};
use wanaku_server::management::{
    PreparedForwardUpdate, apply_forward_update, prepare_forward_update,
};
use wanaku_types::credentials::{CredentialPurpose, binding::NormalizedOrigin};
use wanaku_types::http_response::{json_err, json_ok};
use wanaku_types::persistence::PersistenceError;
use wanaku_types::registry::{ForwardEntry, ForwardRegistry, validate_namespace_name};

const PERSISTENCE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub(super) struct ForwardService {
    pub lifecycle: Lifecycle,
    pub broker: Arc<CredentialBroker>,
    pub audit_sink: Option<Arc<dyn CredentialAuditSink>>,
}

impl ForwardService {
    pub(super) async fn dispatch(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Response<Vec<u8>> {
        let name = path
            .strip_prefix("/api/rhaf/v1/forwards/")
            .map(|segment| percent_decode_str(segment).decode_utf8())
            .transpose();
        let Ok(name) = name else {
            return json_err(StatusCode::BAD_REQUEST, "forward name is not valid UTF-8");
        };
        let name = name.as_deref();
        match (method, name) {
            ("GET", None) => {
                let views = self.lifecycle.commit(Registration::views).await;
                match views {
                    Ok(views) => json_ok(&serde_json::json!(views)),
                    Err(error) => lifecycle_error(&error),
                }
            }
            ("GET", Some(name)) => match self.read(name).await {
                Ok(view) => json_ok(&serde_json::json!(view)),
                Err(error) => error.response(),
            },
            ("POST", None) | ("PUT", Some(_)) => {
                self.dispatch_write(body.unwrap_or_default(), name).await
            }
            _ => json_err(
                StatusCode::NOT_IMPLEMENTED,
                "RHAF lifecycle operation is not available",
            ),
        }
    }

    async fn dispatch_write(&self, body: &str, name: Option<&str>) -> Response<Vec<u8>> {
        let Ok(request) = serde_json::from_str::<ForwardRequest>(body) else {
            return json_err(StatusCode::BAD_REQUEST, "invalid forward request");
        };
        if let Err(message) = validate_request(&request, name) {
            return json_err(StatusCode::BAD_REQUEST, message);
        }
        let service = self.clone();
        let replace = name.is_some();
        // Finish a reservation even if the HTTP client disconnects.
        match tokio::spawn(async move { Box::pin(service.write(request, replace)).await }).await {
            Ok(Ok((status, view))) => {
                let mut response = json_ok(&serde_json::json!(view));
                *response.status_mut() = status;
                response
            }
            Ok(Err(error)) => error.response(),
            Err(_) => json_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "forward update interrupted",
            ),
        }
    }

    async fn read(&self, name: &str) -> Result<Forward, WriteError> {
        self.lifecycle
            .commit(|registry| {
                let registration =
                    Registration::lookup(registry, name)?.ok_or(WriteError::Missing)?;
                Ok(registration.view(registry)?)
            })
            .await
    }

    async fn write(
        &self,
        request: ForwardRequest,
        replace: bool,
    ) -> Result<(StatusCode, Forward), WriteError> {
        let (registration, discovery) = Box::pin(self.begin_write(request, replace)).await?;
        let target = Box::new(discovery.target().clone());
        let prepared = Box::pin(self.prepare(discovery)).await;

        let _commit = self.lifecycle.commit_guard().await;
        let registry = self.lifecycle.registry();
        let status = publish(registry, &registration, &target, *prepared, replace)?;
        wait_for_persistence(registry.clone()).await?;
        Ok((status, registration.view(registry)?))
    }

    async fn prepare(&self, discovery: ForwardDiscoveryRequest) -> Box<PreparedForwardUpdate> {
        Box::new(prepare_forward_update(&self.broker, self.audit_sink.as_deref(), discovery).await)
    }
    async fn begin_write(
        &self,
        request: ForwardRequest,
        replace: bool,
    ) -> Result<(Registration, ForwardDiscoveryRequest), WriteError> {
        let (base, candidate) = discovery_target(request);
        if replace {
            let _commit = self.lifecycle.commit_guard().await;
            return begin_replacement(self.lifecycle.registry(), &base, candidate);
        }
        let registration = self.lifecycle.reserve(&candidate.name, &base).await?;
        let _commit = self.lifecycle.commit_guard().await;
        let discovery =
            begin_discovery(self.lifecycle.registry(), &registration, candidate, false)?;
        Ok((registration, discovery))
    }
}

fn publish(
    registry: &InMemoryRegistry,
    registration: &Registration,
    target: &ForwardEntry,
    prepared: PreparedForwardUpdate,
    replace: bool,
) -> Result<StatusCode, WriteError> {
    validate_discovery(registry, target)?;
    match apply_forward_update(registry, prepared) {
        Ok(_) if replace => Ok(StatusCode::OK),
        Ok(_) => {
            registration.mark_active(registry)?;
            Ok(StatusCode::CREATED)
        }
        Err(ForwardPublicationError::Stale(_)) if !replace => {
            registration.mark_failed(registry)?;
            Ok(StatusCode::ACCEPTED)
        }
        Err(error) => Err(error.into()),
    }
}

fn discovery_target(request: ForwardRequest) -> (String, ForwardEntry) {
    let candidate = ForwardEntry {
        name: request.name,
        address: request.address,
        namespace: None,
        server_info: None,
        labels: request.labels.into_iter().collect(),
        available: false,
        status_message: None,
        credential_bindings: [
            (
                CredentialPurpose::Discovery,
                request.credential_bindings.discovery,
            ),
            (
                CredentialPurpose::Invocation,
                request.credential_bindings.invocation,
            ),
        ]
        .into_iter()
        .filter_map(|(purpose, binding)| binding.map(|id| (purpose, id)))
        .collect(),
    };
    (request.namespace_base, candidate)
}

fn begin_replacement(
    registry: &InMemoryRegistry,
    base: &str,
    candidate: ForwardEntry,
) -> Result<(Registration, ForwardDiscoveryRequest), WriteError> {
    let registration =
        Registration::lookup(registry, &candidate.name)?.ok_or(WriteError::Missing)?;
    if registration.base() != base || registry.get_forward(&candidate.name).is_none() {
        return Err(LifecycleError::Conflict.into());
    }
    let discovery = begin_discovery(registry, &registration, candidate, true)?;
    Ok((registration, discovery))
}

fn begin_discovery(
    registry: &InMemoryRegistry,
    registration: &Registration,
    mut candidate: ForwardEntry,
    replace: bool,
) -> Result<ForwardDiscoveryRequest, WriteError> {
    registration.apply_to_forward(&mut candidate)?;
    validate_discovery(registry, &candidate)?;
    Ok(registry.begin_forward_registration(candidate, replace)?)
}

fn validate_request(request: &ForwardRequest, path_name: Option<&str>) -> Result<(), &'static str> {
    if request.name.is_empty() || path_name.is_some_and(|name| name != request.name) {
        return Err("forward name is invalid or does not match the path");
    }
    if request.namespace_base.len() > 51
        || validate_namespace_name(&request.namespace_base).is_err()
    {
        return Err("namespaceBase is invalid");
    }
    if !valid_address(&request.address) {
        return Err("address must be an absolute HTTP(S) URL without credentials");
    }
    if request
        .labels
        .keys()
        .any(|key| rhaf_portal_core::lifecycle::is_lifecycle_label(key))
    {
        return Err("labels contain a reserved lifecycle key");
    }
    let bindings = &request.credential_bindings;
    if [
        bindings.discovery.as_deref(),
        bindings.invocation.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|id| id.trim().is_empty())
    {
        return Err("credential binding identifiers must be nonempty");
    }
    Ok(())
}

fn valid_address(address: &str) -> bool {
    let authority = address
        .split_once("://")
        .and_then(|(_, rest)| rest.split(['/', '?', '#']).next());
    authority.is_some_and(|authority| !authority.contains('@'))
        && NormalizedOrigin::from_address(address).is_ok()
}

async fn wait_for_persistence(registry: InMemoryRegistry) -> Result<(), WriteError> {
    match tokio::task::spawn_blocking(move || registry.flush_persistence(PERSISTENCE_TIMEOUT)).await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(PersistenceError::Coordination(message)))
            if message == "timed out waiting for registry persistence" =>
        {
            Err(WriteError::Timeout)
        }
        _ => Err(WriteError::Persistence),
    }
}

#[derive(Debug, thiserror::Error)]
enum WriteError {
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
    #[error(transparent)]
    Publication(#[from] ForwardPublicationError),
    #[error("forward registration not found")]
    Missing,
    #[error("registry persistence failed")]
    Persistence,
    #[error("registry persistence timed out")]
    Timeout,
}

impl WriteError {
    fn response(&self) -> Response<Vec<u8>> {
        match self {
            Self::Lifecycle(error) => lifecycle_error(error),
            Self::Publication(ForwardPublicationError::ReplacementFailed(_)) => {
                json_err(StatusCode::BAD_GATEWAY, "replacement discovery failed")
            }
            Self::Publication(_) => json_err(StatusCode::CONFLICT, "forward publication conflict"),
            Self::Missing => json_err(StatusCode::NOT_FOUND, "forward registration not found"),
            Self::Persistence => json_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "registry persistence failed",
            ),
            Self::Timeout => json_err(
                StatusCode::GATEWAY_TIMEOUT,
                "registry persistence timed out",
            ),
        }
    }
}

fn lifecycle_error(error: &LifecycleError) -> Response<Vec<u8>> {
    match error {
        LifecycleError::Conflict => json_err(
            StatusCode::CONFLICT,
            "forward or namespace ownership conflict",
        ),
        LifecycleError::Registry(_) => json_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "cannot write lifecycle metadata",
        ),
        _ => json_err(StatusCode::BAD_REQUEST, &error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use http::{HeaderMap, StatusCode};
    use rhaf_api::models::{Forward, Response};
    use wanaku_types::feature::HttpContext;

    use super::super::tests::service;

    #[tokio::test]
    async fn lifecycle_item_reads_decode_the_name_as_one_segment() -> anyhow::Result<()> {
        let service = service();
        let name = "managed/é +";
        service.forwards.lifecycle.reserve(name, "owned").await?;
        let response = service
            .dispatch(&HttpContext::new(
                "GET",
                "/api/rhaf/v1/forwards/managed%2F%C3%A9%20+",
                None,
                None,
                &HeaderMap::new(),
            ))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body: Response<Forward> = serde_json::from_slice(response.body())?;
        assert_eq!(body.data.name, name);
        assert_eq!(body.data.available, None);
        Ok(())
    }
}
