//! Re-export of the shared `bananas_proto::engine_client` helper.
//!
//! Plugin daemons (cloud, exports, storage, users, dashboard,
//! stats) all use the same channel-builder for dialing the
//! engine's gRPC Unix socket; living in `bananas-proto` keeps
//! that one place. Webadmin's own callsites still write
//! `engine_grpc::channel(...)`, so this thin re-export keeps the
//! existing import paths working.

pub use bananas_proto::engine_client::channel;
