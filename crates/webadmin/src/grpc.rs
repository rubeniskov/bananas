//! gRPC service implementations served on the public TCP face.
//!
//! Webadmin is the entry point for every browser-side gRPC-Web
//! call. The SPA hits `/api/grpc/<package.Service>/<Method>`, and
//! `tonic_web::GrpcWebLayer` translates that into a normal gRPC
//! request that flows through the standard tonic service router.
//!
//! Today this hosts:
//! - `bananas.health.v1.HealthService::Check` — round-trip probe.
//! - `bananas.stats.v1.StatsService::Live` — server-streaming
//!   snapshots, replacing the previous `/api/stats/live`
//!   WebSocket. Webadmin owns its own `LiveBus` that taps
//!   `/run/bananas/stats.sock` (the bananas-stats Unix pub/sub).
//! - `bananas.cloud.v1.CloudService::TailRunLog` — server-streaming
//!   tail of the per-line rclone output engine writes to
//!   `/run/bananas/sync-progress/<idx>.log` while a sync is
//!   running. Replaces the SPA's "show snapshot output once
//!   the run finishes" path with live updates.
//!
//! Subsequent PRs migrate the per-plugin RPCs to their own
//! daemons + add cross-daemon gRPC routing through bananas-router.

use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use bananas_proto::cloud::v1::{
    LogChunk, TailRunLogRequest,
    cloud_service_server::{CloudService, CloudServiceServer},
};
use bananas_proto::health::v1::{
    CheckRequest, CheckResponse,
    health_service_server::{HealthService, HealthServiceServer},
};
use bananas_proto::stats::v1::{
    LiveRequest, LiveSnapshot,
    stats_service_server::{StatsService, StatsServiceServer},
};
use bananas_stats::live_bus::LiveBus;
use futures_util::Stream;
use tokio::io::{AsyncBufReadExt, BufReader};
use tonic::{Request, Response, Status};

#[derive(Default, Clone)]
pub struct HealthSvc;

#[tonic::async_trait]
impl HealthService for HealthSvc {
    async fn check(&self, _req: Request<CheckRequest>) -> Result<Response<CheckResponse>, Status> {
        Ok(Response::new(CheckResponse {
            version: env!("CARGO_PKG_VERSION").to_string(),
            daemon: "bananas-webadmin".to_string(),
        }))
    }
}

#[derive(Clone)]
pub struct StatsSvc {
    bus: LiveBus,
}

impl StatsSvc {
    pub fn new(bus: LiveBus) -> Self {
        Self { bus }
    }
}

#[tonic::async_trait]
impl StatsService for StatsSvc {
    type LiveStream = Pin<Box<dyn Stream<Item = Result<LiveSnapshot, Status>> + Send + 'static>>;

    async fn live(&self, _req: Request<LiveRequest>) -> Result<Response<Self::LiveStream>, Status> {
        let mut rx = self.bus.subscribe();
        let stream = async_stream::stream! {
            loop {
                match rx.recv().await {
                    Ok(payload) => {
                        // The bus payload is already a JSON-encoded
                        // Snapshot; pass it through verbatim. PR-5
                        // will replace the JSON wrapper with a fully
                        // typed StatsSnapshot proto.
                        yield Ok(LiveSnapshot {
                            snapshot_json: payload.as_str().to_string(),
                        });
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // Slow consumer — surface as a stream error
                        // so the SPA reconnects rather than queuing
                        // stale data.
                        yield Err(Status::resource_exhausted(format!(
                            "live stream lagged by {n} messages; reconnect"
                        )));
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
        };
        Ok(Response::new(Box::pin(stream) as Self::LiveStream))
    }
}

/// Cloud service — currently TailRunLog only. Path to the
/// `<idx>.log` directory is configurable via
/// `BANANAS_SYNC_PROGRESS_DIR` so the e2e harness can drop a
/// fixture there. Defaults to `/run/bananas/sync-progress`,
/// matching what bananas-engine writes.
#[derive(Clone)]
pub struct CloudSvc {
    progress_dir: PathBuf,
}

impl CloudSvc {
    pub fn new(progress_dir: PathBuf) -> Self {
        Self { progress_dir }
    }
}

#[tonic::async_trait]
impl CloudService for CloudSvc {
    type TailRunLogStream = Pin<Box<dyn Stream<Item = Result<LogChunk, Status>> + Send + 'static>>;

    async fn tail_run_log(
        &self,
        req: Request<TailRunLogRequest>,
    ) -> Result<Response<Self::TailRunLogStream>, Status> {
        let idx = req.into_inner().sync_idx;
        let path = self.progress_dir.join(format!("{idx}.log"));

        // Empty stream when the file isn't there — finished runs
        // have already had their `<idx>.log` cleaned up by engine.
        // Callers fall back to the unary `output` field on the
        // already-fetched CloudJob.
        if !path.exists() {
            let empty = futures_util::stream::empty();
            return Ok(Response::new(Box::pin(empty) as Self::TailRunLogStream));
        }

        let stream = async_stream::stream! {
            // Open the file and read until EOF; on EOF we keep
            // re-opening with a small delay until the file
            // disappears (engine removes it on run completion).
            let mut emitted = 0usize;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(60 * 60);
            while tokio::time::Instant::now() < deadline {
                if !path.exists() {
                    break;
                }
                let f = match tokio::fs::File::open(&path).await {
                    Ok(f) => f,
                    Err(e) => {
                        yield Err(Status::not_found(format!("open log: {e}")));
                        return;
                    }
                };
                // Skip already-emitted bytes — the file is append-only
                // until engine removes it, so byte offset is a stable
                // resume point.
                let mut reader = BufReader::new(f);
                if emitted > 0 {
                    use tokio::io::AsyncSeekExt;
                    let _ = reader.seek(std::io::SeekFrom::Start(emitted as u64)).await;
                }
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Ok(0) => {
                            // EOF — wait a beat and re-poll.
                            tokio::time::sleep(Duration::from_millis(250)).await;
                            break;
                        }
                        Ok(n) => {
                            emitted += n;
                            yield Ok(LogChunk { text: line.clone() });
                        }
                        Err(e) => {
                            yield Err(Status::internal(format!("read log: {e}")));
                            return;
                        }
                    }
                }
            }
        };
        Ok(Response::new(Box::pin(stream) as Self::TailRunLogStream))
    }
}

/// Build the tower service that handles every gRPC + gRPC-Web
/// request. Returned as an axum-compatible `Router` so the public
/// app can mount it at `/api/grpc/*`. The `GrpcWebLayer` translates
/// browser-issued gRPC-Web frames to native gRPC; native HTTP/2
/// gRPC also works (handy for `grpcurl`).
pub fn build_grpc_router(live_bus: LiveBus, sync_progress_dir: PathBuf) -> tonic::service::Routes {
    tonic::service::Routes::new(HealthServiceServer::new(HealthSvc))
        .add_service(StatsServiceServer::new(StatsSvc::new(live_bus)))
        .add_service(CloudServiceServer::new(CloudSvc::new(sync_progress_dir)))
}
