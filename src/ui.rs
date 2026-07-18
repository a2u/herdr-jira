//! Rendering. One draw function per view; popup pickers render on top of the
//! issue list.

use crate::app::{is_epic, App, View};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, List, ListItem, ListState, Padding, Paragraph, Row,
    Table, TableState, Wrap,
};
use ratatui::Frame;

const ACCENT: Color = Color::Cyan;

pub fn draw(f: &mut Frame, app: &App) {
    if let Some(err) = &app.fatal {
        draw_fatal(f, err);
        return;
    }
    let [main, footer] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(f.area());

    match app.view {
        View::Detail => draw_detail(f, app, main),
        _ => draw_list(f, app, main),
    }
    draw_footer(f, app, footer);

    match app.view {
        View::FilterPicker => draw_filter_picker(f, app),
        View::TransitionPicker => draw_transition_picker(f, app),
        View::AgentPicker => draw_agent_picker(f, app),
        View::NewAgentTypePicker => draw_new_agent_type_picker(f, app),
        View::NewAgentWorkspacePicker => draw_new_agent_workspace_picker(f, app),
        View::NewAgentCwdPicker => draw_new_agent_cwd_picker(f, app),
        View::NewAgentCwdInput => draw_cwd_input(f, app),
        View::SearchInput => draw_search(f, app),
        View::JqlInput => draw_jql(f, app),
        View::Help => draw_help(f),
        _ => {}
    }
}

fn status_color(category: &str) -> Color {
    match category {
        "new" => Color::Blue,
        "indeterminate" => Color::Yellow,
        "done" => Color::Green,
        _ => Color::Gray,
    }
}

fn type_style(issue_type: &str) -> Style {
    match issue_type.to_ascii_lowercase().as_str() {
        "epic" => Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD),
        "bug" | "defect" => Style::new().fg(Color::Red),
        "story" => Style::new().fg(Color::Green),
        "task" => Style::new().fg(Color::Blue),
        "sub-task" | "subtask" => Style::new().fg(Color::Cyan),
        _ => Style::new().fg(Color::Gray),
    }
}

