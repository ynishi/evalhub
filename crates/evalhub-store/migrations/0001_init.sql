-- evalhub initial schema. See crates/evalhub-store/src/lib.rs for the design.
-- Forward-only. A mistake here is corrected by a later migration.

-- ---------------------------------------------------------------------------
-- identity
-- ---------------------------------------------------------------------------

CREATE TABLE users (
    user_id     uuid PRIMARY KEY,
    login       text NOT NULL UNIQUE,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE namespaces (
    ns          text PRIMARY KEY,
    kind        text NOT NULL CHECK (kind IN ('user', 'org')),
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE orgs (
    ns          text PRIMARY KEY REFERENCES namespaces (ns),
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE org_members (
    ns          text NOT NULL REFERENCES orgs (ns),
    user_id     uuid NOT NULL REFERENCES users (user_id),
    role        text NOT NULL CHECK (role IN ('read', 'write', 'admin')),
    PRIMARY KEY (ns, user_id)
);

CREATE TABLE tokens (
    token_id    uuid PRIMARY KEY,
    user_id     uuid NOT NULL REFERENCES users (user_id),
    token_hash  bytea NOT NULL UNIQUE,          -- sha256 of the secret; the secret is never stored
    scope       text NOT NULL CHECK (scope IN ('read', 'write', 'admin')),
    namespaces  text[] NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    revoked_at  timestamptz
);

-- ---------------------------------------------------------------------------
-- records and versions
-- ---------------------------------------------------------------------------

CREATE TABLE records (
    id          uuid PRIMARY KEY,               -- ULID, stored as uuid
    type        text NOT NULL CHECK (type IN ('card', 'eval')),
    ns          text NOT NULL REFERENCES namespaces (ns),
    name        text NOT NULL,
    visibility  text NOT NULL DEFAULT 'private' CHECK (visibility IN ('private', 'public')),
    created_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (type, ns, name)
);

-- Typed cast helpers. IMMUTABLE so they can be used in expression indexes.
-- They return NULL rather than raising when the JSON value has the wrong
-- type, so an old row cannot make an index build fail.
CREATE FUNCTION evalhub_num(j jsonb) RETURNS double precision
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE AS $$
    SELECT CASE WHEN jsonb_typeof(j) = 'number' THEN (j #>> '{}')::double precision END
$$;

CREATE FUNCTION evalhub_str(j jsonb) RETURNS text
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE AS $$
    SELECT CASE WHEN jsonb_typeof(j) = 'string' THEN j #>> '{}' END
$$;

CREATE FUNCTION evalhub_bool(j jsonb) RETURNS boolean
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE AS $$
    SELECT CASE WHEN jsonb_typeof(j) = 'boolean' THEN (j #>> '{}')::boolean END
$$;

CREATE TABLE versions (
    version_id        uuid PRIMARY KEY,         -- ULID, stored as uuid
    record_id         uuid NOT NULL REFERENCES records (id),
    seq               integer NOT NULL CHECK (seq >= 1),
    label             text,
    content_hash      bytea NOT NULL,           -- sha256 of canonical body
    body              jsonb,                    -- canonical record; NULL once tombstoned
    changed           text[] NOT NULL DEFAULT '{}',
    badges            text[] NOT NULL DEFAULT '{}',
    created_at        timestamptz NOT NULL DEFAULT now(),
    tombstoned_at     timestamptz,
    tombstone_reason  text CHECK (tombstone_reason IN ('withdrawn', 'duplicate', 'takedown', 'other')),
    tombstone_note    text,

    -- core facet keys as generated columns; the query compiler references
    -- these by name. Extend by migration when the schema gains a core key.
    title             text GENERATED ALWAYS AS (body #>> '{title}') STORED,
    producer_name     text GENERATED ALWAYS AS (body #>> '{producer,name}') STORED,
    model_id          text GENERATED ALWAYS AS (body #>> '{model,id}') STORED,
    model_provider    text GENERATED ALWAYS AS (body #>> '{model,provider}') STORED,
    model_revision    text GENERATED ALWAYS AS (body #>> '{model,revision}') STORED,
    task_id           text GENERATED ALWAYS AS (body #>> '{task,id}') STORED,
    task_version      text GENERATED ALWAYS AS (body #>> '{task,version}') STORED,
    task_split        text GENERATED ALWAYS AS (body #>> '{task,split}') STORED,
    harness_name      text GENERATED ALWAYS AS (body #>> '{harness,name}') STORED,
    harness_version   text GENERATED ALWAYS AS (body #>> '{harness,version}') STORED,
    gen_temperature   double precision GENERATED ALWAYS AS (evalhub_num(body #> '{generation,temperature}')) STORED,
    gen_top_p         double precision GENERATED ALWAYS AS (evalhub_num(body #> '{generation,top_p}')) STORED,
    gen_max_tokens    double precision GENERATED ALWAYS AS (evalhub_num(body #> '{generation,max_tokens}')) STORED,
    trial_k           double precision GENERATED ALWAYS AS (evalhub_num(body #> '{trial,k}')) STORED,
    env_git_commit    text GENERATED ALWAYS AS (body #>> '{env,git,commit}') STORED,
    env_git_dirty     boolean GENERATED ALWAYS AS (evalhub_bool(body #> '{env,git,dirty}')) STORED,

    UNIQUE (record_id, seq),
    UNIQUE (record_id, label),
    CHECK (label IS NULL OR label !~ '^[0-9]+$'),          -- a label is never purely numeric
    CHECK ((tombstoned_at IS NULL) = (body IS NOT NULL))    -- body present iff live
);

CREATE INDEX versions_record_seq        ON versions (record_id, seq DESC);
CREATE INDEX versions_model_id          ON versions (model_id)        WHERE tombstoned_at IS NULL;
CREATE INDEX versions_task_id           ON versions (task_id)         WHERE tombstoned_at IS NULL;
CREATE INDEX versions_harness           ON versions (harness_name, harness_version) WHERE tombstoned_at IS NULL;
CREATE INDEX versions_gen_temperature   ON versions (gen_temperature) WHERE tombstoned_at IS NULL;
CREATE INDEX versions_created_at        ON versions (created_at DESC);
-- one GIN index serves eq / exists on any unregistered ext path
CREATE INDEX versions_body_gin          ON versions USING gin (body jsonb_path_ops) WHERE tombstoned_at IS NULL;

CREATE TABLE fingerprints (
    version_id        uuid NOT NULL REFERENCES versions (version_id),
    facet             text NOT NULL CHECK (facet IN ('model', 'task', 'harness', 'generation', 'trial', 'grading', 'env')),
    fingerprint       bytea NOT NULL,
    registry_version  text,                     -- ext_schema version folded in, if any
    PRIMARY KEY (version_id, facet)
);
CREATE INDEX fingerprints_lookup ON fingerprints (facet, fingerprint);

CREATE TABLE results (
    version_id    uuid NOT NULL REFERENCES versions (version_id),
    ordinal       integer NOT NULL,
    metric        text NOT NULL,
    aggregation   text NOT NULL,
    value         double precision NOT NULL,
    n             integer,
    by            jsonb,
    PRIMARY KEY (version_id, ordinal)
);
CREATE INDEX results_metric_value ON results (metric, value);

CREATE TABLE relations (
    from_version_id   uuid NOT NULL REFERENCES versions (version_id),
    type              text NOT NULL,
    to_version_id     uuid REFERENCES versions (version_id),
    to_external       text,
    attrs             jsonb,
    CHECK (to_version_id IS NOT NULL OR to_external IS NOT NULL)
);
CREATE INDEX relations_from ON relations (from_version_id, type);
CREATE INDEX relations_to   ON relations (to_version_id, type);

-- ---------------------------------------------------------------------------
-- attachments
-- ---------------------------------------------------------------------------

CREATE TABLE attachments (
    sha256      bytea PRIMARY KEY,
    size        bigint NOT NULL,
    media_type  text,
    state       text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'ready')),
    hashed_by_hub boolean NOT NULL DEFAULT false,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE attachment_refs (
    version_id  uuid NOT NULL REFERENCES versions (version_id),
    sha256      bytea NOT NULL REFERENCES attachments (sha256),
    path        text NOT NULL,
    PRIMARY KEY (version_id, path)
);
CREATE INDEX attachment_refs_sha ON attachment_refs (sha256);

-- ---------------------------------------------------------------------------
-- registry and audit
-- ---------------------------------------------------------------------------

CREATE TABLE registry (
    kind        text NOT NULL CHECK (kind IN ('metrics', 'harnesses', 'relation_types', 'ext_schemas')),
    ns          text NOT NULL,
    id          text NOT NULL,
    version     text NOT NULL,
    body        jsonb NOT NULL,
    state       text NOT NULL DEFAULT 'applied' CHECK (state IN ('applying', 'applied')),
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (kind, ns, id, version)
);

CREATE TABLE audit (
    id              bigserial PRIMARY KEY,
    at              timestamptz NOT NULL DEFAULT now(),
    actor_user_id   uuid,
    actor_token_id  uuid,
    ns              text,
    action          text NOT NULL,
    subject         text,
    detail          jsonb
);
CREATE INDEX audit_ns_at ON audit (ns, at DESC);
