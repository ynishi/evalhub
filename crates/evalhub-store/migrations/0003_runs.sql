-- Runs as rows of an Eval record. See crates/evalhub-store/src/runs.rs for
-- the write path and crates/evalhub-store/src/lib.rs for the table list.
-- Forward-only. A mistake here is corrected by a later migration.
--
-- An Eval record is a header (its `versions`, as before) plus a set of
-- runs. A run is not a version: it is written, overwritten, archived and
-- deleted on its own, keyed by `(record_id, run_id)`, and every write to it
-- locks the `records` row first (the same `FOR UPDATE` the header ingest
-- takes), so header posts and run writes to one Eval are serialised and
-- `records.runs_hash` always agrees with the rows.

-- ---------------------------------------------------------------------------
-- runs
-- ---------------------------------------------------------------------------

-- One row per run of an Eval. `body` is the run as stored (facets the run
-- omitted are materialised from the header at write time) and
-- `content_hash` is sha256(JCS({ ...body, "run_id": run_id })), see
-- `evalhub_core::run`. `status`, `error_kind`, `started_at` and `ended_at`
-- are copied out of the body for filtering and sorting.
--
-- Archive (`archived_at`) hides a run from non-members and from the
-- projection; the row and its content stay. Delete is a tombstone, as for a
-- version: `body` is nulled, the reason and note are recorded, and
-- `content_hash` stays, so `runs_hash` and every Card that used the run
-- still have the digest. A tombstoned `run_id` is never written again.
CREATE TABLE runs (
    record_id        uuid NOT NULL REFERENCES records (id),
    run_id           text NOT NULL,
    status           text NOT NULL CHECK (status IN ('ok', 'error', 'skipped')),
    error_kind       text,
    started_at       timestamptz,
    ended_at         timestamptz,
    body             jsonb,                  -- NULL once tombstoned
    content_hash     bytea NOT NULL,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    archived_at      timestamptz,
    tombstoned_at    timestamptz,
    tombstone_reason text CHECK (tombstone_reason IN ('withdrawn', 'duplicate', 'takedown', 'other')),
    tombstone_note   text,
    PRIMARY KEY (record_id, run_id),
    CHECK ((tombstoned_at IS NULL) = (body IS NOT NULL)),
    CHECK ((status = 'error') = (error_kind IS NOT NULL))
);

-- A run's `metrics` object, one row per key. Kept on tombstone, like a
-- tombstoned version's `results`.
CREATE TABLE run_metrics (
    record_id uuid NOT NULL, run_id text NOT NULL, metric text NOT NULL,
    value double precision NOT NULL,
    PRIMARY KEY (record_id, run_id, metric),
    FOREIGN KEY (record_id, run_id) REFERENCES runs
);
CREATE INDEX run_metrics_metric_value ON run_metrics (metric, value);

-- A run's `attachments[]`: the run's share of an object's reference count.
-- `attachment_refs` cannot hold these because its key is a `version_id`
-- and a run has none; the GC and the download check count both tables.
-- Deleted on tombstone, so the GC may collect the object.
CREATE TABLE run_attachment_refs (
    record_id uuid NOT NULL, run_id text NOT NULL,
    sha256 bytea NOT NULL REFERENCES attachments (sha256), path text NOT NULL,
    PRIMARY KEY (record_id, run_id, path),
    FOREIGN KEY (record_id, run_id) REFERENCES runs
);
CREATE INDEX run_attachment_refs_sha ON run_attachment_refs (sha256);

-- Per-facet fingerprints of a run, after materialisation: the same rule and
-- shape as `fingerprints` for a version, over an Eval's six facets.
CREATE TABLE run_fingerprints (
    record_id uuid NOT NULL, run_id text NOT NULL,
    facet text NOT NULL CHECK (facet IN ('model', 'task', 'harness', 'generation', 'trial', 'env')),
    fingerprint bytea NOT NULL,
    PRIMARY KEY (record_id, run_id, facet),
    FOREIGN KEY (record_id, run_id) REFERENCES runs
);
CREATE INDEX run_fingerprints_lookup ON run_fingerprints (facet, fingerprint);

-- ---------------------------------------------------------------------------
-- the Card side (written by the Card ingest)
-- ---------------------------------------------------------------------------

-- A Card version's `run_results[]`, one row per element, as `results` is
-- for `results[]`.
CREATE TABLE run_results (                   -- a Card version's run_results[]
    version_id uuid NOT NULL REFERENCES versions (version_id),
    ordinal integer NOT NULL,
    eval_record_id uuid NOT NULL, run_id text NOT NULL, metric text NOT NULL,
    value double precision, label text, by jsonb,
    PRIMARY KEY (version_id, ordinal),
    FOREIGN KEY (eval_record_id, run_id) REFERENCES runs
);
CREATE INDEX run_results_run ON run_results (eval_record_id, run_id, metric);

-- The runs of each Eval a Card version used, with each run's content hash
-- at posting time. Compared with `runs.content_hash` it says which runs
-- were overwritten since the Card judged them.
CREATE TABLE card_eval_runs (                -- the used set at posting time
    card_version_id uuid NOT NULL REFERENCES versions (version_id),
    eval_record_id uuid NOT NULL, run_id text NOT NULL,
    content_hash bytea NOT NULL,
    PRIMARY KEY (card_version_id, eval_record_id, run_id),
    FOREIGN KEY (eval_record_id, run_id) REFERENCES runs
);

-- ---------------------------------------------------------------------------
-- bookkeeping
-- ---------------------------------------------------------------------------

-- Data steps that must run once after the SQL migrations (they need Rust:
-- hashes are computed by `evalhub_core`). A row means the step is done.
CREATE TABLE data_migrations (name text PRIMARY KEY, applied_at timestamptz NOT NULL DEFAULT now());

-- The digest over every run of an Eval, archived and tombstoned included:
-- sha256(JCS([[run_id, hex(content_hash)], ...])) by run_id ascending
-- (`evalhub_core::run::runs_hash`). Recomputed in the transaction of every
-- run write that adds or overwrites a run. NULL until the first run is
-- written (and always NULL on a Card).
ALTER TABLE records ADD COLUMN runs_hash bytea;
