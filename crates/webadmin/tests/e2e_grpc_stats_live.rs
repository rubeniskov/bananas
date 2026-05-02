//! Smoke test for the StatsService::Live server-streaming RPC.
//!
//! Verifies the RPC is registered + reachable at
//! `/api/grpc/bananas.stats.v1.StatsService/Live` and the response
//! comes back as gRPC-Web — i.e. the wiring from
//! webadmin's tonic mount → StatsSvc → LiveBus is intact.
//!
//! End-to-end streaming verification (actual snapshot frames
//! flowing) requires a real `bananas-stats` collector to be
//! pumping the live Unix socket; that path gets exercised by the
//! browser-driven SPA at deploy time. Reliable cross-process
//! socket fakes for that kind of test are too timing-sensitive
//! for a unit-style harness and would just produce flakes.

#![cfg(not(target_arch = "wasm32"))]

use std::time::Duration;

use anyhow::Result;
use bananas_devtool::Harness;
use prost::Message;

#[tokio::test]
async fn stats_live_rpc_is_reachable() -> Result<()> {
    let h = Harness::new().await?;
    let url = format!("{}/api/grpc/bananas.stats.v1.StatsService/Live", h.origin());

    let req_msg = bananas_proto::stats::v1::LiveRequest::default();
    let mut req_bytes = Vec::new();
    let req_len = req_msg.encoded_len() as u32;
    req_bytes.push(0u8);
    req_bytes.extend_from_slice(&req_len.to_be_bytes());
    req_msg.encode(&mut req_bytes)?;

    // Drop the request after a short read window — the RPC will
    // try to keep the stream open, but we only need to confirm
    // the response started. No real `bananas-stats` is running in
    // the harness, so the bus has nothing to broadcast and the
    // stream would idle.
    let resp = tokio::time::timeout(
        Duration::from_secs(3),
        h.http()
            .post(&url)
            .header("content-type", "application/grpc-web+proto")
            .header("accept", "application/grpc-web+proto")
            .header("x-grpc-web", "1")
            .body(req_bytes)
            .timeout(Duration::from_millis(500))
            .send(),
    )
    .await?;

    // Either (a) we got a successful streaming response that
    // tonic-web is keeping open — reqwest's per-request timeout
    // surfaces as `is_timeout` AFTER the headers have arrived; or
    // (b) we got an immediate 200 response. Both paths prove the
    // tonic mount is alive and StatsSvc is reachable. What we
    // must NOT see is 404 (route missing) or 5xx (handler bad).
    match resp {
        Ok(r) => {
            let status = r.status();
            let ct = r
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            assert!(status.is_success(), "live status: {status}");
            assert!(
                ct.starts_with("application/grpc-web"),
                "content-type was {ct:?}, expected application/grpc-web*"
            );
        }
        Err(e) if e.is_timeout() => {
            // The request itself timed out before headers — that
            // would indicate tonic-web isn't responding at all.
            // (When headers DID arrive but the body hangs,
            // reqwest's body reader is the one that timed out,
            // and that surfaces inside Ok(r) above as a chunk
            // error, not as a top-level timeout.)
            anyhow::bail!("StatsService::Live did not respond within 500 ms: {e}");
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
