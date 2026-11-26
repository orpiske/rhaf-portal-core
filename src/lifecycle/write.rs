use std::time::Duration;

use rhaf_api::models::{Forward, ForwardRequest};
use wanaku_infra::credentials::{CredentialAuditSink, CredentialBroker};
use wanaku_infra::registry::{ForwardDiscoveryRequest, ForwardPublicationError, InMemoryRegistry};
use wanaku_server::management::{
    PreparedForwardUpdate, apply_forward_update, prepare_forward_update,
};
use wanaku_types::credentials::{CredentialPurpose, binding::NormalizedOrigin};
use wanaku_types::persistence::PersistenceError;
use wanaku_types::registry::{ForwardEntry, ForwardRegistry, validate_namespace_name};

use super::{Lifecycle, LifecycleError, Registration, validate_discovery};

const PERSISTENCE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    Create,
    Replace,
}

pub enum WriteOutcome {
    Created,
    Replaced,
    CleanupPending,
}

impl Lifecycle {
    pub async fn list(&self) -> Result<Vec<Forward>, LifecycleError> {
        self.commit(Registration::views).await
    }

    pub async fn read(&self, name: &str) -> Result<Forward, WriteError> {
        self.commit(|registry| {
            let registration = Registration::lookup(registry, name)?.ok_or(WriteError::Missing)?;
            Ok(registration.view(registry)?)
        })
        .await
    }

    pub async fn write(
        &self,
        request: ForwardRequest,
        mode: WriteMode,
        broker: &CredentialBroker,
        audit_sink: Option<&dyn CredentialAuditSink>,
    ) -> Result<(WriteOutcome, Forward), WriteError> {
        validate_request(&request).map_err(WriteError::InvalidRequest)?;
        let (registration, discovery) = Box::pin(self.begin_write(request, mode)).await?;
        let target = Box::new(discovery.target().clone());
        let prepared = Box::pin(prepare(broker, audit_sink, discovery)).await;

        // Keep publication and the response view together while waiting for the registry flush.
        let _commit = self.commit_guard().await;
        let registry = self.registry();
        let outcome = publish(registry, &registration, &target, *prepared, mode)?;
        wait_for_persistence(registry.clone()).await?;
        Ok((outcome, registration.view(registry)?))
    }

    async fn begin_write(
        &self,
        request: ForwardRequest,
        mode: WriteMode,
    ) -> Result<(Registration, ForwardDiscoveryRequest), WriteError> {
        let (base, candidate) = discovery_target(request);
        if mode == WriteMode::Replace {
            let _commit = self.commit_guard().await;
            return begin_replacement(self.registry(), &base, candidate);
        }
        let registration = self.reserve(&candidate.name, &base).await?;
        let _commit = self.commit_guard().await;
        let discovery = begin_discovery(self.registry(), &registration, candidate, false)?;
        Ok((registration, discovery))
    }
}

async fn prepare(
    broker: &CredentialBroker,
    audit_sink: Option<&dyn CredentialAuditSink>,
    discovery: ForwardDiscoveryRequest,
) -> Box<PreparedForwardUpdate> {
    Box::new(prepare_forward_update(broker, audit_sink, discovery).await)
}

fn publish(
    registry: &InMemoryRegistry,
    registration: &Registration,
    target: &ForwardEntry,
    prepared: PreparedForwardUpdate,
    mode: WriteMode,
) -> Result<WriteOutcome, WriteError> {
    validate_discovery(registry, target)?;
    match (mode, apply_forward_update(registry, prepared)) {
        (WriteMode::Replace, Ok(_)) => Ok(WriteOutcome::Replaced),
        (WriteMode::Create, Ok(_)) => {
            registration.mark_active(registry)?;
            Ok(WriteOutcome::Created)
        }
        (WriteMode::Create, Err(ForwardPublicationError::Stale(_))) => {
            registration.mark_failed(registry)?;
            Ok(WriteOutcome::CleanupPending)
        }
        (_, Err(error)) => Err(error.into()),
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

fn validate_request(request: &ForwardRequest) -> Result<(), &'static str> {
    if request.name.is_empty() {
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
        .any(|key| super::is_lifecycle_label(key))
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
pub enum WriteError {
    #[error("{0}")]
    InvalidRequest(&'static str),
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
