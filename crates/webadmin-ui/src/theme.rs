//! Light / dark / auto theme selection. Three states tracked via the
//! `data-theme` attribute on <html>:
//!
//!   - "auto" (or attribute absent) → CSS uses prefers-color-scheme
//!   - "light"                       → forces light, ignores OS
//!   - "dark"                        → forces dark, ignores OS
//!
//! The user's choice persists in localStorage under `bananas-theme`.
//! Read once at App mount; written + reapplied to the DOM whenever the
//! operator clicks a Theme item in the user menu. Pre-auth (login page)
//! users get whatever the OS prefers because the default selector chain
//! in main.css covers that.

use serde::{Deserialize, Serialize};

const STORAGE_KEY: &str = "bananas-theme";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    Auto,
    Light,
    Dark,
}

impl Theme {
    pub fn slug(self) -> &'static str {
        match self {
            Theme::Auto => "auto",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    pub fn from_slug(s: &str) -> Self {
        match s {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => Theme::Auto,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Theme::Auto => "Auto",
            Theme::Light => "Light",
            Theme::Dark => "Dark",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Theme::Auto => "monitor",
            Theme::Light => "sun",
            Theme::Dark => "moon",
        }
    }
}

/// Read the persisted theme choice. Returns Auto when no stored value
/// or when the storage API is unavailable (private mode / SSR).
pub fn load() -> Theme {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(STORAGE_KEY).ok().flatten())
        .map(|s| Theme::from_slug(&s))
        .unwrap_or(Theme::Auto)
}

/// Apply `theme` to the DOM (sets `<html data-theme="…">`) and persist
/// the choice in localStorage. No-op on platforms without a window.
pub fn apply(theme: Theme) {
    let Some(window) = web_sys::window() else {
        return;
    };
    if let Some(document) = window.document() {
        if let Some(root) = document.document_element() {
            // For Auto we strip the attribute entirely so the
            // prefers-color-scheme media query takes over cleanly.
            match theme {
                Theme::Auto => {
                    let _ = root.remove_attribute("data-theme");
                }
                _ => {
                    let _ = root.set_attribute("data-theme", theme.slug());
                }
            }
        }
    }
    if let Ok(Some(storage)) = window.local_storage() {
        let _ = storage.set_item(STORAGE_KEY, theme.slug());
    }
}
