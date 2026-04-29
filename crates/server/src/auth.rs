//! Login / logout / me endpoints + axum middleware that gates the rest of
//! /api/* on a valid session cookie. The actual password check is delegated
//! to bananas-helper, which has root and can read /etc/shadow.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use bananas_helper::{Command, Response as HelperResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    AppState,
    session::{COOKIE_NAME, Session, SessionKey, extract_cookie},
};

/// Endpoints exempt from the auth middleware (must match the path AFTER
/// the /api nest prefix is stripped).
///
/// `/logout` is public so a user with an expired/tampered cookie can still
/// clear it without first logging in again.
const PUBLIC_ROUTES: &[&str] = &["/login", "/logout", "/healthz"];

/// Cookie TTLs. "Remember me" trades off security for convenience —
/// 30 days is the same default Cockpit uses.
const TTL_SHORT_SECS: u64 = 8 * 3600; // single workday
const TTL_REMEMBER_SECS: u64 = 30 * 24 * 3600;

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub remember: bool,
}

#[derive(Debug, Serialize)]
struct MeResponse {
    username: String,
}

pub async fn login(State(state): State<AppState>, Json(req): Json<LoginRequest>) -> Response {
    if req.username.is_empty() || req.password.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "username and password are required" })),
        )
            .into_response();
    }

    let cmd = Command::Authenticate {
        username: req.username.clone(),
        password: req.password,
    };
    match bananas_helper::call(&state.helper_socket, &cmd).await {
        Ok(HelperResponse { ok: true, .. }) => {
            let ttl = if req.remember {
                TTL_REMEMBER_SECS
            } else {
                TTL_SHORT_SECS
            };
            let value = Session::sign(&state.session_key, &req.username, ttl);
            let mut headers = HeaderMap::new();
            headers.insert(
                header::SET_COOKIE,
                cookie_header(&value, if req.remember { Some(ttl) } else { None }),
            );
            (
                StatusCode::OK,
                headers,
                Json(json!({ "ok": true, "username": req.username })),
            )
                .into_response()
        }
        Ok(HelperResponse { error, .. }) => (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "ok": false,
                "error": error.unwrap_or_else(|| "invalid credentials".into())
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "ok": false, "error": format!("helper unreachable: {e}") })),
        )
            .into_response(),
    }
}

pub async fn logout() -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(header::SET_COOKIE, expire_cookie_header());
    (StatusCode::OK, headers, Json(json!({ "ok": true }))).into_response()
}

pub async fn me(headers: HeaderMap, State(state): State<AppState>) -> Response {
    match current_session(&headers, &state.session_key) {
        Some(s) => Json(MeResponse {
            username: s.username,
        })
        .into_response(),
        None => (StatusCode::UNAUTHORIZED, Json(json!({ "ok": false }))).into_response(),
    }
}

/// Tower middleware: pass through if the request is for a public route or
/// carries a valid session cookie; 401 otherwise.
pub async fn require_session(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if PUBLIC_ROUTES.iter().any(|p| path == *p) {
        return next.run(req).await;
    }
    if current_session(req.headers(), &state.session_key).is_some() {
        return next.run(req).await;
    }
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "ok": false, "error": "not signed in" })),
    )
        .into_response()
}

fn current_session(headers: &HeaderMap, key: &Arc<SessionKey>) -> Option<Session> {
    for header_value in headers.get_all(header::COOKIE).iter() {
        let s = header_value.to_str().ok()?;
        if let Some(cookie) = extract_cookie(s) {
            if let Some(session) = Session::verify(key, cookie) {
                return Some(session);
            }
        }
    }
    None
}

fn cookie_header(value: &str, max_age: Option<u64>) -> HeaderValue {
    // SameSite=Lax: blocks cross-site POSTs (CSRF) while still letting the
    // user follow links in. HttpOnly: keeps JS from reading the cookie.
    // Secure is intentionally NOT set: this admin server runs on plain HTTP
    // on a trusted LAN. Flip to Secure once we put it behind a reverse
    // proxy with TLS.
    let mut attrs = format!(
        "{name}={val}; Path=/; HttpOnly; SameSite=Lax",
        name = COOKIE_NAME,
        val = value
    );
    if let Some(age) = max_age {
        attrs.push_str(&format!("; Max-Age={age}"));
    }
    HeaderValue::from_str(&attrs).expect("ascii cookie")
}

fn expire_cookie_header() -> HeaderValue {
    HeaderValue::from_static(concat!(
        "bananas_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"
    ))
}
