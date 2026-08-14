#!/usr/bin/env bash
# c4: webhook destination + error_rate rule + poll incident after OTLP errors.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_require_health

dest="$(c_gql 'mutation { alertDestinationSave(name: "c4-hook", kind: "webhook", config: "{\"url\":\"http://127.0.0.1:9/c4\"}") { id } }')"
dest_id="$(python3 -c "import json,sys; print(json.loads(sys.argv[1])['alertDestinationSave']['id'])" "$dest")"
slack="$(c_gql 'mutation { alertDestinationSave(name: "c4-slack", kind: "slack_webhook", config: "{\"url\":\"http://127.0.0.1:9/c4-slack\"}") { id kind } }')"
slack_id="$(python3 -c "import json,sys; print(json.loads(sys.argv[1])['alertDestinationSave']['id'])" "$slack")"
echo "c4 slack dest=$slack_id"
rule="$(c_gql "mutation { alertRuleSave(input: { name: \"c4-high-errors\", enabled: true, signalType: \"error_rate\", services: [\"checkout\"], comparator: \"gt\", threshold: 0.2, windowMinutes: 5, minimumSampleCount: 1, consecutiveBreachesRequired: 1, consecutiveHealthyRequired: 1, severity: \"critical\", destinationIds: [\"$dest_id\"] }) { id } }")"
rule_id="$(python3 -c "import json,sys; print(json.loads(sys.argv[1])['alertRuleSave']['id'])" "$rule")"

# Reuse c1 seed so error_rate has material.
"$SCRIPT_DIR/c1-issue-context.sh" >/tmp/c4-c1.out || true

open_id=""
for _ in $(seq 1 36); do
  inc="$(c_gql '{ alertIncidents(limit: 8) { id status } }')"
  open_id="$(python3 -c "import json,sys; rows=json.loads(sys.argv[1]).get('alertIncidents') or []; print(next((r['id'] for r in rows if r.get('status')=='open'), ''))" "$inc")"
  if [[ -n "$open_id" ]]; then break; fi
  sleep 5
done
[[ -n "$open_id" ]] || { echo "c4: no open incident after wait (eval interval)" >&2; exit 1; }
echo "c4 ok incident=$open_id rule=$rule_id dest=$dest_id slack=$slack_id"
