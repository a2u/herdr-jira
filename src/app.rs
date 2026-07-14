//! Application state and key handling. Network and herdr-CLI work runs on
//! background threads; results come back over an mpsc channel as `Resp`.

use crate::config::Config;
use crate::herdr::{self, HerdrAgent};
use crate::jira::{Issue, JiraClient, Transition};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    List,
    Detail,
    FilterPicker,
    TransitionPicker,
    AgentPicker,
    SearchInput,
    JqlInput,
    Help,
}

pub enum Resp {
    Issues {
        title: String,
        result: Result<Vec<Issue>, String>,
    },
    Transitions {
        key: String,
        result: Result<Vec<Transition>, String>,
    },
    Transitioned {
        key: String,
        name: String,
        result: Result<(), String>,
    },
    Agents(Result<Vec<HerdrAgent>, String>),
    Delegated {
        key: String,
        label: String,
        result: Result<(), String>,
    },
}

pub struct App {
    pub cfg: Config,
    pub client: Option<Arc<JiraClient>>,
    pub tx: Sender<Resp>,

    pub view: View,
    pub should_quit: bool,

    pub issues: Vec<Issue>,
    pub selected: usize,
    pub current_title: String,
    pub loading: bool,
    pub fatal: Option<String>, // config/auth error shown full-screen

    pub filter_idx: usize,
    pub picker_sel: usize, // shared selection index for popup pickers

    pub transitions: Vec<Transition>,
    pub transitions_for: String,
    pub transitions_loading: bool,

    pub agents: Vec<HerdrAgent>,
    pub agents_loading: bool,

    pub search_input: String,
    pub jql_input: String,
    pub last_jql: String,
    pub detail_scroll: u16,

    pub toast: Option<(String, bool, Instant)>, // message, is_error, shown_at
}

impl App {
    pub fn new(tx: Sender<Resp>) -> Self {
        let mut app = Self {
            cfg: Config {
                jira: crate::config::JiraConfig {
                    base_url: String::new(),
                    auth: "basic".into(),
                    email: String::new(),
                    api_token: String::new(),
                    api_token_cmd: String::new(),
                    default_project: String::new(),
                    max_results: 50,
                },
                filters: vec![],
                search: Default::default(),
                delegate: Default::default(),
            },
            client: None,
            tx,
            view: View::List,
            should_quit: false,
            issues: vec![],
            selected: 0,
            current_title: String::new(),
            loading: false,
            fatal: None,
            filter_idx: 0,
            picker_sel: 0,
            transitions: vec![],
            transitions_for: String::new(),
            transitions_loading: false,
            agents: vec![],
            agents_loading: false,
            search_input: String::new(),
            jql_input: String::new(),
            last_jql: String::new(),
            detail_scroll: 0,
            toast: None,
        };
        app.reload_config();
        app
    }

    pub fn reload_config(&mut self) {
        match Config::load() {
            Ok(cfg) => match JiraClient::new(&cfg) {
                Ok(client) => {
                    self.cfg = cfg;
                    self.client = Some(Arc::new(client));
                    self.fatal = None;
                    self.filter_idx = self.filter_idx.min(self.cfg.filters.len().saturating_sub(1));
                    self.load_filter(self.filter_idx);
                }
                Err(e) => {
                    self.cfg = cfg;
                    self.fatal = Some(e);
                }
            },
            Err(e) => self.fatal = Some(e),
        }
    }

    pub fn selected_issue(&self) -> Option<&Issue> {
        self.issues.get(self.selected)
    }

    fn toast(&mut self, msg: impl Into<String>, is_error: bool) {
        self.toast = Some((msg.into(), is_error, Instant::now()));
    }

    // ---- background requests ----