fn draw_list(f: &mut Frame, app: &App, area: Rect) {
    let title = if app.current_title.is_empty() {
        "Jira".to_string()
    } else {
        format!("Jira — {} ({})", app.current_title, app.issues.len())
    };
    let suffix = if app.loading { "  ⟳ loading…" } else { "" };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Line::from(vec![
            Span::styled(format!(" {title} "), Style::new().fg(ACCENT).bold()),
            Span::styled(suffix, Style::new().fg(Color::Yellow)),
        ]))
        .title_alignment(Alignment::Left);

    let visible = app.visible();
    let rows: Vec<Row> = visible
        .iter()
        .map(|&(i, depth)| {
            // Epics carry an expand indicator; their children are indented.
            let (prefix, key_style) = if is_epic(i) {
                let arrow = if app.expanded.contains(&i.key) { "▾ " } else { "▸ " };
                (arrow.to_string(), Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD))
            } else if depth > 0 {
                (" └ ".to_string(), Style::new().fg(ACCENT))
            } else {
                ("  ".to_string(), Style::new().fg(ACCENT))
            };
            Row::new(vec![
                Cell::from(format!("{prefix}{}", i.key)).style(key_style),
                Cell::from(i.issue_type.clone()).style(type_style(&i.issue_type)),
                Cell::from(i.status.clone()).style(Style::new().fg(status_color(&i.status_category))),
                Cell::from(i.assignee.clone()).style(Style::new().fg(Color::Magenta)),
                Cell::from(i.updated.clone()).style(Style::new().fg(Color::DarkGray)),
                Cell::from(i.summary.clone()),
            ])
        })
        .collect();

    let is_empty = rows.is_empty();
    let table = Table::new(
        rows,
        [
            Constraint::Length(15),
            Constraint::Length(7),
            Constraint::Length(14),
            Constraint::Length(18),
            Constraint::Length(16),
            Constraint::Min(20),
        ],
    )
    .header(
        Row::new(vec!["KEY", "TYPE", "STATUS", "ASSIGNEE", "UPDATED", "SUMMARY"])
            .style(Style::new().fg(Color::DarkGray).add_modifier(Modifier::BOLD)),
    )
    .row_highlight_style(Style::new().bg(Color::DarkGray).add_modifier(Modifier::BOLD))
    .block(block);

    let mut state = TableState::default();
    state.select(if is_empty { None } else { Some(app.selected) });
    f.render_stateful_widget(table, area, &mut state);

    if is_empty && !app.loading {
        let empty = Paragraph::new("no issues — press r to refresh, f to pick a filter, / to search")
            .style(Style::new().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        let inner = centered_rect(area, 80, 20);
        f.render_widget(empty, inner);
    }
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let Some(issue) = app.selected_issue() else { return };
    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(issue.key.clone(), Style::new().fg(ACCENT).bold()),
            Span::raw("  "),
            Span::styled(issue.summary.clone(), Style::new().bold()),
        ]),
        Line::default(),
        meta_line("Status", &issue.status, status_color(&issue.status_category)),
        meta_line("Type", &issue.issue_type, Color::White),
        meta_line("Priority", &issue.priority, Color::White),
        meta_line("Assignee", &issue.assignee, Color::Magenta),
        meta_line("Reporter", &issue.reporter, Color::Magenta),
        meta_line("Updated", &issue.updated, Color::White),
    ];
    if !issue.labels.is_empty() {
        lines.push(meta_line("Labels", &issue.labels.join(", "), Color::Yellow));
    }
    lines.push(meta_line("Link", &issue.url, Color::Blue));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Description",
        Style::new().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
    )));
    let desc = if issue.description.trim().is_empty() {
        "(no description)".to_string()
    } else {
        issue.description.clone()
    };
    for l in desc.lines() {
        lines.push(Line::from(l.to_string()));
    }

    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((app.detail_scroll, 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .padding(Padding::horizontal(1))
                .title(Span::styled(" issue ", Style::new().fg(ACCENT).bold())),
        );
    f.render_widget(para, area);
}

fn meta_line(label: &str, value: &str, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<10}"), Style::new().fg(Color::DarkGray)),
        Span::styled(value.to_string(), Style::new().fg(color)),
    ])
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    // A toast overrides the hint line for a few seconds.
    if let Some((msg, is_error, at)) = &app.toast {
        if at.elapsed().as_secs() < 5 {
            let style = if *is_error {
                Style::new().fg(Color::White).bg(Color::Red)
            } else {
                Style::new().fg(Color::Black).bg(Color::Green)
            };
            f.render_widget(
                Paragraph::new(format!(" {msg} ")).style(style),
                area,
            );
            return;
        }
    }
    let hints = match app.view {
        View::Detail => "Esc back  ·  j/k scroll  ·  s status  ·  d delegate  ·  o browser  ·  z zoom",
        View::SearchInput => "Enter search  ·  Esc cancel",
        View::JqlInput => "Enter run JQL  ·  Ctrl-U clear  ·  Esc cancel",
        View::NewAgentCwdInput => "Enter start  ·  Ctrl-U clear  ·  Esc back",
        View::AgentPicker => {
            "1-9 pick  ·  n new agent  ·  j/k move  ·  Enter select  ·  Esc cancel"
        }
        View::NewAgentTypePicker
        | View::NewAgentWorkspacePicker
        | View::NewAgentCwdPicker => {
            "1-9 pick  ·  j/k move  ·  Enter select  ·  Esc back"
        }
        View::FilterPicker | View::TransitionPicker => {
            "1-9 quick pick  ·  j/k move  ·  Enter select  ·  Esc cancel"
        }
        _ => "Enter open  ·  →/← epic  ·  f filters  ·  / search  ·  s status  ·  d delegate  ·  z zoom  ·  r refresh  ·  ? help  ·  q quit",
    };
    f.render_widget(
        Paragraph::new(hints).style(Style::new().fg(Color::DarkGray)),
        area,
    );
}

