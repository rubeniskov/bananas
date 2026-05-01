//! Tiny shared error helpers for the remaining webadmin
//! handlers (system + service-config + operations). Used to live
//! in `stats.rs` alongside the /api/stats/* route bodies; that
//! module moved to bananas-stats-web in commit 6, leaving these
//! two helpers homeless. Pulling them out into their own
//! module so removing stats.rs doesn't break the dependents.

use axum::{Json, http::StatusCode, response::Response};
use serde_json::json;

pub fn err_400(msg: String) -> Response {
    use axum::response::IntoResponse;
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "ok": false, "error": msg })),
    )
        .into_response()
}

pub fn err_500(msg: String) -> Response {
    use axum::response::IntoResponse;
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "ok": false, "error": msg })),
    )
        .into_response()
}
