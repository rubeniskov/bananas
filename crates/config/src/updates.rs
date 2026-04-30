//! GitHub release polling + tarball download for the CLI's `update`
//! subcommands. Mirrors crates/server/src/updates.rs but slimmer
//! (no SSE, no caching, no in-flight tracking — one shot per call).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use futures_util::stream::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

const RELEASES_API: &str = "https://api.github.com/repos/rubeniskov/bananas/releases/latest";

/// Same component slugs the server uses, in the same display order.
pub const COMPONENTS: &[&str] = &["server", "helper", "stats", "dashboard", "webadmin"];

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct UpdateReport {
    pub latest_version: String,
    pub release_url: String,
    pub components: HashMap<String, UpdateRow>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateRow {
    pub installed: Option<String>,
    pub outdated: bool,
    pub asset_url: Option<String>,
    pub sha256: Option<String>,
}

fn asset_name(component: &str) -> &'static str {
    match component {
        "server" | "helper" => "bananas-server-armv7.tar.gz",
        "stats" => "bananas-stats-armv7.tar.gz",
        "dashboard" => "bananas-dashboard-armv7.tar.gz",
        "webadmin" => "bananas-webadmin.tar.gz",
        _ => "",
    }
}

pub async fn check(socket: &Path) -> Result<UpdateReport> {
    let installed = crate::cli::read_versions(socket).await.unwrap_or_default();
    let snap = fetch_release().await.context("github release fetch")?;

    let mut report = UpdateReport {
        latest_version: snap.version,
        release_url: snap.release_url,
        components: HashMap::new(),
    };
    for c in COMPONENTS {
        let installed_v = installed.get(*c).cloned();
        let asset_url = snap.assets.get(asset_name(c)).cloned();
        let sha256 = snap.shas.get(asset_name(c)).cloned();
        let outdated = match installed_v.as_deref() {
            Some(i) => semver_lt(i, &report.latest_version),
            None => false,
        };
        report.components.insert(
            c.to_string(),
            UpdateRow {
                installed: installed_v,
                outdated,
                asset_url,
                sha256,
            },
        );
    }
    Ok(report)
}

pub async fn download(url: &str, component: &str) -> Result<PathBuf> {
    let staging_dir = std::env::var_os("BANANAS_UPDATES_STAGING_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/lib/bananas/updates/staging"));
    tokio::fs::create_dir_all(&staging_dir)
        .await
        .with_context(|| format!("creating {}", staging_dir.display()))?;
    let filename = url.rsplit('/').next().unwrap_or("download.tar.gz");
    let dest = staging_dir.join(filename);
    let _ = component;

    let client = reqwest::Client::builder()
        .user_agent(concat!("bananas-config/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(120))
        .build()?;
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()?;
    let mut file = tokio::fs::File::create(&dest)
        .await
        .with_context(|| format!("creating {}", dest.display()))?;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("download chunk")?;
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    Ok(dest)
}

pub async fn verify_sha(path: &Path, expected: &str) -> Result<()> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(anyhow!("expected {expected}, got {actual}"));
    }
    Ok(())
}

#[derive(Debug, Default)]
struct ReleaseSnapshot {
    version: String,
    release_url: String,
    assets: HashMap<String, String>,
    shas: HashMap<String, String>,
}

async fn fetch_release() -> Result<ReleaseSnapshot> {
    #[derive(Deserialize)]
    struct Asset {
        name: String,
        browser_download_url: String,
    }
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
        html_url: String,
        assets: Vec<Asset>,
    }
    let client = reqwest::Client::builder()
        .user_agent(concat!("bananas-config/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(15))
        .build()?;
    let release: Release = client
        .get(RELEASES_API)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let mut assets = HashMap::new();
    let mut sha_url = None;
    for a in release.assets {
        if a.name == "SHA256SUMS" {
            sha_url = Some(a.browser_download_url);
        } else {
            assets.insert(a.name, a.browser_download_url);
        }
    }
    let shas = if let Some(url) = sha_url {
        let body = client
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        parse_shasums(&body)
    } else {
        HashMap::new()
    };

    let version = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name)
        .to_string();

    Ok(ReleaseSnapshot {
        version,
        release_url: release.html_url,
        assets,
        shas,
    })
}

fn parse_shasums(body: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in body.lines() {
        let mut parts = line.splitn(2, ' ');
        let sha = parts.next().unwrap_or("").trim();
        let rest = parts.next().unwrap_or("").trim();
        let name = rest.trim_start_matches('*').trim().to_string();
        if sha.len() == 64 && !name.is_empty() {
            out.insert(name, sha.to_string());
        }
    }
    out
}

fn semver_lt(a: &str, b: &str) -> bool {
    let parts = |s: &str| -> Vec<u64> {
        s.split('.')
            .map(|p| p.split('-').next().unwrap_or("").parse::<u64>().unwrap_or(0))
            .collect()
    };
    let av = parts(a);
    let bv = parts(b);
    for i in 0..av.len().max(bv.len()) {
        let ai = av.get(i).copied().unwrap_or(0);
        let bi = bv.get(i).copied().unwrap_or(0);
        if ai != bi {
            return ai < bi;
        }
    }
    false
}
