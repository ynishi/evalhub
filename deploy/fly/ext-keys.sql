-- Every `ext` key in stored bodies that is not `{ns}/{name}`.
--
-- Read-only. Run before deploying a release that enforces the key form
-- (`ext_key_invalid`, evalhub-core `validate`): a row here is a body a
-- 0.2.0 hub accepted that the new rule would refuse on write. Existing
-- rows are never rewritten; the query only tells whether any exist.
--
-- The pattern is `is_slug` from crates/evalhub-core/src/validate.rs: each
-- part starts with [a-z0-9] and continues with [a-z0-9._-]. The eight
-- paths are the top-level `ext` and the seven facets that carry one
-- (`grading` exists on Cards only).
WITH bodies AS (
    SELECT 'versions' AS tbl, version_id::text AS id, body
    FROM versions
    WHERE body IS NOT NULL
    UNION ALL
    SELECT 'runs', record_id::text || '/' || run_id, body
    FROM runs
    WHERE body IS NOT NULL
), exts AS (
    SELECT b.tbl, b.id, p.path, b.body #> p.path AS ext
    FROM bodies AS b
    CROSS JOIN (VALUES
        ('{ext}'::text[]),
        ('{model,ext}'),
        ('{task,ext}'),
        ('{harness,ext}'),
        ('{generation,ext}'),
        ('{trial,ext}'),
        ('{grading,ext}'),
        ('{env,ext}')
    ) AS p(path)
)
SELECT tbl, id, array_to_string(path, '/') AS at, k.key
FROM exts
CROSS JOIN LATERAL jsonb_object_keys(ext) AS k(key)
WHERE jsonb_typeof(ext) = 'object'
  AND k.key !~ '^[a-z0-9][a-z0-9._-]*/[a-z0-9][a-z0-9._-]*$'
ORDER BY tbl, id, at, k.key;
