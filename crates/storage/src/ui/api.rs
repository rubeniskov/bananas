//! Storage-plugin SPA — HTTP client. Endpoints under
//! `/api/storage/*` (sub-proxied through bananas-router) plus
//! `/api/me` for the auth probe.

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

// --- Storage report ------------------------------------------------------

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct StorageReport {
    #[serde(default)]
    pub disks: Vec<Disk>,
    #[serde(default)]
    pub other: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Disk {
    pub name: String,
    pub kname: String,
    pub model: Option<String>,
    pub size: Option<u64>,
    #[serde(default)]
    pub readonly: bool,
    #[serde(default)]
    pub partitions: Vec<Partition>,
    pub smart: Option<serde_json::Value>,
    pub smart_error: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Partition {
    pub name: String,
    pub kname: String,
    pub size: Option<u64>,
    pub fstype: Option<String>,
    pub label: Option<String>,
    pub uuid: Option<String>,
    pub mountpoint: Option<String>,
    pub used: Option<u64>,
    pub available: Option<u64>,
    pub total: Option<u64>,
}

pub async fn fetch_storage() -> Result<StorageReport, ApiError> {
    let resp = Request::get("/api/storage")
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
    resp.json::<StorageReport>()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))
}

// --- fstab ---------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct FstabRow {
    pub idx: usize,
    pub source: String,
    pub mountpoint: String,
    pub fstype: String,
    pub options: String,
    pub dump: u32,
    pub pass: u32,
    pub parsed: FstabOpts,
    #[serde(default)]
    pub protected: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub struct FstabOpts {
    #[serde(default)]
    pub defaults: bool,
    #[serde(default)]
    pub noatime: bool,
    #[serde(default)]
    pub nofail: bool,
    #[serde(default)]
    pub ro: bool,
    #[serde(default)]
    pub discard: bool,
    #[serde(default)]
    pub noexec: bool,
    #[serde(default)]
    pub nosuid: bool,
    #[serde(default)]
    pub nodev: bool,
    pub device_timeout: Option<u32>,
    #[serde(default)]
    pub extra: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FstabList {
    pub rows: Vec<FstabRow>,
    #[serde(default)]
    pub preview: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddFstab {
    pub source: String,
    pub mountpoint: String,
    pub fstype: String,
    pub defaults: bool,
    pub noatime: bool,
    pub nofail: bool,
    pub ro: bool,
    pub discard: bool,
    pub noexec: bool,
    pub nosuid: bool,
    pub nodev: bool,
    pub device_timeout: Option<u32>,
    pub dump: u32,
    pub pass: u32,
}

pub async fn list_fstab() -> Result<FstabList, ApiError> {
    let resp = Request::get("/api/storage/fstab")
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
    resp.json::<FstabList>()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))
}

pub async fn add_fstab(body: &AddFstab) -> Result<(), ApiError> {
    let resp = Request::post("/api/storage/fstab")
        .json(body)
        .map_err(|e| ApiError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

pub async fn update_fstab(idx: usize, body: &AddFstab) -> Result<(), ApiError> {
    let resp = Request::put(&format!("/api/storage/fstab/{idx}"))
        .json(body)
        .map_err(|e| ApiError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

pub async fn delete_fstab(idx: usize) -> Result<(), ApiError> {
    let resp = Request::delete(&format!("/api/storage/fstab/{idx}"))
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

// --- Browse + mkdir ------------------------------------------------------

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
    let url = format!("/api/storage/browse?path={}", urlencode(path));
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
    let resp = Request::post("/api/storage/mkdir")
        .json(&serde_json::json!({ "path": path }))
        .map_err(|e| ApiError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

// --- Permissions ---------------------------------------------------------

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
    let url = format!("/api/storage/permissions?path={}", urlencode(path));
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
    let resp = Request::put("/api/storage/permissions")
        .json(req)
        .map_err(|e| ApiError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
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
