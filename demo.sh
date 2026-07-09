#!/usr/bin/env bash
set -euo pipefail

# One-command demo: playground stack + baseline traffic -> local parallax serve.
if ! nc -z 127.0.0.1 4317 2>/dev/null; then
  echo "No OTLP listener on 127.0.0.1:4317."
  echo "Start Parallax first (in the parallax repo):  parallax serve"
  echo "(or the fan-out lab if you are comparing backends)"
  exit 1
fi

docker compose -f deploy/docker-compose.yml --profile demo up --build -d

echo "Stack up. Baseline traffic is running (k6 loadgen, ~1-2 rps)."
echo "Fire a story scenario:   scenarios/run.sh a1"
echo "Open Parallax:           http://localhost:4000"
echo "Stop everything:         docker compose -f deploy/docker-compose.yml --profile demo down"
