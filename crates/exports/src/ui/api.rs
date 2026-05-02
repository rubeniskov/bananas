//! Exports-plugin SPA — HTTP client for the endpoints this plugin
//! hits. Subset of webadmin-ui's api.rs covering exactly:
//!   - /api/me                       (auth probe)
//!   - /api/exports{,/<idx>}         (CRUD)
//!   - /api/exports/browse           (path picker)
//!   - /api/exports/mkdir            (create-on-the-fly)
//!   - /api/permissions              (per-export chmod/chown)
//!
//! Duplicated rather than shared because Dioxus crates are
//! binary-only and there's no library boundary to share through.
//! A future `webadmin-common` extraction will fold the common
//! bits (ApiError, Me, urlencode, helper_status, browse,
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

// --- Exports CRUD --------------------------------------------------------

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ExportRow {
    pub idx: usize,
    pub path: String,
    pub host: String,
    pub options: String,
    pub parsed: ExportOpts,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub struct ExportOpts {
    #[serde(default)]
    pub rw: bool,
    #[serde(default)]
    pub sync: bool,
    #[serde(default)]
    pub no_subtree_check: bool,
    #[serde(default)]
    pub squash: String,
    pub anonuid: Option<u32>,
    pub anongid: Option<u32>,
    #[serde(default)]
    pub insecure: bool,
    #[serde(default)]
    pub extra: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExportsList {
    pub rows: Vec<ExportRow>,
    #[serde(default)]
    pub preview: String,
    #[serde(default)]
    pub nfs_server_status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddExport {
    pub path: String,
    pub host: String,
    pub rw: bool,
    pub sync: bool,
    pub no_subtree_check: bool,
    pub squash: String,
    pub anonuid: Option<u32>,
    pub anongid: Option<u32>,
    pub insecure: bool,
}

pub async fn list_exports() -> Result<ExportsList, ApiError> {
    let resp = Request::get("/api/exports")
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if !resp.ok() {
        return Err(ApiError::Other(format!("HTTP {}", resp.status())));
    }
    resp.json::<ExportsList>()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))
}

pub async fn add_export(body: &AddExport) -> Result<(), ApiError> {
    let resp = Request::post("/api/exports")
        .json(body)
        .map_err(|e| ApiError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

pub async fn update_export(idx: usize, body: &AddExport) -> Result<(), ApiError> {
    let resp = Request::put(&format!("/api/exports/{idx}"))
        .json(body)
        .map_err(|e| ApiError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

pub async fn delete_export(idx: usize) -> Result<(), ApiError> {
    let resp = Request::delete(&format!("/api/exports/{idx}"))
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

// --- Browse + mkdir (path picker) ----------------------------------------

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
    let url = format!("/api/exports/browse?path={}", urlencode(path));
    let resp = Request::get(&url)
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

pub async fn mkdir(path: &str) -> Result<(), ApiError> {
    let resp = Request::post("/api/exports/mkdir")
        .json(&serde_json::json!({ "path": path }))
        .map_err(|e| ApiError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

// --- Permissions (file-system chmod/chown for export targets) -------------
//
// Lives at /api/permissions on the host today; the storage plugin
// will own that endpoint after commit 4. Until then this hits
// webadmin's catch-all `/api` manifest, which is fine.

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PermsInfo {
    pub path: String,
    pub uid: u32,
    pub gid: u32,
    pub user: String,
    pub group: String,
    pub mode: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetPermsRequest {
    pub path: String,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub mode: Option<String>,
    pub recursive: bool,
}

pub async fn fetch_perms(path: &str) -> Result<PermsInfo, ApiError> {
    let url = format!("/api/permissions?path={}", urlencode(path));
    let resp = Request::get(&url)
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
    resp.json::<PermsInfo>()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))
}

pub async fn put_perms(req: &SetPermsRequest) -> Result<(), ApiError> {
    let resp = Request::put("/api/permissions")
        .json(req)
        .map_err(|e| ApiError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

// --- helpers --------------------------------------------------------------

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
