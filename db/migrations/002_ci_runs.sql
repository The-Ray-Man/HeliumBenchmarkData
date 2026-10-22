-- Tables for benchmarking Helium in CI (see ci-benchmark-plan.md, Phase 3).
--
-- bench_runs marks a run as CI-owned: garbage collection only ever deletes
-- rows whose run_id is in it, so local and manual runs are never touched.
-- bench_targets records why a Helium commit's CI-owned rows are kept (a main
-- commit, or the head of an open PR).
--
-- Only adds a type, tables and indexes, so every existing row is kept.
--
-- Apply to an existing database (idempotent):
--
--   docker compose exec -T postgres psql -v ON_ERROR_STOP=1 -U helium -d helium_bench < db/migrations/002_ci_runs.sql
--
-- A fresh database gets these from db/schema.sql.

DO $$
BEGIN
    CREATE TYPE bench_run_status AS ENUM ('RUNNING', 'COMPLETE', 'FAILED', 'CANCELLED');
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

CREATE TABLE IF NOT EXISTS bench_runs (
    run_id              VARCHAR          PRIMARY KEY,  -- same value as in the *_runs tables
    helium_commit       VARCHAR          NOT NULL,     -- = helium_runs.backend_version
    host                VARCHAR,
    github_run_url      VARCHAR,
    status              bench_run_status NOT NULL,
    started_at          TIMESTAMPTZ      NOT NULL,
    finished_at         TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS bench_targets (
    id                  BIGINT        GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    helium_commit       VARCHAR       NOT NULL,
    pr_number           INTEGER,                   -- NULL: a main commit
    base_commit         VARCHAR,                   -- PRs: merge-base with main; main: first parent
    main_position       INTEGER,                   -- main: index from BENCH_START_COMMIT, for ordering
    updated_at          TIMESTAMPTZ   NOT NULL,

    UNIQUE NULLS NOT DISTINCT (helium_commit, pr_number)
);
