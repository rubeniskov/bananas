//! ratatui App. Three tabs:
//!   - Status: versions + service state, refresh-on-key.
//!   - Network: timezone editor.
//!   - Reboot: confirmation pane.
//!
//! Key bindings:
//!   - Tab / Shift+Tab — switch tabs
//!   - r — refresh (in any tab)
//!   - q / Ctrl+C — quit
//!   - Enter — confirm (Reboot tab) / save (Network tab)
//!
//! Designed to fit the 5″ LCD's 800×480 in software-rendered terminal
//! emulation, but uses crossterm so anything from `ssh` to `tmux` to
//! the BPI's serial console works.

use std::{path::Path, time::Duration};

use anyhow::Result;
use bananas_helper::Command as HelperCommand;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Tabs, Wrap},
};

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Status,
    Network,
    Reboot,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Status => "Status",
            Tab::Network => "Network",
            Tab::Reboot => "Reboot",
        }
    }
    const ALL: [Tab; 3] = [Tab::Status, Tab::Network, Tab::Reboot];
    fn index(self) -> usize {
        Self::ALL.iter().position(|&t| t == self).unwrap_or(0)
    }
    fn next(self) -> Tab {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }
    fn prev(self) -> Tab {
        let n = Self::ALL.len();
        Self::ALL[(self.index() + n - 1) % n]
    }
}

#[derive(Default)]
struct AppState {
    tab: Option<Tab>,
    versions: std::collections::HashMap<String, String>,
    units: std::collections::HashMap<String, String>,
    timezone: String,
    tz_input: String,
    tz_busy: bool,
    reboot_busy: bool,
    flash: Option<(bool, String)>,
}

