//! Login / logout / me endpoints + axum middleware that gates the rest of
//! /api/* on a valid session cookie. The actual password check is delegated
//! to bananas-engine, which has root and can read /etc/shadow.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use bananas_proto::engine::v1::{
    AuthenticateRequest, ChangeOwnPasswordRequest, engine_service_client::EngineServiceClient,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use bananas_server_common::{COOKIE_NAME, Session, SessionKey, extract_cookie};

use crate::{AppState, engine_grpc};

/// Endpoints exempt from the auth middleware (must match the path AFTER
/// the /api nest prefix is stripped).
///
/// `/logout` is public so a user with an expired/tampered cookie can still
/// clear it without first logging in again. `/password` is public because
/// it's the only escape hatch when login returns `password_expired` —
/// the caller has to prove the old password anyway, so an extra session
/// cookie would be redundant.
const PUBLIC_ROUTES: &[&str] = &["/login", "/logout", "/healthz", "/password"];

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

    // Authenticate via tonic-on-Unix (PR-4 of the gRPC migration).
    // Wire shape:
    //   - Ok(lastchg_zero=false) → password good, normal session.
    //   - Ok(lastchg_zero=true)  → password good, but /etc/shadow's
    //     lastchg field is 0 ("must rotate"). The SPA recognises the
    //     `password_expired` error string and routes to /password.
    //   - Err(Unauthenticated)   → wrong password. Surfaced as 401.
    //   - Err(other)             → engine unreachable / transport.
    //     Surfaced as 502 to match the legacy newline-JSON path.
    let channel = match engine_grpc::channel(&state.helper_grpc_socket).await {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "ok": false, "error": format!("helper unreachable: {e}") })),
            )
                .into_response();
        }
    };
    let mut client = EngineServiceClient::new(channel);
    let rpc = client
        .authenticate(AuthenticateRequest {
            username: req.username.clone(),
            password: req.password,
        })
        .await;
    match rpc {
        Ok(resp) if resp.get_ref().lastchg_zero => (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "ok": false, "error": "password_expired" })),
        )
            .into_response(),
        Ok(_) => {
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
        Err(status) if status.code() == tonic::Code::Unauthenticated => (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "ok": false, "error": "invalid credentials" })),
        )
            .into_response(),
        Err(status) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "ok": false, "error": format!("helper unreachable: {status}") })),
        )
            .into_response(),
    }
}

pub async fn logout() -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(header::SET_COOKIE, expire_cookie_header());
    (StatusCode::OK, headers, Json(json!({ "ok": true }))).into_response()
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub username: String,
    pub old_password: String,
    pub new_password: String,
    #[serde(default)]
    pub remember: bool,
}

/// Self-service password change. Used both for "first login" expiry
/// recovery (when /login returns `password_expired`) and as a generic
/// "change my password" path. On success we also issue a session cookie
/// so the UI can drop the user straight into the admin panel without a
/// follow-up /login round-trip.
pub async fn change_password(
    State(state): State<AppState>,
    Json(req): Json<ChangePasswordRequest>,
) -> Response {
    if req.username.is_empty() || req.old_password.is_empty() || req.new_password.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "all fields are required" })),
        )
            .into_response();
    }
    if req.new_password == req.old_password {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "new password must differ from the old one" })),
        )
            .into_response();
    }

    // Same gRPC migration shape as `login`: every failure mode
    // collapses to Unauthenticated on the wire (the engine
    // refuses to distinguish between "user doesn't exist", "old
    // password wrong", and "new password rejected"), so we can
    // dispatch on the tonic Code without inspecting strings.
    let channel = match engine_grpc::channel(&state.helper_grpc_socket).await {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "ok": false, "error": format!("helper unreachable: {e}") })),
            )
                .into_response();
        }
    };
    let mut client = EngineServiceClient::new(channel);
    let rpc = client
        .change_own_password(ChangeOwnPasswordRequest {
            username: req.username.clone(),
            old_password: req.old_password,
            new_password: req.new_password,
        })
        .await;
    match rpc {
        Ok(_) => {
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
        Err(status) if status.code() == tonic::Code::Unauthenticated => (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "ok": false, "error": "invalid credentials" })),
        )
            .into_response(),
        Err(status) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "ok": false, "error": format!("helper unreachable: {status}") })),
        )
            .into_response(),
    }
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
