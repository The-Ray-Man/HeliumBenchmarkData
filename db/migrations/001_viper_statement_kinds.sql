-- Counts the Viper statement kinds the metrics missed, and magic wands.
--
-- Only adds nullable columns, so every existing row is kept. The runner treats
-- a viper_metrics row with a NULL column as incomplete and re-measures the
-- file on its next pass, which fills the new columns (and recomputes the old
-- ones with the new counting rules: `asserts` no longer includes `refute`,
-- `heap_update_exps` now also counts `folding` / `applying` / `packaging`).
--
-- Apply to an existing database (idempotent):
--
--   docker compose exec -T postgres psql -U helium -d helium_bench < db/migrations/001_viper_statement_kinds.sql
--
-- A fresh database gets these columns from db/schema.sql.

ALTER TABLE viper_metrics
    ADD COLUMN IF NOT EXISTS refutes     INTEGER,
    ADD COLUMN IF NOT EXISTS var_decls   INTEGER,
    ADD COLUMN IF NOT EXISTS packages    INTEGER,
    ADD COLUMN IF NOT EXISTS applies     INTEGER,
    ADD COLUMN IF NOT EXISTS other_stmts INTEGER,
    ADD COLUMN IF NOT EXISTS magic_wands INTEGER;
