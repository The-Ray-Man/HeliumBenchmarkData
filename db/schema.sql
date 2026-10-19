-- Results schema for the Helium benchmark suites (PostgreSQL).
--
-- Applied by the postgres container on first start (mounted into
-- /docker-entrypoint-initdb.d, see compose.yaml). The Rust structs in
-- runner/src/db/entities are generated from the live database with sea-orm-cli,
-- so regenerate them after changing this file (see README).


-- Outcome of one backend on one file. When members (methods) disagree, the
-- first matching value in this order wins:
--   CRASHED      the backend process failed (non-zero exit, panic, OOM)
--   TIMED_OUT    the timeout was hit
--   FAILED       at least one member reported a verification error
--                (rustc: the file did not compile)
--   INCONCLUSIVE no failure, but some member could not be decided
--                (unsupported feature, skipped, internal error)
--   VERIFIED     every member verified (rustc: the file compiled)
--   SKIPPED      the backend was not run on this file (e.g. size limit)
CREATE TYPE run_status AS ENUM
    ('VERIFIED', 'FAILED', 'INCONCLUSIVE', 'CRASHED', 'TIMED_OUT', 'SKIPPED');


-- File metrics: one table per language, one row per file content, keyed by
-- that file's sha256. Content keys mean a file that moves between
-- suites/families keeps its metrics, and Rust and Viper metrics stay correct
-- when different .rs files encode to the same .vpr (e.g. the windows_* shim
-- crates and cfg-if all yield just the preamble). A file is measured once;
-- `measured_at` / `metrics_commit` record when and by which harness. Join from
-- the run tables:
--
--   ... JOIN rust_metrics  USING (rs_sha256)
--   ... JOIN viper_metrics USING (vpr_sha256)
--
-- A .rs file points to the .vpr Prusti encoded it to (rust_metrics.vpr_sha256),
-- so viper_metrics must hold that file first. Several .rs files can share one
-- .vpr, so the link is stored on the Rust side only; from a Viper file:
--
--   viper_metrics v JOIN rust_metrics r USING (vpr_sha256)
--
-- Derived values such as the Viper/Rust size ratio
-- (viper_metrics.loc / rust_metrics.loc) are left to queries.

CREATE TABLE viper_metrics (
    vpr_sha256              VARCHAR     NOT NULL,  -- sha256 of the .vpr file; its .rs files: rust_metrics.vpr_sha256
    measured_at             TIMESTAMPTZ NOT NULL,
    metrics_commit          VARCHAR,

    -- whole file
    loc                     INTEGER,               -- lines of code (`lines` counts every line)
    lines                   INTEGER,
    bytes                   BIGINT,
    methods                 INTEGER,
    functions               INTEGER,
    predicates              INTEGER,
    domains                 INTEGER,
    domain_functions        INTEGER,
    axioms                  INTEGER,
    fields                  INTEGER,
    adts                    INTEGER,

    -- summed over all members
    stmts                   INTEGER,
    exps                    INTEGER,
    folds                   INTEGER,
    unfolds                 INTEGER,
    heap_update_exps        INTEGER,
    inhales                 INTEGER,
    exhales                 INTEGER,
    asserts                 INTEGER,
    assumes                 INTEGER,
    accs                    INTEGER,
    quantifiers             INTEGER,
    labels                  INTEGER,
    gotos                   INTEGER,
    ifs                     INTEGER,
    whiles                  INTEGER,
    assigns                 INTEGER,
    method_calls            INTEGER,
    function_apps           INTEGER,
    old_exps                INTEGER,
    contract_clauses        INTEGER,

    -- added by db/migrations/001_viper_statement_kinds.sql (kept last so a
    -- fresh and a migrated database have the same column order)
    refutes                 INTEGER,               -- `asserts` counts only `assert`
    var_decls               INTEGER,               -- `var` statements, initialised or not
    packages                INTEGER,               -- `package` statements
    applies                 INTEGER,               -- `apply` statements
    other_stmts             INTEGER,               -- quasihavoc, fresh, constraining, local define, ...
    magic_wands             INTEGER,               -- `A --* B` expressions

    PRIMARY KEY (vpr_sha256)
);

