use std::sync::Arc;

use http::{Response, StatusCode};
use percent_encoding::percent_decode_str;
use rhaf_api::models::{Forward, ForwardRequest};
use rhaf_portal_core::lifecycle::{Lifecycle, LifecycleError, WriteError, WriteMode, WriteOutcome};
use wanaku_infra::credentials::{CredentialAuditSink, CredentialBroker};
use wanaku_infra::registry::ForwardPublicationError;
use wanaku_types::http_response::{json_err, json_ok};

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
                let views = self.lifecycle.list().await;
                match views {
                    Ok(views) => json_ok(&serde_json::json!(views)),
                    Err(error) => lifecycle_error(&error),
                }
            }
            ("GET", Some(name)) => match self.lifecycle.read(name).await {
                Ok(view) => json_ok(&serde_json::json!(view)),
                Err(error) => write_error(&error),
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
        if name.is_some_and(|name| name != request.name) {
            return json_err(
                StatusCode::BAD_REQUEST,
                "forward name is invalid or does not match the path",
            );
        }
        let service = self.clone();
        let mode = if name.is_some() {
            WriteMode::Replace
        } else {
            WriteMode::Create
        };
        // Finish a reservation even if the HTTP client disconnects.
        let result = tokio::spawn(async move {
            Box::pin(service.lifecycle.write(
                request,
                mode,
                &service.broker,
                service.audit_sink.as_deref(),
            ))
            .await
        })
        .await;
        write_response(result)
    }
}

fn write_response(
    result: Result<Result<(WriteOutcome, Forward), WriteError>, tokio::task::JoinError>,
) -> Response<Vec<u8>> {
    match result {
        Ok(Ok((outcome, view))) => {
            let mut response = json_ok(&serde_json::json!(view));
            *response.status_mut() = match outcome {
                WriteOutcome::Created => StatusCode::CREATED,
                WriteOutcome::Replaced => StatusCode::OK,
                WriteOutcome::CleanupPending => StatusCode::ACCEPTED,
            };
            response
        }
        Ok(Err(error)) => write_error(&error),
        Err(_) => json_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "forward update interrupted",
        ),
    }
}

fn write_error(error: &WriteError) -> Response<Vec<u8>> {
    match error {
        WriteError::Lifecycle(error) => lifecycle_error(error),
        WriteError::Publication(ForwardPublicationError::ReplacementFailed(_)) => {
            json_err(StatusCode::BAD_GATEWAY, "replacement discovery failed")
        }
        WriteError::Publication(_) => {
            json_err(StatusCode::CONFLICT, "forward publication conflict")
        }
        WriteError::InvalidRequest(message) => json_err(StatusCode::BAD_REQUEST, message),
        WriteError::Missing => json_err(StatusCode::NOT_FOUND, "forward registration not found"),
        WriteError::Persistence => json_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "registry persistence failed",
        ),
        WriteError::Timeout => json_err(
            StatusCode::GATEWAY_TIMEOUT,
            "registry persistence timed out",
        ),
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
    async fn invalid_address_does_not_reserve_the_registration() -> anyhow::Result<()> {
        let service = service();
        let response = service
            .forwards
            .dispatch_write(
                r#"{"name":"managed","namespaceBase":"owned","address":"file:///invalid"}"#,
                None,
            )
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        service
            .forwards
            .lifecycle
            .reserve("managed", "owned")
            .await?;
        Ok(())
    }

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
