#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${PARALLAX_RUN_ID:-}" || -z "${TRACEPARENT:-}" ]]; then
  echo "error: run through: parallax run start -- scripts/observable-test-session.sh <rust|java|web>" >&2
  exit 2
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
stack="${1:-}"

case "$stack" in
  rust)
    cd "$root"
    export PLAYGROUND_TEST_TELEMETRY=1
    mise exec -- cargo nextest run --locked --workspace --profile ci --no-tests=fail
    mise exec -- cargo run --locked -p playground-cli -- \
      test-report target/nextest/ci/junit.xml
    ;;
  java)
    for service in catalog payment fulfillment; do
      echo "running observable Java tests: $service"
      (
        cd "$root/services/$service"
        mise exec -- env GRADLE_USER_HOME=/tmp/parallax-gradle \
          ./gradlew --no-daemon test -Dorg.gradle.native=false
      )
    done
    ;;
  web)
    cd "$root/web"
    export PLAYGROUND_TEST_OTLP_ENDPOINT="${PLAYGROUND_TEST_OTLP_ENDPOINT:-${PARALLAX_OTLP_HTTP_TRACES_ENDPOINT:-}}"
    if [[ -z "$PLAYGROUND_TEST_OTLP_ENDPOINT" ]]; then
      echo "error: Parallax did not provide its OTLP/HTTP traces endpoint" >&2
      exit 2
    fi
    mise exec -- bun run test
    mise exec -- bun ./node_modules/@playwright/test/cli.js test --project=chromium
    ;;
  *)
    echo "usage: $0 <rust|java|web>" >&2
    exit 2
    ;;
esac
