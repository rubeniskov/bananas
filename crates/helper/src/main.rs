//! bananas-helper daemon — root-privileged service that the unprivileged
//! HTTP frontend talks to over a Unix socket.

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{Context, Result};
use bananas_helper::{Command, Response};
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    process::Command as TokioCommand,
};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "bananas_helper=info".into()),
        )
        .init();

    let socket_path: PathBuf = std::env::var_os("BANANAS_HELPER_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/run/bananas-helper.sock".into());
    let exports_path: PathBuf = std::env::var_os("BANANAS_EXPORTS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/etc/exports".into());

    if socket_path.exists() {
        fs::remove_file(&socket_path).await.ok();
    }
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("binding {}", socket_path.display()))?;
    fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o660))
        .await
        .ok();

    tracing::info!(
        socket=%socket_path.display(),
        exports=%exports_path.display(),
        "bananas-helper listening"
    );

    loop {
        let (stream, _) = match listener.accept().await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error=%e, "accept failed");
                continue;
            }
        };
        let exports_path = exports_path.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, &exports_path).await {
                tracing::error!(error=%e, "request handler failed");
            }
        });
    }
}

async fn handle(stream: UnixStream, exports_path: &Path) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let response = match serde_json::from_str::<Command>(line.trim()) {
        Ok(cmd) => dispatch(cmd, exports_path).await,
        Err(e) => Response::err(format!("bad command: {e}"), String::new()),
    };

    let mut payload = serde_json::to_vec(&response)?;
    payload.push(b'\n');
    write_half.write_all(&payload).await?;
    Ok(())
}

async fn dispatch(cmd: Command, exports_path: &Path) -> Response {
    match cmd {
        Command::WriteExports { content } => match write_exports(&content, exports_path).await {
            Ok(out) => Response::ok(out),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::ReloadExports => match exportfs_reload().await {
            Ok(out) => Response::ok(out),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
    }
}

async fn write_exports(content: &str, exports_path: &Path) -> Result<String> {
    validate_exports(content)?;
    let dir = exports_path.parent().unwrap_or_else(|| Path::new("/"));
    let tmp = dir.join(format!(
        ".{}.tmp",
        exports_path.file_name().and_then(|s| s.to_str()).unwrap_or("exports")
    ));
    fs::write(&tmp, content)
        .await
        .with_context(|| format!("writing {}", tmp.display()))?;
    fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))
        .await
        .ok();
    fs::rename(&tmp, exports_path)
        .await
        .with_context(|| format!("renaming {} -> {}", tmp.display(), exports_path.display()))?;

    let reload = exportfs_reload().await.unwrap_or_else(|e| format!("(exportfs failed: {e})"));
    Ok(format!("wrote {} ({} bytes)\n{}", exports_path.display(), content.len(), reload))
}

/// Reject obviously malformed exports content before atomic-replacing the
/// file. The full validity check is delegated to exportfs which the daemon
/// runs immediately after.
fn validate_exports(content: &str) -> Result<()> {
    for (i, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // First whitespace-separated field is the export path; bail if it
        // doesn't start with '/' (no relative paths, no shell metacharacters
        // we forgot to escape).
        let first = line.split_whitespace().next().unwrap_or("").trim_matches('"');
        if !first.starts_with('/') {
            anyhow::bail!("line {}: export path must be absolute (got {first:?})", i + 1);
        }
    }
    Ok(())
}

async fn exportfs_reload() -> Result<String> {
    let out = TokioCommand::new("exportfs")
        .arg("-rv")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("spawning exportfs -rv")?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        anyhow::bail!("exportfs failed (status {}): {combined}", out.status);
    }
    Ok(combined)
}
