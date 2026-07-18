//! Talk to the herdr server through its CLI (`HERDR_BIN_PATH`), as the plugin
//! docs recommend. Used to list running agents, start a new agent, and hand an
//! issue prompt to one.

use serde_json::Value;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct HerdrAgent {
    /// Stable send target (terminal id or agent name).
    pub target: String,
    pub label: String,
    pub status: String,
    pub cwd: String,
    pub pane_id: String,
}

/// How to place a newly started agent pane.
#[derive(Debug, Clone)]
pub struct StartAgentOpts {
    pub name: String,
    pub cwd: String,
    pub argv: Vec<String>,
    /// "tab" | "right" | "down".
    pub placement: String,
    pub focus: bool,
    pub workspace_id: String,
    /// Optional label for a newly created tab (usually the issue key).
    pub tab_label: String,
}

#[derive(Debug, Clone)]
pub struct HerdrWorkspace {
    pub id: String,
    pub label: String,
    pub number: u64,
    pub focused: bool,
    pub tab_count: u64,
    pub pane_count: u64,
    pub agent_status: String,
}

fn herdr_bin() -> String {
    std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into())
}

fn run(args: &[&str]) -> Result<Value, String> {
    let out = Command::new(herdr_bin())
        .args(args)
        .output()
        .map_err(|e| format!("failed to run herdr: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "herdr {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    // Some commands print nothing on success (e.g. send-keys proxied to a
    // mirror-plugin pane) — a clean exit with empty output is still success.
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&stdout)
        .map_err(|e| format!("herdr {}: bad JSON: {e}", args.join(" ")))
}

fn run_owned(args: &[String]) -> Result<Value, String> {
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    run(&refs)
}

/// List agent panes visible to herdr, excluding our own pane.
pub fn list_agents() -> Result<Vec<HerdrAgent>, String> {
    let v = run(&["agent", "list"])?;
    let own_pane = std::env::var("HERDR_PANE_ID").unwrap_or_default();
    let agents = v["result"]["agents"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|a| parse_agent(a, &own_pane))
        .collect();
    Ok(agents)
}

fn parse_agent(a: &Value, own_pane: &str) -> Option<HerdrAgent> {
    let pane_id = a["pane_id"].as_str().unwrap_or("").to_string();
    if !own_pane.is_empty() && pane_id == own_pane {
        return None;
    }
    // `agent list` also returns plain panes with no detected agent
    // (no label, status "unknown") — those are not delegable.
    let label = a["display_agent"]
        .as_str()
        .or_else(|| a["agent"].as_str())?
        .to_string();
    let status = a["agent_status"].as_str().unwrap_or("unknown").to_string();
    if status == "unknown" {
        return None;
    }
    Some(HerdrAgent {
        target: a["terminal_id"]
            .as_str()
            .or_else(|| a["name"].as_str())
            .unwrap_or(pane_id.as_str())
            .to_string(),
        label,
        status,
        cwd: a["cwd"].as_str().unwrap_or("").to_string(),
        pane_id,
    })
}

/// List herdr workspaces ("spaces" in the UI).
pub fn list_workspaces() -> Result<Vec<HerdrWorkspace>, String> {
    let v = run(&["workspace", "list"])?;
    let list = v["result"]["workspaces"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(list
        .iter()
        .map(|w| HerdrWorkspace {
            id: w["workspace_id"].as_str().unwrap_or("").to_string(),
            label: w["label"].as_str().unwrap_or("(unnamed)").to_string(),
            number: w["number"].as_u64().unwrap_or(0),
            focused: w["focused"].as_bool().unwrap_or(false),
            tab_count: w["tab_count"].as_u64().unwrap_or(0),
            pane_count: w["pane_count"].as_u64().unwrap_or(0),
            agent_status: w["agent_status"].as_str().unwrap_or("").to_string(),
        })
        .filter(|w| !w.id.is_empty())
        .collect())
}

/// Result of creating a tab: the tab id plus its single root shell pane.
#[derive(Debug, Clone)]
pub struct CreatedTab {
    #[allow(dead_code)]
    pub tab_id: String,
    pub pane_id: String,
    pub terminal_id: String,
}

/// Create a new tab in a workspace. Returns the tab and its root pane
/// (one terminal — do not also `agent start` into this tab or you get two panes).
pub fn create_tab(
    workspace_id: &str,
    cwd: &str,
    label: &str,
    focus: bool,
) -> Result<CreatedTab, String> {
    let mut args: Vec<String> = vec!["tab".into(), "create".into()];
    if !workspace_id.is_empty() {
        args.push("--workspace".into());
        args.push(workspace_id.into());
    }
    if !cwd.is_empty() {
        args.push("--cwd".into());
        args.push(cwd.into());
    }
    if !label.is_empty() {
        args.push("--label".into());
        args.push(label.into());
    }
    if focus {
        args.push("--focus".into());
    } else {
        args.push("--no-focus".into());
    }
    let v = run_owned(&args)?;
    let result = &v["result"];
    let tab = &result["tab"];
    let root = &result["root_pane"];

    let tab_id = tab["tab_id"]
        .as_str()
        .or_else(|| result["tab_id"].as_str())
        .unwrap_or("")
        .to_string();
    let pane_id = root["pane_id"].as_str().unwrap_or("").to_string();
    let terminal_id = root["terminal_id"].as_str().unwrap_or("").to_string();

    if tab_id.is_empty() {
        return Err(format!("tab create: no tab_id in response: {result}"));
    }
    if pane_id.is_empty() {
        // Older herdr: fall back to pane list for this tab.
        if let Ok(pid) = first_pane_in_tab(workspace_id, &tab_id) {
            return Ok(CreatedTab {
                tab_id,
                pane_id: pid,
                terminal_id,
            });
        }
        return Err(format!(
            "tab create: no root_pane.pane_id in response: {result}"
        ));
    }
    Ok(CreatedTab {
        tab_id,
        pane_id,
        terminal_id,
    })
}

fn first_pane_in_tab(workspace_id: &str, tab_id: &str) -> Result<String, String> {
    let v = if workspace_id.is_empty() {
        run(&["pane", "list"])?
    } else {
        run(&["pane", "list", "--workspace", workspace_id])?
    };
    let panes = v["result"]["panes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    panes
        .iter()
        .find(|p| p["tab_id"].as_str() == Some(tab_id))
        .and_then(|p| p["pane_id"].as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("no pane found in tab {tab_id}"))
}

/// Shell-quote argv into a single command line for `pane run`.
fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if a.is_empty() {
                "''".to_string()
            } else if a
                .chars()
                .any(|c| c.is_whitespace() || "\"'\\$`!&|;<>(){}[]".contains(c))
            {
                format!("'{}'", a.replace('\'', "'\\''"))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Start the agent binary inside an existing pane (replaces the shell process
/// in that pane — keeps a single terminal in the tab).
fn run_agent_in_pane(
    pane_id: &str,
    terminal_id: &str,
    name: &str,
    argv: &[String],
    cwd: &str,
) -> Result<HerdrAgent, String> {
    let cmd = shell_join(argv);
    if cmd.is_empty() {
        return Err("agent command is empty".into());
    }
    // pane run submits text + Enter atomically into the existing pane.
    run(&["pane", "run", pane_id, &cmd])?;
    // Give the process a moment to become foreground before rename.
    std::thread::sleep(std::time::Duration::from_millis(300));
    // Assign a stable agent name for send/wait (best-effort).
    let _ = run(&["agent", "rename", pane_id, name]);

    let label = argv
        .first()
        .cloned()
        .unwrap_or_else(|| name.to_string());
    let target = if !name.is_empty() {
        name.to_string()
    } else if !terminal_id.is_empty() {
        terminal_id.to_string()
    } else {
        pane_id.to_string()
    };
    Ok(HerdrAgent {
        target,
        label,
        status: "unknown".into(),
        cwd: cwd.to_string(),
        pane_id: pane_id.to_string(),
    })
}

/// Spawn a new agent. Returns the agent identity we can later send to.
///
/// - `placement = "tab"`: create a new tab and run the agent **in its root
///   pane** (one terminal). Using `agent start` after `tab create` would add a
///   second pane — we deliberately avoid that.
/// - `placement = "right"|"down"`: `herdr agent start --split …` in the workspace.
pub fn start_agent(opts: &StartAgentOpts) -> Result<HerdrAgent, String> {
    if opts.argv.is_empty() {
        return Err("agent command is empty".into());
    }
    if opts.name.trim().is_empty() {
        return Err("agent name is empty".into());
    }
    if opts.workspace_id.trim().is_empty() {
        return Err("workspace is empty".into());
    }

    let placement = opts.placement.trim().to_ascii_lowercase();

    if placement == "tab" || placement.is_empty() {
        let label = if opts.tab_label.is_empty() {
            opts.name.as_str()
        } else {
            opts.tab_label.as_str()
        };
        let tab = create_tab(&opts.workspace_id, &opts.cwd, label, opts.focus)?;
        return run_agent_in_pane(
            &tab.pane_id,
            &tab.terminal_id,
            &opts.name,
            &opts.argv,
            &opts.cwd,
        );
    }

    // Split placement: herdr agent start creates one new agent pane.
    let mut args: Vec<String> = vec!["agent".into(), "start".into(), opts.name.clone()];
    if !opts.cwd.is_empty() {
        args.push("--cwd".into());
        args.push(opts.cwd.clone());
    }
    args.push("--workspace".into());
    args.push(opts.workspace_id.clone());
    if placement == "right" || placement == "down" {
        args.push("--split".into());
        args.push(placement);
    }
    if opts.focus {
        args.push("--focus".into());
    } else {
        args.push("--no-focus".into());
    }
    args.push("--".into());
    args.extend(opts.argv.iter().cloned());

    let v = run_owned(&args)?;
    let agent = &v["result"]["agent"];
    if agent.is_null() {
        return Ok(HerdrAgent {
            target: opts.name.clone(),
            label: opts.argv.first().cloned().unwrap_or_else(|| opts.name.clone()),
            status: "unknown".into(),
            cwd: opts.cwd.clone(),
            pane_id: String::new(),
        });
    }

    let pane_id = agent["pane_id"].as_str().unwrap_or("").to_string();
    let name = agent["name"].as_str().unwrap_or(&opts.name);
    let terminal_id = agent["terminal_id"].as_str().unwrap_or("");
    let target = if !name.is_empty() {
        name.to_string()
    } else if !terminal_id.is_empty() {
        terminal_id.to_string()
    } else if !pane_id.is_empty() {
        pane_id.clone()
    } else {
        opts.name.clone()
    };
    let label = agent["display_agent"]
        .as_str()
        .or_else(|| agent["agent"].as_str())
        .unwrap_or(opts.argv.first().map(|s| s.as_str()).unwrap_or(name))
        .to_string();

    Ok(HerdrAgent {
        target,
        label,
        status: agent["agent_status"]
            .as_str()
            .unwrap_or("unknown")
            .to_string(),
        cwd: agent["cwd"]
            .as_str()
            .unwrap_or(&opts.cwd)
            .to_string(),
        pane_id,
    })
}

/// Block until the agent reports the given status (or timeout). Best-effort:
/// callers may ignore errors when a fallback delay is enough.
pub fn wait_agent_status(target: &str, status: &str, timeout_ms: u64) -> Result<(), String> {
    run(&[
        "agent",
        "wait",
        target,
        "--status",
        status,
        "--timeout",
        &timeout_ms.to_string(),
    ])?;
    Ok(())
}

/// Send `text` to an agent; optionally press Enter after a short delay so the
/// agent CLI submits the prompt. `agent send` writes literal text (newlines
/// insert line breaks in agent CLIs, they do not submit).
pub fn send_to_agent(
    agent: &HerdrAgent,
    text: &str,
    submit: bool,
    submit_delay_ms: u64,
) -> Result<(), String> {
    run(&["agent", "send", &agent.target, text])?;
    if submit {
        std::thread::sleep(std::time::Duration::from_millis(submit_delay_ms));
        let pane = if !agent.pane_id.is_empty() {
            agent.pane_id.clone()
        } else {
            // pane_id may be unknown after a partial start response; target
            // often also works for send-keys via agent identity.
            agent.target.clone()
        };
        // Prefer pane send-keys; if that fails (unknown pane), try agent wait
        // path is already done — fall through with error from send-keys.
        if let Err(e) = run(&["pane", "send-keys", &pane, "Enter"]) {
            // Some targets accept the agent name as a pane alias; last resort
            // re-send with a trailing newline is not possible via agent send.
            return Err(e);
        }
    }
    Ok(())
}

/// Start a new agent, wait until it looks ready, then send the prompt.
pub fn start_and_delegate(
    opts: &StartAgentOpts,
    text: &str,
    submit: bool,
    submit_delay_ms: u64,
    startup_delay_ms: u64,
    wait_ready_ms: u64,
) -> Result<HerdrAgent, String> {
    let agent = start_agent(opts)?;
    if startup_delay_ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(startup_delay_ms));
    }
    if wait_ready_ms > 0 {
        // Best-effort: many agents flip to idle when the prompt is ready.
        // Prefer pane_id (always valid after tab-create flow); then name.
        let waited = if !agent.pane_id.is_empty() {
            wait_agent_status(&agent.pane_id, "idle", wait_ready_ms)
        } else {
            Err("no pane".into())
        };
        if waited.is_err() {
            let _ = wait_agent_status(&agent.target, "idle", wait_ready_ms);
        }
    }
    // Prefer pane_id for send when present — works before agent detection
    // finishes renaming/identifying the process.
    let send_agent = if !agent.pane_id.is_empty() {
        HerdrAgent {
            target: agent.pane_id.clone(),
            ..agent.clone()
        }
    } else {
        agent.clone()
    };
    send_to_agent(&send_agent, text, submit, submit_delay_ms)?;
    Ok(agent)
}

/// Toggle fullscreen (zoom) for our own pane. Only works when running inside
/// a herdr pane (HERDR_PANE_ID is set by herdr for plugin panes).
pub fn zoom_toggle() -> Result<(), String> {
    let pane = std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| "not running inside a herdr pane".to_string())?;
    run(&["pane", "zoom", &pane, "--toggle"])?;
    Ok(())
}

/// Show a herdr toast notification (best effort).
pub fn notify(message: &str) {
    let _ = Command::new(herdr_bin())
        .args(["notification", "show", message])
        .output();
}

/// Expand `~` / `$HOME` at the start of a path.
pub fn expand_path(path: &str) -> String {
    let p = path.trim();
    if p == "~" {
        return std::env::var("HOME").unwrap_or_else(|_| p.to_string());
    }
    if let Some(rest) = p.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            return format!("{home}/{rest}");
        }
    }
    if let Some(rest) = p.strip_prefix("$HOME/") {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            return format!("{home}/{rest}");
        }
    }
    p.to_string()
}