pub async fn run(socket: &Path) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = AppState {
        tab: Some(Tab::Status),
        ..Default::default()
    };

    refresh(socket, &mut state).await;

    loop {
        terminal.draw(|f| render(f, &state))?;

        // Use a short poll so async tasks can wake us — the ratatui
        // event loop is sync, so we tick every 100 ms and step the
        // tokio runtime in between.
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match (key.modifiers, key.code) {
                    (_, KeyCode::Char('q')) | (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                        break;
                    }
                    (_, KeyCode::Tab) => state.tab = Some(state.tab.unwrap_or(Tab::Status).next()),
                    (KeyModifiers::SHIFT, KeyCode::BackTab) => {
                        state.tab = Some(state.tab.unwrap_or(Tab::Status).prev())
                    }
                    (_, KeyCode::Char('r')) => refresh(socket, &mut state).await,
                    _ => {
                        handle_tab_key(socket, &mut state, key).await;
                    }
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

async fn refresh(socket: &Path, state: &mut AppState) {
    state.versions = crate::cli::read_versions(socket).await.unwrap_or_default();
    state.units = crate::cli::unit_states().await;
    state.timezone = current_timezone(socket).await.unwrap_or_default();
    state.tz_input = state.timezone.clone();
}

async fn current_timezone(socket: &Path) -> Result<String> {
    let resp = bananas_helper::call(
        socket,
        &HelperCommand::ReadServiceConfig {
            name: "system".into(),
        },
    )
    .await?;
    if !resp.ok {
        return Ok(String::new());
    }
    let parsed: toml::Table = resp.output.parse().unwrap_or_default();
    Ok(parsed
        .get("system")
        .and_then(|v| v.as_table())
        .and_then(|t| t.get("timezone"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_default())
}

async fn handle_tab_key(socket: &Path, state: &mut AppState, key: event::KeyEvent) {
    match state.tab.unwrap_or(Tab::Status) {
        Tab::Status => {}
        Tab::Network => match key.code {
            KeyCode::Enter => {
                if state.tz_busy {
                    return;
                }
                state.tz_busy = true;
                let resp = bananas_helper::call(
                    socket,
                    &HelperCommand::SetTimezone {
                        tz: state.tz_input.clone(),
                    },
                )
                .await;
                state.tz_busy = false;
                match resp {
                    Ok(r) if r.ok => {
                        state.timezone = state.tz_input.clone();
                        state.flash = Some((true, format!("Timezone set to {}.", state.timezone)));
                    }
                    Ok(r) => {
                        state.flash = Some((
                            false,
                            r.error.unwrap_or_else(|| "helper refused".into()),
                        ));
                    }
                    Err(e) => state.flash = Some((false, format!("helper unreachable: {e}"))),
                }
            }
            KeyCode::Char(c) if !c.is_control() => {
                state.tz_input.push(c);
            }
            KeyCode::Backspace => {
                state.tz_input.pop();
            }
            _ => {}
        },
        Tab::Reboot => {
            if let KeyCode::Enter = key.code {
                if state.reboot_busy {
                    return;
                }
                state.reboot_busy = true;
                let resp = bananas_helper::call(socket, &HelperCommand::RebootSystem).await;
                state.reboot_busy = false;
                match resp {
                    Ok(r) if r.ok => {
                        state.flash =
                            Some((true, "Reboot triggered. Connection will drop.".into()));
                    }
                    Ok(r) => {
                        state.flash = Some((
                            false,
                            r.error.unwrap_or_else(|| "helper refused".into()),
                        ));
                    }
                    Err(e) => state.flash = Some((false, format!("helper unreachable: {e}"))),
                }
            }
        }
    }
}

fn render(f: &mut ratatui::Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // tabs
            Constraint::Min(3),    // body
            Constraint::Length(2), // status bar
        ])
        .split(f.area());

    // Tabs.
    let titles: Vec<Line> = Tab::ALL
        .iter()
        .map(|t| Line::from(Span::raw(t.label())))
        .collect();
    let tabs = Tabs::new(titles)
        .select(state.tab.unwrap_or(Tab::Status).index())
        .block(Block::default().title("bananas-config").borders(Borders::ALL))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(tabs, chunks[0]);

    // Body.
    match state.tab.unwrap_or(Tab::Status) {
        Tab::Status => render_status(f, chunks[1], state),
        Tab::Network => render_network(f, chunks[1], state),
        Tab::Reboot => render_reboot(f, chunks[1], state),
    }

    // Status bar.
    let bar = Paragraph::new(Line::from(vec![
        Span::styled(" Tab ", Style::default().add_modifier(Modifier::REVERSED)),
        Span::raw(" switch tab    "),
        Span::styled(" r ", Style::default().add_modifier(Modifier::REVERSED)),
        Span::raw(" refresh    "),
        Span::styled(" q ", Style::default().add_modifier(Modifier::REVERSED)),
        Span::raw(" quit"),
    ]))
    .block(Block::default().borders(Borders::TOP));
    f.render_widget(bar, chunks[2]);
}

fn render_status(f: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let header = Line::from(vec![
        Span::styled("Component        ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled("Installed     ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled("Service", Style::default().add_modifier(Modifier::BOLD)),
    ]);
    let mut items: Vec<ListItem> = vec![ListItem::new(header)];

    for (component, unit) in [
        ("server", Some("bananas-server.service")),
        ("helper", Some("bananas-helper.service")),
        ("stats", Some("bananas-stats.service")),
        ("dashboard", Some("bananas-dashboard.service")),
        ("webadmin", None),
    ] {
        let installed = state
            .versions
            .get(component)
            .map(|s| s.as_str())
            .unwrap_or("—");
        let unit_state = unit
            .and_then(|u| state.units.get(u))
            .map(|s| s.as_str())
            .unwrap_or("n/a");
        let color = match unit_state {
            "active" => Color::Green,
            "inactive" | "failed" => Color::Red,
            _ => Color::Gray,
        };
        items.push(ListItem::new(Line::from(vec![
            Span::raw(format!("{component:<17}")),
            Span::raw(format!("{installed:<14}")),
            Span::styled(unit_state.to_string(), Style::default().fg(color)),
        ])));
    }

    let list = List::new(items)
        .block(Block::default().title(" Status (press r to refresh) ").borders(Borders::ALL));
    f.render_widget(list, area);
}

fn render_network(f: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // current
            Constraint::Length(3), // editor
            Constraint::Min(0),    // flash + help
        ])
        .split(area);

    let current = Paragraph::new(format!(
        "Current timezone: {}",
        if state.timezone.is_empty() {
            "(unset — defaults to UTC)"
        } else {
            &state.timezone
        }
    ))
    .block(Block::default().title(" Timezone ").borders(Borders::ALL));
    f.render_widget(current, chunks[0]);

    let editor = Paragraph::new(state.tz_input.clone()).block(
        Block::default()
            .title(if state.tz_busy {
                " Setting… "
            } else {
                " Edit (Enter to apply) "
            })
            .borders(Borders::ALL),
    );
    f.render_widget(editor, chunks[1]);

    let mut help: Vec<Line> = vec![Line::from(
        "IANA tzdata zone name (e.g. Europe/Madrid, America/New_York). \
         Persists to system.toml and runs `timedatectl set-timezone`.",
    )];
    if let Some((ok, msg)) = &state.flash {
        let style = if *ok {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Red)
        };
        help.push(Line::from(""));
        help.push(Line::from(Span::styled(msg.clone(), style)));
    }
    let help = Paragraph::new(help).wrap(Wrap { trim: false });
    f.render_widget(help, chunks[2]);
}

fn render_reboot(f: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Reboot the system?",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  • Web UI / SSH session drops for ~30 seconds."),
        Line::from("  • Cloud-sync runs in flight are interrupted."),
        Line::from("  • Cron resumes after boot."),
        Line::from(""),
        Line::from(Span::styled(
            if state.reboot_busy {
                "Sending reboot command…"
            } else {
                "Press Enter to confirm, q to cancel."
            },
            Style::default().add_modifier(Modifier::ITALIC),
        )),
    ];
    if let Some((ok, msg)) = &state.flash {
        lines.push(Line::from(""));
        let style = if *ok {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Red)
        };
        lines.push(Line::from(Span::styled(msg.clone(), style)));
    }
    let para = Paragraph::new(lines)
        .block(Block::default().title(" Reboot ").borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    f.render_widget(para, area);
}
