#!/usr/bin/env bash
# B17 cron — weighted success/failure/stuck. Run under the lab via:
#   parallax run start -- scenarios/b17-cron.sh
set -euo pipefail
exec "$(dirname "$0")/../target/debug/playground" cron