CREATE TABLE rust_metrics (
    rs_sha256               VARCHAR     NOT NULL,  -- sha256 of the .rs file
    vpr_sha256              VARCHAR     REFERENCES viper_metrics,  -- its Prusti encoding; NULL if none
    measured_at             TIMESTAMPTZ NOT NULL,  -- when these metrics were computed
    metrics_commit          VARCHAR,               -- harness commit that computed them

    -- whole file
    loc                     INTEGER,
    fns                     INTEGER,
    structs                 INTEGER,
    enums                   INTEGER,

    -- summed over all functions (NULL when the file has none)
    fn_loc                  INTEGER,               -- lines inside function bodies
    paths                   DOUBLE PRECISION,      -- control-flow paths; exponential, so not an integer type
    stmts                   INTEGER,
    exprs                   INTEGER,
    loops                   INTEGER,
    ifs                     INTEGER,
    early_returns           INTEGER,
    matches                 INTEGER,
    match_arms              INTEGER,
    args                    INTEGER,
    args_by_value           INTEGER,
    args_by_ref             INTEGER,
    args_by_mut_ref         INTEGER,
    arg_fields              INTEGER,
    ret_fields              INTEGER,
    calls                   INTEGER,
    distinct_callees        INTEGER,
    calls_in_loops          INTEGER,
    calls_in_branches       INTEGER,
    assigns                 INTEGER,
    compound_assigns        INTEGER,
    mut_ref_writes          INTEGER,
    reborrows               INTEGER,
    struct_borrows          INTEGER,
    arith_ops               INTEGER,
    divisions               INTEGER,
    indexing                INTEGER,
    casts                   INTEGER,
    unwraps                 INTEGER,
    panics                  INTEGER,
    enums_touched           INTEGER,
    generic_insts           INTEGER,
    cyclomatic              INTEGER,

    -- maxima over all functions
    max_loop_depth          INTEGER,
    max_if_depth            INTEGER,
    max_seq_branches        INTEGER,
    max_branch_depth        INTEGER,
    max_match_arms          INTEGER,
    max_match_depth         INTEGER,
    max_enum_variants       INTEGER,
    max_payload_depth       INTEGER,
    max_call_depth          INTEGER,
    max_write_path          INTEGER,
    max_struct_depth        INTEGER,

    PRIMARY KEY (rs_sha256)
);


-- Benchmarks: one row per test of a suite (suite, family, test, as in the run
-- tables) with the hashes of its current files, so metrics can be grouped by
-- suite or family without any runs:
--
--   benchmarks b JOIN viper_metrics v USING (vpr_sha256) WHERE b.suite = ...
--
-- Rewritten when the runner records a benchmark's metrics; `recorded_at` says
-- when. `id` is a surrogate key for the same reason as in the run tables.

CREATE TABLE benchmarks (
    id                  BIGINT        GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    suite               VARCHAR       NOT NULL,
    family              VARCHAR,                   -- NULL for suites without a family level
    test                VARCHAR       NOT NULL,    -- path below the family dir, no extension
    vpr_sha256          VARCHAR       REFERENCES viper_metrics,  -- NULL if the test has no .vpr
    rs_sha256           VARCHAR       REFERENCES rust_metrics,   -- NULL if the test has no .rs
    recorded_at         TIMESTAMPTZ   NOT NULL,

    UNIQUE NULLS NOT DISTINCT (suite, family, test)
);


