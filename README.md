# HeliumBenchmarkData

Benchmark suites (`suites`) for multiple backends, plus a local Postgres + Grafana setup for storing results and building dashboards.

## Layout

- `suites/<suite>/<family>/<test>.vpr` – benchmark programs; `suites/suite.json` describes every suite and family.
- `generation/` – generators for the `scaling` suite.
- `db/schema.sql` – results schema: `rust_metrics` and `viper_metrics` (metrics per file, keyed by the file's sha256), and `helium_runs`, `silicon_runs`, `rustc_runs` (one row per execution of that backend on a file, with backend-specific metric columns).
- `runner/` – Rust workspace of the benchmark runner; `runner/db-generated` holds the SeaORM entities for `db/schema.sql`.
- `grafana/` – Grafana provisioning (Postgres data source) and dashboards (`grafana/dashboards/*.json`).

## Quick start

```bash
docker compose up -d
```

- Grafana: http://localhost:3000 (admin / admin unless `GRAFANA_ADMIN_USER` / `GRAFANA_ADMIN_PASSWORD` are set)
- Postgres: `postgres://helium:helium@localhost:5432/helium_bench` (user / password from `POSTGRES_USER` / `POSTGRES_PASSWORD`)

`db/schema.sql` is applied when the Postgres volume is first created. After changing the schema, recreate the database (this deletes all results) and regenerate the entities:

Results are stored as Parquet files in `data/`, one per run and table (`data/<table>/<run_id>.parquet`), which are the source of truth and are not committed. Grafana reads them through an in-memory DuckDB, so adding new runs while Grafana is running is fine; they show up on the next query. The same files can be queried directly with DuckDB:

```sql
SELECT suite, family, test, median(wall_s) AS median_s, max(egraph_nodes_peak) AS nodes
FROM 'data/helium_runs/*.parquet'
WHERE status = 'VERIFIED'
GROUP BY ALL ORDER BY median_s DESC;
```

`docker compose down -v` deletes Grafana's state (users, UI-made dashboards) and the downloaded plugin, but not `data/`.
