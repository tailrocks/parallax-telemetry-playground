#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNNER="$ROOT/scenarios/run.sh"
TEMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TEMP_DIR"' EXIT

while IFS= read -r script; do
  bash -n "$script"
done < <(find "$ROOT/scenarios" -maxdepth 1 -type f -name '*.sh' -print | sort)

"$RUNNER" |
  awk 'NR > 1 && NF { print $1, $2 }' |
  LC_ALL=C sort >"$TEMP_DIR/catalog"
sed -nE 's/^    ([a-z0-9-]+)\) echo "([^|]+)\|.*/\1 \2/p' "$RUNNER" |
  LC_ALL=C sort >"$TEMP_DIR/dispatch"
sed -nE 's/^\| ([a-z0-9-]+) \| `([^ `]+).*/\1 \2/p' \
  "$ROOT/scenarios/README.md" |
  LC_ALL=C sort >"$TEMP_DIR/readme"

if ! cmp -s "$TEMP_DIR/catalog" "$TEMP_DIR/dispatch"; then
  echo "scenario catalog and dispatcher mappings differ" >&2
  diff -u "$TEMP_DIR/catalog" "$TEMP_DIR/dispatch" >&2 || true
  exit 1
fi

if ! cmp -s "$TEMP_DIR/catalog" "$TEMP_DIR/readme"; then
  echo "scenario README and executable catalog mappings differ" >&2
  diff -u "$TEMP_DIR/catalog" "$TEMP_DIR/readme" >&2 || true
  exit 1
fi

while read -r id mapping; do
  path="$ROOT/scenarios/$mapping"
  if [[ "$id" == "a7" ]]; then
    [[ -f "$path" ]] || { echo "scenario $id is missing $mapping" >&2; exit 1; }
  else
    [[ -x "$path" ]] || { echo "scenario $id is not executable: $mapping" >&2; exit 1; }
  fi
done <"$TEMP_DIR/dispatch"

count="$(wc -l <"$TEMP_DIR/dispatch" | tr -d ' ')"
printf 'scenario contract is complete: %s dispatches\n' "$count"
