use anyhow::{Result, ensure};
use reqwest::Method;
use serde_json::json;

use super::Harness;

impl Harness {
    pub async fn verify_management_wrapper(&self) -> Result<()> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(15))
            .build()?;
        self.verify_rhaf_schema(&client).await?;
        self.verify_rhaf_dispatch(&client).await?;
        self.verify_raw_delegation().await?;
        self.verify_management_body_errors(&client).await
    }

    async fn verify_rhaf_schema(&self, client: &reqwest::Client) -> Result<()> {
        let response = client
            .get(self.management_url("/api/rhaf/v1/openapi.json"))
            .send()
            .await?
            .error_for_status()?;
        ensure!(response.headers()["cache-control"] == "no-store");
        let schema: serde_json::Value = response.json().await?;
        ensure!(schema["paths"]["/api/rhaf/v1/forwards"].is_object());
        Ok(())
    }

    async fn verify_rhaf_dispatch(&self, client: &reqwest::Client) -> Result<()> {
        // The RHAF boundary must not delegate unavailable operations as raw mutations.
        let response = client
            .post(self.management_url("/api/rhaf/v1/forwards"))
            .header("x-rhaf-internal", "true")
            .header("x-rhaf-bypass", "true")
            .json(&json!({"name": "not-created"}))
            .send()
            .await?;
        ensure!(response.status() == 501);
        ensure!(response.headers()["cache-control"] == "no-store");
        let body: serde_json::Value = response.json().await?;
        ensure!(body["data"].is_null() && body["error"].is_string());
        let missing = self
            .http
            .request(
                Method::GET,
                &self.management_url("/api/rhaf/v1/unknown"),
                None,
            )
            .await?;
        ensure!(
            missing.status == 404
                && missing.body["data"].is_null()
                && missing.body["error"].is_string()
        );
        Ok(())
    }

    async fn verify_raw_delegation(&self) -> Result<()> {
        let created = self
            .http
            .request(
                Method::POST,
                &self.management_url("/api/v1/namespaces"),
                Some(&json!({"name": "wrapper-shared", "description": "shared state"})),
            )
            .await?;
        ensure!(created.status == 200, "namespace create: {}", created.body);
        let read = self
            .http
            .request(
                Method::GET,
                &self.management_url("/api/v1/namespaces/wrapper-shared"),
                None,
            )
            .await?;
        ensure!(
            read.status == 200 && read.body["data"]["name"] == "wrapper-shared",
            "delegation did not retain registry state: {}",
            read.body
        );
        let mcp = format!("http://{}/wrapper-shared/mcp", self.mcp);
        self.http.initialize(&mcp, "management-wrapper").await?;
        ensure!(self.http.tools(&mcp).await?.is_empty());
        Ok(())
    }

    async fn verify_management_body_errors(&self, client: &reqwest::Client) -> Result<()> {
        for (body, status) in [(vec![0xff], 400), (vec![b' '; 1_048_577], 413)] {
            let response = client
                .post(self.management_url("/api/v1/namespaces"))
                .body(body)
                .send()
                .await?;
            ensure!(response.status().as_u16() == status);
            ensure!(response.headers()["cache-control"] == "no-store");
            let body: rhaf_api::models::ErrorResponse = response.json().await?;
            ensure!(!body.error.is_empty());
        }
        Ok(())
    }
}
