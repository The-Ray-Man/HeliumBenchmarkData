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

```bash
docker compose down -v && docker compose up -d
cd runner
sea-orm-cli generate entity -u postgres://helium:helium@localhost:5432/helium_bench -o db-generated/src --lib --with-copy-enums
```

The CLI's major version must match the `sea-orm` dependency (`cargo install sea-orm-cli@2.0.4`).

Example query:

```sql
SELECT suite, family, test, percentile_cont(0.5) WITHIN GROUP (ORDER BY wall_s) AS median_s,
       max(egraph_nodes_peak) AS nodes
FROM helium_runs
WHERE status = 'VERIFIED'
GROUP BY suite, family, test ORDER BY median_s DESC;
```

`docker compose down -v` deletes all results, Grafana's state (users, UI-made dashboards) and the database.
