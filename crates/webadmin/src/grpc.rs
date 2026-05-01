//! gRPC service implementations served on the public TCP face.
//!
//! Webadmin is the entry point for every browser-side gRPC-Web
//! call. The SPA hits `/api/grpc/<package.Service>/<Method>`, and
//! `tonic_web::GrpcWebLayer` translates that into a normal gRPC
//! request that flows through the standard tonic service router.
//!
//! At PR-1 we host a single trivial `HealthService::Check` RPC —
//! the SPA's gRPC-Web round-trip probe. Real plugin services
//! (cloud, stats, …) move in subsequent PRs and will live in
//! their own daemons; webadmin keeps just the host-level RPCs
//! (auth, version, system, …) once the rest are migrated.

use bananas_proto::health::v1::{
    CheckRequest, CheckResponse,
    health_service_server::{HealthService, HealthServiceServer},
};
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

/// Build the tower service that handles every gRPC + gRPC-Web
/// request. Returned as an axum-compatible `Router` so the public
/// app can mount it at `/api/grpc/*`. The `GrpcWebLayer` translates
/// browser-issued gRPC-Web frames to native gRPC; native HTTP/2
/// gRPC also works (handy for `grpcurl`).
pub fn build_grpc_router() -> tonic::service::Routes {
    tonic::service::Routes::new(HealthServiceServer::new(HealthSvc))
}
