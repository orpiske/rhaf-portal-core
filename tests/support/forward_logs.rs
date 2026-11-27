use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use reqwest::Method;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use super::{ExitPolicy, Harness, Running};

const FORWARD: &str = "/api/v1/forwards/log-probe";
const LOG: &str = "forward-discovery.log";
const MARKER: &str = "synthetic-forward-credential";

impl Harness {
    pub async fn verify_forward_url_redaction(&self) -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let target = format!("/mcp/probe?token={MARKER}&tag=a%2Fb");
        let address = format!("http://{}{target}", listener.local_addr()?);
        let (requests, mut targets) = mpsc::unbounded_channel();
        let failure = Arc::new(AtomicBool::new(false));
        let mode = Arc::clone(&failure);
        let fixture = tokio::spawn(async move { serve(listener, requests, &mode).await });

        let result = self
            .exercise_forward_url_redaction(&address, &target, &failure, &mut targets)
            .await;
        fixture.abort();
        if let Ok(result) = fixture.await {
            result?;
        }
        result
    }

    async fn exercise_forward_url_redaction(
        &self,
        address: &str,
        target: &str,
        failure: &AtomicBool,
        targets: &mut mpsc::UnboundedReceiver<String>,
    ) -> Result<()> {
        let core = self.start_log_probe(address).await?;
        self.verify_forward_available(true).await?;
        verify_targets(targets, target, "startup discovery")?;

        let refresh = self.management_url(&format!("{FORWARD}/refreshes"));
        for (available, phase) in [(true, "successful refresh"), (false, "failed refresh")] {
            failure.store(!available, Ordering::Relaxed);
            let reply = self.http.request(Method::POST, &refresh, None).await?;
            ensure!(reply.status == 200, "forward refresh failed to publish");
            self.verify_forward_available(available).await?;
            verify_targets(targets, target, phase)?;
        }

        core.stop().await?;
        self.verify_forward_logs()
    }

    fn verify_forward_logs(&self) -> Result<()> {
        let output = std::fs::read_to_string(self.evidence.join(LOG))?;
        ensure!(
            output.contains("discovering from forward")
                && output.contains("connected to MCP server for forward discovery")
                && output.contains("published forward with status message"),
            "expected startup, debug, and failure diagnostics in {LOG}"
        );
        ensure!(!output.contains(MARKER), "query credential leaked in {LOG}");
        Ok(())
    }

    async fn start_log_probe(&self, address: &str) -> Result<Running> {
        std::fs::write(
            self.config(),
            format!("forwards:\n  - name: log-probe\n    address: {address}\n"),
        )?;
        let mut command = self.core_command(&self.config(), &self.data_dir());
        command.env(
            "RUST_LOG",
            "info,wanaku_infra::mcp_client=debug,wanaku_server=debug",
        );
        self.start(
            command,
            LOG,
            &self.management_url("/api/v1/tools"),
            ExitPolicy::Core,
        )
        .await
    }

    async fn verify_forward_available(&self, available: bool) -> Result<()> {
        let reply = self
            .http
            .request(Method::GET, &self.management_url(FORWARD), None)
            .await?;
        ensure!(
            reply.status == 200,
            "forward lookup failed with {}",
            reply.status
        );
        assert_eq!(reply.body["data"]["available"], available);
        Ok(())
    }
}

fn verify_targets(
    targets: &mut mpsc::UnboundedReceiver<String>,
    expected: &str,
    phase: &str,
) -> Result<()> {
    let mut received = false;
    while let Ok(target) = targets.try_recv() {
        assert_eq!(target, expected, "request URL changed during {phase}");
        received = true;
    }
    ensure!(received, "no upstream request during {phase}");
    Ok(())
}

async fn serve(
    listener: TcpListener,
    requests: mpsc::UnboundedSender<String>,
    failure: &AtomicBool,
) -> Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        tokio::time::timeout(
            Duration::from_secs(5),
            respond(stream, &requests, failure.load(Ordering::Relaxed)),
        )
        .await??;
    }
}

// Core uses HTTP/1.1 with a content length for JSON-RPC requests. Close each
// response so the fixture only needs to handle one request per connection.
async fn respond(
    stream: TcpStream,
    requests: &mpsc::UnboundedSender<String>,
    failure: bool,
) -> Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let target = line
        .split_whitespace()
        .nth(1)
        .context("request target missing")?;
    requests.send(target.to_owned())?;
    let length = content_length(&mut reader, &mut line).await?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await?;
    let rpc = serde_json::from_slice::<Value>(&body)?;
    let (status, body) = fixture_reply(&rpc, failure)?;
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    reader.get_mut().write_all(response.as_bytes()).await?;
    Ok(())
}

async fn content_length(reader: &mut BufReader<TcpStream>, line: &mut String) -> Result<usize> {
    let mut length = None;
    loop {
        line.clear();
        ensure!(
            reader.read_line(line).await? > 0,
            "incomplete request headers"
        );
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = Some(value.trim().parse()?);
        }
    }

    length.context("request content length missing")
}

fn fixture_reply(rpc: &Value, failure: bool) -> Result<(&'static str, String)> {
    if failure {
        return Ok(("502 Bad Gateway", json!({"error": MARKER}).to_string()));
    }
    let result = match rpc["method"].as_str() {
        Some("initialize") => json!({
            "protocolVersion": rpc["params"]["protocolVersion"],
            "serverInfo": {"name": "forward-log-probe", "version": "1"},
            "capabilities": {"tools": {}, "resources": {}, "prompts": {}},
        }),
        Some("tools/list") => json!({"tools": []}),
        Some("resources/list") => json!({"resources": []}),
        Some("resources/templates/list") => json!({"resourceTemplates": []}),
        Some("prompts/list") => json!({"prompts": []}),
        Some("notifications/initialized") => return Ok(("202 Accepted", String::new())),
        method => anyhow::bail!("unexpected fixture request: {method:?}"),
    };
    Ok((
        "200 OK",
        json!({"jsonrpc": "2.0", "id": rpc["id"], "result": result}).to_string(),
    ))
}
