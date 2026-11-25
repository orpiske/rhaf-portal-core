use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use praxis_core::PingoraServerRuntime;
use praxis_core::config::Config;
use praxis_protocol::Protocol as _;
use praxis_protocol::http::PingoraHttp;
use rhaf_portal_core::lifecycle::Lifecycle;
use tracing::info;
use wanaku_server::app::App;

use crate::reconnect::Reconnect;
use crate::startup::Args;

pub fn run(
    args: &Args,
    pipeline: &Config,
    app: App,
    listener: std::net::TcpListener,
) -> Result<()> {
    let registry = app.registry.clone();
    let mut server = PingoraServerRuntime::new(pipeline);
    let _cert_shutdowns = Box::new(PingoraHttp)
        .register(&mut server, pipeline, &app.pipelines)
        .context("cannot register MCP listener")?;
    let lifecycle = Lifecycle::new(registry.clone());
    register_background_services(&app, &mut server, lifecycle.clone());
    let management = crate::management::ManagementService::new(
        app.management,
        lifecycle,
        Arc::clone(&app.broker),
        Some(Arc::clone(&app.audit_sink)),
    );
    let management = crate::listeners::BoundManagement::new(listener, management)?;
    server.server_mut().add_service(management);
    info!(mcp = %args.mcp_listen, management = %args.management_listen,
        version = crate::VERSION, "starting RHAF Portal Core");
    server.run_with_args(pingora_core::server::RunArgs::default());
    registry
        .flush_persistence(Duration::from_secs(10))
        .map_err(|_| anyhow::anyhow!("registry shutdown flush failed"))?;
    drop(app.pipelines);
    drop(app.audit_sink);
    drop(app.audit_store);
    Ok(())
}

fn register_background_services(
    app: &App,
    server: &mut PingoraServerRuntime,
    lifecycle: Lifecycle,
) {
    let persistence = wanaku_server::management::registry_persistence_service(app.registry.clone());
    let persistence = pingora_core::services::background::GenBackgroundService::new(
        "rhaf-registry-persistence".to_owned(),
        persistence,
    );
    server.server_mut().add_service(persistence);
    if let Some(interval) = wanaku_types::config::ENV.forward_healthcheck_interval {
        let reconnect = Arc::new(Reconnect {
            lifecycle,
            broker: Arc::clone(&app.broker),
            audit_sink: Arc::clone(&app.audit_sink),
            interval,
        });
        let reconnect = pingora_core::services::background::GenBackgroundService::new(
            "rhaf-forward-reconnect".to_owned(),
            reconnect,
        );
        server.server_mut().add_service(reconnect);
    }
}