fn popup(f: &mut Frame, title: &str, width_pct: u16, height: u16) -> Rect {
    let area = f.area();
    let w = (area.width * width_pct / 100).max(30).min(area.width);
    let h = height.min(area.height);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(ACCENT))
            .title(Span::styled(format!(" {title} "), Style::new().fg(ACCENT).bold())),
        rect,
    );
    Rect {
        x: rect.x + 1,
        y: rect.y + 1,
        width: rect.width.saturating_sub(2),
        height: rect.height.saturating_sub(2),
    }
}

fn render_picker_list(f: &mut Frame, inner: Rect, items: Vec<ListItem>, sel: usize) {
    let mut state = ListState::default();
    state.select(if items.is_empty() { None } else { Some(sel) });
    let list = List::new(items)
        .highlight_style(Style::new().bg(Color::DarkGray).add_modifier(Modifier::BOLD))
        .highlight_symbol("▸ ");
    f.render_stateful_widget(list, inner, &mut state);
}

fn draw_filter_picker(f: &mut Frame, app: &App) {
    let h = app.cfg.filters.len() as u16 + 2;
    let inner = popup(f, "filters", 50, h.max(3));
    let items: Vec<ListItem> = app
        .cfg
        .filters
        .iter()
        .enumerate()
        .map(|(i, flt)| {
            let marker = if i == app.filter_idx { "● " } else { "  " };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{}{}. ", marker, i + 1), Style::new().fg(Color::DarkGray)),
                Span::raw(flt.name.clone()),
            ]))
        })
        .collect();
    render_picker_list(f, inner, items, app.picker_sel);
}

fn draw_transition_picker(f: &mut Frame, app: &App) {
    let title = format!("set status — {}", app.transitions_for);
    let h = (app.transitions.len() as u16 + 2).max(3);
    let inner = popup(f, &title, 50, h);
    if app.transitions_loading {
        f.render_widget(
            Paragraph::new("loading transitions…").style(Style::new().fg(Color::DarkGray)),
            inner,
        );
        return;
    }
    let items: Vec<ListItem> = app
        .transitions
        .iter()
        .enumerate()
        .map(|(i, t)| {
            ListItem::new(Line::from(vec![
                num_span(i),
                Span::raw(t.name.clone()),
                Span::styled(format!("  → {}", t.to_status), Style::new().fg(Color::DarkGray)),
            ]))
        })
        .collect();
    render_picker_list(f, inner, items, app.picker_sel);
}

