//! Tonic-served gRPC half of the engine. Hosted on
//! `BANANAS_ENGINE_GRPC_SOCKET` (default
//! `/run/bananas/engine-grpc.sock`), alongside the legacy
//! newline-JSON socket at `BANANAS_ENGINE_SOCKET`. Migration
//! between them is per-command: each Command variant moves to
//! gRPC at its own pace, and during the transition both paths
//! work.

use std::path::PathBuf;

use bananas_proto::engine::v1::{
    AuthenticateRequest, AuthenticateResponse,
    engine_service_server::{EngineService, EngineServiceServer},
};
use tonic::{Request, Response, Status};

use crate::{authenticate, verify_shadow_password};

/// Engine gRPC service. Holds the same shadow path the
/// newline-JSON dispatch threads through `Cx`.
pub struct EngineGrpc {
    pub shadow_path: PathBuf,
}

#[tonic::async_trait]
impl EngineService for EngineGrpc {
    async fn authenticate(
        &self,
        req: Request<AuthenticateRequest>,
    ) -> Result<Response<AuthenticateResponse>, Status> {
        let body = req.into_inner();
        // Same flow as the newline-JSON `Command::Authenticate`:
        // verify the hash, check group membership, surface the
        // password_expired sentinel as a structured field. Errors
        // collapse into a single generic message — same redaction
        // policy as the legacy path.
        match authenticate(&body.username, &body.password, &self.shadow_path).await {
            Ok(()) => Ok(Response::new(AuthenticateResponse {
                lastchg_zero: false,
            })),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("password_expired") {
                    // Authenticated, but password rotation forced.
                    return Ok(Response::new(AuthenticateResponse { lastchg_zero: true }));
                }
                tracing::warn!(user=%body.username, error=%msg, "auth failed (gRPC)");
                Err(Status::unauthenticated("invalid credentials"))
            }
        }
    }
}

/// Bind the gRPC Unix socket and serve the EngineService. Called
/// from `main` as a separate `tokio::spawn` so the legacy
/// newline-JSON listener continues to run on the original socket.
pub async fn serve(grpc_socket: PathBuf, shadow_path: PathBuf) -> anyhow::Result<()> {
    if grpc_socket.exists() {
        let _ = std::fs::remove_file(&grpc_socket);
    }
    if let Some(parent) = grpc_socket.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let listener = tokio::net::UnixListener::bind(&grpc_socket)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&grpc_socket, std::fs::Permissions::from_mode(0o660))?;
    tracing::info!(socket=%grpc_socket.display(), "engine gRPC listening");

    let svc = EngineGrpc { shadow_path };
    let incoming = futures_util::stream::unfold(listener, |listener| async move {
        match listener.accept().await {
            Ok((stream, _addr)) => Some((Ok::<_, std::io::Error>(stream), listener)),
            Err(e) => {
                tracing::warn!(error=%e, "engine gRPC accept failed");
                Some((Err(e), listener))
            }
        }
    });

    tonic::transport::Server::builder()
        .add_service(EngineServiceServer::new(svc))
        .serve_with_incoming(incoming)
        .await?;
    Ok(())
}

// silence unused-import warning when bananas-engine is built
// with both the lib and bin: `verify_shadow_password` is used
// only when the gRPC mount migrates additional RPCs.
#[allow(dead_code)]
fn _keep_used() {
    let _ = verify_shadow_password;
}
