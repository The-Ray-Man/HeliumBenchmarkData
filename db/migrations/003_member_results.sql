-- Per-member results: what Helium reported for each member (method, function,
-- predicate) of a file, and the Viper source, so a dashboard can put Helium's
-- and Silicon's verdicts next to the code (grafana/dashboards/helium-file.json).
--
-- Only adds a column and a table, so every existing row is kept. Runs from
-- before this migration have no per-member results (helium_runs.members is
-- NULL); `runner index` stores the source of every benchmark file that has
-- none yet.
--
-- Apply to an existing database (idempotent):
--
--   docker compose exec -T postgres psql -v ON_ERROR_STOP=1 -U helium -d helium_bench < db/migrations/003_member_results.sql
--
-- A fresh database gets these from db/schema.sql.

ALTER TABLE helium_runs ADD COLUMN IF NOT EXISTS members JSONB;

CREATE TABLE IF NOT EXISTS viper_sources (
    vpr_sha256          VARCHAR     PRIMARY KEY REFERENCES viper_metrics,
    source              TEXT        COMPRESSION lz4 NOT NULL
);
