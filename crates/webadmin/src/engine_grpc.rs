//! Tonic client wrapper that talks to bananas-engine's gRPC Unix
//! socket (`BANANAS_ENGINE_GRPC_SOCKET`, default
//! `/run/bananas/engine-grpc.sock`).
//!
//! Engine still serves the legacy newline-JSON socket too — the
//! gRPC migration is per-RPC. PR-4 covers `Authenticate` only;
//! every other call in webadmin keeps using
//! `bananas_engine::call(&Command::…)` until its RPC migrates.

use std::path::{Path, PathBuf};

use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

/// Build a tonic Channel that dials the engine gRPC Unix socket.
/// The URI is a placeholder — the custom connector ignores it and
/// opens a UnixStream instead. Tonic still requires *some* URI to
/// instantiate the Endpoint.
pub async fn channel(socket: &Path) -> Result<Channel, tonic::transport::Error> {
    let path: PathBuf = socket.to_path_buf();
    Endpoint::try_from("http://[::]:50051")
        .expect("static placeholder URI parses")
        .connect_with_connector(service_fn(move |_: Uri| {
            let p = path.clone();
            async move {
                let stream = tokio::net::UnixStream::connect(p).await?;
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(stream))
            }
        }))
        .await
}
