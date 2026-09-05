#!/bin/sh
set -eu

migration_root="${MIGRATION_DIR:-/migrations/migrations}"
schema_verifier="${SCHEMA_VERIFIER:-/migrations/verify-schema.sh}"
startup_timeout="${MIGRATION_TIMEOUT_SECONDS:-120}"
startup_poll_seconds="${MIGRATION_POLL_SECONDS:-1}"
migration_statement_timeout_ms="${MIGRATION_STATEMENT_TIMEOUT_MS:-10000}"
migration_lock_timeout_ms="${MIGRATION_LOCK_TIMEOUT_MS:-5000}"
pg_connect_timeout="${PGCONNECT_TIMEOUT:-5}"
pg_host="${PGHOST:-${POSTGRES_HOST:-postgres}}"
pg_port="${PGPORT:-${POSTGRES_PORT:-5432}}"
pg_database="${PGDATABASE:-${POSTGRES_DB:-playground}}"
pg_user="${PGUSER:-${POSTGRES_USER:-postgres}}"
pg_password="${PGPASSWORD:-${POSTGRES_PASSWORD:-}}"

case "$startup_timeout" in
  ''|*[!0-9]*|0)
    echo "MIGRATION_TIMEOUT_SECONDS must be a positive integer: $startup_timeout" >&2
    exit 1
    ;;
esac
case "$startup_poll_seconds" in
  ''|*[!0-9]*|0)
    echo "MIGRATION_POLL_SECONDS must be a positive integer: $startup_poll_seconds" >&2
    exit 1
    ;;
esac
if [ "$startup_poll_seconds" -gt "$startup_timeout" ]; then
  echo "MIGRATION_POLL_SECONDS must not exceed MIGRATION_TIMEOUT_SECONDS: $startup_poll_seconds" >&2
  exit 1
fi
case "$migration_statement_timeout_ms" in
  ''|*[!0-9]*|0)
    echo "MIGRATION_STATEMENT_TIMEOUT_MS must be a positive integer: $migration_statement_timeout_ms" >&2
    exit 1
    ;;
esac
case "$migration_lock_timeout_ms" in
  ''|*[!0-9]*|0)
    echo "MIGRATION_LOCK_TIMEOUT_MS must be a positive integer: $migration_lock_timeout_ms" >&2
    exit 1
    ;;
esac
case "$pg_connect_timeout" in
  ''|*[!0-9]*|0)
    echo "PGCONNECT_TIMEOUT must be a positive integer: $pg_connect_timeout" >&2
    exit 1
    ;;
esac

if [ ! -d "$migration_root" ]; then
  echo "PostgreSQL migration directory is missing: $migration_root" >&2
  exit 1
fi
if [ ! -f "$schema_verifier" ] || [ ! -r "$schema_verifier" ]; then
  echo "PostgreSQL schema verifier is missing or unreadable: $schema_verifier" >&2
  exit 1
fi

migration_count=0
for migration_path in "$migration_root"/*.sql; do
  [ -f "$migration_path" ] || continue
  migration_count=$((migration_count + 1))
  migration_name=${migration_path##*/}
  migration_version=${migration_name%.sql}

  case "$migration_version" in
    ''|*[!A-Za-z0-9._-]*)
      echo "invalid migration filename: $migration_name" >&2
      exit 1
      ;;
  esac
done

if [ "$migration_count" -eq 0 ]; then
  echo "no PostgreSQL migrations found in $migration_root" >&2
  exit 1
fi

export PGHOST="$pg_host"
export PGPORT="$pg_port"
export PGDATABASE="$pg_database"
export PGUSER="$pg_user"
export PGPASSWORD="$pg_password"
export PGCONNECT_TIMEOUT="$pg_connect_timeout"
# Bound DDL, advisory-lock acquisition, and every verifier query in the
# session. Preserve caller options, but make these safety limits authoritative
# for this migration process by appending them last.
export PGOPTIONS="${PGOPTIONS:+$PGOPTIONS }-c statement_timeout=${migration_statement_timeout_ms}ms -c lock_timeout=${migration_lock_timeout_ms}ms"

ready_waited=0
while ! pg_isready -q; do
  if [ "$ready_waited" -ge "$startup_timeout" ]; then
    echo "PostgreSQL did not become ready within ${startup_timeout}s" >&2
    echo "  host=$pg_host port=$pg_port database=$pg_database user=$pg_user" >&2
    pg_isready -d "$pg_database" -h "$pg_host" -p "$pg_port" -U "$pg_user" >&2 || true
    exit 1
  fi
  sleep "$startup_poll_seconds"
  ready_waited=$((ready_waited + startup_poll_seconds))
done

for migration_path in "$migration_root"/*.sql; do
  [ -f "$migration_path" ] || continue
  migration_name=${migration_path##*/}
  migration_version=${migration_name%.sql}

  if ! psql -v ON_ERROR_STOP=1 \
      -v "migration_path=$migration_path" \
      -v "migration_version=$migration_version" <<'SQL'
BEGIN;
SET LOCAL search_path TO public;
SELECT pg_try_advisory_xact_lock(hashtextextended('telemetry-playground:schema-migrations', 0))
    AS migration_lock_acquired \gset

\if :migration_lock_acquired
\else
    \echo migration lock is busy; refusing to wait past the verifier bound
    ROLLBACK;
    \quit 1
\endif

CREATE TABLE IF NOT EXISTS public.schema_migrations (
    version TEXT PRIMARY KEY,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

SELECT EXISTS (
    SELECT 1
    FROM public.schema_migrations
    WHERE version = :'migration_version'
) AS migration_applied \gset

\if :migration_applied
    \echo migration :migration_version already applied
\else
    \echo applying migration :migration_version
    \ir :migration_path
    INSERT INTO public.schema_migrations (version)
    VALUES (:'migration_version');
\endif

COMMIT;
SQL
  then
    echo "PostgreSQL migration failed and was rolled back: $migration_name" >&2
    echo "  host=$pg_host port=$pg_port database=$pg_database" >&2
    exit 1
  fi
done

if ! sh "$schema_verifier"; then
  echo "PostgreSQL schema verification failed after migration run" >&2
  echo "  host=$pg_host port=$pg_port database=$pg_database" >&2
  exit 1
fi
