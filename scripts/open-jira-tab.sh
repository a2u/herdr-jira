#!/usr/bin/env bash
# Launch-or-focus the Jira pane in its own tab.
set -uo pipefail

herdr_bin="${HERDR_BIN_PATH:-herdr}"

if "$herdr_bin" plugin pane focus --plugin herdr-jira --entrypoint jira >/dev/null 2>&1; then
  exit 0
fi

exec "$herdr_bin" plugin pane open \
  --plugin herdr-jira \
  --entrypoint jira \
  --placement tab \
  --focus
