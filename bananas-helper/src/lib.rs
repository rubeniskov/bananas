//! Wire protocol + client for bananas-helper.
//!
//! Newline-delimited JSON over a Unix domain socket. One request per
//! connection; the server writes a single Response line and closes.
//! Keeps things simple to audit — no streaming, no multiplexing.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

/// Commands the unprivileged HTTP frontend can ask the root helper to
/// perform. New variants must be added carefully — each one is a privilege
/// escalation path. Validate args inside the helper, never trust the
/// caller's framing alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Atomically replace /etc/exports with `content`, then run
    /// `exportfs -rv` to reload. The helper validates that `content`
    /// parses as a sane exports file before writing.
    WriteExports { content: String },
    /// Run `exportfs -rv` without changing /etc/exports.
    ReloadExports,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    /// Combined stdout+stderr from any command run, for display in the UI.
    pub output: String,
    /// Set when ok=false. Display verbatim; the helper redacts any
    /// sensitive tail before sending.
    pub error: Option<String>,
}

impl Response {
    pub fn ok(output: impl Into<String>) -> Self {
        Self { ok: true, output: output.into(), error: None }
    }

    pub fn err(msg: impl Into<String>, output: impl Into<String>) -> Self {
        Self { ok: false, output: output.into(), error: Some(msg.into()) }
    }
}

/// Connect to the helper, send `cmd`, read one Response line, return it.
pub async fn call(socket: &Path, cmd: &Command) -> Result<Response> {
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connecting to helper at {}", socket.display()))?;
    let (read_half, mut write_half) = stream.into_split();
    let mut req = serde_json::to_vec(cmd).context("serializing command")?;
    req.push(b'\n');
    write_half.write_all(&req).await.context("writing request")?;
    write_half.shutdown().await.ok();

    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    reader.read_line(&mut line).await.context("reading response")?;
    serde_json::from_str(line.trim()).context("parsing response")
}
