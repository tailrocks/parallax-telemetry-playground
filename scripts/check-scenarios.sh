#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNNER="$ROOT/scenarios/run.sh"

find "$ROOT/scenarios" -maxdepth 1 -type f -name '*.sh' -print0 |
  sort -z |
  xargs -0 -r -n 1 bash -n

mapfile -t catalog_ids < <(
  "$RUNNER" | awk 'NR > 1 && NF { print $1 }' | sort
)
mapfile -t dispatch_ids < <(
  sed -nE 's/^    ([a-z0-9-]+)\) echo .*/\1/p' "$RUNNER" | sort
)

if [[ "${catalog_ids[*]}" != "${dispatch_ids[*]}" ]]; then
  echo "scenario catalog and dispatcher IDs differ" >&2
  diff -u \
    <(printf '%s\n' "${catalog_ids[@]}") \
    <(printf '%s\n' "${dispatch_ids[@]}") >&2 || true
  exit 1
fi

for id in "${dispatch_ids[@]}"; do
  mapping="$(sed -nE "s/^    ${id//-/\\-}\\) echo \\\"([^|]+)\\|.*/\\1/p" "$RUNNER")"
  if [[ -z "$mapping" ]]; then
    echo "scenario $id has no script mapping" >&2
    exit 1
  fi
  path="$ROOT/scenarios/$mapping"
  if [[ "$id" == "a7" ]]; then
    [[ -f "$path" ]] || { echo "scenario $id is missing $mapping" >&2; exit 1; }
  else
    [[ -x "$path" ]] || { echo "scenario $id is not executable: $mapping" >&2; exit 1; }
  fi
done

printf 'scenario contract is complete: %s dispatches\n' "${#dispatch_ids[@]}"