-- Runs: one table per backend, one row per execution of that backend on one
-- file. Every timed iteration is its own row (warm-up iterations are not
-- recorded); a backend that was not run gets a single SKIPPED row.
--
-- All three tables start with the same common columns, in the same order.
-- After `test` come the hashes of the files the backend ran on, each a key
-- into its metrics table: helium_runs has vpr_sha256 and rs_sha256 (NULL for
-- Viper-only files), silicon_runs vpr_sha256, rustc_runs rs_sha256.
--
--   run_id          VARCHAR          benchmark run, e.g. 2026-10-06_bd5a136f97a5
--   started_at      TIMESTAMPTZ      when the benchmark run started
--   host            VARCHAR          machine it ran on
--   suite           VARCHAR          location of the file as of this run
--   family          VARCHAR          (NULL for suites without a family level)
--   test            VARCHAR          path below the family dir, no extension (e.g. acyclic/dll)
--   backend_version VARCHAR          Helium: commit under test; Silicon/rustc: version string
--   iteration       INTEGER          0-based timed iteration
--   status          run_status       outcome, see the enum above
--   timeout_s       DOUBLE PRECISION timeout the backend ran under
--   wall_s          DOUBLE PRECISION wall-clock time measured by the harness
--   peak_rss_mb     DOUBLE PRECISION peak resident memory
--   message         VARCHAR          crash / error output, if any
--   meta            JSONB            anything else: harness settings, backend arguments, ...
--
-- Run-level context is repeated on every row, which keeps queries join-free.
-- Scaling knobs are not stored: they follow from family + test via the
-- family's pattern in suites/suite.json.
--
-- `id` is a surrogate key (SeaORM entities need a primary key, and `family`
-- is nullable so the natural key cannot be one); the natural key is the
-- UNIQUE constraint, with NULL families comparing equal.

CREATE TABLE helium_runs (
    id                  BIGINT        GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    run_id              VARCHAR       NOT NULL,
    started_at          TIMESTAMPTZ   NOT NULL,
    host                VARCHAR,
    suite               VARCHAR       NOT NULL,
    family              VARCHAR,
    test                VARCHAR       NOT NULL,
    vpr_sha256          VARCHAR       NOT NULL REFERENCES viper_metrics,
    rs_sha256           VARCHAR       REFERENCES rust_metrics,
    backend_version     VARCHAR,
    iteration           INTEGER       NOT NULL,
    status              run_status    NOT NULL,
    timeout_s           DOUBLE PRECISION,
    wall_s              DOUBLE PRECISION,
    peak_rss_mb         DOUBLE PRECISION,
    message             VARCHAR,
    meta                JSONB,

    dirty               BOOLEAN,                   -- built from uncommitted changes on top of backend_version
    self_s              DOUBLE PRECISION,          -- total time Helium reports for itself

    -- members (methods) by outcome
    members_ok          INTEGER,
    members_fail        INTEGER,
    members_unsupported INTEGER,
    members_skip        INTEGER,
    members_error       INTEGER,

    -- phases, seconds
    parse_s             DOUBLE PRECISION,
    macros_s            DOUBLE PRECISION,
    idents_s            DOUBLE PRECISION,
    disambiguate_s      DOUBLE PRECISION,
    globals_s           DOUBLE PRECISION,
    typecheck_s         DOUBLE PRECISION,
    analyze_s           DOUBLE PRECISION,
    translate_s         DOUBLE PRECISION,
    verify_s            DOUBLE PRECISION,

    -- verifier time breakdown, seconds
    search_s            DOUBLE PRECISION,          -- rule search
    apply_s             DOUBLE PRECISION,          -- rule application
    rebuild_s           DOUBLE PRECISION,          -- e-graph rebuilding
    ground_s            DOUBLE PRECISION,          -- graph work by kind: ground graph
    scratch_s           DOUBLE PRECISION,          --   scratch graphs
    probe_s             DOUBLE PRECISION,          --   probes
    scratch_clone_s     DOUBLE PRECISION,          --   cloning scratch graphs

    -- verifier counters
    saturations                 BIGINT,
    sat_iterations              BIGINT,
    reduces                     BIGINT,
    rule_applications           BIGINT,
    insts_processed             BIGINT,
    egraph_nodes_peak           BIGINT,
    egraph_classes_peak         BIGINT,
    probe_saturations           BIGINT,
    probe_iterations            BIGINT,
    probe_reduces               BIGINT,
    probe_early_stops           BIGINT,
    block_scratch_clones        BIGINT,
    block_scratch_saturations   BIGINT,
    block_scratch_iterations    BIGINT,
    block_scratch_freehits      BIGINT,
    prove_calls                 BIGINT,
    prove_goal_true             BIGINT,
    prove_inconsistent          BIGINT,
    prove_dead_block            BIGINT,
    prove_memo                  BIGINT,
    prove_saturate              BIGINT,
    prove_probe                 BIGINT,
    prove_ite_decompose         BIGINT,

    -- keyed by rule name, so the key set changes with the rule set
    per_rule            JSONB,                     -- {"<rule>": applications, ...}
    rule_timing         JSONB,                     -- {"<rule>": {"search": s, "apply": s}, ...} (slowest rules)

    UNIQUE NULLS NOT DISTINCT (run_id, suite, family, test, iteration)
);

