//! Thin client for the /api/* endpoints. All calls return parsed JSON or
//! an error string suitable for surfacing to the user.

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};

/// Error type that lets callers cheaply detect "session expired, bounce to
/// login" without parsing strings. Anything else surfaces as `Other`.
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
    /// Canonical column-aligned `/etc/exports` text — what the helper
    /// writes on save. Use for the read-only preview so it can't drift.
    #[serde(default)]
    pub preview: String,
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

// --- Auth ------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    pub remember: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Me {
    pub username: String,
}

/// Result of a /api/me probe at app load:
/// - Ok(Some(me)) → signed in
/// - Ok(None) → 401, show login
/// - Err(_) → network/server error, show login + banner
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

/// Sentinel error string returned by `login` when /etc/shadow's lastchg
/// field is 0 — the user must rotate their password before the session
/// is granted. The login UI checks for this exact value and switches to
/// the "set new password" form.
pub const PASSWORD_EXPIRED: &str = "password_expired";

pub async fn login(body: &LoginRequest) -> Result<Me, String> {
    let resp = Request::post("/api/login")
        .json(body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        // Try to parse the JSON body so callers can detect the
        // password-expired sentinel without string-matching the entire
        // "HTTP 401: …" line.
        let status = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        if let Ok(body) = serde_json::from_str::<serde_json::Value>(&txt) {
            if let Some(msg) = body.get("error").and_then(|v| v.as_str()) {
                return Err(msg.to_string());
            }
        }
        return Err(format!("HTTP {status}: {txt}"));
    }
    Ok(Me {
        username: body.username.clone(),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangePasswordRequest {
    pub username: String,
    pub old_password: String,
    pub new_password: String,
    pub remember: bool,
}

/// POST /api/password — self-service password rotation. On success the
/// server also issues a session cookie, so the caller can treat this
/// like a successful login.
pub async fn change_password(body: &ChangePasswordRequest) -> Result<Me, String> {
    let resp = Request::post("/api/password")
        .json(body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        let status = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        if let Ok(body) = serde_json::from_str::<serde_json::Value>(&txt) {
            if let Some(msg) = body.get("error").and_then(|v| v.as_str()) {
                return Err(msg.to_string());
            }
        }
        return Err(format!("HTTP {status}: {txt}"));
    }
    Ok(Me {
        username: body.username.clone(),
    })
}

pub async fn logout() -> Result<(), String> {
    let resp = Request::post("/api/logout")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.ok() {
        Ok(())
    } else {
        Err(format!("HTTP {}", resp.status()))
    }
}

// --- Exports ---------------------------------------------------------------

// --- Users -----------------------------------------------------------------

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
    json_post_or_helper_error("/api/users", body).await
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

async fn json_post_or_helper_error<B: Serialize>(path: &str, body: &B) -> Result<(), ApiError> {
    let resp = Request::post(path)
        .json(body)
        .map_err(|e| ApiError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

/// The helper-routes return `{ok:true,...}` on success and `{ok:false, error,...}`
/// on validation failures (400) — pull out `error` for nicer messages.
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

// --- Config (TOML export/import) ------------------------------------------

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ImportSummary {
    pub ok: bool,
    pub exports_written: usize,
    pub fstab_written: usize,
    #[serde(default)]
    pub users_created: usize,
    pub users_skipped: usize,
    #[serde(default)]
    pub cloud_accounts: usize,
    #[serde(default)]
    pub cloud_syncs: usize,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Fetch /api/config — the response body is the raw TOML (Content-Type
/// `application/toml`). Caller is responsible for triggering the file
/// download.
pub async fn fetch_config_toml() -> Result<String, ApiError> {
    let resp = Request::get("/api/config")
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
    resp.text()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))
}

pub async fn upload_config_toml(toml_body: &str) -> Result<ImportSummary, ApiError> {
    let resp = Request::post("/api/config")
        .header("content-type", "application/toml")
        .body(toml_body.to_string())
        .map_err(|e| ApiError::Other(e.to_string()))?
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
    resp.json::<ImportSummary>()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))
}

// --- fstab -----------------------------------------------------------------

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
    /// True when the row is a system mount (root, /proc, /sys, …) that the
    /// server-side guard refuses to edit or delete. UI hides the action
    /// buttons and shows a lock badge.
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
    /// Canonical column-aligned `/etc/fstab` text — what the helper
    /// writes on save.
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
    let resp = Request::get("/api/fstab")
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
    let resp = Request::post("/api/fstab")
        .json(body)
        .map_err(|e| ApiError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

pub async fn update_fstab(idx: usize, body: &AddFstab) -> Result<(), ApiError> {
    let resp = Request::put(&format!("/api/fstab/{idx}"))
        .json(body)
        .map_err(|e| ApiError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

pub async fn delete_fstab(idx: usize) -> Result<(), ApiError> {
    let resp = Request::delete(&format!("/api/fstab/{idx}"))
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    helper_status(resp).await
}

// --- Stats (live + history) -----------------------------------------------

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub struct StatsSnapshot {
    #[serde(default)]
    pub ts_unix: i64,
    #[serde(default)]
    pub cpu: CpuStats,
    #[serde(default)]
    pub mem: MemStats,
    #[serde(default)]
    pub network: Vec<NetIface>,
    #[serde(default)]
    pub disks: Vec<DiskIo>,
    #[serde(default)]
    pub parts: Vec<PartitionStat>,
    #[serde(default)]
    pub temps: Vec<TempReading>,
    #[serde(default)]
    pub stats_db_present: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TempReading {
    /// Sensor name — `cpu_thermal`, `sda`, `nvme0n1`, …
    pub sensor: String,
    pub celsius: f32,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub struct CpuStats {
    #[serde(default)]
    pub busy_pct: f32,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub struct MemStats {
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub used: u64,
    #[serde(default)]
    pub available: u64,
    #[serde(default)]
    pub free: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct NetIface {
    pub name: String,
    pub rx_bps: u64,
    pub tx_bps: u64,
    #[serde(default)]
    pub rx_total: u64,
    #[serde(default)]
    pub tx_total: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct DiskIo {
    pub device: String,
    pub read_bps: u64,
    pub write_bps: u64,
    #[serde(default)]
    pub read_iops: u64,
    #[serde(default)]
    pub write_iops: u64,
    #[serde(default)]
    pub util_pct: f32,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PartitionStat {
    pub mount: String,
    pub device: String,
    pub used: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub struct SeriesKeys {
    #[serde(default)]
    pub interfaces: Vec<String>,
    #[serde(default)]
    pub disks: Vec<String>,
    #[serde(default)]
    pub temps: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct NetSeriesPoint {
    pub ts: i64,
    pub rx_bps: u64,
    pub tx_bps: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct DiskSeriesPoint {
    pub ts: i64,
    pub read_bps: u64,
    pub write_bps: u64,
    #[serde(default)]
    pub util_pct: f32,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TempSeriesPoint {
    pub ts: i64,
    pub celsius: f32,
}

#[derive(Debug, Clone, Deserialize)]
struct PointsEnvelope<T> {
    points: Vec<T>,
}

pub async fn fetch_stats_snapshot() -> Result<StatsSnapshot, ApiError> {
    let resp = Request::get("/api/stats/snapshot")
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if !resp.ok() {
        return Err(ApiError::Other(format!("HTTP {}", resp.status())));
    }
    resp.json::<StatsSnapshot>()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))
}

pub async fn fetch_stats_series() -> Result<SeriesKeys, ApiError> {
    let resp = Request::get("/api/stats/series")
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if !resp.ok() {
        return Err(ApiError::Other(format!("HTTP {}", resp.status())));
    }
    resp.json::<SeriesKeys>()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))
}

pub async fn fetch_net_range(iface: &str, window: &str) -> Result<Vec<NetSeriesPoint>, ApiError> {
    let url = format!(
        "/api/stats/range?metric=net&key={}&window={}",
        urlencode(iface),
        urlencode(window)
    );
    let resp = Request::get(&url)
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if !resp.ok() {
        return Err(ApiError::Other(format!("HTTP {}", resp.status())));
    }
    let env: PointsEnvelope<NetSeriesPoint> = resp
        .json()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    Ok(env.points)
}

/// Live-snapshot websocket. Wraps gloo-net's `WebSocket` with a small
/// `next_snapshot()` so callers don't have to know about the framing.
pub struct StatsWs {
    inner: gloo_net::websocket::futures::WebSocket,
}

impl StatsWs {
    /// Returns the next decoded snapshot, or `None` if the socket
    /// closes / errors. Caller is expected to back off and reopen.
    pub async fn next_snapshot(&mut self) -> Option<StatsSnapshot> {
        use futures::StreamExt;
        use gloo_net::websocket::Message;
        loop {
            match self.inner.next().await {
                Some(Ok(Message::Text(text))) => {
                    match serde_json::from_str::<StatsSnapshot>(&text) {
                        Ok(snap) => return Some(snap),
                        Err(e) => {
                            tracing::warn!(?e, "stats ws: bad payload");
                            // Skip and wait for the next one.
                        }
                    }
                }
                Some(Ok(Message::Bytes(_))) => {
                    // Server only sends text frames.
                }
                Some(Err(e)) => {
                    tracing::warn!(?e, "stats ws: error");
                    return None;
                }
                None => return None,
            }
        }
    }
}

/// Open the stats websocket relative to the document's origin
/// (so http→ws / https→wss).
pub fn open_stats_ws() -> Result<StatsWs, ApiError> {
    let location = web_sys::window()
        .and_then(|w| w.location().host().ok())
        .ok_or_else(|| ApiError::Other("no window.location.host".into()))?;
    let proto = web_sys::window()
        .and_then(|w| w.location().protocol().ok())
        .unwrap_or_else(|| "http:".into());
    let ws_proto = if proto.starts_with("https") {
        "wss"
    } else {
        "ws"
    };
    let url = format!("{ws_proto}://{location}/api/stats/live");
    let inner = gloo_net::websocket::futures::WebSocket::open(&url)
        .map_err(|e| ApiError::Other(format!("ws open: {e}")))?;
    Ok(StatsWs { inner })
}

pub async fn fetch_disk_range(
    device: &str,
    window: &str,
) -> Result<Vec<DiskSeriesPoint>, ApiError> {
    let url = format!(
        "/api/stats/range?metric=disk&key={}&window={}",
        urlencode(device),
        urlencode(window)
    );
    let resp = Request::get(&url)
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if !resp.ok() {
        return Err(ApiError::Other(format!("HTTP {}", resp.status())));
    }
    let env: PointsEnvelope<DiskSeriesPoint> = resp
        .json()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    Ok(env.points)
}

pub async fn fetch_temp_range(
    sensor: &str,
    window: &str,
) -> Result<Vec<TempSeriesPoint>, ApiError> {
    let url = format!(
        "/api/stats/range?metric=temp&key={}&window={}",
        urlencode(sensor),
        urlencode(window)
    );
    let resp = Request::get(&url)
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if !resp.ok() {
        return Err(ApiError::Other(format!("HTTP {}", resp.status())));
    }
    let env: PointsEnvelope<TempSeriesPoint> = resp
        .json()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    Ok(env.points)
}

// --- Permissions ----------------------------------------------------------

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PermsInfo {
    pub path: String,
    pub uid: u32,
    pub gid: u32,
    pub user: String,
    pub group: String,
    /// Octal mode string ("755", "700", …).
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

// --- Storage ---------------------------------------------------------------

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
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if resp.ok() {
        Ok(())
    } else {
        let status = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        Err(ApiError::Other(format!("HTTP {status}: {txt}")))
    }
}

pub async fn update_export(idx: usize, body: &AddExport) -> Result<(), ApiError> {
    let resp = Request::put(&format!("/api/exports/{idx}"))
        .json(body)
        .map_err(|e| ApiError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if resp.ok() {
        Ok(())
    } else {
        let status = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        Err(ApiError::Other(format!("HTTP {status}: {txt}")))
    }
}

pub async fn delete_export(idx: usize) -> Result<(), ApiError> {
    let resp = Request::delete(&format!("/api/exports/{idx}"))
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if resp.ok() {
        Ok(())
    } else {
        let status = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        Err(ApiError::Other(format!("HTTP {status}: {txt}")))
    }
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

// --- Cloud ----------------------------------------------------------------

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
    /// True when /etc/bananas/cloud.toml has a non-empty token field.
    /// Used by the UI to render "✓ token set" vs "no token (re-auth)".
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

#[derive(Debug, Clone, Serialize)]
pub struct UpdateCloudAccount {
    pub provider: String,
    /// Empty string = keep the existing token (the redacted GET path
    /// can not surface the real value back, and re-pasting just to
    /// change the provider would be annoying).
    pub token: String,
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

/// Trigger a sync. Returns immediately with the new (or existing) job_id.
/// Caller polls /api/cloud/runs/{job_id} until status is no longer
/// `running` to follow the run.
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
    /// Live transfer percent (0..=100) when `status == running`. None
    /// while rclone is in its initial directory scan / on terminal
    /// states. Drives the circular progress indicator in the Recent
    /// runs panel.
    #[serde(default)]
    pub progress: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
struct RunsResp {
    runs: Vec<CloudJob>,
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
