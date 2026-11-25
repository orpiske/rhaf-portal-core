use anyhow::{Context, Result, ensure};
use reqwest::Method;
use serde_json::{Value, json};
use wanaku_types::persistence::RegistrySnapshot;

use super::{Harness, http::Reply};

const COLLECTION: &str = "/api/rhaf/v1/forwards";
const ITEM: &str = "/api/rhaf/v1/forwards/managed-fixture";

impl Harness {
    pub async fn verify_unavailable_registration(&self) -> Result<()> {
        let request = lifecycle_request(&self.management_url("/api/rhaf/v1/unknown"));
        let mut invalid = request.clone();
        invalid["available"] = json!(true);
        let rejected = self
            .lifecycle_request(Method::POST, COLLECTION, Some(&invalid))
            .await?;
        ensure!(
            rejected.status == 400,
            "client runtime state accepted: {}",
            rejected.body
        );
        let created = self.create_lifecycle_forward(&request).await?;
        ensure!(
            created["available"] == false,
            "unavailable forward was not retained"
        );
        self.verify_lifecycle_reads(&created).await
    }

    pub async fn verify_lifecycle_replacement(&self) -> Result<()> {
        let mut request = lifecycle_request(&self.fixture_url("/mcp"));
        let created = self.create_lifecycle_forward(&request).await?;
        ensure!(created["available"] == true);
        let mcp = format!("http://{}/managed-all/mcp", self.mcp);
        let tools = self.http.tools(&mcp).await?;
        ensure!(tools.contains("read_state") && tools.contains("set_state"));

        self.verify_failed_replacement(&request, &created).await?;
        ensure!(self.http.tools(&mcp).await? == tools);

        request["labels"]["version"] = json!("replacement");
        let replaced = self
            .lifecycle_request(Method::PUT, ITEM, Some(&request))
            .await?;
        ensure!(
            replaced.status == 200,
            "replacement failed: {}",
            replaced.body
        );
        let replaced = &replaced.body["data"];
        ensure!(replaced["registrationId"] == created["registrationId"]);
        ensure!(replaced["labels"] == request["labels"]);
        ensure!(replaced["available"] == true);
        self.verify_lifecycle_persistence(replaced)?;
        self.verify_lifecycle_reads(replaced).await
    }

    async fn verify_failed_replacement(&self, request: &Value, original: &Value) -> Result<()> {
        let mut replacement = request.clone();
        replacement["namespaceBase"] = json!("different");
        let response = self
            .lifecycle_request(Method::PUT, ITEM, Some(&replacement))
            .await?;
        ensure!(
            response.status == 409,
            "namespace base changed: {}",
            response.body
        );

        replacement["namespaceBase"] = request["namespaceBase"].clone();
        replacement["address"] = json!(self.management_url("/api/rhaf/v1/unknown"));
        replacement["labels"] = json!({"version": "failed"});
        let response = self
            .lifecycle_request(Method::PUT, ITEM, Some(&replacement))
            .await?;
        ensure!(
            response.status == 502,
            "unexpected discovery failure response: {}",
            response.body
        );
        ensure!(response.body["data"].is_null() && response.body["error"].is_string());
        self.verify_lifecycle_reads(original).await?;
        self.verify_lifecycle_persistence(original)
    }

    async fn create_lifecycle_forward(&self, request: &Value) -> Result<Value> {
        let created = self
            .lifecycle_request(Method::POST, COLLECTION, Some(request))
            .await?;
        ensure!(created.status == 201, "creation failed: {}", created.body);
        let forward = created.body["data"].clone();
        ensure!(created.body["error"].is_null());
        ensure!(forward["name"] == request["name"]);
        ensure!(forward["address"] == request["address"]);
        ensure!(forward["namespaceBase"] == "managed");
        ensure!(forward["conventionalNamespace"] == "managed-all");
        ensure!(forward["progressiveNamespace"] == "managed-progressive");
        ensure!(forward["state"] == "active");
        ensure!(
            forward["registrationId"]
                .as_str()
                .is_some_and(|id| !id.is_empty())
        );
        ensure!(forward["labels"] == request["labels"]);
        // Read persistent state before issuing any other requests or stopping Core.
        self.verify_lifecycle_persistence(&forward)?;

        let duplicate = self
            .lifecycle_request(Method::POST, COLLECTION, Some(request))
            .await?;
        ensure!(
            duplicate.status == 409,
            "duplicate creation: {}",
            duplicate.body
        );
        Ok(forward)
    }

    async fn verify_lifecycle_reads(&self, expected: &Value) -> Result<()> {
        let item = self.lifecycle_request(Method::GET, ITEM, None).await?;
        ensure!(item.status == 200 && item.body["data"] == *expected);
        let list = self
            .lifecycle_request(Method::GET, COLLECTION, None)
            .await?;
        ensure!(list.status == 200 && list.body["data"] == json!([expected]));
        Ok(())
    }

    fn verify_lifecycle_persistence(&self, expected: &Value) -> Result<()> {
        let bytes = std::fs::read(self.data_dir().join("registry.json"))?;
        let snapshot: RegistrySnapshot = serde_json::from_slice(&bytes)?;
        let persisted = snapshot
            .forwards
            .iter()
            .find(|entry| entry.name == "managed-fixture")
            .context("forward missing from persistent state")?;
        ensure!(persisted.address == expected["address"]);
        ensure!(
            persisted.labels.get("version").map(String::as_str)
                == expected["labels"]["version"].as_str()
        );
        for name in ["managed-all", "managed-progressive"] {
            ensure!(snapshot.namespaces.iter().any(|entry| entry.name == name));
        }
        Ok(())
    }

    async fn lifecycle_request(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Reply> {
        self.http
            .request(method, &self.management_url(path), body)
            .await
    }
}

fn lifecycle_request(address: &str) -> Value {
    json!({
        "name": "managed-fixture", "address": address, "namespaceBase": "managed",
        "labels": {"version": "initial"},
    })
}
