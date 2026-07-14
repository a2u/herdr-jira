//! Talk to the herdr server through its CLI (`HERDR_BIN_PATH`), as the plugin
//! docs recommend. Used to list running agents and hand an issue prompt to one.

use serde_json::Value;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct HerdrAgent {
    /// Stable send target (terminal id).
    pub target: String,
    pub label: String,
    pub status: String,
    pub cwd: String,
    pub pane_id: String,
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
    serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("herdr {}: bad JSON: {e}", args.join(" ")))
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
        .filter_map(|a| {
            let pane_id = a["pane_id"].as_str().unwrap_or("").to_string();
            if !own_pane.is_empty() && pane_id == own_pane {
                return None;
            }
            Some(HerdrAgent {
                target: a["terminal_id"]
                    .as_str()
                    .unwrap_or(pane_id.as_str())
                    .to_string(),
                label: a["display_agent"]
                    .as_str()
                    .or_else(|| a["agent"].as_str())
                    .unwrap_or("agent")
                    .to_string(),
                status: a["agent_status"].as_str().unwrap_or("unknown").to_string(),
                cwd: a["cwd"].as_str().unwrap_or("").to_string(),
                pane_id,
            })
        })
        .collect();
    Ok(agents)
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
        run(&["pane", "send-keys", &agent.pane_id, "Enter"])?;
    }
    Ok(())
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
