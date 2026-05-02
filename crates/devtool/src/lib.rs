//! Workspace-internal e2e harness for BanaNAS.
//!
//! Per-crate integration tests pull this in as a `[dev-dependencies]`
//! and use `Harness::new()` to bring up engine + every plugin daemon
//! against an isolated tmpdir + ephemeral port. `Harness::login()`
//! exercises real `/api/login`, returning the cookie value tests
//! attach to subsequent requests.
//!
//! Replaces the Node + playwright harness at `tests/e2e/`. See
//! `/home/rubeniskov/.claude/plans/gleaming-stargazing-boot.md` for
//! the migration plan.

mod browser;
mod container;
mod harness;

pub use browser::{Browser, Page};
pub use container::{CONTAINER_TEST_PASSWORD, Container, ExecOutput, IMAGE_TAG};
pub use harness::{Harness, HarnessHandle, TEST_PASSWORD};
