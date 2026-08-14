#!/usr/bin/env bash
# Deterministic core for inventory W4a: every list surface has a heading
# while /health is green. Full screenshot walk lives in artifacts/ui/.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_require_health

if ! command -v agent-browser >/dev/null 2>&1; then
  echo "c11-ui: agent-browser not on PATH" >&2
  exit 1
fi

export AGENT_BROWSER_SESSION="${AGENT_BROWSER_SESSION:-$(agent-browser session id --scope worktree --prefix c11ui)}"
ab() { agent-browser --session "$AGENT_BROWSER_SESSION" "$@"; }

check() {
  local path="$1" expect="$2"
  ab open "${PARALLAX_URL}${path}"
  ab wait --load networkidle || true
  sleep 0.4
  local snap
  snap="$(ab snapshot -i -c || true)"
  if ! printf '%s' "$snap" | grep -Eiq "$expect"; then
    echo "c11-ui: $path missing /$expect/" >&2
    printf '%s\n' "$snap" | head -40 >&2
    exit 1
  fi
  echo "c11-ui ok $path"
}

ab set viewport 1440 900
check "/" "Overview"
check "/issues" "Issues"
check "/traces" "Traces"
check "/logs" "Logs"
check "/metrics" "Metrics"
check "/services" "Services"
check "/ecosystem" "Ecosystem"
check "/invocations" "CLI Apps|Invocations"
check "/alerts" "Alerts"
check "/dashboards" "Dashboards"
check "/investigations" "Investigations"
check "/sql" "SQL"
check "/tests" "Tests"
echo "c11-ui ok"
