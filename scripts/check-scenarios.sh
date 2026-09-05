#!/usr/bin/env bash
set -Eeuo pipefail
# shellcheck disable=SC2016 # Single-quoted regex and awk programs intentionally contain literal syntax.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNNER="$ROOT/scenarios/run.sh"
README="$ROOT/scenarios/README.md"
MATRIX="$ROOT/docs/corner-case-matrix.md"
TEMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TEMP_DIR"' EXIT

while IFS= read -r script; do
  bash -n "$script"
done < <(find "$ROOT/scenarios" -maxdepth 1 -type f -name '*.sh' -print | sort)

"$RUNNER" |
  awk 'NR > 1 && NF { print $1, $2 }' |
  LC_ALL=C sort >"$TEMP_DIR/catalog"

sed -nE 's/^[[:space:]]*([a-z0-9-]+)\) echo "([^|]+)\|.*/\1 \2/p' \
  "$RUNNER" >"$TEMP_DIR/dispatch-raw"
awk 'NF { print $1, $2 }' "$TEMP_DIR/dispatch-raw" |
  LC_ALL=C sort >"$TEMP_DIR/dispatch"

duplicates="$(awk 'seen[$1]++ { print $1 }' "$TEMP_DIR/dispatch-raw")"
[[ -z "$duplicates" ]] || {
  echo "scenario dispatcher has duplicate ids: $duplicates" >&2
  exit 1
}

# shellcheck disable=SC2016
sed -nE 's/^\| `([a-z0-9-]+)` \| `([^ `]+)` \|.*/\1 \2/p' \
  "$README" |
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
  [[ -n "$id" && -n "$mapping" ]] || continue
  path="$ROOT/scenarios/$mapping"
  [[ -f "$path" ]] || { echo "scenario $id is missing $mapping" >&2; exit 1; }
  if [[ "$mapping" == *.sh ]]; then
    [[ -x "$path" ]] || { echo "scenario $id is not executable: $mapping" >&2; exit 1; }
  fi
done <"$TEMP_DIR/dispatch"

while IFS= read -r script; do
  [[ -x "$script" ]] || continue
  name="${script##*/}"
  case "$name" in
    lib-c.sh|run.sh) continue
  esac
  if ! awk -v mapping="$name" '$2 == mapping { found = 1 } END { exit found ? 0 : 1 }' \
      "$TEMP_DIR/dispatch"; then
    echo "executable scenario script is orphaned from the dispatcher: $name" >&2
    exit 1
  fi
done < <(find "$ROOT/scenarios" -maxdepth 1 -type f -name '*.sh' -print | sort)

# shellcheck disable=SC2016
awk -F'|' 'NR > 2 && $2 ~ /`[a-z0-9-]+`/ { print $2 }' "$MATRIX" |
  grep -oE '`[a-z0-9-]+`' |
  tr -d '`' |
  LC_ALL=C sort -u >"$TEMP_DIR/matrix"

[[ -s "$TEMP_DIR/matrix" ]] || {
  echo "corner-case matrix contains no dispatcher ids" >&2
  exit 1
}
while IFS= read -r id; do
  if ! awk -v id="$id" '$1 == id { found = 1 } END { exit found ? 0 : 1 }' \
      "$TEMP_DIR/dispatch"; then
    echo "corner-case matrix references an unknown dispatcher id: $id" >&2
    exit 1
  fi
done <"$TEMP_DIR/matrix"

# shellcheck disable=SC2016
awk -F'|' 'NR > 2 { print $2; print $3 }' "$MATRIX" |
  grep -oE '`[^`]+`' |
  tr -d '`' |
  LC_ALL=C sort -u >"$TEMP_DIR/matrix-refs"
while IFS= read -r ref; do
  case "$ref" in
    checkoutFlow|playground\ console\ ...|shapes\ \<id\>|shapes\ eco-external|tok_decline|tok_visa|updatePrice)
      continue
      ;;
  esac
  if ! awk -v id="$ref" '$1 == id { found = 1 } END { exit found ? 0 : 1 }' \
      "$TEMP_DIR/dispatch"; then
    echo "corner-case matrix trigger references an unknown dispatcher id: $ref" >&2
    exit 1
  fi
done <"$TEMP_DIR/matrix-refs"

count="$(wc -l <"$TEMP_DIR/dispatch" | tr -d ' ')"
printf 'scenario contract is complete: %s dispatches; matrix mappings validated\n' "$count"
