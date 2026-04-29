//! Directory listing for the path-picker UI.
//!
//! The server runs as the `bananas` user, so listings are subject to its
//! filesystem permissions. Anything not readable just gets skipped from the
//! result (instead of erroring) so the picker can keep navigating.

use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

#[derive(Debug, Serialize)]
pub struct Listing {
    pub path: String,
    pub parent: Option<String>,
    /// Path components from "/" to the current path, each with a navigable
    /// link. Lets the UI render a breadcrumb without parsing the path
    /// itself.
    pub breadcrumb: Vec<Crumb>,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Serialize)]
pub struct Crumb {
    pub name: String,
    pub path: String,
}

pub fn list(target: &Path) -> std::io::Result<Listing> {
    // Reject empty/relative paths up-front. read_dir would error on these
    // anyway, but with a generic ENOENT — the explicit message gives the
    // UI something it can display ("provide an absolute path") instead of
    // a confusing "No such file or directory".
    if target.as_os_str().is_empty() || !target.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path must be absolute (start with '/')",
        ));
    }

    // Canonicalise to a real, absolute path so symlinks don't confuse the
    // breadcrumb. Fall back to the raw path if canonicalize fails (e.g.
    // permission denied somewhere along the chain).
    let target = target.canonicalize().unwrap_or_else(|_| target.to_path_buf());

    let read = std::fs::read_dir(&target)?;
    let mut entries: Vec<Entry> = read
        .filter_map(|r| r.ok())
        .filter_map(|e| {
            let ft = e.file_type().ok()?;
            // Hide dotfiles by default to keep the picker tidy. Power users
            // can type the path directly if they need them.
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                return None;
            }
            Some(Entry {
                name,
                path: e.path().to_string_lossy().to_string(),
                is_dir: ft.is_dir(),
            })
        })
        .collect();

    // Directories first, then alphabetical.
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));

    let breadcrumb = build_breadcrumb(&target);
    let parent = target.parent().map(|p| p.to_string_lossy().to_string());

    Ok(Listing {
        path: target.to_string_lossy().to_string(),
        parent,
        breadcrumb,
        entries,
    })
}

fn build_breadcrumb(path: &Path) -> Vec<Crumb> {
    let mut crumbs = vec![Crumb { name: "/".into(), path: "/".into() }];
    let mut acc = PathBuf::from("/");
    for comp in path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().to_string()),
            _ => None,
        })
    {
        acc.push(&comp);
        crumbs.push(Crumb {
            name: comp,
            path: acc.to_string_lossy().to_string(),
        });
    }
    crumbs
}