CREATE TABLE silicon_runs (
    id                  BIGINT        GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    run_id              VARCHAR       NOT NULL,
    started_at          TIMESTAMPTZ   NOT NULL,
    host                VARCHAR,
    suite               VARCHAR       NOT NULL,
    family              VARCHAR,
    test                VARCHAR       NOT NULL,
    vpr_sha256          VARCHAR       NOT NULL REFERENCES viper_metrics,
    backend_version     VARCHAR,
    iteration           INTEGER       NOT NULL,
    status              run_status    NOT NULL,
    timeout_s           DOUBLE PRECISION,
    wall_s              DOUBLE PRECISION,
    peak_rss_mb         DOUBLE PRECISION,
    message             VARCHAR,
    meta                JSONB,

    verify_s            DOUBLE PRECISION,          -- verification time Silicon reports
    cached              BOOLEAN,                   -- result reused from the harness cache, not re-run
    error_count         INTEGER,                   -- verification errors reported
    failed_members      INTEGER,                   -- members with at least one error
    errors              JSONB,                     -- [{"member": .., "line": .., "message": ..}, ...]

    UNIQUE NULLS NOT DISTINCT (run_id, suite, family, test, iteration)
);

CREATE TABLE rustc_runs (
    id                  BIGINT        GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    run_id              VARCHAR       NOT NULL,
    started_at          TIMESTAMPTZ   NOT NULL,
    host                VARCHAR,
    suite               VARCHAR       NOT NULL,
    family              VARCHAR,
    test                VARCHAR       NOT NULL,
    rs_sha256           VARCHAR       NOT NULL REFERENCES rust_metrics,
    backend_version     VARCHAR,
    iteration           INTEGER       NOT NULL,
    status              run_status    NOT NULL,
    timeout_s           DOUBLE PRECISION,
    wall_s              DOUBLE PRECISION,          -- `cargo check` wall time
    peak_rss_mb         DOUBLE PRECISION,
    message             VARCHAR,
    meta                JSONB,

    self_s              DOUBLE PRECISION,          -- total of `-Z time-passes`
    phases              JSONB,                     -- {"<pass>": s, ...}; the pass set changes between rustc versions

    UNIQUE NULLS NOT DISTINCT (run_id, suite, family, test, iteration)
);


-- CI bookkeeping (added by db/migrations/002_ci_runs.sql).
--
-- A run is CI-owned when its run_id is in bench_runs. Garbage collection only
-- ever deletes CI-owned rows, so local and manual runs are never touched.
-- bench_targets says why a Helium commit's CI-owned rows are kept: one row per
-- (commit, reason). A commit's CI-owned rows are kept while it has at least one
-- target; each PR has at most one target (its current head); a target is
-- complete when its commit has a COMPLETE bench_runs row.
--
-- `id` in bench_targets is a surrogate key for the same reason as in the run
-- tables (pr_number is nullable).

CREATE TYPE bench_run_status AS ENUM ('RUNNING', 'COMPLETE', 'FAILED', 'CANCELLED');

CREATE TABLE bench_runs (
    run_id              VARCHAR          PRIMARY KEY,  -- same value as in the *_runs tables
    helium_commit       VARCHAR          NOT NULL,     -- = helium_runs.backend_version
    host                VARCHAR,
    github_run_url      VARCHAR,
    status              bench_run_status NOT NULL,
    started_at          TIMESTAMPTZ      NOT NULL,
    finished_at         TIMESTAMPTZ
);

CREATE TABLE bench_targets (
    id                  BIGINT        GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    helium_commit       VARCHAR       NOT NULL,
    pr_number           INTEGER,                   -- NULL: a main commit
    base_commit         VARCHAR,                   -- PRs: merge-base with main; main: first parent
    main_position       INTEGER,                   -- main: index from BENCH_START_COMMIT, for ordering
    updated_at          TIMESTAMPTZ   NOT NULL,

    UNIQUE NULLS NOT DISTINCT (helium_commit, pr_number)
);

