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
