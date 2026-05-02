//! Dashboard-config plugin SPA — minimal HTTP client. Only does
//! the /api/me auth probe; everything else (`/api/dashboard/config`)
//! is hit directly from `dashboard_config::DashboardConfigForm`
//! via gloo_net::http::Request.

use gloo_net::http::Request;
use serde::Deserialize;

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
