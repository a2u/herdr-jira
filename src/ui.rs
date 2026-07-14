//! Rendering. One draw function per view; popup pickers render on top of the
//! issue list.

use crate::app::{App, View};
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
        View::SearchInput => draw_search(f, app),
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

    let rows: Vec<Row> = app
        .issues
        .iter()
        .map(|i| {
            Row::new(vec![
                Cell::from(i.key.clone()).style(Style::new().fg(ACCENT)),
                Cell::from(i.status.clone()).style(Style::new().fg(status_color(&i.status_category))),
                Cell::from(i.assignee.clone()).style(Style::new().fg(Color::Magenta)),
                Cell::from(i.updated.clone()).style(Style::new().fg(Color::DarkGray)),
                Cell::from(i.summary.clone()),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(14),
            Constraint::Length(18),
            Constraint::Length(16),
            Constraint::Min(20),
        ],
    )
    .header(
        Row::new(vec!["KEY", "STATUS", "ASSIGNEE", "UPDATED", "SUMMARY"])
            .style(Style::new().fg(Color::DarkGray).add_modifier(Modifier::BOLD)),
    )
    .row_highlight_style(Style::new().bg(Color::DarkGray).add_modifier(Modifier::BOLD))
    .block(block);

    let mut state = TableState::default();
    state.select(if app.issues.is_empty() { None } else { Some(app.selected) });
    f.render_stateful_widget(table, area, &mut state);

    if app.issues.is_empty() && !app.loading {
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
        View::Detail => "Esc back  ·  j/k scroll  ·  s status  ·  d delegate  ·  o browser",
        View::SearchInput => "Enter search  ·  Esc cancel",
        View::FilterPicker | View::TransitionPicker | View::AgentPicker => {
            "j/k move  ·  Enter select  ·  Esc cancel"
        }
        _ => "Enter open  ·  f filters  ·  / search  ·  s status  ·  d delegate  ·  o browser  ·  r refresh  ·  ? help  ·  q quit",
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
        .map(|t| {
            ListItem::new(Line::from(vec![
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
    let h = (app.agents.len() as u16 + 2).max(3);
    let inner = popup(f, &title, 70, h);
    if app.agents_loading {
        f.render_widget(
            Paragraph::new("listing agents…").style(Style::new().fg(Color::DarkGray)),
            inner,
        );
        return;
    }
    let items: Vec<ListItem> = app
        .agents
        .iter()
        .map(|a| {
            let status_color = match a.status.as_str() {
                "idle" | "done" => Color::Green,
                "working" => Color::Yellow,
                "blocked" => Color::Red,
                _ => Color::Gray,
            };
            let cwd = short_path(&a.cwd);
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<10}", a.label), Style::new().fg(ACCENT).bold()),
                Span::styled(format!("{:<9}", a.status), Style::new().fg(status_color)),
                Span::styled(format!("{:<8}", a.pane_id), Style::new().fg(Color::DarkGray)),
                Span::raw(cwd),
            ]))
        })
        .collect();
    render_picker_list(f, inner, items, app.picker_sel);
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

fn draw_help(f: &mut Frame) {
    let inner = popup(f, "help", 60, 18);
    let rows = [
        ("j/k ↑/↓", "move / scroll"),
        ("Enter", "open issue details"),
        ("f, 1-9", "switch filter"),
        ("/", "search (text ~ query)"),
        ("s", "change issue status"),
        ("d", "delegate issue to an agent"),
        ("o", "open issue in browser"),
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
