-- Results schema for the Helium benchmark suites (DuckDB).
--
-- Storage: Parquet files under data/ are the source of truth, one file per
-- benchmark run and table:
--
--   data/rust_metrics/<run_id>.parquet
--   data/viper_metrics/<run_id>.parquet
--   data/helium_runs/<run_id>.parquet
--   data/silicon_runs/<run_id>.parquet
--   data/rustc_runs/<run_id>.parquet
--
-- Whatever writes results creates these tables (e.g. in an in-memory DuckDB,
-- so every row is checked against the types below) and copies them out to
-- Parquet. Grafana reads the Parquet files through views of the same name (see
-- grafana/provisioning/datasources/duckdb.yaml); nothing holds a lock on a
-- shared database file. Parquet has no enum type, so status columns are stored
-- as text there and cast back by those views.


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


-- File metrics: one table per language, one row per measured file content,
-- keyed by that file's sha256. Content keys mean a file that moves between
-- suites/families keeps its metrics, and Rust and Viper metrics stay correct
-- when different .rs files encode to the same .vpr (e.g. the windows_* shim
-- crates and cfg-if all yield just the preamble). Join from the run tables:
--
--   ... JOIN rust_metrics  USING (rs_sha256)
--   ... JOIN viper_metrics USING (vpr_sha256)
--
-- Files are re-measured in every run; `measured_at` / `metrics_commit` tell
-- measurements by a changed metrics extractor apart (the Grafana views keep
-- the latest one per hash). Derived values such as the Viper/Rust size ratio
-- (viper_metrics.loc / rust_metrics.loc) are left to queries.

CREATE TABLE rust_metrics (
    rs_sha256               VARCHAR     NOT NULL,  -- sha256 of the .rs file
    measured_at             TIMESTAMPTZ NOT NULL,  -- when these metrics were computed
    metrics_commit          VARCHAR,               -- harness commit that computed them

    -- whole file
    loc                     INTEGER,
    fns                     INTEGER,
    structs                 INTEGER,
    enums                   INTEGER,

    -- summed over all functions (NULL when the file has none)
    fn_loc                  INTEGER,               -- lines inside function bodies
    paths                   DOUBLE,                -- control-flow paths; exponential, so not an integer type
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

    PRIMARY KEY (rs_sha256, measured_at)
);

CREATE TABLE viper_metrics (
    vpr_sha256              VARCHAR     NOT NULL,  -- sha256 of the .vpr file
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

    PRIMARY KEY (vpr_sha256, measured_at)
);


-- Runs: one table per backend, one row per execution of that backend on one
-- file. Every timed iteration is its own row (warm-up iterations are not
-- recorded); a backend that was not run gets a single SKIPPED row.
--
-- All three tables start with the same common columns, in the same order:
--
--   run_id          VARCHAR      benchmark run, e.g. 2026-10-06_bd5a136f97a5
--   started_at      TIMESTAMPTZ  when the benchmark run started
--   host            VARCHAR      machine it ran on
--   suite           VARCHAR      location of the file as of this run
--   family          VARCHAR      (NULL for suites without a family level)
--   test            VARCHAR      path below the family dir, no extension (e.g. acyclic/dll)
--   vpr_sha256      VARCHAR      join viper_metrics USING (vpr_sha256)
--   rs_sha256       VARCHAR      join rust_metrics USING (rs_sha256); NULL for Viper-only files
--   backend_version VARCHAR      Helium: commit under test; Silicon/rustc: version string
--   iteration       INTEGER      0-based timed iteration
--   status          run_status   outcome, see the enum above
--   timeout_s       DOUBLE       timeout the backend ran under
--   wall_s          DOUBLE       wall-clock time measured by the harness
--   peak_rss_mb     DOUBLE       peak resident memory
--   message         VARCHAR      crash / error output, if any
--   meta            JSON         anything else: harness settings, backend arguments, ...
--
-- Run-level context is repeated on every row; Parquet's dictionary encoding
-- makes that cheap and keeps queries join-free. Scaling knobs are not stored:
-- they follow from family + test via the family's pattern in suites/suite.json.

