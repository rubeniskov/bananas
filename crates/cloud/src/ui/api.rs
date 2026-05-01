//! Cloud-only API client. Subset of the webadmin-ui's api.rs covering
//! exactly the endpoints the cloud SPA hits: /api/me (auth check),
//! /api/browse (path picker), and /api/cloud/* (the cloud surface
//! itself). Everything else (exports, fstab, stats, …) is the
//! responsibility of bananas-webadmin's SPA at /.
//!
//! Duplicated rather than shared with webadmin-ui's api.rs because
//! Dioxus crates are binary-only and there's no library boundary to
//! share through. A future `webadmin-common` extraction will fold
//! the common bits (ApiError, Me, urlencode, helper_status, browse,
//! Listing/Entry/Crumb) into a sibling crate.

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

// --- Auth probe ----------------------------------------------------------

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Me {
    pub username: String,
}

/// Result of a /api/me probe at app load:
/// - Ok(Some(me)) → signed in
/// - Ok(None)     → 401, bounce back to /
/// - Err(_)       → network/server error
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

// --- /api/browse (path picker) -------------------------------------------

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Listing {
    pub path: String,
    pub parent: Option<String>,
    pub breadcrumb: Vec<Crumb>,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Crumb {
    pub name: String,
    pub path: String,
}

pub async fn browse(path: &str) -> Result<Listing, ApiError> {
    let resp = Request::get(&format!("/api/browse?path={}", urlencode(path)))
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
    resp.json::<Listing>()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))
}

// --- /api/cloud/* --------------------------------------------------------

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CloudProvider {
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ProvidersResp {
    providers: Vec<CloudProvider>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CloudAccount {
    pub name: String,
    pub provider: String,
    pub token_present: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct AccountsResp {
    accounts: Vec<CloudAccount>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddCloudAccount {
    pub name: String,
    pub provider: String,
    pub token: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CloudSync {
    pub idx: usize,
    pub account: String,
    pub local_path: String,
    pub remote_path: String,
    pub direction: String,
    pub schedule: String,
}

#[derive(Debug, Clone, Deserialize)]
struct SyncsResp {
    syncs: Vec<CloudSync>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddCloudSync {
    pub account: String,
    pub local_path: String,
    pub remote_path: String,
    pub direction: String,
    pub schedule: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateCloudAccount {
    pub provider: String,
    pub token: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CloudJobStatus {
    Running,
    Success,
    Failure,
}
impl CloudJobStatus {
    pub fn css(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Success => "ok",
            Self::Failure => "err",
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Success => "success",
            Self::Failure => "failed",
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CloudJob {
    pub id: u64,
    pub sync_idx: usize,
    pub status: CloudJobStatus,
    pub started_unix: i64,
    pub finished_unix: Option<i64>,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub progress: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
struct RunsResp {
    runs: Vec<CloudJob>,
}

pub async fn list_cloud_providers() -> Result<Vec<CloudProvider>, ApiError> {
    let resp = Request::get("/api/cloud/providers")
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if !resp.ok() {
        return Err(ApiError::Other(format!("HTTP {}", resp.status())));
    }
    let body: ProvidersResp = resp
        .json()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    Ok(body.providers)
}

pub async fn list_cloud_accounts() -> Result<Vec<CloudAccount>, ApiError> {
    let resp = Request::get("/api/cloud/accounts")
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if !resp.ok() {
        return Err(ApiError::Other(format!("HTTP {}", resp.status())));
    }
    let body: AccountsResp = resp
        .json()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    Ok(body.accounts)
}

pub async fn add_cloud_account(req: &AddCloudAccount) -> Result<(), ApiError> {
    let resp = Request::post("/api/cloud/accounts")
        .json(req)
        .map_err(|e| ApiError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

pub async fn update_cloud_account(name: &str, req: &UpdateCloudAccount) -> Result<(), ApiError> {
    let resp = Request::put(&format!("/api/cloud/accounts/{}", urlencode(name)))
        .json(req)
        .map_err(|e| ApiError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

pub async fn delete_cloud_account(name: &str) -> Result<(), ApiError> {
    let resp = Request::delete(&format!("/api/cloud/accounts/{}", urlencode(name)))
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

pub async fn list_cloud_syncs() -> Result<Vec<CloudSync>, ApiError> {
    let resp = Request::get("/api/cloud/syncs")
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if !resp.ok() {
        return Err(ApiError::Other(format!("HTTP {}", resp.status())));
    }
    let body: SyncsResp = resp
        .json()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    Ok(body.syncs)
}

pub async fn add_cloud_sync(req: &AddCloudSync) -> Result<(), ApiError> {
    let resp = Request::post("/api/cloud/syncs")
        .json(req)
        .map_err(|e| ApiError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

pub async fn update_cloud_sync(idx: usize, req: &AddCloudSync) -> Result<(), ApiError> {
    let resp = Request::put(&format!("/api/cloud/syncs/{idx}"))
        .json(req)
        .map_err(|e| ApiError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

pub async fn delete_cloud_sync(idx: usize) -> Result<(), ApiError> {
    let resp = Request::delete(&format!("/api/cloud/syncs/{idx}"))
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

pub async fn run_cloud_sync(idx: usize) -> Result<u64, ApiError> {
    let resp = Request::post(&format!("/api/cloud/syncs/{idx}/run"))
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    let txt = resp
        .text()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    let body: serde_json::Value = serde_json::from_str(&txt)
        .unwrap_or_else(|_| serde_json::json!({ "ok": false, "error": txt }));
    if body.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        body.get("job_id")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ApiError::Other("server did not return a job_id".into()))
    } else {
        let msg = body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("rclone failed")
            .to_string();
        Err(ApiError::Other(msg))
    }
}

pub async fn cancel_cloud_sync(idx: usize) -> Result<(), ApiError> {
    let resp = Request::post(&format!("/api/cloud/syncs/{idx}/cancel"))
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
    Ok(())
}

pub async fn list_cloud_runs() -> Result<Vec<CloudJob>, ApiError> {
    let resp = Request::get("/api/cloud/runs")
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if !resp.ok() {
        return Err(ApiError::Other(format!("HTTP {}", resp.status())));
    }
    let body: RunsResp = resp
        .json()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    Ok(body.runs)
}

pub async fn get_cloud_run(job_id: u64) -> Result<CloudJob, ApiError> {
    let resp = Request::get(&format!("/api/cloud/runs/{job_id}"))
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if !resp.ok() {
        return Err(ApiError::Other(format!("HTTP {}", resp.status())));
    }
    resp.json::<CloudJob>()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))
}

// --- helpers -------------------------------------------------------------

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
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
                vec![b as char]
            } else {
                format!("%{b:02X}").chars().collect()
            }
        })
        .collect()
}
