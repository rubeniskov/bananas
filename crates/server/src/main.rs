use std::{net::SocketAddr, path::PathBuf};

use anyhow::Result;
use axum::{
    Form, Router,
    extract::State,
    response::{Html, IntoResponse, Redirect},
    routing::get,
};
use bananas_helper::{Command, Response};
use serde::Deserialize;
use tower_http::trace::TraceLayer;

mod exports;

#[derive(Clone)]
struct AppState {
    exports_path: PathBuf,
    helper_socket: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "bananas_server=info,tower_http=info".into()),
        )
        .init();

    let state = AppState {
        exports_path: std::env::var_os("BANANAS_EXPORTS_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| "/etc/exports".into()),
        helper_socket: std::env::var_os("BANANAS_HELPER_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(|| "/run/bananas-helper.sock".into()),
    };

    let app = Router::new()
        .route("/", get(|| async { Redirect::permanent("/exports") }))
        .route("/exports", get(get_exports).post(post_exports))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr: SocketAddr = std::env::var("BANANAS_LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()?;
    tracing::info!(%addr, "listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn get_exports(State(state): State<AppState>) -> impl IntoResponse {
    let raw = std::fs::read_to_string(&state.exports_path).unwrap_or_default();
    let parsed = exports::parse(&raw);
    Html(render(&raw, &parsed, None)).into_response()
}

#[derive(Debug, Deserialize)]
struct ExportsForm {
    content: String,
}

async fn post_exports(
    State(state): State<AppState>,
    Form(form): Form<ExportsForm>,
) -> impl IntoResponse {
    let cmd = Command::WriteExports { content: form.content.clone() };
    let result = bananas_helper::call(&state.helper_socket, &cmd).await;
    let parsed = exports::parse(&form.content);
    let banner = match result {
        Ok(Response { ok: true, output, .. }) => {
            Banner::ok(format!("Saved.\n\n{output}"))
        }
        Ok(Response { ok: false, error, output, .. }) => Banner::err(format!(
            "{}\n\n{}",
            error.as_deref().unwrap_or("helper rejected the change"),
            output
        )),
        Err(e) => Banner::err(format!(
            "Could not reach helper at {}: {e}",
            state.helper_socket.display()
        )),
    };
    Html(render(&form.content, &parsed, Some(banner))).into_response()
}

struct Banner {
    kind: &'static str,
    msg: String,
}

impl Banner {
    fn ok(msg: String) -> Self { Self { kind: "ok", msg } }
    fn err(msg: String) -> Self { Self { kind: "err", msg } }
}

fn render(raw: &str, parsed: &[exports::Export], banner: Option<Banner>) -> String {
    let banner_html = banner
        .map(|b| {
            format!(
                r#"<div class="banner {kind}"><pre>{msg}</pre></div>"#,
                kind = b.kind,
                msg = html_escape(&b.msg)
            )
        })
        .unwrap_or_default();
    let table = if parsed.is_empty() {
        r#"<p class="empty">No exports defined.</p>"#.to_string()
    } else {
        let rows: String = parsed
            .iter()
            .map(|e| {
                format!(
                    "<tr><td><code>{}</code></td><td>{}</td></tr>",
                    html_escape(&e.path),
                    e.clients
                        .iter()
                        .map(|c| format!(
                            "<code>{}({})</code>",
                            html_escape(&c.host),
                            html_escape(&c.options)
                        ))
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            })
            .collect();
        format!(
            "<table><thead><tr><th>Path</th><th>Clients</th></tr></thead><tbody>{rows}</tbody></table>"
        )
    };
    format!(
        r#"<!doctype html>
<html lang="en"><head>
<meta charset="utf-8"><title>bananas-server — NFS exports</title>
<style>
body {{ font: 14px/1.5 system-ui, sans-serif; max-width: 1000px; margin: 2em auto; padding: 0 1em; }}
h1 {{ font-size: 18px; }}
table {{ border-collapse: collapse; width: 100%; margin-bottom: 1.5em; }}
th, td {{ text-align: left; padding: 6px 10px; border-bottom: 1px solid #ddd; }}
code {{ font: 13px/1 ui-monospace, monospace; background: #f4f4f4; padding: 1px 4px; border-radius: 3px; }}
textarea {{ width: 100%; min-height: 220px; font: 13px/1.4 ui-monospace, monospace; padding: 8px; box-sizing: border-box; }}
button {{ padding: 8px 16px; font-size: 14px; cursor: pointer; }}
.empty {{ color: #888; padding: 2em; text-align: center; }}
.banner {{ padding: 10px 14px; margin-bottom: 1em; border-radius: 4px; }}
.banner.ok {{ background: #e6ffed; border: 1px solid #34d058; }}
.banner.err {{ background: #ffeef0; border: 1px solid #d73a49; }}
.banner pre {{ margin: 0; white-space: pre-wrap; word-break: break-word; }}
</style>
</head>
<body>
<h1>NFS exports</h1>
{banner_html}
{table}
<form method="post" action="/exports">
<label for="content">Edit <code>/etc/exports</code></label>
<textarea id="content" name="content" spellcheck="false">{raw}</textarea>
<p><button type="submit">Save &amp; reload</button></p>
</form>
</body></html>"#,
        raw = html_escape(raw),
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
