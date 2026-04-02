use std::collections::HashMap;
use std::io::stdout;
use std::path::PathBuf;

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Terminal,
};

mod loader;
mod suggest;

setup_command! {
    /// Path to a .jsonl session file, project directory, or Claude projects directory.
    /// Defaults to ~/.claude/projects/
    path: Option<PathBuf>,
}

fn short_project_name(project: &str) -> &str {
    if let Some(pos) = project.find("code-") {
        let after = &project[pos + 5..];
        if !after.is_empty() {
            return after;
        }
    }
    project.trim_start_matches('-')
}

fn short_session_id(session_id: &str) -> &str {
    let uuid_part = session_id.rsplit('/').next().unwrap_or(session_id);
    &uuid_part[..uuid_part.len().min(8)]
}

fn build_lines(result: &suggest::SuggestResult, width: u16) -> Vec<Line<'static>> {
    // Group session IDs by project
    let mut by_project: HashMap<String, Vec<String>> = HashMap::new();
    for session_id in result.sessions.keys() {
        let project = session_id.split('/').next().unwrap_or("").to_string();
        by_project
            .entry(project)
            .or_default()
            .push(session_id.clone());
    }
    for project in result.project_files.keys() {
        by_project.entry(project.clone()).or_default();
    }

    let mut projects: Vec<String> = by_project.keys().cloned().collect();
    projects.sort();

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::default());

    // ── Per-project sections ──────────────────────────────────────────────────
    for project in &projects {
        let project_color = Color::Green;
        let label = short_project_name(project).to_string();
        let rule_len = (width as usize).saturating_sub(label.len() + 5);
        let rule = "─".repeat(rule_len);

        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("─ ", Style::new().fg(project_color).dim()),
            Span::styled(label, Style::new().fg(project_color).bold()),
            Span::styled(format!(" {}", rule), Style::new().fg(project_color).dim()),
        ]));
        lines.push(Line::default());

        // Co-read pairs for this project
        if let Some(pairs) = result.coread_pairs.get(project) {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled("always together  ", Style::new().dark_gray()),
            ]));
            for (a, b, count) in pairs.iter().take(5) {
                lines.push(Line::from(vec![
                    Span::raw("      "),
                    Span::styled(a.clone(), Style::new().fg(Color::White)),
                    Span::styled(" + ", Style::new().dark_gray()),
                    Span::styled(b.clone(), Style::new().fg(Color::White)),
                    Span::raw("  "),
                    Span::styled(format!("{}×", count), Style::new().dark_gray()),
                ]));
            }
            lines.push(Line::default());
        }

        // Sort sessions by max_severity descending, then date descending
        let mut session_ids = by_project[project].clone();
        session_ids.sort_by(|a, b| {
            let sa = result.sessions.get(a);
            let sb = result.sessions.get(b);
            let sev_a = sa.map(|s| s.max_severity).unwrap_or(0);
            let sev_b = sb.map(|s| s.max_severity).unwrap_or(0);
            sev_b.cmp(&sev_a).then_with(|| {
                let date_a = sa.map(|s| s.start_date.as_str()).unwrap_or("");
                let date_b = sb.map(|s| s.start_date.as_str()).unwrap_or("");
                date_b.cmp(date_a)
            })
        });

        for session_id in &session_ids {
            let session_result = match result.sessions.get(session_id) {
                Some(sr) => sr,
                None => continue,
            };

            let suggestions = &session_result.suggestions;
            let n = suggestions.len();
            let short_id = short_session_id(session_id).to_string();
            let start_date = session_result.start_date.clone();

            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(start_date, Style::new().fg(Color::White)),
                Span::raw("  "),
                Span::styled(short_id, Style::new().dark_gray()),
                Span::raw("  "),
                Span::styled(
                    format!("{} suggestion{}", n, if n == 1 { "" } else { "s" }),
                    Style::new().dark_gray(),
                ),
            ]));

            for s in suggestions {
                lines.push(Line::default());

                let (sev_label, sev_style) = match s.severity.as_str() {
                    "high" => ("high", Style::new().fg(Color::Red).bold()),
                    "medium" => ("med ", Style::new().fg(Color::Yellow).bold()),
                    _ => ("low ", Style::new().dark_gray()),
                };

                lines.push(Line::from(vec![
                    Span::raw("      "),
                    Span::styled(sev_label, sev_style),
                    Span::raw("  "),
                    Span::styled(s.title.clone(), Style::new().bold()),
                ]));

                if let Some(after) = &s.example_after {
                    lines.push(Line::from(vec![
                        Span::raw("            "),
                        Span::styled("→ ", Style::new().fg(Color::Cyan)),
                        Span::styled(after.clone(), Style::new().fg(Color::Cyan)),
                    ]));
                }
            }
            lines.push(Line::default());
        }
    }

    lines
}

