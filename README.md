# herdr-jira

A Jira TUI that lives in a [herdr](https://herdr.dev) pane: browse issues through
configurable JQL filters, search, change issue status, and delegate an issue to
any AI agent in herdr with one key — pick a running agent, or start a new one
in a chosen directory. The agent receives a prompt built from a configurable
template (issue key, summary, description, link, …).

```
╭ Jira — My open issues (23) ─────────────────────────────────────────╮
│ KEY         STATUS        ASSIGNEE          UPDATED          SUMMARY│
│ PROJ-142    In Progress   Vitalii R.        2026-07-14 10:02 Fix …  │
│ PROJ-137    To Do         Vitalii R.        2026-07-13 18:40 Add …  │
╰─────────────────────────────────────────────────────────────────────╯
 Enter open · f filters · / search · s status · d delegate · o browser
```

## Features

- **Filters** — named JQL filters from the config (`f` or `1`–`9`): my issues,
  a specific project, anything JQL can express.
- **Search** — `/` runs a `text ~ "…"` search (template configurable), and `J`
  runs any raw JQL you type, prefilled with the current query for quick tweaks.
- **Epics** — epics stand out with a magenta type badge and a `▸` marker;
  `→` expands one inline to show its child issues (`parent = …`, with a
  `"Epic Link"` fallback for Server/DC), `←` collapses it.
- **Issue details** — `Enter` opens a scrollable view with the description
  (Cloud ADF documents are flattened to plain text).
- **Status transitions** — `s` lists the transitions available for the issue
  and applies the one you pick.
- **Delegate to an agent** — `d` lists agents currently running in herdr
  (claude, codex, grok, …) with status and cwd; pick one and the issue is sent
  as a prompt from your `[delegate].prompt` template, then submitted with Enter
  (configurable). Or choose **+ start new agent…** (`n`) to pick an agent type
  and working directory — herdr spawns it via `agent start` and the same Jira
  prompt is sent as soon as the agent is ready.

Works with Jira Cloud (email + API token) and Jira Server / Data Center
(personal access token). Cloud's newer `/rest/api/2/search/jql` endpoint is
used when available, with automatic fallback to the classic `/rest/api/2/search`.

## Install

Requires a Rust toolchain (https://rustup.rs) at install time.

```sh
herdr plugin install a2u/herdr-jira
```

or for local development:

```sh
git clone git@github.com:a2u/herdr-jira.git
herdr plugin link ./herdr-jira
```

## Configure

```sh
mkdir -p "$(herdr plugin config-dir herdr-jira)"
cp config.example.toml "$(herdr plugin config-dir herdr-jira)/config.toml"
```

Edit `config.toml`:

```toml
[jira]
base_url = "https://yourcompany.atlassian.net"
auth = "basic"                      # "bearer" for Server/DC PAT
email = "you@company.com"
api_token_cmd = "security find-generic-password -s jira-api-token -w"
default_project = "PROJ"

[[filters]]
name = "My open issues"
jql = "assignee = currentUser() AND resolution = Unresolved ORDER BY updated DESC"

[[filters]]
name = "Project board"
jql = "project = {project} AND statusCategory != Done ORDER BY updated DESC"

[delegate]
prompt = """
You are asked to work on Jira issue {key}: {summary}
Link: {url}

Description:
{description}
"""
submit = true          # press Enter in the agent pane after sending

# Agents you can spawn from the delegate picker ("+ start new agent…")
[[delegate.agents]]
name = "claude"
command = ["claude"]

[[delegate.agents]]
name = "codex"
command = ["codex"]

[[delegate.agents]]
name = "grok"
command = ["grok"]

# default_cwd = "~/Work"
placement = "tab"      # "tab" | "right" | "down"
focus_new = false
startup_delay_ms = 1500
wait_ready_ms = 30000
```

For Jira Cloud, create an API token at
<https://id.atlassian.com/manage-profile/security/api-tokens> and store it in
the macOS Keychain so it never touches the config file:

```sh
security add-generic-password -s jira-api-token -a "$USER" -w '<TOKEN>'
```

The running pane reloads the config on `R`.

## Open the pane

From the herdr action palette: **Jira: open (split)** or **Jira: open (tab)** —
or bind a key in `~/.config/herdr/config.toml`:

```toml
[[keys.command]]              # open in a split beside your work
key = "prefix+j"
type = "plugin_action"
command = "herdr-jira.open-jira"

[[keys.command]]              # …or in its own tab
key = "prefix+shift+j"
type = "plugin_action"
command = "herdr-jira.open-jira-tab"
```

(then `herdr server reload-config`)

## Keys

| Key | Action |
| --- | --- |
| `j`/`k`, `↑`/`↓` | move / scroll |
| `Enter` | open issue details |
| `→`/`l`, `←`/`h` | expand / collapse an epic (shows its child issues inline) |
| `f`, `1`–`9` | switch filter |
| `/` | search |
| `J` | run a custom JQL query (prefilled with the current one) |
| `s` | change issue status |
| `d` | delegate issue to a running agent, or start a new one |
| `n` | in the delegate picker: start a new agent |
| `1`–`9` | quick pick inside any popup (agents, transitions, filters) |
| `o` | open issue in the browser |
| `r` | refresh current filter |
| `R` | reload config |
| `?` | help |
| `q` | quit |

## Delegate prompt placeholders

`{key}` `{summary}` `{description}` `{url}` `{status}` `{assignee}`
`{reporter}` `{priority}` `{type}` `{labels}`

The prompt is sent with `herdr agent send` (literal text — newlines insert
line breaks in agent CLIs, they don't submit), followed by an Enter keypress
after `submit_delay_ms` when `submit = true`.

### Starting a new agent

From the delegate picker, **+ start new agent…** (or `n`) opens a short wizard:

1. **Agent type** — from `[[delegate.agents]]` (name + `command` argv).
2. **Space (workspace)** — pick which herdr space gets the agent (current space
   is pre-selected).
3. **Working directory** — unique cwds from running agents, optional
   `default_cwd`, plus common paths; or **type path…** for a free-text path
   (`~` and `$HOME/` expand).

With `placement = "tab"` (default) the plugin creates a new tab labelled with
the issue key and runs the agent **in that tab’s single root pane** (so you get
one terminal, not a shell + agent split):

```sh
herdr tab create --workspace <space> --cwd <dir> --label <ISSUE-KEY> --no-focus
herdr pane run <root-pane> '<command...>'
herdr agent rename <root-pane> <issue-agent-id>
```

With `placement = "right"` or `"down"` it uses `herdr agent start --split …`
in the chosen space instead.

Then it waits `startup_delay_ms` (and up to `wait_ready_ms` for `idle`), and
sends the same rendered Jira prompt into the new agent.

## License

MIT