fn draw_agent_picker(f: &mut Frame, app: &App) {
    let key = app
        .selected_issue()
        .map(|i| i.key.clone())
        .unwrap_or_default();
    let title = format!("delegate {key} to agent");
    let h = (app.agent_picker_len() as u16 + 2).max(3);
    let inner = popup(f, &title, 70, h);
    if app.agents_loading {
        f.render_widget(
            Paragraph::new("listing agents…").style(Style::new().fg(Color::DarkGray)),
            inner,
        );
        return;
    }
    let mut items: Vec<ListItem> = Vec::with_capacity(app.agent_picker_len());
    let offset = app.agent_list_offset();
    if offset > 0 {
        items.push(ListItem::new(Line::from(vec![
            num_span(0),
            Span::styled(
                "+ start new agent…",
                Style::new().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  (n)", Style::new().fg(Color::DarkGray)),
        ])));
    }
    for (i, a) in app.agents.iter().enumerate() {
        let row = i + offset;
        let status_color = match a.status.as_str() {
            "idle" | "done" => Color::Green,
            "working" => Color::Yellow,
            "blocked" => Color::Red,
            _ => Color::Gray,
        };
        let cwd = short_path(&a.cwd);
        items.push(ListItem::new(Line::from(vec![
            num_span(row),
            Span::styled(format!("{:<10}", a.label), Style::new().fg(ACCENT).bold()),
            Span::styled(format!("{:<9}", a.status), Style::new().fg(status_color)),
            Span::styled(format!("{:<8}", a.pane_id), Style::new().fg(Color::DarkGray)),
            Span::raw(cwd),
        ])));
    }
    if items.is_empty() {
        f.render_widget(
            Paragraph::new("no agents — add [[delegate.agents]] in config to start new ones")
                .style(Style::new().fg(Color::DarkGray)),
            inner,
        );
        return;
    }
    render_picker_list(f, inner, items, app.picker_sel);
}

fn draw_new_agent_type_picker(f: &mut Frame, app: &App) {
    let key = app
        .selected_issue()
        .map(|i| i.key.clone())
        .unwrap_or_default();
    let title = format!("start agent for {key} — pick agent");
    let agents = &app.cfg.delegate.agents;
    let h = (agents.len() as u16 + 2).max(3);
    let inner = popup(f, &title, 55, h);
    let items: Vec<ListItem> = agents
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let cmd = a.command.join(" ");
            ListItem::new(Line::from(vec![
                num_span(i),
                Span::styled(
                    format!("{:<12}", a.name),
                    Style::new().fg(ACCENT).bold(),
                ),
                Span::styled(cmd, Style::new().fg(Color::DarkGray)),
            ]))
        })
        .collect();
    render_picker_list(f, inner, items, app.picker_sel);
}

fn draw_new_agent_workspace_picker(f: &mut Frame, app: &App) {
    let agent_name = app
        .pending_spawn
        .as_ref()
        .map(|s| s.name.as_str())
        .unwrap_or("agent");
    let place = app.cfg.delegate.placement.trim().to_ascii_lowercase();
    let place_hint = if place == "right" || place == "down" {
        format!("split {place}")
    } else {
        "new tab".into()
    };
    let title = format!("space for {agent_name} ({place_hint})");
    let h = (app.workspaces.len() as u16 + 2).max(3);
    let inner = popup(f, &title, 70, h);
    if app.workspaces_loading {
        f.render_widget(
            Paragraph::new("listing spaces…").style(Style::new().fg(Color::DarkGray)),
            inner,
        );
        return;
    }
    let current = std::env::var("HERDR_WORKSPACE_ID").unwrap_or_default();
    let items: Vec<ListItem> = app
        .workspaces
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let marker = if !current.is_empty() && w.id == current {
                "● "
            } else if w.focused {
                "○ "
            } else {
                "  "
            };
            let status_color = match w.agent_status.as_str() {
                "idle" | "done" => Color::Green,
                "working" => Color::Yellow,
                "blocked" => Color::Red,
                _ => Color::Gray,
            };
            ListItem::new(Line::from(vec![
                num_span(i),
                Span::styled(marker, Style::new().fg(Color::Green)),
                Span::styled(
                    format!("{:<4}", format!("#{}", w.number)),
                    Style::new().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("{:<24}", truncate_label(&w.label, 24)),
                    Style::new().fg(ACCENT).bold(),
                ),
                Span::styled(
                    format!("{:<8}", w.agent_status),
                    Style::new().fg(status_color),
                ),
                Span::styled(
                    format!("{} tabs · {} panes", w.tab_count, w.pane_count),
                    Style::new().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();
    render_picker_list(f, inner, items, app.picker_sel);
}

fn truncate_label(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        let take = max.saturating_sub(1);
        format!("{}…", s.chars().take(take).collect::<String>())
    }
}

fn draw_new_agent_cwd_picker(f: &mut Frame, app: &App) {
    let agent_name = app
        .pending_spawn
        .as_ref()
        .map(|s| s.name.as_str())
        .unwrap_or("agent");
    let ws = app
        .pending_workspace
        .as_ref()
        .map(|w| w.label.as_str())
        .unwrap_or("?");
    let title = format!("cwd for {agent_name} in {ws}");
    let n = app.cwd_choices.len() + 1;
    let h = (n as u16 + 2).max(3);
    let inner = popup(f, &title, 70, h.max(4));
    let mut items: Vec<ListItem> = app
        .cwd_choices
        .iter()
        .enumerate()
        .map(|(i, p)| {
            ListItem::new(Line::from(vec![
                num_span(i),
                Span::raw(short_path(p)),
            ]))
        })
        .collect();
    items.push(ListItem::new(Line::from(vec![
        num_span(app.cwd_choices.len()),
        Span::styled(
            "type path…",
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  (/)", Style::new().fg(Color::DarkGray)),
    ])));
    render_picker_list(f, inner, items, app.picker_sel);
}

fn draw_cwd_input(f: &mut Frame, app: &App) {
    let agent_name = app
        .pending_spawn
        .as_ref()
        .map(|s| s.name.as_str())
        .unwrap_or("agent");
    let title = format!("cwd for {agent_name}");
    let inner = popup(f, &title, 75, 3);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(app.cwd_input.clone()),
            Span::styled("▏", Style::new().fg(ACCENT)),
        ])),
        inner,
    );
}

