#!/usr/bin/env bash
set -euo pipefail

root=$(git rev-parse --show-toplevel)
web="$root/web"
config="$web/tsconfig.json"

forbidden=$(git -C "$root" ls-files '*.js' '*.jsx' '*.mjs' '*.cjs' '*.mts' '*.cts' | while read -r path; do
  [[ ! -e "$root/$path" ]] || printf '%s\n' "$path"
done)
if [[ -n "$forbidden" ]]; then
  printf '%s\n' "$forbidden"
  printf 'tracked JavaScript source/config is forbidden; use strict TypeScript\n' >&2
  exit 1
fi

for option in \
  strict \
  noUncheckedIndexedAccess \
  exactOptionalPropertyTypes \
  noImplicitOverride \
  noImplicitReturns \
  noPropertyAccessFromIndexSignature \
  noUnusedLocals \
  noUnusedParameters \
  noFallthroughCasesInSwitch \
  noUncheckedSideEffectImports \
  forceConsistentCasingInFileNames \
  isolatedModules \
  noEmit; do
  [[ $(jq -r ".compilerOptions.$option" "$config") == true ]]
done
[[ $(jq -r '.compilerOptions.allowJs' "$config") == false ]]
[[ $(jq -r '.compilerOptions.checkJs' "$config") == false ]]
[[ $(jq -r '.compilerOptions.allowUnusedLabels' "$config") == false ]]
[[ $(jq -r '.compilerOptions.allowUnreachableCode' "$config") == false ]]
[[ $(jq -r '.compilerOptions.moduleDetection' "$config") == force ]]

selected=$(cd "$web" && bunx --bun --no-install tsc --showConfig)
for file in server.ts ../loadgen/checkout.ts ../loadgen/demo.ts; do
  jq -e --arg suffix "$file" '.files | any(endswith($suffix))' <<<"$selected" >/dev/null
done

(cd "$web" && bun run typecheck)
printf 'strict TypeScript-only policy passed\n'
