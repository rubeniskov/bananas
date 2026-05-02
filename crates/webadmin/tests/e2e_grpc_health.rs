//! gRPC-Web round-trip smoke test for the Health service mounted
//! at `/api/grpc/*` on webadmin's public TCP face.
//!
//! Sends a hand-crafted gRPC-Web frame over plain HTTP/1.1
//! (POST /api/grpc/bananas.health.v1.HealthService/Check) — the
//! same wire shape the wasm SPA's `tonic-web-wasm-client` would
//! produce. tonic-web translates the frame into native gRPC,
//! tonic dispatches to `webadmin::grpc::HealthSvc::check`, and
//! the response comes back as gRPC-Web framed bytes.
//!
//! This proves the full PR-1 stack:
//!   browser → /api/grpc → tonic-web → tonic → HealthSvc.

#![cfg(not(target_arch = "wasm32"))]

use anyhow::Result;
use bananas_devtool::Harness;
use prost::Message;

#[tokio::test]
async fn health_check_roundtrip_via_grpc_web() -> Result<()> {
    let h = Harness::new().await?;
    let origin = h.origin();
    // No login needed — the Health RPC is public (PR-1 hosts no
    // auth interceptor on the gRPC stack yet; auth metadata
    // arrives via cookies in PR-5).
    let http = h.http();

    // Encode the CheckRequest body. The message is an empty
    // proto3, so its encoded length is 0.
    let req_msg = bananas_proto::health::v1::CheckRequest::default();
    let mut req_bytes = Vec::with_capacity(5);
    let req_len = req_msg.encoded_len() as u32;
    // gRPC-Web frame header: 1-byte flag + 4-byte big-endian length.
    req_bytes.push(0x00); // flag = 0 (uncompressed data frame)
    req_bytes.extend_from_slice(&req_len.to_be_bytes());
    req_msg.encode(&mut req_bytes)?;

    let url = format!("{origin}/api/grpc/bananas.health.v1.HealthService/Check");
    let resp = http
        .post(&url)
        // tonic-web accepts both `application/grpc-web` and
        // `application/grpc-web+proto`. The latter is what the
        // wasm client emits.
        .header("content-type", "application/grpc-web+proto")
        .header("accept", "application/grpc-web+proto")
        .header("x-grpc-web", "1")
        .body(req_bytes)
        .send()
        .await?;
    assert!(
        resp.status().is_success(),
        "gRPC-Web Check returned {}: {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );

    let body = resp.bytes().await?.to_vec();
    // Response = data frame (5-byte header + encoded message)
    // followed by a trailers frame (flag=0x80, len=N, N bytes
    // of "grpc-status:0\r\n…").
    assert!(body.len() > 5, "response too short: {} bytes", body.len());
    let data_len = u32::from_be_bytes([body[1], body[2], body[3], body[4]]) as usize;
    let data = &body[5..5 + data_len];
    let parsed = bananas_proto::health::v1::CheckResponse::decode(data)?;
    assert_eq!(parsed.daemon, "bananas-webadmin");
    assert_eq!(parsed.version, env!("CARGO_PKG_VERSION"));
    Ok(())
}
