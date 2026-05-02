//! Generated tonic + prost types for every BanaNAS service.
//! `build.rs` compiles every `.proto` under `proto/` and writes
//! the resulting Rust to `$OUT_DIR/<package>.rs`. We `include!`
//! each one under a Rust-friendly module path here.
//!
//! Daemons import the server-side traits (e.g.
//! `bananas_proto::health::v1::health_service_server::*`); the
//! wasm SPA imports the client stubs (e.g.
//! `bananas_proto::health::v1::health_service_client::HealthServiceClient`).
//! Both come out of the same `include!`.

#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

pub mod health {
    pub mod v1 {
        // Generated file is named after the proto package: `bananas.health.v1`
        // → `bananas.health.v1.rs`. We pull it in under `bananas::health::v1`
        // so consumers write `bananas_proto::health::v1::HealthService…`.
        include!(concat!(env!("OUT_DIR"), "/bananas.health.v1.rs"));
    }
}

pub mod stats {
    pub mod v1 {
        include!(concat!(env!("OUT_DIR"), "/bananas.stats.v1.rs"));
    }
}

pub mod cloud {
    pub mod v1 {
        include!(concat!(env!("OUT_DIR"), "/bananas.cloud.v1.rs"));
    }
}

pub mod engine {
    pub mod v1 {
        include!(concat!(env!("OUT_DIR"), "/bananas.engine.v1.rs"));
    }
}

/// Tiny shared helper for daemons that need to dial bananas-engine's
/// gRPC Unix socket. Keeps every plugin daemon from re-implementing
/// the tonic + tower service-fn + tokio Unix connector dance.
///
/// Server-side only: the wasm SPA leaves the `server` feature off
/// because `tonic::transport` and `hyper-util` don't build for
/// wasm32-unknown-unknown.
#[cfg(feature = "server")]
pub mod engine_client {
    use std::path::{Path, PathBuf};

    use tonic::transport::{Channel, Endpoint, Uri};
    use tower::service_fn;

    /// Build a tonic Channel that dials the engine's gRPC Unix
    /// socket. The URI is a placeholder — the custom connector
    /// ignores it and opens a UnixStream instead. Tonic still
    /// requires *some* URI to instantiate the Endpoint.
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
}
