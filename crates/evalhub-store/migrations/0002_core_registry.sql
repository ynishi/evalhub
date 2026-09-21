-- The `core/` vocabulary: the metrics and relation types that ship with
-- the hub, as registry rows.
--
-- These mirror `evalhub_core::registry::{CORE_METRICS, CORE_RELATION_TYPES}`
-- at version `CORE_VERSION`. A migration cannot call Rust, so the rows are
-- written out here and `tests/registry.rs` asserts that the two agree —
-- drift fails the build rather than the hub.
--
-- `core/` entries are read-only over the API (`registry::put` refuses the
-- namespace), so this is the only place they are written.

INSERT INTO namespaces (ns, kind) VALUES ('core', 'org') ON CONFLICT DO NOTHING;
INSERT INTO orgs (ns) VALUES ('core') ON CONFLICT DO NOTHING;

INSERT INTO registry (kind, ns, id, version, body, state) VALUES
    ('metrics', 'core', 'pass_rate', '1',
     '{"id": "core/pass_rate", "lower_is_better": false, "description": "Fraction of items graded as passing."}',
     'applied'),
    ('metrics', 'core', 'accuracy', '1',
     '{"id": "core/accuracy", "lower_is_better": false, "description": "Fraction of items answered correctly."}',
     'applied'),
    ('metrics', 'core', 'f1', '1',
     '{"id": "core/f1", "lower_is_better": false, "description": "Harmonic mean of precision and recall."}',
     'applied'),
    ('metrics', 'core', 'exact_match', '1',
     '{"id": "core/exact_match", "lower_is_better": false, "description": "Fraction of items whose output matched the reference exactly."}',
     'applied'),
    ('metrics', 'core', 'pass_at_k', '1',
     '{"id": "core/pass_at_k", "lower_is_better": false, "description": "Probability that at least one of k samples passes."}',
     'applied'),
    ('metrics', 'core', 'mean_score', '1',
     '{"id": "core/mean_score", "lower_is_better": false, "description": "Mean of a per-item score on the producer''s scale."}',
     'applied'),
    ('metrics', 'core', 'latency_ms', '1',
     '{"id": "core/latency_ms", "lower_is_better": true, "description": "Wall-clock time per item, in milliseconds."}',
     'applied'),
    ('metrics', 'core', 'cost_usd', '1',
     '{"id": "core/cost_usd", "lower_is_better": true, "description": "Cost per item, in US dollars."}',
     'applied');

INSERT INTO registry (kind, ns, id, version, body, state) VALUES
    ('relation_types', 'core', 'uses_eval', '1',
     '{"id": "core/uses_eval", "from": "card", "to": "eval", "inverse": "used_by"}', 'applied'),
    ('relation_types', 'core', 'retry_of', '1',
     '{"id": "core/retry_of", "from": "card", "to": "card", "inverse": "retried_by"}', 'applied'),
    ('relation_types', 'core', 'rerun_of', '1',
     '{"id": "core/rerun_of", "from": "card", "to": "card", "inverse": "rerun_by"}', 'applied'),
    ('relation_types', 'core', 'judged_by', '1',
     '{"id": "core/judged_by", "from": "card", "to": "card", "inverse": "judges"}', 'applied'),
    ('relation_types', 'core', 'baseline_of', '1',
     '{"id": "core/baseline_of", "from": "card", "to": "card", "inverse": "compared_to"}', 'applied'),
    ('relation_types', 'core', 'derived_from', '1',
     '{"id": "core/derived_from", "from": "eval", "to": "eval", "inverse": "derives"}', 'applied'),
    ('relation_types', 'core', 'subset_of', '1',
     '{"id": "core/subset_of", "from": "eval", "to": "eval", "inverse": "superset_of"}', 'applied');
