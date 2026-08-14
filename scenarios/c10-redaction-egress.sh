#!/usr/bin/env bash
# c10: after a18-shaped canary seed (or c1), assert GraphQL bundle markdown
# does not contain the labeled canary tokens.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_require_health

data="$(c_gql '{ issues(limit: 5) { items { fingerprint } } }')"
fp="$(python3 -c "import json,sys; items=(json.loads(sys.argv[1]) or {}).get('issues',{}).get('items') or []; print(items[0]['fingerprint'] if items else '')" "$data")"
if [[ -z "$fp" ]]; then
  "$SCRIPT_DIR/c1-issue-context.sh" >/tmp/c10-c1.out
  data="$(c_gql '{ issues(limit: 5) { items { fingerprint } } }')"
  fp="$(python3 -c "import json,sys; items=(json.loads(sys.argv[1]) or {}).get('issues',{}).get('items') or []; print(items[0]['fingerprint'] if items else '')" "$data")"
fi
[[ -n "$fp" ]] || { echo "c10: no fingerprint" >&2; exit 1; }
bundle="$(c_gql "{ bundle(fingerprint: \"$fp\") { markdown json } }")"
python3 -c "import json,sys
d=json.loads(sys.argv[1])
text=(d['bundle']['markdown']+' '+d['bundle']['json']).lower()
for label, token in [('email','canary@example.com'),('token','sk_live_canarytoken'),('card','4111111111111111'),('jwt','eyJhbGciOiJIUzI1NiJ9')]:
    if token.lower() in text:
        raise SystemExit(f'c10 leak {label}')
" "$bundle"
echo "c10 ok fingerprint=$fp"