/// "1. " index prefix shown in pickers — rows past 9 have no hotkey.
fn num_span(i: usize) -> Span<'static> {
    let text = if i < 9 {
        format!("{}. ", i + 1)
    } else {
        "   ".to_string()
    };
    Span::styled(text, Style::new().fg(Color::DarkGray))
}

fn short_path(p: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && p.starts_with(&home) {
        format!("~{}", &p[home.len()..])
    } else {
        p.to_string()
    }
}

fn draw_search(f: &mut Frame, app: &App) {
    let inner = popup(f, "search (text ~ …)", 60, 3);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(app.search_input.clone()),
            Span::styled("▏", Style::new().fg(ACCENT)),
        ])),
        inner,
    );
}

fn draw_jql(f: &mut Frame, app: &App) {
    let inner = popup(f, "custom JQL", 80, 4);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(app.jql_input.clone()),
            Span::styled("▏", Style::new().fg(ACCENT)),
        ]))
        .wrap(Wrap { trim: false }),
        inner,
    );
}

fn draw_help(f: &mut Frame) {
    let inner = popup(f, "help", 60, 20);
    let rows = [
        ("j/k ↑/↓", "move / scroll"),
        ("Enter", "open issue details"),
        ("→/l ←/h", "expand / collapse epic"),
        ("f, 1-9", "switch filter"),
        ("/", "search (text ~ query)"),
        ("J", "run custom JQL (prefilled with current)"),
        ("1-9", "quick pick in any popup"),
        ("s", "change issue status"),
        ("d", "delegate issue to an agent"),
        ("n", "in delegate picker: start a new agent"),
        ("o", "open issue in browser"),
        ("z", "zoom pane (fullscreen toggle)"),
        ("r", "refresh current filter"),
        ("R", "reload config.toml"),
        ("g/G", "top / bottom"),
        ("Esc", "back / cancel"),
        ("q", "quit"),
    ];
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!("  {k:<10}"), Style::new().fg(ACCENT)),
                Span::raw(v.to_string()),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_fatal(f: &mut Frame, err: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::Red))
        .padding(Padding::uniform(1))
        .title(Span::styled(" herdr-jira: configuration ", Style::new().fg(Color::Red).bold()));
    let text = format!("{err}\n\nR — retry after fixing the config, q — quit");
    f.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: false }).block(block),
        f.area(),
    );
}

fn centered_rect(area: Rect, width_pct: u16, height_pct: u16) -> Rect {
    let w = area.width * width_pct / 100;
    let h = (area.height * height_pct / 100).max(1);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}
