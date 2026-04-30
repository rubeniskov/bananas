//! HMAC-SHA256 signed session cookies. No server-side state; the cookie
//! payload itself encodes the username + expiry, with an HMAC tag binding
//! them to a server-side key. If the key is rotated (or the disk-file
//! deleted), all existing sessions are invalidated.

use std::{path::Path, time::SystemTime};

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub const COOKIE_NAME: &str = "bananas_session";

/// 32-byte random key used to sign cookies. Persisted to a file so server
/// restarts don't log everyone out. Owned by the bananas service user
/// (mode 0600). Lost-key recovery: just delete the file — a new one is
/// generated on next start, invalidating any in-flight cookies.
#[derive(Clone)]
pub struct SessionKey(Vec<u8>);

impl SessionKey {
    pub fn load_or_create(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) if bytes.len() >= 32 => {
                tracing::info!(path=%path.display(), "loaded existing session key");
                Ok(Self(bytes))
            }
            Ok(_) => {
                tracing::warn!(
                    path=%path.display(),
                    "session key file too short, regenerating"
                );
                Self::generate_and_save(path)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!(path=%path.display(), "generating new session key");
                Self::generate_and_save(path)
            }
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    fn generate_and_save(path: &Path) -> Result<Self> {
        use rand::TryRngCore;
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(|e| anyhow::anyhow!("OsRng: {e}"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).ok();
        }
        Ok(Self(bytes.to_vec()))
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub username: String,
}

impl Session {
    /// Sign a cookie value for `username`, valid for `ttl_secs` seconds.
    pub fn sign(key: &SessionKey, username: &str, ttl_secs: u64) -> String {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let exp = now + ttl_secs;
        let payload = format!("{}|{}", username, exp);
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload.as_bytes());

        let mut mac = HmacSha256::new_from_slice(&key.0).expect("hmac key");
        mac.update(payload_b64.as_bytes());
        let tag = mac.finalize().into_bytes();
        let tag_b64 = URL_SAFE_NO_PAD.encode(tag);
        format!("{payload_b64}.{tag_b64}")
    }

    /// Parse + verify a cookie value. Returns the embedded session if the
    /// signature matches and the expiry is in the future.
    pub fn verify(key: &SessionKey, cookie: &str) -> Option<Self> {
        let (payload_b64, tag_b64) = cookie.split_once('.')?;
        let mut mac = HmacSha256::new_from_slice(&key.0).ok()?;
        mac.update(payload_b64.as_bytes());
        let expected_tag = URL_SAFE_NO_PAD.decode(tag_b64).ok()?;
        mac.verify_slice(&expected_tag).ok()?;

        let payload = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
        let payload = std::str::from_utf8(&payload).ok()?;
        let (username, exp_str) = payload.split_once('|')?;
        let exp: u64 = exp_str.parse().ok()?;
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if exp < now {
            return None;
        }
        let _ = exp; // verified above; not exposed
        Some(Self {
            username: username.to_string(),
        })
    }
}

/// Pull the session cookie value out of the `Cookie:` header(s).
pub fn extract_cookie(header_value: &str) -> Option<&str> {
    for piece in header_value.split(';') {
        let piece = piece.trim();
        if let Some(v) = piece.strip_prefix(&format!("{COOKIE_NAME}=")) {
            return Some(v);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_key() -> SessionKey {
        SessionKey(vec![7u8; 32])
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let k = fake_key();
        let cookie = Session::sign(&k, "alice", 3600);
        let s = Session::verify(&k, &cookie).expect("verify");
        assert_eq!(s.username, "alice");
    }

    #[test]
    fn tamper_invalidates() {
        let k = fake_key();
        let mut cookie = Session::sign(&k, "alice", 3600);
        // flip the last char of the tag
        let last = cookie.pop().unwrap();
        cookie.push(if last == 'A' { 'B' } else { 'A' });
        assert!(Session::verify(&k, &cookie).is_none());
    }

    #[test]
    fn expired_rejected() {
        // ttl=0 → exp == now; verify() requires exp > now-ish, so a 0 ttl
        // cookie is on the boundary. Bump back: sign manually with a past
        // expiry and check it's rejected.
        let k = fake_key();
        let payload = "alice|1";
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload);
        let mut mac = HmacSha256::new_from_slice(&k.0).unwrap();
        mac.update(payload_b64.as_bytes());
        let tag = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        let cookie = format!("{payload_b64}.{tag}");
        assert!(Session::verify(&k, &cookie).is_none());
    }

    #[test]
    fn extract_cookie_picks_right_kv() {
        assert_eq!(
            extract_cookie("foo=bar; bananas_session=hello.world; baz=qux"),
            Some("hello.world")
        );
        assert_eq!(extract_cookie("other=1"), None);
    }
}
