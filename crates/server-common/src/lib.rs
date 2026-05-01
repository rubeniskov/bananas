//! Shared infrastructure used by every BanaNAS HTTP daemon
//! (`bananas-webadmin`, `bananas-cloud`, future plugins). The public
//! surface is intentionally thin — each daemon owns its own routes,
//! its own state, and its own engine-call ergonomics; this crate
//! holds only what would otherwise be copy-paste across daemons.
//!
//! Today: HMAC-signed session cookies. The router forwards `Cookie`
//! headers verbatim, so each daemon needs to validate sessions
//! locally; doing it in a shared crate means there's one
//! implementation to keep correct.
//!
//! Future additions land here when a second daemon starts importing
//! them — until then, code stays in its origin daemon to keep the
//! shared surface narrow.

pub mod manifest;
pub mod session;

pub use manifest::{Manifest, load_all, match_prefix};
pub use session::{COOKIE_NAME, Session, SessionKey, extract_cookie};