    fn spawn_search(&mut self, jql: String, title: String) {
        let Some(client) = self.client.clone() else { return };
        self.last_jql = jql.clone();
        self.loading = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = client.search(&jql);
            let _ = tx.send(Resp::Issues { title, result });
        });
    }

    pub fn load_filter(&mut self, idx: usize) {
        let Some(filter) = self.cfg.filters.get(idx).cloned() else { return };
        self.filter_idx = idx;
        let jql = self.cfg.expand_jql(&filter.jql);
        self.spawn_search(jql, filter.name);
    }

    fn run_search(&mut self, query: String) {
        let jql = self
            .cfg
            .expand_jql(&self.cfg.search.jql.clone())
            .replace("{query}", &query.replace('"', "\\\""));
        self.spawn_search(jql, format!("search: {query}"));
    }

    fn request_transitions(&mut self) {
        let Some(issue) = self.selected_issue() else { return };
        let Some(client) = self.client.clone() else { return };
        let key = issue.key.clone();
        self.transitions_for = key.clone();
        self.transitions.clear();
        self.transitions_loading = true;
        self.picker_sel = 0;
        self.view = View::TransitionPicker;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = client.transitions(&key);
            let _ = tx.send(Resp::Transitions { key, result });
        });
    }

    fn apply_transition(&mut self, t: Transition) {
        let Some(client) = self.client.clone() else { return };
        let key = self.transitions_for.clone();
        let tx = self.tx.clone();
        self.toast(format!("{key}: applying \"{}\"…", t.name), false);
        std::thread::spawn(move || {
            let result = client.apply_transition(&key, &t.id);
            let _ = tx.send(Resp::Transitioned { key, name: t.to_status, result });
        });
    }

    fn request_agents(&mut self) {
        if self.selected_issue().is_none() {
            return;
        }
        self.agents.clear();
        self.agents_loading = true;
        self.picker_sel = 0;
        self.view = View::AgentPicker;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Resp::Agents(herdr::list_agents()));
        });
    }

    fn delegate_to(&mut self, agent: HerdrAgent) {
        let Some(issue) = self.selected_issue() else { return };
        let text = build_prompt(&self.cfg, issue);
        let key = issue.key.clone();
        let submit = self.cfg.delegate.submit;
        let delay = self.cfg.delegate.submit_delay_ms;
        let tx = self.tx.clone();
        self.toast(format!("sending {key} to {}…", agent.label), false);
        std::thread::spawn(move || {
            let result = herdr::send_to_agent(&agent, &text, submit, delay);
            let _ = tx.send(Resp::Delegated { key, label: agent.label, result });
        });
    }

    fn zoom_toggle(&mut self) {
        if let Err(e) = herdr::zoom_toggle() {
            self.toast(format!("zoom: {e}"), true);
        }
    }

    fn open_in_browser(&mut self) {
        let Some(issue) = self.selected_issue() else { return };
        let url = issue.url.clone();
        #[cfg(target_os = "macos")]
        let opener = "open";
        #[cfg(not(target_os = "macos"))]
        let opener = "xdg-open";
        let _ = std::process::Command::new(opener).arg(&url).spawn();
        self.toast(format!("opened {}", url), false);
    }

    // ---- responses ----

    pub fn on_resp(&mut self, resp: Resp) {
        match resp {
            Resp::Issues { title, result } => {
                self.loading = false;
                match result {
                    Ok(issues) => {
                        self.current_title = title;
                        self.selected = self.selected.min(issues.len().saturating_sub(1));
                        self.issues = issues;
                    }
                    Err(e) => self.toast(format!("Jira: {e}"), true),
                }
            }
            Resp::Transitions { key, result } => {
                if key != self.transitions_for {
                    return;
                }
                self.transitions_loading = false;
                match result {
                    Ok(ts) => self.transitions = ts,
                    Err(e) => {
                        self.view = View::List;
                        self.toast(format!("{key}: transitions: {e}"), true);
                    }
                }
            }
            Resp::Transitioned { key, name, result } => match result {
                Ok(()) => {
                    self.toast(format!("{key} → {name}"), false);
                    self.load_filter(self.filter_idx);
                }
                Err(e) => self.toast(format!("{key}: {e}"), true),
            },
            Resp::Agents(result) => {
                self.agents_loading = false;
                match result {
                    Ok(agents) => {
                        if agents.is_empty() {
                            self.view = View::List;
                            self.toast("no running agents found in herdr", true);
                        } else {
                            self.agents = agents;
                        }
                    }
                    Err(e) => {
                        self.view = View::List;
                        self.toast(format!("agents: {e}"), true);
                    }
                }
            }
            Resp::Delegated { key, label, result } => match result {
                Ok(()) => {
                    self.toast(format!("{key} delegated to {label}"), false);
                    herdr::notify(&format!("Jira {key} delegated to {label}"));
                }
                Err(e) => self.toast(format!("delegate {key}: {e}"), true),
            },
        }
    }

    // ---- keys ----

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return;
        }
        if self.fatal.is_some() {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
                KeyCode::Char('R') | KeyCode::Char('r') => self.reload_config(),
                _ => {}
            }
            return;
        }
        match self.view {
            View::List => self.keys_list(key),
            View::Detail => self.keys_detail(key),
            View::FilterPicker => self.keys_filter_picker(key),
            View::TransitionPicker => self.keys_transition_picker(key),
            View::AgentPicker => self.keys_agent_picker(key),
            View::SearchInput => self.keys_search(key),
            View::JqlInput => self.keys_jql(key),
            View::Help => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => self.view = View::List,
                _ => {}
            },
        }
    }

    fn move_sel(len: usize, sel: usize, delta: i32) -> usize {
        if len == 0 {
            return 0;
        }
        let max = len as i32 - 1;
        (sel as i32 + delta).clamp(0, max) as usize
    }

    fn keys_list(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('j') | KeyCode::Down => {
                self.selected = Self::move_sel(self.issues.len(), self.selected, 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected = Self::move_sel(self.issues.len(), self.selected, -1)
            }
            KeyCode::PageDown => {
                self.selected = Self::move_sel(self.issues.len(), self.selected, 15)
            }
            KeyCode::PageUp => self.selected = Self::move_sel(self.issues.len(), self.selected, -15),
            KeyCode::Char('g') | KeyCode::Home => self.selected = 0,
            KeyCode::Char('G') | KeyCode::End => {
                self.selected = self.issues.len().saturating_sub(1)
            }
            KeyCode::Enter => {
                if self.selected_issue().is_some() {
                    self.detail_scroll = 0;
                    self.view = View::Detail;
                }
            }
            KeyCode::Char('f') => {
                self.picker_sel = self.filter_idx;
                self.view = View::FilterPicker;
            }
            KeyCode::Char(c @ '1'..='9') => {
                let idx = (c as u8 - b'1') as usize;
                if idx < self.cfg.filters.len() {
                    self.load_filter(idx);
                }
            }
            KeyCode::Char('/') => {
                self.search_input.clear();
                self.view = View::SearchInput;
            }
            KeyCode::Char('J') => {
                self.jql_input = self.last_jql.clone();
                self.view = View::JqlInput;
            }
            KeyCode::Char('r') => self.load_filter(self.filter_idx),
            KeyCode::Char('R') => {
                self.reload_config();
                self.toast("config reloaded", false);
            }
            KeyCode::Char('s') => self.request_transitions(),
            KeyCode::Char('d') => self.request_agents(),
            KeyCode::Char('o') => self.open_in_browser(),
            KeyCode::Char('z') => self.zoom_toggle(),
            KeyCode::Char('?') => self.view = View::Help,
            _ => {}
        }
    }

    fn keys_detail(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.view = View::List,
            KeyCode::Char('j') | KeyCode::Down => {
                self.detail_scroll = self.detail_scroll.saturating_add(1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.detail_scroll = self.detail_scroll.saturating_sub(1)
            }
            KeyCode::PageDown => self.detail_scroll = self.detail_scroll.saturating_add(15),
            KeyCode::PageUp => self.detail_scroll = self.detail_scroll.saturating_sub(15),
            KeyCode::Char('g') => self.detail_scroll = 0,
            KeyCode::Char('s') => self.request_transitions(),
            KeyCode::Char('d') => self.request_agents(),
            KeyCode::Char('o') => self.open_in_browser(),
            KeyCode::Char('z') => self.zoom_toggle(),
            _ => {}
        }
    }

    /// Number hotkeys shared by all popup pickers: `1`-`9` picks that row.
    fn picker_number(key: &KeyEvent, len: usize) -> Option<usize> {
        if let KeyCode::Char(c @ '1'..='9') = key.code {
            let idx = (c as u8 - b'1') as usize;
            if idx < len {
                return Some(idx);
            }
        }
        None
    }

    fn keys_filter_picker(&mut self, key: KeyEvent) {
        if let Some(idx) = Self::picker_number(&key, self.cfg.filters.len()) {
            self.view = View::List;
            self.load_filter(idx);
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.view = View::List,
            KeyCode::Char('j') | KeyCode::Down => {
                self.picker_sel = Self::move_sel(self.cfg.filters.len(), self.picker_sel, 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.picker_sel = Self::move_sel(self.cfg.filters.len(), self.picker_sel, -1)
            }
            KeyCode::Enter => {
                self.view = View::List;
                self.load_filter(self.picker_sel);
            }
            _ => {}
        }
    }

    fn keys_transition_picker(&mut self, key: KeyEvent) {
        if let Some(idx) = Self::picker_number(&key, self.transitions.len()) {
            if let Some(t) = self.transitions.get(idx).cloned() {
                self.view = View::List;
                self.apply_transition(t);
            }
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.view = View::List,
            KeyCode::Char('j') | KeyCode::Down => {
                self.picker_sel = Self::move_sel(self.transitions.len(), self.picker_sel, 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.picker_sel = Self::move_sel(self.transitions.len(), self.picker_sel, -1)
            }
            KeyCode::Enter => {
                if let Some(t) = self.transitions.get(self.picker_sel).cloned() {
                    self.view = View::List;
                    self.apply_transition(t);
                }
            }
            _ => {}
        }
    }

    fn keys_agent_picker(&mut self, key: KeyEvent) {
        if let Some(idx) = Self::picker_number(&key, self.agents.len()) {
            if let Some(agent) = self.agents.get(idx).cloned() {
                self.view = View::List;
                self.delegate_to(agent);
            }
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.view = View::List,
            KeyCode::Char('j') | KeyCode::Down => {
                self.picker_sel = Self::move_sel(self.agents.len(), self.picker_sel, 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.picker_sel = Self::move_sel(self.agents.len(), self.picker_sel, -1)
            }
            KeyCode::Enter => {
                if let Some(agent) = self.agents.get(self.picker_sel).cloned() {
                    self.view = View::List;
                    self.delegate_to(agent);
                }
            }
            _ => {}
        }
    }

    fn keys_search(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.view = View::List,
            KeyCode::Enter => {
                let q = self.search_input.trim().to_string();
                self.view = View::List;
                if !q.is_empty() {
                    self.run_search(q);
                }
            }
            KeyCode::Backspace => {
                self.search_input.pop();
            }
            KeyCode::Char(c) => self.search_input.push(c),
            _ => {}
        }
    }

    fn keys_jql(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.view = View::List,
            KeyCode::Enter => {
                let jql = self.jql_input.trim().to_string();
                self.view = View::List;
                if !jql.is_empty() {
                    let expanded = self.cfg.expand_jql(&jql);
                    self.spawn_search(expanded, "custom JQL".into());
                }
            }
            KeyCode::Backspace => {
                self.jql_input.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.jql_input.clear();
            }
            KeyCode::Char(c) => self.jql_input.push(c),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_issue() -> Issue {
        Issue {
            key: "PROJ-7".into(),
            summary: "Fix login".into(),
            status: "To Do".into(),
            status_category: "new".into(),
            issue_type: "Bug".into(),
            priority: "High".into(),
            assignee: "Vitalii".into(),
            reporter: "Someone".into(),
            updated: "2026-07-14 10:00".into(),
            labels: vec!["auth".into(), "urgent".into()],
            description: "Steps to reproduce…".into(),
            url: "https://x.atlassian.net/browse/PROJ-7".into(),
        }
    }

    fn test_cfg(prompt: &str, max_desc: usize) -> Config {
        let mut cfg: Config = toml::from_str(
            "[jira]\nbase_url = \"https://x.atlassian.net\"\n",
        )
        .unwrap();
        cfg.delegate.prompt = prompt.into();
        cfg.delegate.max_description_chars = max_desc;
        cfg
    }

    #[test]
    fn prompt_placeholders_are_filled() {
        let cfg = test_cfg("{key}: {summary} [{status}/{priority}] {labels}\n{description}\n{url}", 0);
        let p = build_prompt(&cfg, &test_issue());
        assert_eq!(
            p,
            "PROJ-7: Fix login [To Do/High] auth, urgent\nSteps to reproduce…\nhttps://x.atlassian.net/browse/PROJ-7"
        );
    }

    #[test]
    fn long_descriptions_are_truncated() {
        let cfg = test_cfg("{description}", 10);
        let mut issue = test_issue();
        issue.description = "x".repeat(50);
        let p = build_prompt(&cfg, &issue);
        assert!(p.starts_with("xxxxxxxxxx\n[… description truncated]"));
    }

    #[test]
    fn empty_description_gets_placeholder() {
        let cfg = test_cfg("{description}", 0);
        let mut issue = test_issue();
        issue.description = "  ".into();
        assert_eq!(build_prompt(&cfg, &issue), "(no description)");
    }
}

/// Fill the delegate prompt template with issue fields.
pub fn build_prompt(cfg: &Config, issue: &Issue) -> String {
    let mut desc = issue.description.clone();
    if desc.trim().is_empty() {
        desc = "(no description)".into();
    }
    let max = cfg.delegate.max_description_chars;
    if max > 0 && desc.chars().count() > max {
        desc = desc.chars().take(max).collect::<String>() + "\n[… description truncated]";
    }
    cfg.delegate
        .prompt
        .replace("{key}", &issue.key)
        .replace("{summary}", &issue.summary)
        .replace("{description}", &desc)
        .replace("{url}", &issue.url)
        .replace("{status}", &issue.status)
        .replace("{assignee}", &issue.assignee)
        .replace("{reporter}", &issue.reporter)
        .replace("{priority}", &issue.priority)
        .replace("{type}", &issue.issue_type)
        .replace("{labels}", &issue.labels.join(", "))
        .trim()
        .to_string()
}