CREATE TABLE helium_runs (
    run_id              VARCHAR       NOT NULL,
    started_at          TIMESTAMPTZ   NOT NULL,
    host                VARCHAR,
    suite               VARCHAR       NOT NULL,
    family              VARCHAR,
    test                VARCHAR       NOT NULL,
    vpr_sha256          VARCHAR       NOT NULL,
    rs_sha256           VARCHAR,
    backend_version     VARCHAR,
    iteration           INTEGER       NOT NULL,
    status              run_status    NOT NULL,
    timeout_s           DOUBLE,
    wall_s              DOUBLE,
    peak_rss_mb         DOUBLE,
    message             VARCHAR,
    meta                JSON,

    dirty               BOOLEAN,               -- built from uncommitted changes on top of backend_version
    self_s              DOUBLE,                -- total time Helium reports for itself

    -- members (methods) by outcome
    members_ok          INTEGER,
    members_fail        INTEGER,
    members_unsupported INTEGER,
    members_skip        INTEGER,
    members_error       INTEGER,

    -- phases, seconds
    parse_s             DOUBLE,
    macros_s            DOUBLE,
    idents_s            DOUBLE,
    disambiguate_s      DOUBLE,
    globals_s           DOUBLE,
    typecheck_s         DOUBLE,
    analyze_s           DOUBLE,
    translate_s         DOUBLE,
    verify_s            DOUBLE,

    -- verifier time breakdown, seconds
    search_s            DOUBLE,                -- rule search
    apply_s             DOUBLE,                -- rule application
    rebuild_s           DOUBLE,                -- e-graph rebuilding
    ground_s            DOUBLE,                -- graph work by kind: ground graph
    scratch_s           DOUBLE,                --   scratch graphs
    probe_s             DOUBLE,                --   probes
    scratch_clone_s     DOUBLE,                --   cloning scratch graphs

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
    per_rule            JSON,                  -- {"<rule>": applications, ...}
    rule_timing         JSON,                  -- {"<rule>": {"search": s, "apply": s}, ...} (slowest rules)

    UNIQUE (run_id, suite, family, test, iteration)
);

CREATE TABLE silicon_runs (
    run_id              VARCHAR       NOT NULL,
    started_at          TIMESTAMPTZ   NOT NULL,
    host                VARCHAR,
    suite               VARCHAR       NOT NULL,
    family              VARCHAR,
    test                VARCHAR       NOT NULL,
    vpr_sha256          VARCHAR       NOT NULL,
    rs_sha256           VARCHAR,
    backend_version     VARCHAR,
    iteration           INTEGER       NOT NULL,
    status              run_status    NOT NULL,
    timeout_s           DOUBLE,
    wall_s              DOUBLE,
    peak_rss_mb         DOUBLE,
    message             VARCHAR,
    meta                JSON,

    verify_s            DOUBLE,                -- verification time Silicon reports
    cached              BOOLEAN,               -- result reused from the harness cache, not re-run
    error_count         INTEGER,               -- verification errors reported
    failed_members      INTEGER,               -- members with at least one error
    errors              JSON,                  -- [{"member": .., "line": .., "message": ..}, ...]

    UNIQUE (run_id, suite, family, test, iteration)
);

CREATE TABLE rustc_runs (
    run_id              VARCHAR       NOT NULL,
    started_at          TIMESTAMPTZ   NOT NULL,
    host                VARCHAR,
    suite               VARCHAR       NOT NULL,
    family              VARCHAR,
    test                VARCHAR       NOT NULL,
    vpr_sha256          VARCHAR       NOT NULL,
    rs_sha256           VARCHAR,
    backend_version     VARCHAR,
    iteration           INTEGER       NOT NULL,
    status              run_status    NOT NULL,
    timeout_s           DOUBLE,
    wall_s              DOUBLE,                -- `cargo check` wall time
    peak_rss_mb         DOUBLE,
    message             VARCHAR,
    meta                JSON,

    self_s              DOUBLE,                -- total of `-Z time-passes`
    phases              JSON,                  -- {"<pass>": s, ...}; the pass set changes between rustc versions

    UNIQUE (run_id, suite, family, test, iteration)
);

