use std::{net::SocketAddr, path::PathBuf};

use anyhow::Result;
use axum::{
    Form, Router,
    extract::{Path, State},
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
};
use bananas_helper::{Command, Response};
use serde::Deserialize;
use tower_http::trace::TraceLayer;

mod exports;
use exports::{Opts, Row, Squash};

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
            .unwrap_or_else(|| "/run/bananas/helper.sock".into()),
    };

    let app = Router::new()
        .route("/", get(|| async { Redirect::permanent("/exports") }))
        .route("/exports", get(get_exports))
        .route("/exports/add", post(post_add))
        .route("/exports/:idx/delete", post(post_delete))
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
    Html(render(exports::rows(&raw), None)).into_response()
}

#[derive(Debug, Deserialize)]
struct AddForm {
    path: String,
    host: String,
    #[serde(default = "default_access")]
    access: String,
    #[serde(default)]
    sync: Option<String>,
    #[serde(default)]
    no_subtree_check: Option<String>,
    #[serde(default = "default_squash")]
    squash: String,
    anonuid: Option<u32>,
    anongid: Option<u32>,
    #[serde(default)]
    insecure: Option<String>,
}

fn default_access() -> String { "rw".into() }
fn default_squash() -> String { "all_squash".into() }

async fn post_add(
    State(state): State<AppState>,
    Form(form): Form<AddForm>,
) -> impl IntoResponse {
    if form.path.trim().is_empty() || form.host.trim().is_empty() {
        let raw = std::fs::read_to_string(&state.exports_path).unwrap_or_default();
        return Html(render(
            exports::rows(&raw),
            Some(Banner::err("path and host are required".into())),
        )).into_response();
    }
    let opts = Opts {
        rw: form.access == "rw",
        sync: form.sync.is_some(),
        no_subtree_check: form.no_subtree_check.is_some(),
        squash: Squash::from_form(&form.squash),
        anonuid: form.anonuid,
        anongid: form.anongid,
        insecure: form.insecure.is_some(),
        extra: vec![],
    };
    let new_row = Row {
        path: form.path.trim().to_string(),
        host: form.host.trim().to_string(),
        options: opts.to_options_string(),
    };
    let raw = std::fs::read_to_string(&state.exports_path).unwrap_or_default();
    let mut rows = exports::rows(&raw);
    rows.push(new_row);
    apply_and_redirect(&state, &rows).await
}

async fn post_delete(
    State(state): State<AppState>,
    Path(idx): Path<usize>,
) -> impl IntoResponse {
    let raw = std::fs::read_to_string(&state.exports_path).unwrap_or_default();
    let mut rows = exports::rows(&raw);
    if idx >= rows.len() {
        return Html(render(rows, Some(Banner::err(format!("row {} not found", idx))))).into_response();
    }
    rows.remove(idx);
    apply_and_redirect(&state, &rows).await
}

async fn apply_and_redirect(state: &AppState, rows: &[Row]) -> axum::response::Response {
    let content = exports::serialize(rows);
    let cmd = Command::WriteExports { content };
    match bananas_helper::call(&state.helper_socket, &cmd).await {
        Ok(Response { ok: true, .. }) => Redirect::to("/exports").into_response(),
        Ok(Response { error, output, .. }) => Html(render(
            rows.to_vec(),
            Some(Banner::err(format!(
                "{}\n\n{}",
                error.as_deref().unwrap_or("helper rejected the change"),
                output
            ))),
        )).into_response(),
        Err(e) => Html(render(
            rows.to_vec(),
            Some(Banner::err(format!(
                "Could not reach helper at {}: {e}",
                state.helper_socket.display()
            ))),
        )).into_response(),
    }
}

struct Banner {
    kind: &'static str,
    msg: String,
}

impl Banner {
    fn err(msg: String) -> Self { Self { kind: "err", msg } }
}

