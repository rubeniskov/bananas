//! Form-based dashboard config editor — symmetric with stats_config.
//! Loads /etc/bananas/dashboard.toml through /api/dashboard/config,
//! parses the flat schema into typed form fields, lets the operator
//! edit each value, and serializes a clean TOML payload on save.
//!
//! No service restart on save — bananas-dashboard polls the file's
//! mtime every 2 s and reapplies theme + refresh-rate changes in
//! place.
//!
//! The whole form is derived from the `Cfg` struct via
//! `#[derive(ConfigForm)]`. The runtime renders rows, hydrates from
//! the API, composes the TOML on submit, and surfaces the
//! Generated TOML preview.

#![allow(non_snake_case)]

use bananas_config_form::{ConfigForm, ConfigFormProps};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::AuthCtx;

#[derive(Debug, Clone, Serialize, Deserialize, ConfigForm)]
#[serde(default)]
#[config_form(
    api = "/api/dashboard/config",
    form_id = "dashboard-config-form",
    description = "Edits land in /etc/bananas/dashboard.toml. The LCD app polls every 2 s and hot-reloads — no service restart.",
    success_msg = "Saved. The LCD picks the new theme/refresh-rate up within ~2 s.",
    save_label = "Save"
)]
struct Cfg {
    #[field(
        legend = "Render target",
        label = "Width (px)",
        min = 320,
        tooltip = "Render width in pixels. Native panel is 800. Change only if you swap the LCD."
    )]
    width: u32,
    #[field(
        label = "Height (px)",
        min = 240,
        tooltip = "Render height in pixels. Native panel is 480."
    )]
    height: u32,
    #[field(
        label = "Title",
        required = false,
        tooltip = "Window title (only visible if the dashboard ever runs in a desktop window manager)."
    )]
    title: String,

    #[field(
        legend = "Appearance",
        label = "Theme",
        select = [
            ("auto", "auto (dark/light by clock)"),
            ("dark", "dark"),
            ("light", "light"),
        ],
        tooltip = "auto = dark in PM hours / light in AM. Or pin to dark/light."
    )]
    theme: String,
    #[field(
        label = "Sparkline window",
        min = 10,
        max = 1000,
        tooltip = "Number of historical points kept on the live sparklines."
    )]
    spark_window: usize,
    #[field(
        label = "Refresh interval (ms)",
        min = 250,
        step = 250,
        tooltip = "Min interval between LCD repaints (ms). 2000 ms keeps Mali-400 + lima happy at ~3% CPU."
    )]
    refresh_ms: u64,

    /// Round-tripped via serde, never shown in the form. The
    /// dashboard SLINT app reads this directly from the TOML; the
    /// web UI doesn't surface it because operators rarely change
    /// it.
    #[field(skip)]
    socket: String,
}

impl Default for Cfg {
    fn default() -> Self {
        Self {
            width: 800,
            height: 480,
            title: "bananas-dashboard".into(),
            theme: "auto".into(),
            spark_window: 60,
            refresh_ms: 2000,
            socket: "/run/bananas/stats.sock".into(),
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct DashboardConfigFormProps {
    #[props(default = true)]
    pub inline_save: bool,
}

/// Plugin wrapper — resolves the `AuthCtx` and forwards the
/// `signal_unauthorized` callback into the macro-generated view.
#[component]
pub fn DashboardConfigForm(props: DashboardConfigFormProps) -> Element {
    let auth_ctx = use_context::<AuthCtx>();
    Cfg::view(ConfigFormProps {
        inline_save: props.inline_save,
        on_unauthorized: EventHandler::new(move |_| auth_ctx.signal_unauthorized()),
    })
}
