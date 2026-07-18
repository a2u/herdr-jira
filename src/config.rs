//! Plugin configuration: loaded from the herdr-managed plugin config dir
//! (`HERDR_PLUGIN_CONFIG_DIR`, falling back to
//! `~/.config/herdr/plugins/config/herdr-jira/config.toml` for standalone runs).

use serde::Deserialize;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub jira: JiraConfig,
    #[serde(default)]
    pub filters: Vec<Filter>,
    #[serde(default)]
    pub search: SearchConfig,
    #[serde(default)]
    pub delegate: DelegateConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JiraConfig {
    pub base_url: String,
    #[serde(default = "default_auth")]
    pub auth: String, // "basic" | "bearer"
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub api_token: String,
    #[serde(default)]
    pub api_token_cmd: String,
    #[serde(default)]
    pub default_project: String,
    #[serde(default = "default_max_results")]
    pub max_results: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Filter {
    pub name: String,
    pub jql: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchConfig {
    #[serde(default = "default_search_jql")]
    pub jql: String,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self { jql: default_search_jql() }
    }
}

/// One agent binary that can be spawned via `herdr agent start` when
/// delegating with "start new agent".
#[derive(Debug, Clone, Deserialize)]
pub struct SpawnAgent {
    /// Display label and default herdr agent name prefix (e.g. "claude").
    pub name: String,
    /// Argv passed after `--` to `herdr agent start` (e.g. `["claude"]`).
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DelegateConfig {
    #[serde(default = "default_prompt")]
    pub prompt: String,
    #[serde(default = "default_true")]
    pub submit: bool,
    #[serde(default = "default_submit_delay")]
    pub submit_delay_ms: u64,
    #[serde(default = "default_max_desc")]
    pub max_description_chars: usize,
    /// Agents offered when starting a new one (not only listing running).
    #[serde(default = "default_spawn_agents")]
    pub agents: Vec<SpawnAgent>,
    /// Preferred cwd prefilled / listed first when starting a new agent.
    #[serde(default)]
    pub default_cwd: String,
    /// Where to put a newly started agent:
    ///   "tab"   — new tab in the chosen workspace (default)
    ///   "right" — split right in the chosen workspace
    ///   "down"  — split down in the chosen workspace
    /// Legacy alias: `split` is accepted with the same values.
    #[serde(default = "default_placement", alias = "split")]
    pub placement: String,
    /// Focus the new agent pane / tab after start.
    #[serde(default)]
    pub focus_new: bool,
    /// Always wait this long after `agent start` before sending the prompt
    /// (gives the CLI time to paint its input). Milliseconds.
    #[serde(default = "default_startup_delay")]
    pub startup_delay_ms: u64,
    /// After the startup delay, wait up to this many ms for the agent to
    /// report `idle` before sending. 0 skips the wait.
    #[serde(default = "default_wait_ready")]
    pub wait_ready_ms: u64,
}

impl Default for DelegateConfig {
    fn default() -> Self {
        Self {
            prompt: default_prompt(),
            submit: true,
            submit_delay_ms: default_submit_delay(),
            max_description_chars: default_max_desc(),
            agents: default_spawn_agents(),
            default_cwd: String::new(),
            placement: default_placement(),
            focus_new: false,
            startup_delay_ms: default_startup_delay(),
            wait_ready_ms: default_wait_ready(),
        }
    }
}

fn default_auth() -> String {
    "basic".into()
}
fn default_max_results() -> u32 {
    50
}
fn default_search_jql() -> String {
    r#"text ~ "{query}" ORDER BY updated DESC"#.into()
}
fn default_true() -> bool {
    true
}
fn default_submit_delay() -> u64 {
    500
}
fn default_max_desc() -> usize {
    6000
}
fn default_placement() -> String {
    "tab".into()
}
fn default_startup_delay() -> u64 {
    1500
}
fn default_wait_ready() -> u64 {
    30_000
}
fn default_spawn_agents() -> Vec<SpawnAgent> {
    ["claude", "codex", "grok", "cursor", "opencode"]
        .into_iter()
        .map(|name| SpawnAgent {
            name: name.into(),
            command: vec![name.into()],
        })
        .collect()
}
fn default_prompt() -> String {
    "You are asked to work on Jira issue {key}: {summary}\n\n\
     Link: {url}\n\nDescription:\n{description}\n\n\
     Please analyze the issue, implement what it describes, and summarize \
     what you changed when you are done."
        .into()
}

pub fn config_path() -> PathBuf {
    if let Ok(dir) = std::env::var("HERDR_PLUGIN_CONFIG_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("config.toml");
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config/herdr/plugins/config/herdr-jira/config.toml")
}

impl Config {
    pub fn load() -> Result<Self, String> {
        let path = config_path();
        let raw = std::fs::read_to_string(&path).map_err(|e| {
            format!(
                "cannot read config {}: {e}\n\ncopy config.example.toml there and fill in your Jira credentials",
                path.display()
            )
        })?;
        let mut cfg: Config =
            toml::from_str(&raw).map_err(|e| format!("invalid config {}: {e}", path.display()))?;
        cfg.jira.base_url = cfg.jira.base_url.trim_end_matches('/').to_string();
        if cfg.filters.is_empty() {
            cfg.filters.push(Filter {
                name: "My open issues".into(),
                jql: "assignee = currentUser() AND resolution = Unresolved ORDER BY updated DESC"
                    .into(),
            });
            if !cfg.jira.default_project.is_empty() {
                cfg.filters.push(Filter {
                    name: format!("Project {}", cfg.jira.default_project),
                    jql: "project = {project} ORDER BY updated DESC".into(),
                });
            }
        }
        Ok(cfg)
    }

    /// Expand {project} in a JQL template.
    pub fn expand_jql(&self, template: &str) -> String {
        template.replace("{project}", &self.jira.default_project)
    }

    /// Resolve the API token: inline value wins, else run `api_token_cmd`.
    pub fn resolve_token(&self) -> Result<String, String> {
        let inline = self.jira.api_token.trim();
        if !inline.is_empty() {
            return Ok(inline.to_string());
        }
        let cmd = self.jira.api_token_cmd.trim();
        if cmd.is_empty() {
            return Err("no api_token or api_token_cmd set in [jira] config".into());
        }
        let out = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
            .map_err(|e| format!("api_token_cmd failed to start: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "api_token_cmd exited with {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if token.is_empty() {
            return Err("api_token_cmd produced no output".into());
        }
        Ok(token)
    }
}
