#!/usr/bin/env bash
# c5: dashboard, investigation, SQL snippet surfaces via GraphQL.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_require_health

dash="$(c_gql 'mutation { dashboardSave(name: "c5-dash", layout: "{\"widgets\":[]}") { id name } }')"
dash_id="$(python3 -c "import json,sys; print(json.loads(sys.argv[1])['dashboardSave']['id'])" "$dash")"
inv="$(c_gql 'mutation { investigationSave(name: "c5-case", state: "{\"version\":1}") { id name } }')"
inv_id="$(python3 -c "import json,sys; print(json.loads(sys.argv[1])['investigationSave']['id'])" "$inv")"
sql="$(c_gql '{ sql(query: "SELECT 1") { columns rowCount } }')"
echo "$sql" | python3 -c "import json,sys; d=json.loads(sys.argv[1]); assert 'sql' in d" "$sql" 
echo "c5 ok dash=$dash_id inv=$inv_id"
