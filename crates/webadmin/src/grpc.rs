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
//!
//! Subsequent PRs migrate the per-plugin RPCs to their own
//! daemons + add cross-daemon gRPC routing through bananas-router.

use std::pin::Pin;

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

/// Build the tower service that handles every gRPC + gRPC-Web
/// request. Returned as an axum-compatible `Router` so the public
/// app can mount it at `/api/grpc/*`. The `GrpcWebLayer` translates
/// browser-issued gRPC-Web frames to native gRPC; native HTTP/2
/// gRPC also works (handy for `grpcurl`).
pub fn build_grpc_router(live_bus: LiveBus) -> tonic::service::Routes {
    tonic::service::Routes::new(HealthServiceServer::new(HealthSvc))
        .add_service(StatsServiceServer::new(StatsSvc::new(live_bus)))
}
