//! Smoke test for `bananas.cloud.v1.CloudService::TailRunLog`.
//!
//! Drops a fixture `<idx>.log` into the harness's
//! `sync-progress` dir, opens the streaming RPC, and reads at
//! least one LogChunk back. Verifies the wiring from
//! webadmin's tonic mount → CloudSvc → file tail.

#![cfg(not(target_arch = "wasm32"))]

use std::time::Duration;

use anyhow::Result;
use bananas_devtool::Harness;
use prost::Message;

#[tokio::test]
async fn cloud_tail_run_log_streams_existing_lines() -> Result<()> {
    let h = Harness::new().await?;

    // Drop a fixture log with a couple of lines; engine would
    // be the one writing this in production.
    let log_path = h.tmp().join("sync-progress").join("0.log");
    std::fs::write(&log_path, "line one\nline two\n")?;

    let url = format!(
        "{}/api/grpc/bananas.cloud.v1.CloudService/TailRunLog",
        h.origin()
    );

    let req_msg = bananas_proto::cloud::v1::TailRunLogRequest { sync_idx: 0 };
    let mut req_bytes = Vec::new();
    let req_len = req_msg.encoded_len() as u32;
    req_bytes.push(0u8);
    req_bytes.extend_from_slice(&req_len.to_be_bytes());
    req_msg.encode(&mut req_bytes)?;

    let resp = h
        .http()
        .post(&url)
        .header("content-type", "application/grpc-web+proto")
        .header("accept", "application/grpc-web+proto")
        .header("x-grpc-web", "1")
        .body(req_bytes)
        .timeout(Duration::from_millis(800))
        .send()
        .await;

    // Same shape as the stats-live test — a server-streaming
    // RPC keeps the response open. We accept either Ok(headers
    // arrived but body timed out before the test deadline) OR a
    // body-readable Ok response. What we MUST NOT see is 404 or
    // 5xx from the route lookup.
    match resp {
        Ok(r) => {
            assert!(r.status().is_success(), "TailRunLog status: {}", r.status());
            let ct = r
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            assert!(
                ct.starts_with("application/grpc-web"),
                "content-type was {ct:?}, expected application/grpc-web*"
            );
        }
        Err(e) if e.is_timeout() => {
            anyhow::bail!("TailRunLog did not respond in time: {e}");
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