struct App {
    lines: Vec<Line<'static>>,
    scroll: usize,
    header: String,
    footer: String,
}

impl App {
    fn scroll_down(&mut self, viewport_height: usize) {
        let max = self.lines.len().saturating_sub(viewport_height);
        self.scroll = (self.scroll + 1).min(max);
    }

    fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    fn page_down(&mut self, viewport_height: usize) {
        let max = self.lines.len().saturating_sub(viewport_height);
        self.scroll = (self.scroll + viewport_height / 2).min(max);
    }

    fn page_up(&mut self, viewport_height: usize) {
        self.scroll = self.scroll.saturating_sub(viewport_height / 2);
    }

    fn go_top(&mut self) {
        self.scroll = 0;
    }

    fn go_bottom(&mut self, viewport_height: usize) {
        self.scroll = self.lines.len().saturating_sub(viewport_height);
    }
}

fn draw(app: &App, frame: &mut ratatui::Frame) {
    let area = frame.area();

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    let viewport_height = chunks[1].height as usize;

    // Header
    frame.render_widget(
        Paragraph::new(app.header.clone())
            .style(Style::new().bg(Color::DarkGray).fg(Color::White).bold()),
        chunks[0],
    );

    // Content — slice the lines manually for correct scroll behaviour
    let visible: Vec<Line> = app
        .lines
        .iter()
        .skip(app.scroll)
        .take(viewport_height)
        .cloned()
        .collect();
    frame.render_widget(Paragraph::new(visible), chunks[1]);

    // Scrollbar
    let content_len = app.lines.len().saturating_sub(viewport_height);
    let mut sb_state = ScrollbarState::default()
        .content_length(content_len)
        .position(app.scroll);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None),
        chunks[1],
        &mut sb_state,
    );

    // Footer
    frame.render_widget(
        Paragraph::new(app.footer.clone())
            .style(Style::new().bg(Color::DarkGray).fg(Color::White)),
        chunks[2],
    );
}

pub async fn run(opts: Options) -> Result<()> {
    let path = opts.path.unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(format!("{}/.claude/projects", home))
    });

    let mut sessions = loader::load_path(&path);

    if sessions.is_empty() {
        println!();
        println!("  No sessions found.");
        println!();
        return Ok(());
    }

    suggest::fingerprint_sessions(&mut sessions);
    let result = suggest::generate_suggestions(&sessions);

    if result.sessions.is_empty() && result.project_files.is_empty() {
        println!();
        println!("  ✓  No suggestions — all sessions look clean.");
        println!();
        return Ok(());
    }

    let total_count: usize = result.sessions.values().map(|sr| sr.suggestions.len()).sum();
    let with_suggestions = result.sessions.len();
    let total = result.total_sessions;

    // Set up terminal
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;

    let width = terminal.size()?.width;
    let lines = build_lines(&result, width);

    let header = format!(
        "  edgee suggest  ·  {}  ·  {} sessions",
        path.display(),
        total
    );
    let footer = format!(
        "  ↑↓ jk  scroll    d/u  half-page    g/G  top/bottom    q  quit    \
         {} suggestion{}  ·  {}/{} sessions",
        total_count,
        if total_count == 1 { "" } else { "s" },
        with_suggestions,
        total,
    );

    let mut app = App {
        lines,
        scroll: 0,
        header,
        footer,
    };

    let run_result = (|| -> Result<()> {
        loop {
            terminal.draw(|f| draw(&app, f))?;

            if event::poll(std::time::Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    let vh = (terminal.size()?.height as usize).saturating_sub(2);
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Down | KeyCode::Char('j') => app.scroll_down(vh),
                        KeyCode::Up | KeyCode::Char('k') => app.scroll_up(),
                        KeyCode::Char('d') | KeyCode::PageDown => app.page_down(vh),
                        KeyCode::Char('u') | KeyCode::PageUp => app.page_up(vh),
                        KeyCode::Char('g') => app.go_top(),
                        KeyCode::Char('G') => app.go_bottom(vh),
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    })();

    // Always restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    run_result
}
