//! Integration tests for the engine's authentication path.
//!
//! Spawns `bananas-engine` as a subprocess pointed at a tmpdir
//! shadow file (via `BANANAS_SHADOW_PATH`), then drives it through
//! the public `Command::Authenticate` RPC. Proves that the auth
//! flow is exercisable without root privileges and without
//! touching the real `/etc/shadow` — the foundation the test
//! harness will lean on for full-stack `/api/login` coverage.
//!
//! Stays single-threaded inside this file: the engine binary
//! itself is spawned per test, so concurrency is fine across test
//! files. Each test writes its own shadow + uses its own socket.

use std::time::Duration;

use bananas_engine::{Command, call};
use tokio::process::{Child, Command as TokioCommand};

const ENGINE_BIN: &str = env!("CARGO_BIN_EXE_bananas-engine");

/// Hash a password for `/etc/shadow` (`$6$…` SHA-512). Produces a
/// random salt every call so two test runs of the same password
/// don't share a hash — matches what the real BanaNAS image does
/// via `mkpasswd -m sha-512`.
fn hash_password(password: &str) -> String {
    let params = sha_crypt::Sha512Params::new(sha_crypt::ROUNDS_DEFAULT)
        .expect("Sha512Params from default rounds");
    sha_crypt::sha512_simple(password, &params).expect("sha512_simple")
}

/// `<user>:<hash>:<lastchg>:0:99999:7:::` — minimal shadow row.
/// `lastchg = 20000` keeps the row "not zero", so authenticate()
/// won't trip the password_expired sentinel.
fn shadow_line(user: &str, password: &str) -> String {
    format!(
        "{user}:{hash}:20000:0:99999:7:::\n",
        hash = hash_password(password)
    )
}

struct Engine {
    _tmp: tempfile::TempDir,
    socket: std::path::PathBuf,
    child: Child,
}

impl Engine {
    /// Spawn an engine pointed at `<tmp>/etc/shadow`, with the
    /// supplied shadow lines pre-written. Block until the engine
    /// has bound its socket (poll up to 2 s).
    async fn spawn(shadow_contents: &str) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let shadow = tmp.path().join("shadow");
        let socket = tmp.path().join("engine.sock");
        let exports = tmp.path().join("exports");
        std::fs::write(&shadow, shadow_contents).expect("write shadow");
        std::fs::write(&exports, "").expect("write exports");

        let child = TokioCommand::new(ENGINE_BIN)
            .env("BANANAS_ENGINE_SOCKET", &socket)
            .env("BANANAS_SHADOW_PATH", &shadow)
            .env("BANANAS_EXPORTS_PATH", &exports)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn engine");

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(socket.exists(), "engine never bound {}", socket.display());

        Self {
            _tmp: tmp,
            socket,
            child,
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

#[tokio::test]
async fn authenticate_accepts_correct_password() {
    let shadow = shadow_line("root", "bananas-test");
    let engine = Engine::spawn(&shadow).await;

    let resp = call(
        &engine.socket,
        &Command::Authenticate {
            username: "root".into(),
            password: "bananas-test".into(),
        },
    )
    .await
    .expect("call");

    assert!(resp.ok, "expected ok, got {resp:?}");
    assert!(resp.output.contains("authenticated root"));
}

#[tokio::test]
async fn authenticate_rejects_wrong_password() {
    let shadow = shadow_line("root", "bananas-test");
    let engine = Engine::spawn(&shadow).await;

    let resp = call(
        &engine.socket,
        &Command::Authenticate {
            username: "root".into(),
            password: "wrong".into(),
        },
    )
    .await
    .expect("call");

    assert!(!resp.ok);
    assert_eq!(resp.error.as_deref(), Some("invalid credentials"));
}

#[tokio::test]
async fn authenticate_rejects_unknown_user() {
    // Shadow has no entry for `root` — user-not-found path.
    let engine = Engine::spawn("nobody:!:20000:0:99999:7:::\n").await;

    let resp = call(
        &engine.socket,
        &Command::Authenticate {
            username: "root".into(),
            password: "bananas-test".into(),
        },
    )
    .await
    .expect("call");

    assert!(!resp.ok);
    assert_eq!(resp.error.as_deref(), Some("invalid credentials"));
}

#[tokio::test]
async fn authenticate_surfaces_password_expired() {
    // lastchg=0 means `chage -d 0` was applied — surfaces as the
    // distinguishable "password_expired" sentinel so the UI can
    // pop the rotate-password form on first sign-in.
    let hash = hash_password("bananas-test");
    let shadow = format!("root:{hash}:0:0:99999:7:::\n");
    let engine = Engine::spawn(&shadow).await;

    let resp = call(
        &engine.socket,
        &Command::Authenticate {
            username: "root".into(),
            password: "bananas-test".into(),
        },
    )
    .await
    .expect("call");

    assert!(!resp.ok);
    assert_eq!(resp.error.as_deref(), Some("password_expired"));
}

#[tokio::test]
async fn authenticate_rejects_locked_account() {
    // `!` prefix means `passwd -l` — locked. Same generic
    // "invalid credentials" surface as a wrong password, so the
    // API can't be used to enumerate which accounts are locked.
    let hash = hash_password("bananas-test");
    let shadow = format!("root:!{hash}:20000:0:99999:7:::\n");
    let engine = Engine::spawn(&shadow).await;

    let resp = call(
        &engine.socket,
        &Command::Authenticate {
            username: "root".into(),
            password: "bananas-test".into(),
        },
    )
    .await
    .expect("call");

    assert!(!resp.ok);
    assert_eq!(resp.error.as_deref(), Some("invalid credentials"));
}
