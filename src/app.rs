//! Application state and key handling. Network and herdr-CLI work runs on
//! background threads; results come back over an mpsc channel as `Resp`.

use crate::config::{Config, SpawnAgent};
use crate::herdr::{self, HerdrAgent, HerdrWorkspace, StartAgentOpts};
use crate::jira::{Issue, JiraClient, Transition};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    List,
    Detail,
    FilterPicker,
    TransitionPicker,
    AgentPicker,
    /// Pick which agent binary to spawn (from `[delegate].agents`).
    NewAgentTypePicker,
    /// Pick which herdr workspace ("space") to place the new agent in.
    NewAgentWorkspacePicker,
    /// Pick a working directory for the new agent.
    NewAgentCwdPicker,
    /// Free-text cwd entry (from "type path…" in the cwd picker).
    NewAgentCwdInput,
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
    Workspaces(Result<Vec<HerdrWorkspace>, String>),
    Delegated {
        key: String,
        label: String,
        result: Result<(), String>,
    },
    Children {
        epic: String,
        result: Result<Vec<Issue>, String>,
    },
}

pub fn is_epic(issue: &Issue) -> bool {
    issue.issue_type.eq_ignore_ascii_case("epic")
}

pub struct App {
    pub cfg: Config,
    pub client: Option<Arc<JiraClient>>,
    pub tx: Sender<Resp>,

    pub view: View,
    pub should_quit: bool,

    pub issues: Vec<Issue>,
    /// Epic expansion state: children keyed by epic key, plus which epics are
    /// currently open and which are still fetching.
    pub children: HashMap<String, Vec<Issue>>,
    pub expanded: HashSet<String>,
    pub loading_children: HashSet<String>,
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

    /// Selected spawn agent while walking the "start new" wizard.
    pub pending_spawn: Option<SpawnAgent>,
    /// Selected workspace for the new agent.
    pub pending_workspace: Option<HerdrWorkspace>,
    pub workspaces: Vec<HerdrWorkspace>,
    pub workspaces_loading: bool,
    /// Unique cwd candidates for the new-agent cwd picker.
    pub cwd_choices: Vec<String>,
    pub cwd_input: String,

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
            children: HashMap::new(),
            expanded: HashSet::new(),
            loading_children: HashSet::new(),
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
            pending_spawn: None,
            pending_workspace: None,
            workspaces: vec![],
            workspaces_loading: false,
            cwd_choices: vec![],
            cwd_input: String::new(),
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

    /// Rows currently on screen: top-level issues, with the children of every
    /// expanded epic inlined right below it. `u8` is the indent depth.
    pub fn visible(&self) -> Vec<(&Issue, u8)> {
        let mut rows = Vec::with_capacity(self.issues.len());
        for issue in &self.issues {
            rows.push((issue, 0));
            if is_epic(issue) && self.expanded.contains(&issue.key) {
                if let Some(kids) = self.children.get(&issue.key) {
                    rows.extend(kids.iter().map(|k| (k, 1)));
                }
            }
        }
        rows
    }

