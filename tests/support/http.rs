use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use reqwest::{Client, Method, header::HeaderMap};
use serde_json::{Value, json};

pub struct Http {
    client: Client,
}

pub struct Reply {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Value,
}

impl Http {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(15))
                .build()?,
        })
    }

    pub async fn ready(&self, url: &str) -> bool {
        self.client
            .get(url)
            .timeout(Duration::from_secs(1))
            .send()
            .await
            .is_ok_and(|response| response.status().as_u16() == 200)
    }

    pub fn request<'a>(
        &'a self,
        method: Method,
        url: &'a str,
        body: Option<&'a Value>,
    ) -> impl std::future::Future<Output = Result<Reply>> + 'a {
        Box::pin(async move {
            let mut request = self
                .client
                .request(method, url)
                .header("Accept", "application/json, text/event-stream")
                .header("MCP-Protocol-Version", "2025-03-26");
            if let Some(body) = body {
                request = request.json(body);
            }
            let response = Box::pin(request.send())
                .await
                .with_context(|| format!("request failed: {url}"))?;
            let status = response.status().as_u16();
            let headers = response.headers().clone();
            let bytes = response.bytes().await?;
            let body = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes)
                    .with_context(|| format!("invalid JSON response: {url}"))?
            };
            Ok(Reply {
                status,
                headers,
                body,
            })
        })
    }

    pub fn rpc<'a>(
        &'a self,
        url: &'a str,
        method: &'a str,
        params: Option<Value>,
        id: Option<u64>,
    ) -> impl std::future::Future<Output = Result<Reply>> + 'a {
        Box::pin(async move {
            let mut body = json!({"jsonrpc": "2.0", "method": method});
            if let Some(id) = id {
                body["id"] = json!(id);
            }
            if let Some(params) = params {
                body["params"] = params;
            }
            self.request(Method::POST, url, Some(&body))
                .await?
                .rpc_result(id)
        })
    }

    pub async fn initialize(&self, url: &str, name: &str) -> Result<()> {
        let reply = self
            .rpc(
                url,
                "initialize",
                Some(json!({
                    "protocolVersion": "2025-03-26", "capabilities": {},
                    "clientInfo": {"name": name, "version": "1"},
                })),
                Some(1),
            )
            .await?;
        ensure!(
            reply.body["protocolVersion"] == "2025-03-26",
            "unexpected MCP protocol"
        );
        ensure!(
            !reply.headers.contains_key("Mcp-Session-Id"),
            "expected stateless downstream MCP"
        );
        self.rpc(url, "notifications/initialized", None, None)
            .await?;
        let ping = self.rpc(url, "ping", None, Some(1)).await?;
        ensure!(ping.body == json!({}), "MCP ping failed");
        Ok(())
    }

    pub async fn tools(&self, url: &str) -> Result<BTreeSet<String>> {
        let reply = self.rpc(url, "tools/list", None, Some(1)).await?;
        names(&reply.body["tools"])
    }

    pub async fn call(&self, url: &str, name: &str, arguments: Value) -> Result<Value> {
        let reply = self
            .rpc(
                url,
                "tools/call",
                Some(json!({"name": name, "arguments": arguments})),
                Some(1),
            )
            .await?;
        ensure!(
            reply.body.get("isError").and_then(Value::as_bool) != Some(true),
            "tool failed: {}",
            reply.body
        );
        let text = reply.body["content"][0]["text"]
            .as_str()
            .context("tool result text missing")?;
        serde_json::from_str(text).context("tool result is not JSON")
    }
}

impl Reply {
    fn rpc_result(mut self, id: Option<u64>) -> Result<Self> {
        let Some(id) = id else {
            ensure!(
                self.status == 202,
                "notification failed: {} {}",
                self.status,
                self.body
            );
            return Ok(self);
        };
        ensure!(
            self.status == 200,
            "MCP HTTP failure: {} {}",
            self.status,
            self.body
        );
        ensure!(
            self.body["id"] == id,
            "MCP response ID changed: {}",
            self.body
        );
        ensure!(self.body.get("error").is_none(), "MCP error: {}", self.body);
        self.body = self
            .body
            .get("result")
            .cloned()
            .context("MCP result missing")?;
        Ok(self)
    }
}

pub fn names(value: &Value) -> Result<BTreeSet<String>> {
    value
        .as_array()
        .context("tool array missing")?
        .iter()
        .map(|entry| {
            entry["name"]
                .as_str()
                .map(str::to_owned)
                .context("tool name missing")
        })
        .collect()
}