fn render(rows: Vec<Row>, banner: Option<Banner>) -> String {
    let banner_html = banner
        .map(|b| {
            format!(
                r#"<div class="banner {kind}"><pre>{msg}</pre></div>"#,
                kind = b.kind,
                msg = html_escape(&b.msg)
            )
        })
        .unwrap_or_default();

    let table_html = if rows.is_empty() {
        r#"<p class="empty">No exports defined yet — add one below.</p>"#.to_string()
    } else {
        let body: String = rows
            .iter()
            .enumerate()
            .map(|(idx, r)| {
                let opts = Opts::parse(&r.options);
                let badges = options_badges(&opts);
                format!(
                    r#"<tr>
  <td><code>{path}</code></td>
  <td><code>{host}</code></td>
  <td>{badges}</td>
  <td>
    <form method="post" action="/exports/{idx}/delete" onsubmit="return confirm('Delete this export?')">
      <button type="submit" class="del" title="Delete">×</button>
    </form>
  </td>
</tr>"#,
                    path = html_escape(&r.path),
                    host = html_escape(&r.host),
                )
            })
            .collect();
        format!(
            r#"<table class="rows">
  <thead><tr><th>Path</th><th>Client</th><th>Options</th><th></th></tr></thead>
  <tbody>{body}</tbody>
</table>"#
        )
    };

    let preview = exports::serialize(&rows);

    format!(
        r#"<!doctype html>
<html lang="en"><head>
<meta charset="utf-8"><title>bananas-server — NFS exports</title>
<style>
:root {{ --line:#e0e0e0; --bg-soft:#fafafa; --accent:#0366d6; --danger:#d73a49; }}
body {{ font: 14px/1.5 system-ui, sans-serif; max-width: 1100px; margin: 2em auto; padding: 0 1em; color: #222; }}
h1 {{ font-size: 22px; margin-bottom: .2em; }}
h2 {{ font-size: 16px; margin-top: 2em; }}
nav {{ font-size: 13px; color: #666; margin-bottom: 1.5em; }}
nav a {{ color: var(--accent); text-decoration: none; margin-right: 1em; }}
table {{ border-collapse: collapse; width: 100%; margin: .5em 0 1.5em; }}
th, td {{ text-align: left; padding: 8px 10px; border-bottom: 1px solid var(--line); vertical-align: top; }}
th {{ font-weight: 600; color: #555; font-size: 12px; text-transform: uppercase; letter-spacing: .04em; }}
table.rows tr:hover {{ background: var(--bg-soft); }}
code {{ font: 13px/1 ui-monospace, monospace; background: #f4f4f4; padding: 1px 5px; border-radius: 3px; }}
.empty {{ color: #888; padding: 1em; text-align: center; border: 1px dashed var(--line); border-radius: 4px; }}
.badges {{ display: flex; flex-wrap: wrap; gap: 4px; }}
.badge {{ font: 11px/1.4 ui-monospace, monospace; background: #eef; color: #225; padding: 2px 6px; border-radius: 3px; }}
.badge.ro {{ background: #fee; color: #722; }}
.badge.rw {{ background: #efe; color: #272; }}
.badge.warn {{ background: #ffe; color: #850; }}
button {{ font: 14px system-ui, sans-serif; padding: 7px 14px; border: 1px solid #ccc; background: white; border-radius: 4px; cursor: pointer; }}
button.del {{ padding: 0 10px; font-size: 18px; line-height: 28px; color: var(--danger); border-color: transparent; }}
button.del:hover {{ background: #fee; border-color: #fbb; }}
button.primary {{ background: var(--accent); color: white; border-color: var(--accent); font-weight: 600; }}
form.add {{ display: grid; gap: 12px; padding: 1em; border: 1px solid var(--line); border-radius: 6px; background: var(--bg-soft); }}
form.add .row {{ display: grid; grid-template-columns: 100px 1fr; align-items: center; gap: 8px; }}
form.add input[type=text], form.add input[type=number] {{ font: 14px ui-monospace, monospace; padding: 6px 8px; border: 1px solid #ccc; border-radius: 4px; }}
form.add input[type=number] {{ width: 100px; }}
fieldset {{ border: 1px solid var(--line); border-radius: 4px; padding: .8em 1em; margin: 0; }}
legend {{ font-size: 12px; color: #555; padding: 0 .4em; }}
fieldset .opts {{ display: flex; flex-wrap: wrap; gap: 1em 1.5em; align-items: center; }}
fieldset label {{ display: inline-flex; align-items: center; gap: 4px; cursor: pointer; }}
textarea {{ width: 100%; min-height: 120px; font: 13px/1.4 ui-monospace, monospace; padding: 8px; box-sizing: border-box; background: #f9f9f9; color: #444; border: 1px solid var(--line); border-radius: 4px; }}
.banner {{ padding: 10px 14px; margin-bottom: 1em; border-radius: 4px; }}
.banner.err {{ background: #ffeef0; border: 1px solid var(--danger); }}
.banner pre {{ margin: 0; white-space: pre-wrap; word-break: break-word; }}
.preview-label {{ font-size: 12px; color: #666; margin-bottom: 4px; }}
</style>
</head>
<body>
<h1>NFS exports</h1>
<nav><a href="/exports">Exports</a></nav>
{banner_html}

<h2>Active exports</h2>
{table_html}

<h2>Add a new export</h2>
<form class="add" method="post" action="/exports/add">
  <div class="row"><label for="path">Path</label><input type="text" id="path" name="path" placeholder="/srv/something" required></div>
  <div class="row"><label for="host">Client</label><input type="text" id="host" name="host" placeholder="192.168.1.0/24 or *" required></div>
  <fieldset>
    <legend>Options</legend>
    <div class="opts">
      <label><input type="radio" name="access" value="rw" checked> rw</label>
      <label><input type="radio" name="access" value="ro"> ro</label>
      <label><input type="checkbox" name="sync" checked> sync</label>
      <label><input type="checkbox" name="no_subtree_check" checked> no_subtree_check</label>
      <label><input type="checkbox" name="insecure" checked> insecure</label>
      <label>squash:
        <select name="squash">
          <option value="all_squash" selected>all_squash</option>
          <option value="root_squash">root_squash</option>
          <option value="no_root_squash">no_root_squash</option>
        </select>
      </label>
      <label>anonuid: <input type="number" name="anonuid" value="1000" min="0" max="65535"></label>
      <label>anongid: <input type="number" name="anongid" value="1000" min="0" max="65535"></label>
    </div>
  </fieldset>
  <div><button type="submit" class="primary">Add export</button></div>
</form>

<h2>/etc/exports preview</h2>
<p class="preview-label">Read-only — generated from the rows above.</p>
<textarea readonly disabled>{preview}</textarea>

</body></html>"#,
        preview = html_escape(&preview),
    )
}

fn options_badges(o: &Opts) -> String {
    let mut out = String::from(r#"<div class="badges">"#);
    if o.rw {
        out.push_str(r#"<span class="badge rw">rw</span>"#);
    } else {
        out.push_str(r#"<span class="badge ro">ro</span>"#);
    }
    if o.sync {
        out.push_str(r#"<span class="badge">sync</span>"#);
    } else {
        out.push_str(r#"<span class="badge warn">async</span>"#);
    }
    if o.no_subtree_check {
        out.push_str(r#"<span class="badge">no_subtree_check</span>"#);
    }
    out.push_str(&format!(r#"<span class="badge">{}</span>"#, o.squash.as_str()));
    if let Some(uid) = o.anonuid {
        out.push_str(&format!(r#"<span class="badge">anonuid={}</span>"#, uid));
    }
    if let Some(gid) = o.anongid {
        out.push_str(&format!(r#"<span class="badge">anongid={}</span>"#, gid));
    }
    if o.insecure {
        out.push_str(r#"<span class="badge warn">insecure</span>"#);
    }
    for x in &o.extra {
        out.push_str(&format!(
            r#"<span class="badge">{}</span>"#,
            html_escape(x)
        ));
    }
    out.push_str("</div>");
    out
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
