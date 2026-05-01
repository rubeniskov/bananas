//! gRPC variant of the auth integration tests. Mirrors
//! `tests/auth.rs` (which exercises the legacy newline-JSON
//! socket) but goes through the new `EngineService::Authenticate`
//! RPC mounted at `BANANAS_ENGINE_GRPC_SOCKET`.
//!
//! The pattern: spawn the bananas-engine binary as a subprocess
//! pointed at a tmpdir shadow, dial its gRPC Unix socket via a
//! tonic Channel that uses a custom Unix connector, and call
//! Authenticate.

use std::path::PathBuf;
use std::time::Duration;

use bananas_proto::engine::v1::{
    AuthenticateRequest, ChangeOwnPasswordRequest, engine_service_client::EngineServiceClient,
};
use tokio::process::{Child, Command as TokioCommand};
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

const ENGINE_BIN: &str = env!("CARGO_BIN_EXE_bananas-engine");

fn hash_password(password: &str) -> String {
    let params = sha_crypt::Sha512Params::new(sha_crypt::ROUNDS_DEFAULT)
        .expect("Sha512Params from default rounds");
    sha_crypt::sha512_simple(password, &params).expect("sha512_simple")
}

fn shadow_line(user: &str, password: &str) -> String {
    format!(
        "{user}:{hash}:20000:0:99999:7:::\n",
        hash = hash_password(password)
    )
}

struct Engine {
    _tmp: tempfile::TempDir,
    grpc_socket: PathBuf,
    child: Child,
}

impl Engine {
    async fn spawn(shadow_contents: &str) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let shadow = tmp.path().join("shadow");
        let socket = tmp.path().join("engine.sock");
        let grpc_socket = tmp.path().join("engine-grpc.sock");
        let exports = tmp.path().join("exports");
        std::fs::write(&shadow, shadow_contents).expect("write shadow");
        std::fs::write(&exports, "").expect("write exports");

        let child = TokioCommand::new(ENGINE_BIN)
            .env("BANANAS_ENGINE_SOCKET", &socket)
            .env("BANANAS_ENGINE_GRPC_SOCKET", &grpc_socket)
            .env("BANANAS_SHADOW_PATH", &shadow)
            .env("BANANAS_EXPORTS_PATH", &exports)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn engine");

        // Wait for the gRPC socket to bind. tonic doesn't have a
        // built-in retry; we just poll until the file appears.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if grpc_socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            grpc_socket.exists(),
            "engine never bound gRPC socket at {}",
            grpc_socket.display()
        );

        Self {
            _tmp: tmp,
            grpc_socket,
            child,
        }
    }

    async fn channel(&self) -> Channel {
        let path = self.grpc_socket.clone();
        // The URI is a placeholder — the connector ignores it and
        // dials the Unix socket. tonic still needs *some* URI to
        // satisfy `Endpoint`.
        Endpoint::try_from("http://[::]:50051")
            .unwrap()
            .connect_with_connector(service_fn(move |_: Uri| {
                let p = path.clone();
                async move {
                    let stream = tokio::net::UnixStream::connect(p).await?;
                    Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(stream))
                }
            }))
            .await
            .expect("tonic Channel over Unix")
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

#[tokio::test]
async fn grpc_authenticate_accepts_correct_password() {
    let shadow = shadow_line("root", "bananas-test");
    let engine = Engine::spawn(&shadow).await;
    let mut client = EngineServiceClient::new(engine.channel().await);

    let resp = client
        .authenticate(AuthenticateRequest {
            username: "root".into(),
            password: "bananas-test".into(),
        })
        .await
        .expect("RPC ok");
    let body = resp.into_inner();
    assert!(!body.lastchg_zero);
}

#[tokio::test]
async fn grpc_authenticate_rejects_wrong_password() {
    let shadow = shadow_line("root", "bananas-test");
    let engine = Engine::spawn(&shadow).await;
    let mut client = EngineServiceClient::new(engine.channel().await);

    let err = client
        .authenticate(AuthenticateRequest {
            username: "root".into(),
            password: "wrong".into(),
        })
        .await
        .expect_err("expected unauthenticated");
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
}

#[tokio::test]
async fn grpc_authenticate_surfaces_password_expired() {
    let hash = hash_password("bananas-test");
    let shadow = format!("root:{hash}:0:0:99999:7:::\n");
    let engine = Engine::spawn(&shadow).await;
    let mut client = EngineServiceClient::new(engine.channel().await);

    let resp = client
        .authenticate(AuthenticateRequest {
            username: "root".into(),
            password: "bananas-test".into(),
        })
        .await
        .expect("RPC ok");
    assert!(resp.into_inner().lastchg_zero);
}

#[tokio::test]
async fn grpc_change_own_password_rejects_wrong_old() {
    // Same redaction as `Authenticate`: a wrong old-password
    // surfaces as `Unauthenticated`. We can't exercise the
    // success path here because `change_own_password` shells out
    // to `/usr/sbin/chpasswd`, which isn't available (or wouldn't
    // be safe to run) inside `cargo test`. The success path is
    // covered by the e2e harness against a real systemd image.
    let shadow = shadow_line("root", "bananas-test");
    let engine = Engine::spawn(&shadow).await;
    let mut client = EngineServiceClient::new(engine.channel().await);

    let err = client
        .change_own_password(ChangeOwnPasswordRequest {
            username: "root".into(),
            old_password: "wrong".into(),
            new_password: "new-bananas-test".into(),
        })
        .await
        .expect_err("expected unauthenticated");
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
}