    pub fn selected_issue(&self) -> Option<&Issue> {
        self.visible().get(self.selected).map(|(i, _)| *i)
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

    /// Expand the selected epic (fetching its children on first open).
    fn expand_epic(&mut self) {
        let Some(issue) = self.selected_issue() else { return };
        if !is_epic(issue) {
            return;
        }
        let key = issue.key.clone();
        if self.expanded.contains(&key) {
            return;
        }
        if self.children.contains_key(&key) {
            self.expanded.insert(key);
            return;
        }
        let Some(client) = self.client.clone() else { return };
        if !self.loading_children.insert(key.clone()) {
            return; // fetch already in flight
        }
        self.toast(format!("loading issues in {key}…"), false);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            // Jira Cloud links children via `parent`; classic Server/DC epics
            // use the "Epic Link" field. Try both.
            let result = client
                .search(&format!("parent = {key} ORDER BY created ASC"))
                .or_else(|_| client.search(&format!("\"Epic Link\" = {key} ORDER BY created ASC")));
            let _ = tx.send(Resp::Children { epic: key, result });
        });
    }

    /// Collapse the selected epic — or, on a child row, collapse its parent
    /// epic and move the selection onto it.
    fn collapse_epic(&mut self) {
        let vis = self.visible();
        let Some(&(issue, depth)) = vis.get(self.selected) else { return };
        let epic_key = if depth == 0 {
            if !(is_epic(issue) && self.expanded.contains(&issue.key)) {
                return;
            }
            issue.key.clone()
        } else {
            // Walk back to the nearest top-level row: that's the parent epic.
            match vis[..self.selected]
                .iter()
                .rev()
                .find(|(_, d)| *d == 0)
                .map(|(i, _)| i.key.clone())
            {
                Some(k) => k,
                None => return,
            }
        };
        self.expanded.remove(&epic_key);
        // Land the selection on the epic row itself.
        if let Some(idx) = self.visible().iter().position(|(i, _)| i.key == epic_key) {
            self.selected = idx;
        }
    }

    fn request_agents(&mut self) {
        if self.selected_issue().is_none() {
            return;
        }
        self.agents.clear();
        self.agents_loading = true;
        self.picker_sel = 0;
        self.pending_spawn = None;
        self.view = View::AgentPicker;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Resp::Agents(herdr::list_agents()));
        });
    }

    /// Rows in the agent picker: optional "start new" at index 0, then running agents.
    pub fn can_start_new_agent(&self) -> bool {
        !self.cfg.delegate.agents.is_empty()
    }

    /// Index of the first running-agent row in AgentPicker (0 if no "start new").
    pub fn agent_list_offset(&self) -> usize {
        if self.can_start_new_agent() {
            1
        } else {
            0
        }
    }

    pub fn agent_picker_len(&self) -> usize {
        self.agent_list_offset() + self.agents.len()
    }

    fn open_new_agent_type_picker(&mut self) {
        if self.cfg.delegate.agents.is_empty() {
            self.toast(
                "no [[delegate.agents]] configured — add agents in config.toml",
                true,
            );
            return;
        }
        self.picker_sel = 0;
        self.pending_spawn = None;
        self.pending_workspace = None;
        self.view = View::NewAgentTypePicker;
    }

    /// After agent type: load workspaces and open the space picker.
    fn open_new_agent_workspace_picker(&mut self, spawn: SpawnAgent) {
        self.pending_spawn = Some(spawn);
        self.pending_workspace = None;
        self.workspaces.clear();
        self.workspaces_loading = true;
        self.picker_sel = 0;
        self.view = View::NewAgentWorkspacePicker;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Resp::Workspaces(herdr::list_workspaces()));
        });
    }

    fn open_new_agent_cwd_picker(&mut self) {
        self.cwd_choices = collect_cwd_choices(&self.cfg, &self.agents);
        self.picker_sel = 0;
        self.view = View::NewAgentCwdPicker;
    }

    fn open_cwd_input(&mut self) {
        let pref = if !self.cfg.delegate.default_cwd.trim().is_empty() {
            self.cfg.delegate.default_cwd.clone()
        } else if let Some(first) = self.cwd_choices.first() {
            first.clone()
        } else {
            std::env::var("HOME").unwrap_or_default()
        };
        self.cwd_input = pref;
        self.view = View::NewAgentCwdInput;
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
            let _ = tx.send(Resp::Delegated {
                key,
                label: agent.label,
                result,
            });
        });
    }

    /// Spawn a fresh agent in the chosen workspace + cwd, wait until ready,
    /// send the Jira prompt.
    fn start_new_and_delegate(&mut self, cwd_raw: String) {
        let Some(issue) = self.selected_issue() else { return };
        let Some(spawn) = self.pending_spawn.clone() else {
            self.toast("no agent selected", true);
            return;
        };
        let Some(ws) = self.pending_workspace.clone() else {
            self.toast("no workspace selected", true);
            return;
        };
        let cwd = herdr::expand_path(&cwd_raw);
        if cwd.is_empty() {
            self.toast("cwd is empty", true);
            return;
        }
        if !Path::new(&cwd).is_dir() {
            self.toast(format!("not a directory: {cwd}"), true);
            return;
        }
        if spawn.command.is_empty() {
            self.toast(format!("{}: empty command", spawn.name), true);
            return;
        }

        let text = build_prompt(&self.cfg, issue);
        let key = issue.key.clone();
        let submit = self.cfg.delegate.submit;
        let delay = self.cfg.delegate.submit_delay_ms;
        let startup = self.cfg.delegate.startup_delay_ms;
        let wait_ready = self.cfg.delegate.wait_ready_ms;
        let placement = self.cfg.delegate.placement.clone();
        let focus = self.cfg.delegate.focus_new;
        let name = unique_agent_name(&key, &spawn.name);
        let agent_label = spawn.name.clone();
        let ws_label = ws.label.clone();
        let opts = StartAgentOpts {
            name,
            cwd: cwd.clone(),
            argv: spawn.command.clone(),
            placement: placement.clone(),
            focus,
            workspace_id: ws.id.clone(),
            tab_label: key.clone(),
        };
        let place_hint = if placement.eq_ignore_ascii_case("tab") || placement.is_empty() {
            format!("new tab in {ws_label}")
        } else {
            format!("split {placement} in {ws_label}")
        };
        let tx = self.tx.clone();
        self.toast(
            format!(
                "starting {agent_label} for {key} ({place_hint}, {})…",
                short_home(&cwd)
            ),
            false,
        );
        std::thread::spawn(move || {
            let result = herdr::start_and_delegate(
                &opts,
                &text,
                submit,
                delay,
                startup,
                wait_ready,
            )
            .map(|a| a.label);
            let label = match &result {
                Ok(l) => l.clone(),
                Err(_) => agent_label,
            };
            let result = result.map(|_| ());
            let _ = tx.send(Resp::Delegated { key, label, result });
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
                        self.issues = issues;
                        self.children.clear();
                        self.expanded.clear();
                        self.loading_children.clear();
                        self.selected = self.selected.min(self.issues.len().saturating_sub(1));
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
                        self.agents = agents;
                        if self.agents.is_empty() && !self.can_start_new_agent() {
                            self.view = View::List;
                            self.toast(
                                "no running agents and no [[delegate.agents]] to start",
                                true,
                            );
                        }
                    }
                    Err(e) => {
                        // Still allow starting a new agent if list failed.
                        if self.can_start_new_agent() {
                            self.agents.clear();
                            self.toast(format!("list agents failed ({e}); start new is available"), true);
                        } else {
                            self.view = View::List;
                            self.toast(format!("agents: {e}"), true);
                        }
                    }
                }
            }
            Resp::Workspaces(result) => {
                self.workspaces_loading = false;
                if self.view != View::NewAgentWorkspacePicker {
                    return;
                }
                match result {
                    Ok(list) if list.is_empty() => {
                        self.view = View::NewAgentTypePicker;
                        self.toast("no workspaces found in herdr", true);
                    }
                    Ok(list) => {
                        // Prefer the workspace that hosts this Jira pane.
                        let current = std::env::var("HERDR_WORKSPACE_ID").unwrap_or_default();
                        self.picker_sel = list
                            .iter()
                            .position(|w| !current.is_empty() && w.id == current)
                            .or_else(|| list.iter().position(|w| w.focused))
                            .unwrap_or(0);
                        self.workspaces = list;
                    }
                    Err(e) => {
                        self.view = View::NewAgentTypePicker;
                        self.toast(format!("workspaces: {e}"), true);
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
            Resp::Children { epic, result } => {
                self.loading_children.remove(&epic);
                match result {
                    Ok(kids) if kids.is_empty() => {
                        self.toast(format!("{epic}: no issues in this epic"), false)
                    }
                    Ok(kids) => {
                        self.children.insert(epic.clone(), kids);
                        self.expanded.insert(epic);
                    }
                    Err(e) => self.toast(format!("{epic}: children: {e}"), true),
                }
            }
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
            View::NewAgentTypePicker => self.keys_new_agent_type(key),
            View::NewAgentWorkspacePicker => self.keys_new_agent_workspace(key),
            View::NewAgentCwdPicker => self.keys_new_agent_cwd(key),
            View::NewAgentCwdInput => self.keys_cwd_input(key),
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
        let vis_len = self.visible().len();
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('j') | KeyCode::Down => {
                self.selected = Self::move_sel(vis_len, self.selected, 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected = Self::move_sel(vis_len, self.selected, -1)
            }
            KeyCode::PageDown => self.selected = Self::move_sel(vis_len, self.selected, 15),
            KeyCode::PageUp => self.selected = Self::move_sel(vis_len, self.selected, -15),
            KeyCode::Char('g') | KeyCode::Home => self.selected = 0,
            KeyCode::Char('G') | KeyCode::End => self.selected = vis_len.saturating_sub(1),
            KeyCode::Char('l') | KeyCode::Right => self.expand_epic(),
            KeyCode::Char('h') | KeyCode::Left => self.collapse_epic(),
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

    fn pick_agent_row(&mut self, row: usize) {
        let offset = self.agent_list_offset();
        if offset > 0 && row == 0 {
            self.open_new_agent_type_picker();
            return;
        }
        let agent_idx = row.saturating_sub(offset);
        if let Some(agent) = self.agents.get(agent_idx).cloned() {
            self.view = View::List;
            self.delegate_to(agent);
        }
    }

    fn keys_agent_picker(&mut self, key: KeyEvent) {
        let len = self.agent_picker_len();
        if let Some(idx) = Self::picker_number(&key, len) {
            self.pick_agent_row(idx);
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.view = View::List,
            KeyCode::Char('n') => self.open_new_agent_type_picker(),
            KeyCode::Char('j') | KeyCode::Down => {
                self.picker_sel = Self::move_sel(len, self.picker_sel, 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.picker_sel = Self::move_sel(len, self.picker_sel, -1)
            }
            KeyCode::Enter => self.pick_agent_row(self.picker_sel),
            _ => {}
        }
    }

    fn keys_new_agent_type(&mut self, key: KeyEvent) {
        let agents = &self.cfg.delegate.agents;
        if let Some(idx) = Self::picker_number(&key, agents.len()) {
            if let Some(spawn) = agents.get(idx).cloned() {
                self.open_new_agent_workspace_picker(spawn);
            }
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                // Back to the running-agents list (not all the way to List).
                self.picker_sel = 0;
                self.view = View::AgentPicker;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.picker_sel = Self::move_sel(agents.len(), self.picker_sel, 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.picker_sel = Self::move_sel(agents.len(), self.picker_sel, -1)
            }
            KeyCode::Enter => {
                if let Some(spawn) = agents.get(self.picker_sel).cloned() {
                    self.open_new_agent_workspace_picker(spawn);
                }
            }
            _ => {}
        }
    }

    fn keys_new_agent_workspace(&mut self, key: KeyEvent) {
        if self.workspaces_loading {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
                self.view = View::NewAgentTypePicker;
            }
            return;
        }
        let n = self.workspaces.len();
        if let Some(idx) = Self::picker_number(&key, n) {
            self.pick_workspace_row(idx);
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.picker_sel = 0;
                self.view = View::NewAgentTypePicker;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.picker_sel = Self::move_sel(n, self.picker_sel, 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.picker_sel = Self::move_sel(n, self.picker_sel, -1)
            }
            KeyCode::Enter => self.pick_workspace_row(self.picker_sel),
            _ => {}
        }
    }

    fn pick_workspace_row(&mut self, row: usize) {
        let Some(ws) = self.workspaces.get(row).cloned() else {
            return;
        };
        self.pending_workspace = Some(ws);
        self.open_new_agent_cwd_picker();
    }

    fn keys_new_agent_cwd(&mut self, key: KeyEvent) {
        // Last row is always "type path…"; rows above are concrete cwds.
        let n = self.cwd_choices.len() + 1;
        if let Some(idx) = Self::picker_number(&key, n) {
            self.pick_cwd_row(idx);
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.picker_sel = self
                    .pending_workspace
                    .as_ref()
                    .and_then(|pw| self.workspaces.iter().position(|w| w.id == pw.id))
                    .unwrap_or(0);
                self.view = View::NewAgentWorkspacePicker;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.picker_sel = Self::move_sel(n, self.picker_sel, 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.picker_sel = Self::move_sel(n, self.picker_sel, -1)
            }
            KeyCode::Char('/') | KeyCode::Char('e') => self.open_cwd_input(),
            KeyCode::Enter => self.pick_cwd_row(self.picker_sel),
            _ => {}
        }
    }

    fn pick_cwd_row(&mut self, row: usize) {
        if row >= self.cwd_choices.len() {
            self.open_cwd_input();
            return;
        }
        let cwd = self.cwd_choices[row].clone();
        self.view = View::List;
        self.start_new_and_delegate(cwd);
    }

    fn keys_cwd_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.view = View::NewAgentCwdPicker;
            }
            KeyCode::Enter => {
                let cwd = self.cwd_input.trim().to_string();
                if cwd.is_empty() {
                    self.toast("cwd is empty", true);
                    return;
                }
                self.view = View::List;
                self.start_new_and_delegate(cwd);
            }
            KeyCode::Backspace => {
                self.cwd_input.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cwd_input.clear();
            }
            KeyCode::Char(c) => self.cwd_input.push(c),
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

/// Build cwd options: configured default, unique cwds from running agents,
/// then common parents. Paths are stored expanded.
fn collect_cwd_choices(cfg: &Config, agents: &[HerdrAgent]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |raw: &str| {
        let p = herdr::expand_path(raw);
        if p.is_empty() || !Path::new(&p).is_dir() {
            return;
        }
        if !out.iter().any(|x| x == &p) {
            out.push(p);
        }
    };

    if !cfg.delegate.default_cwd.trim().is_empty() {
        push(&cfg.delegate.default_cwd);
    }
    for a in agents {
        if !a.cwd.is_empty() {
            // Skip herdr-mirror helper panes — not useful work dirs.
            if a.cwd.contains("herdr-mirror") {
                continue;
            }
            push(&a.cwd);
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        push(&cwd.to_string_lossy());
    }
    if let Ok(home) = std::env::var("HOME") {
        push(&home);
        push(&format!("{home}/Work"));
        push(&format!("{home}/Projects"));
        push(&format!("{home}/src"));
    }
    out
}

/// Unique herdr agent name: issue key + agent label + short time suffix so a
/// second delegate of the same issue does not collide.
fn unique_agent_name(issue_key: &str, agent_label: &str) -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() % 10_000)
        .unwrap_or(0);
    // Keep names shell/CLI friendly.
    let key = issue_key.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "-");
    let label = agent_label.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "-");
    format!("{key}-{label}-{secs}")
}

fn short_home(p: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && p.starts_with(&home) {
        format!("~{}", &p[home.len()..])
    } else {
        p.to_string()
    }
}
