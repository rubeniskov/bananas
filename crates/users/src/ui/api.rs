//! Users-plugin SPA — HTTP client for /api/users/* + /api/me.

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    Unauthorized,
    Other(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Unauthorized => write!(f, "Session expired — please sign in again."),
            ApiError::Other(s) => f.write_str(s),
        }
    }
}

impl<T: Into<String>> From<T> for ApiError {
    fn from(value: T) -> Self {
        ApiError::Other(value.into())
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Me {
    pub username: String,
}

pub async fn fetch_me() -> Result<Option<Me>, String> {
    let resp = Request::get("/api/me")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    match resp.status() {
        200 => resp.json::<Me>().await.map(Some).map_err(|e| e.to_string()),
        401 => Ok(None),
        s => Err(format!("HTTP {s}")),
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct UsersList {
    #[serde(default)]
    pub users: Vec<UserAccount>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct UserAccount {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub primary_group: Option<String>,
    pub full_name: Option<String>,
    pub home: String,
    pub shell: String,
    pub locked: bool,
    #[serde(default)]
    pub groups: Vec<String>,
    pub system: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateUser {
    pub username: String,
    pub password: String,
    pub full_name: Option<String>,
    pub admin: bool,
}

pub async fn list_users() -> Result<UsersList, ApiError> {
    let resp = Request::get("/api/users")
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if !resp.ok() {
        let status = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        return Err(ApiError::Other(format!("HTTP {status}: {txt}")));
    }
    resp.json::<UsersList>()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))
}

pub async fn create_user(body: &CreateUser) -> Result<(), ApiError> {
    let resp = Request::post("/api/users")
        .json(body)
        .map_err(|e| ApiError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

pub async fn delete_user(username: &str) -> Result<(), ApiError> {
    let resp = Request::delete(&format!("/api/users/{}", urlencode(username)))
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

pub async fn set_user_password(username: &str, password: &str) -> Result<(), ApiError> {
    let body = serde_json::json!({ "password": password });
    let resp = Request::put(&format!("/api/users/{}/password", urlencode(username)))
        .json(&body)
        .map_err(|e| ApiError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

pub async fn set_user_admin(username: &str, admin: bool) -> Result<(), ApiError> {
    let body = serde_json::json!({ "admin": admin });
    let resp = Request::put(&format!("/api/users/{}/admin", urlencode(username)))
        .json(&body)
        .map_err(|e| ApiError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

async fn helper_status(resp: gloo_net::http::Response) -> Result<(), ApiError> {
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if resp.ok() {
        return Ok(());
    }
    let status = resp.status();
    if let Ok(body) = resp.json::<serde_json::Value>().await {
        if let Some(msg) = body.get("error").and_then(|v| v.as_str()) {
            return Err(ApiError::Other(msg.to_string()));
        }
    }
    Err(ApiError::Other(format!("HTTP {status}")))
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .flat_map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                vec![b as char]
            } else {
                format!("%{b:02X}").chars().collect()
            }
        })
        .collect()
}
