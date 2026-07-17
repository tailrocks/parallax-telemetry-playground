#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${CLI_INVOCATION_ID:-}" || -z "${TRACEPARENT:-}" ]]; then
  echo "error: run through: parallax invocation start -- scripts/observable-test-session.sh <rust|java|web>" >&2
  exit 2
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
stack="${1:-}"
acceptance="${2:-}"
if [[ -n "$acceptance" && "$acceptance" != "--acceptance" ]]; then
  echo "usage: $0 <rust|java|web> [--acceptance]" >&2
  exit 2
fi
export GIT_SHA="${GIT_SHA:-$(git -C "$root" rev-parse HEAD)}"
export VCS_REF="${VCS_REF:-$GIT_SHA}"

case "$stack" in
  rust)
    cd "$root"
    export PLAYGROUND_TEST_TELEMETRY=1
    mise exec -- cargo nextest run --locked --workspace --profile ci --no-tests=fail
    mise exec -- cargo run --locked -p playground-cli -- \
      test-report target/nextest/ci/junit.xml
    if [[ "$acceptance" == "--acceptance" ]]; then
      PLAYGROUND_TEST_FLAKY_FIXTURE=1 mise exec -- \
        cargo nextest run --locked -p playground-cli --profile w4-acceptance \
        -E 'test(/w4_.*_passes_on_retry/)' --no-tests=fail
      mise exec -- cargo run --locked -p playground-cli -- \
        test-report target/nextest/w4-acceptance/junit.xml
    fi
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
    if [[ "$acceptance" == "--acceptance" ]]; then
      token="${PLAYGROUND_TEST_ATTEMPT_TOKEN:-${GIT_SHA}-$$}"
      (
        cd "$root/services/payment"
        mise exec -- env GRADLE_USER_HOME=/tmp/parallax-gradle \
          PLAYGROUND_TEST_FLAKY_FIXTURE=1 PLAYGROUND_TEST_ATTEMPT_TOKEN="$token" \
          ./gradlew --no-daemon test \
          --tests dev.tailrocks.payment.TestTelemetryAcceptanceTest \
          -Dorg.gradle.native=false
      )
    fi
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
    if [[ "$acceptance" == "--acceptance" ]]; then
      PLAYGROUND_TEST_FLAKY_FIXTURE=1 PLAYWRIGHT_RETRIES=1 mise exec -- \
        bun ./node_modules/@playwright/test/cli.js test --project=chromium --grep 'W4 retry fixture'
    fi
    ;;
  *)
    echo "usage: $0 <rust|java|web> [--acceptance]" >&2
    exit 2
    ;;
esac
