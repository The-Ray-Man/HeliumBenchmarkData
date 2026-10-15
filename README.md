# HeliumBenchmarkData

Benchmark suites (`suites`) for multiple backends, plus a local Postgres + Grafana setup for storing results and building dashboards.

## Layout

- `suites/<suite>/<family>/<test>.vpr` – benchmark programs.
- `generation/` – generators for the `scaling` suite.
- `db/schema.sql` – results schema: `rust_metrics` and `viper_metrics` (metrics per file, keyed by the file's sha256), and `helium_runs`, `silicon_runs`, `rustc_runs` (one row per execution of that backend on a file, with backend-specific metric columns).
- `runner/` – Rust workspace of the benchmark runner; `runner/db-generated` holds the SeaORM entities for `db/schema.sql`. `runner/suite.example.json` describes every suite and family and how to run them (see [Suite configuration](#suite-configuration)).
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

## Suite configuration

The runner reads a suite configuration: which tools to run, how often, and which suites. `runner/suite.example.json` is the template. Copy it to `runner/suite.local.json` (git-ignored) and adapt it to your machine:

```bash
cp runner/suite.example.json runner/suite.local.json
```

Relative paths are resolved against the folder of the config file (`runner/`). When the config is loaded, every path is checked to exist, and every family `pattern` must be a valid regex.

| Field | Required | Meaning |
|---|---|---|
| `rustc_toolchain` | no | rustup toolchain for `rustc` (e.g. `nightly-2026-08-19`); omitted uses `rust-toolchain.toml` / rustup's default |
| `java` | no (`"java"`) | Java command; a bare name is looked up on `PATH` |
| `silicon.jar` | yes | Silicon's fat jar (`sbt assembly` in the Silicon repo) |
| `silicon.jvm_args` | no (`[]`) | JVM arguments, e.g. `["-Xss128m"]` |
| `silicon.args` | no (`[]`) | extra Silicon arguments for every file |
| `silicon.warm.corpus` | yes | directories whose `.vpr` files warm up each new JVM before measuring, e.g. Silicon's and Silver's `src/test/resources` |
| `silicon.warm.warmup_s` | no (`60`) | seconds of warm-up per JVM |
| `silicon.warm.file_timeout_s` | no (`10`) | Silicon `--timeout` per warm-up file |
| `helium` | yes | Helium's `verify` binary (`verify.exe` on Windows) |
| `repeat_runs` | no (`5`) | measured runs per benchmark (at least 1) |
| `timeout` | no (no limit) | seconds after which a run is killed |
| `suites` | yes | the benchmark suites, see below |

Each suite has a `name` (its folder under `suites/`), an optional `description` and `tags`, and `activated` (default `true`; set it to `false` to skip the suite without removing it). `families` is optional; without it the suite is flat. Each family has a `name` (its subfolder), an optional `description`, and optionally a `pattern`: a regex whose named groups, listed in `knobs`, extract knob values from the file name (e.g. `depth_d(?P<depth>\d+)` for `depth_d3`).
