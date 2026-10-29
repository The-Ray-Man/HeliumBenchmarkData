# HeliumBenchmarkData

Benchmark suites (`suites`) for multiple backends, plus a local Postgres + Grafana setup for storing results and building dashboards.

## Layout

- `suites/<suite>/<family>/<test>.vpr` – benchmark programs.
- `generation/` – generators for the `scaling` suite, and `sample_split.py`, which builds `cratesTop100Split2000` (a seeded random sample of 2000 `cratesTop100Split` functions).
- `db/schema.sql` – results schema: `rust_metrics` and `viper_metrics` (metrics per file, keyed by the file's sha256), and `helium_runs`, `silicon_runs`, `rustc_runs` (one row per execution of that backend on a file, with backend-specific metric columns).
- `runner/` – the benchmark runner (a single Rust crate). `runner/suite.example.json` describes every suite and family and how to run them (see [Suite configuration](#suite-configuration)).
  - `runner/src/db/` – database access: the SeaORM entities for `db/schema.sql` (`entities/`, generated), connecting (`connection.rs`, reads `DATABASE_URL`, defaulting to the compose Postgres), and metric lookups (`queries.rs`).
  - `runner/src/file_analyzer/` – static metrics of `.rs` and `.vpr` files.
  - `runner/src/metrics.rs` – stores those metrics in the database.
- `grafana/` – Grafana provisioning (Postgres data source) and dashboards (`grafana/dashboards/*.json`).
  - `helium-stats.json` – one base Helium commit against several heads: coverage and soundness, speedup by phase (including the Z3 timings in `helium_runs.meta`) and between every pair, verdicts per file. Branches and pull requests come live from GitHub through the Infinity plugin (`grafana/provisioning/datasources/github.yaml`; unauthenticated GitHub allows 60 requests per hour, the file says how to add a token). File names link to this repository on GitHub.
  - `helium-file.json` – one benchmark file: Helium's verdict per method, function and predicate next to Silicon's, with the code coloured by where they differ. Needs `db/migrations/003_member_results.sql`: Helium's per-member results (`helium_runs.members`) exist only for runs recorded after it, and the source (`viper_sources`) for files `runner index` / `bench` have stored. Opened from the "details" links in Helium Stats' file tables.
  - `scaling.json` – Helium, Silicon and rustc on one scaling family over one knob (absolute, relative to each backend's smallest value, and the best-fitting linear / polynomial / exponential curve). Knob values are the numbers in the test name; their names live in the hidden `knobmap` variable, copied from the scaling families' `knobs` in `runner/suite.example.json`, so update it when a family is added.

## Quick start

```bash
docker compose up -d
```

- Grafana: http://localhost:3000 (admin / admin unless `GRAFANA_ADMIN_USER` / `GRAFANA_ADMIN_PASSWORD` are set)
- Postgres: `postgres://helium:helium@localhost:5432/helium_bench` (user / password from `POSTGRES_USER` / `POSTGRES_PASSWORD`)
- pgAdmin: http://localhost:5050 (admin@example.com / admin unless `PGADMIN_EMAIL` / `PGADMIN_PASSWORD` are set); the `helium-bench` server is preconfigured, it asks for the Postgres password on first connect. From another machine, tunnel it: `ssh -L 5050:localhost:5050 <bench-host>`.

`db/schema.sql` is applied when the Postgres volume is first created. After changing the schema, recreate the database (this deletes all results) and regenerate the entities:

```bash
docker compose down -v && docker compose up -d
cd runner
sea-orm-cli generate entity -u postgres://helium:helium@localhost:5432/helium_bench -o src/db/entities --with-copy-enums
```

To keep existing results, change an existing database with a script in `db/migrations/` instead (each also updates `db/schema.sql`, so a fresh database ends up the same). The scripts are idempotent; apply them in order:

```bash
docker compose exec -T postgres psql -v ON_ERROR_STOP=1 -U helium -d helium_bench < db/migrations/001_viper_statement_kinds.sql
```

A migration that adds `viper_metrics` / `rust_metrics` columns needs no backfill: the runner re-measures every file whose row has a NULL column on its next pass.

The CLI's major version must match the `sea-orm` dependency (`cargo install sea-orm-cli@2.0.4`). Generation overwrites only `src/db/entities/`; the rest of the `db` module is hand-written and re-exports the entities, so both `db::rustc_runs` and `db::entities::rustc_runs` work.

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

Relative paths are resolved against the folder of the config file (`runner/`). The config is the only source of settings: there are no defaults for required fields and no overrides. When it is loaded, before anything runs, every required field must be present, every path must exist (Helium's `target_dir` is created by cargo), and every family `pattern` must be a valid regex.

| Field | Required | Meaning |
|---|---|---|
| `rustc_toolchain` | no | rustup toolchain for `rustc` and for rebuilding Helium (e.g. `nightly-2026-08-19`); omitted uses `rust-toolchain.toml` / rustup's default |
| `java` | yes | Java command; a bare name is looked up on `PATH` |
| `silicon.jar` | yes | Silicon's fat jar (`sbt assembly` in the Silicon repo) |
| `silicon.jvm_args` | yes | JVM arguments, e.g. `["-Xss128m"]` |
| `silicon.args` | yes | extra Silicon arguments for every file (`[]` for none) |
| `silicon.warm.corpus` | yes | directories whose `.vpr` files warm up each new JVM before measuring, e.g. Silicon's and Silver's `src/test/resources` |
| `silicon.warm.warmup_s` | yes | seconds of warm-up per JVM |
| `silicon.warm.file_timeout_s` | yes | Silicon `--timeout` per warm-up file |
| `helium.crate` | yes | Helium's crate; `verify` is rebuilt there with `cargo build --release` before measuring |
| `helium.start_commit` | yes | first commit on Helium's `main` that CI benchmarks (full hash); `runner sync` keeps every main commit from it on |
| `helium.target_dir` | yes | cargo target dir for that build, e.g. `<crate>/target`, or a folder outside the checkout to keep the build cache. Always passed as `--target-dir`, so a `CARGO_TARGET_DIR` in the environment does not redirect the build |
| `suites_dir` | yes | folder with one subfolder per suite, e.g. `../suites` |
| `repeat_runs` | yes | measured runs per benchmark (at least 1) |
| `warmup_runs` | yes | untimed runs per benchmark before the measured ones (`0` for none) |
| `timeout` | no (no limit) | seconds after which a run is killed |
| `suites` | yes | the benchmark suites, see below |

Each suite has a `name` (its folder under `suites/`), `activated` (`false` skips the suite without removing it), and an optional `description` and `tags`. `families` is optional; without it the suite is flat. Each family has a `name` (its subfolder), an optional `description`, and optionally a `pattern`: a regex whose named groups, listed in `knobs`, extract knob values from the file name (e.g. `depth_d(?P<depth>\d+)` for `depth_d3`).

`runner/suite.ci.json` is the config of the CI benchmark machine: every suite, with that machine's fixed paths (`/opt/bench/...`) and the Helium checkout next to this repository's.

## Command line

`runner` without a subcommand is `runner bench`, so `cargo run` in `runner/` measures with `./suite.local.json`. `--config <file>` uses another config, e.g. `cargo run --release -- bench --config suite.ci.json`.

### Indexing

```bash
runner index [--config <file>]
```

Stores the static metrics (`rust_metrics`, `viper_metrics`) of every benchmark file of the activated suites, like `bench` does before measuring, but sets up and runs no tool. Files whose rows are complete are skipped, so it is also the quick way to fill a column a migration added.

### CI mode

```bash
runner bench --config suite.ci.json --ci --run-id gh-<run id>-<attempt> --helium-commit <full sha> [--github-run-url <url>] [--pr <number>]
```

- Registers `--helium-commit` in `bench_targets` (see below) before anything else, unless it already is: as a head of PR `--pr`, or as a main commit without it. So a run's rows always have a target, also when it fails or is cancelled.
- Checks out `--helium-commit` (fetched from `origin` if missing) in `helium.crate`, with its submodules, and builds it. Refuses when the checkout has local changes to tracked files, and fails unless `verify --version` then reports exactly that commit.
- Records the run in `bench_runs`, which makes its rows CI-owned. The row is inserted as `RUNNING` (a taken `--run-id` fails), and ends as `COMPLETE` once every benchmark was gone through, whatever the verification results; `FAILED` if the runner crashes midway; `CANCELLED` on Ctrl-C / SIGTERM (what GitHub Actions sends on cancel).
- A file is skipped only when CI-owned runs of the same commit already measured it, so a run of the same commit on another machine does not count. A re-run of a cancelled or failed commit measures only what is missing. A timeout ends a file's measurement after one run, so a re-run measures Helium's timed-out files again. Silicon and rustc runs are shared as before: they don't depend on Helium's commit, and a file one of them timed out on (with the same build) is skipped like a measured one, here and outside CI mode.

### Which commits CI keeps

CI keeps runs for every main commit it benchmarked, and for every head of an open PR it benchmarked (a push to a PR adds a head; earlier heads keep their runs). `bench_targets` lists these commits (one row per main commit, one per PR and head with the PR's number); CI-owned Helium runs of any other commit are removed. Everything is driven by the GitHub workflow, nothing is run by hand:

- `bench --ci` on a push to main registers the pushed commit as a main target; on a PR (`--pr <number>`), its head as a target of that PR. Main targets are never removed. Targets registered this way have no `main_position`.
- When a PR is closed (merged or not), `runner close-pr --pr <number>` removes all its targets and the runs of every commit no longer targeted. Merging needs nothing else: with squash and merge the squash commit is a new main commit, which the push to main benchmarks.

```bash
runner close-pr --pr 12
runner sync --config suite.ci.json --pr 12=<head sha> --pr 15=<head sha>
```

- `close-pr` (needs only `DATABASE_URL`) removes a closed PR's targets and its runs.
- `sync` is not part of the workflow. It replaces all targets: it is how runs from before `bench` registered targets get one, and should be run once, with the open PRs, before the first `close-pr` (which would otherwise remove every run without a target). It fetches `main` from `origin` in `helium.crate` (the working tree is not touched) and takes its commits from `helium.start_commit` on, following first parents; the open PRs come from GitHub (`--pr`, one per PR). It sets the targets to exactly these main commits and PR heads, removes the runs of every commit that is no longer one (a PR's previous head, a closed PR), and prints the targeted commits without a `COMPLETE` run, one per line on stdout: main commits oldest first, then PR heads. It fails, changing nothing, if the start commit is not on `origin/main`.
- Removing a run deletes its `helium_runs` rows and its `bench_runs` row. Runs still `RUNNING` are kept until they end, Silicon and rustc runs are kept (they don't depend on Helium's commit), and rows not owned by CI are never touched.

## Benchmark machine

CI benchmarks run on a self-hosted GitHub Actions runner (label `bench`) that runs in a container next to Postgres and Grafana, so jobs see only the image and its mounts, never the host. `ci-runner/Dockerfile` is its image: Ubuntu, the GitHub runner, rustup with Helium's pinned nightly, a JDK, and Z3 for Silicon. These versions are part of what is measured; change them there deliberately. `compose.ci.yaml` adds it to `compose.yaml`. Use a Linux host: on Windows and macOS, Docker runs inside a VM, which adds noise.

One-time setup:

1. A Silicon checkout with its jar built (`sbt assembly`), e.g. in `/srv/silicon`. It is mounted read-only at the paths in `runner/suite.ci.json`.
2. `.env` next to `compose.yaml` (git-ignored):

   ```
   DATABASE_URL=postgres://bench_ci:<a new password>@postgres:5432/helium_bench
   SILICON_DIR=/srv/silicon
   RUNNER_TOKEN=<registration token>
   ```

   The token comes from the Helium repo's Settings > Actions > Runners > New self-hosted runner, and is valid for an hour. It is only needed for the first start, which registers the runner; remove it afterwards. `RUNNER_NAME` (default `bench-1`) and `RUNNER_URL` (default the Helium repo) can be set there too.
3. Start the database, create the role CI connects as (it can read and write rows, but not change tables), then the runner:

   ```bash
   docker compose up -d postgres
   docker compose exec -T postgres psql -v ON_ERROR_STOP=1 -U helium -d helium_bench -v password='<the password in DATABASE_URL>' < db/bench_ci_role.sql
   docker compose -f compose.yaml -f compose.ci.yaml up -d --build
   ```

4. Settings > Actions > Runners shows the runner as Idle.

Volumes keep the registration, the job workspace and the build caches across restarts and image rebuilds. To register again, remove the `runner-home` volume and start with a new `RUNNER_TOKEN`. Register exactly one runner with the label `bench`, so measurements never run in parallel. Uncomment `cpuset` in `compose.ci.yaml` to keep the benchmarks on fixed cores.
