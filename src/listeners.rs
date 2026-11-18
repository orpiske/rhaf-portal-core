use std::net::TcpListener;
use std::os::fd::IntoRawFd;

use anyhow::{Context, Result};
use pingora_core::server::{ListenFds, ShutdownWatch};
use pingora_core::services::Service;
use wanaku_server::management::WanakuManagementService;

pub struct BoundManagement {
    service: pingora_core::services::listening::Service<WanakuManagementService>,
    listener: Option<TcpListener>,
    address: String,
}

impl BoundManagement {
    pub fn new(listener: TcpListener, management: WanakuManagementService) -> Result<Self> {
        listener
            .set_nonblocking(true)
            .context("cannot configure management listener")?;
        let address = listener.local_addr()?.to_string();
        let mut service = pingora_core::services::listening::Service::new(
            "rhaf-management".to_owned(),
            management,
        );
        service.add_tcp(&address);
        Ok(Self {
            service,
            listener: Some(listener),
            address,
        })
    }
}

#[async_trait::async_trait]
impl Service for BoundManagement {
    async fn start_service(
        &mut self,
        fds: Option<ListenFds>,
        shutdown: ShutdownWatch,
        listeners_per_fd: usize,
    ) {
        let Some(fds) = fds else {
            tracing::error!("management listener requires Pingora's descriptor table");
            return;
        };
        if let Some(listener) = self.listener.take() {
            // Pingora adopts this owned descriptor instead of binding the address again.
            fds.lock().add(self.address.clone(), listener.into_raw_fd());
        }
        self.service
            .start_service(Some(fds), shutdown, listeners_per_fd)
            .await;
    }

    fn name(&self) -> &str {
        self.service.name()
    }

    fn is_listening(&self) -> bool {
        self.service.is_listening()
    }

    fn listen_addresses(&self) -> Option<Vec<String>> {
        self.service.listen_addresses()
    }
}
