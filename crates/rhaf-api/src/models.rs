#![expect(
    clippy::large_stack_frames,
    reason = "utoipa generates schema builders with large stack frames; these run during schema generation"
)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Successful responses contain data and a null error. Errors contain null data.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Response<T> {
    #[schema(required = true, nullable = true)]
    pub data: Option<T>,
    #[schema(required = true, nullable = true)]
    pub error: Option<String>,
}

/// Desired configuration only. Runtime state and internal labels are not accepted.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForwardRequest {
    /// Immutable name. A replacement body must match the path name.
    #[schema(min_length = 1)]
    pub name: String,
    /// Absolute HTTP(S) MCP endpoint. Do not include embedded credentials.
    #[schema(format = "uri", pattern = "^https?://[^/?#@]+([/?#].*)?$")]
    pub address: String,
    /// Immutable base. Core derives <base>-all and <base>-progressive.
    #[schema(
        min_length = 1,
        max_length = 51,
        pattern = "^[a-z0-9]([a-z0-9-]{0,49}[a-z0-9])?$"
    )]
    pub namespace_base: String,
    /// User labels only. Reserved RHAF lifecycle labels are rejected.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub credential_bindings: CredentialBindings,
}

/// References to configured binding identifiers. Secret values are never accepted here.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialBindings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(min_length = 1)]
    pub discovery: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(min_length = 1)]
    pub invocation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum LifecycleState {
    Registering,
    Active,
    FailedRegistrationCleanup,
    Deleting,
    Deleted,
}

/// Lifecycle state is independent of upstream reachability and progressive repair.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Forward {
    pub name: String,
    pub address: String,
    pub namespace_base: String,
    pub conventional_namespace: String,
    pub progressive_namespace: String,
    /// Opaque read-only identity. Stable through replacement; changes after recreation.
    #[schema(read_only, min_length = 1)]
    pub registration_id: String,
    pub labels: BTreeMap<String, String>,
    pub credential_bindings: CredentialBindings,
    pub state: LifecycleState,
    /// Null means availability has not been determined. It does not mean false.
    #[schema(required = true, nullable = true)]
    pub available: Option<bool>,
    /// Safe diagnostic text. Does not contain secrets or internal labels.
    #[schema(required = true, nullable = true)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum CatalogStatus {
    Loaded,
    Absent,
    Invalid,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Catalog {
    pub status: CatalogStatus,
    /// Empty when status is absent or invalid. Product IDs must be unique.
    pub products: Vec<Product>,
    #[schema(required = true, nullable = true)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Product {
    /// Unique within the catalog.
    #[schema(min_length = 1)]
    pub id: String,
    #[schema(min_length = 1)]
    pub display_name: String,
    #[serde(default)]
    pub links: Vec<ProductLink>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProductLink {
    #[schema(min_length = 1)]
    pub label: String,
    /// Absolute HTTP(S) URL without embedded credentials. Core does not probe it.
    #[schema(format = "uri", pattern = "^https?://[^/?#@]+([/?#].*)?$")]
    pub url: String,
}
