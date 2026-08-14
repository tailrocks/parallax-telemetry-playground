#!/usr/bin/env bash
# c11: agent-browser snapshot of / is non-blank while /health is green.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_require_health

if ! command -v agent-browser >/dev/null 2>&1; then
  echo "c11: agent-browser not on PATH" >&2
  exit 1
fi
export AGENT_BROWSER_SESSION="${AGENT_BROWSER_SESSION:-$(agent-browser session id --scope worktree --prefix c11)}"
agent-browser --session "$AGENT_BROWSER_SESSION" open "$PARALLAX_URL/"
agent-browser --session "$AGENT_BROWSER_SESSION" wait --load networkidle
body="$(agent-browser --session "$AGENT_BROWSER_SESSION" eval "document.body.innerText")"
if ! printf '%s' "$body" | grep -q "Parallax\|Overview\|Issues"; then
  echo "c11: blank or unexpected snapshot" >&2
  exit 1
fi
echo "c11 ok"
