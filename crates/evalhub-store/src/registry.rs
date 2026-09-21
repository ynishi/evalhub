//! Registry entries and the `applying → applied` transition.
//!
//! `registry (kind, ns, id, version, body jsonb, state)`, unique on
//! `(kind, ns, id, version)`. Entries are immutable; a change is a new
//! version. `core/` entries are seeded from `evalhub_core::registry` by the
//! first migration and rejected on `PUT`.
//!
//! `PUT /registry/{kind}/{ns}/{id}@{version}` requires `write` on `ns`.
//! For `metrics`, `harnesses` and `relation_types` it is a plain insert and
//! returns `201`. For `ext_schemas` it returns `202` with `state:
//! applying`, and [`crate::index`] is asked to create the expression
//! indexes for the schema's typed paths; the state becomes `applied` when
//! every index reports valid. The read side reports the state so the UI
//! and the query type check can tell the caller why a path is not yet
//! sortable.
//!
//! Registering a harness or metric also enqueues a badge recomputation for
//! versions that cite it (`harness_registered` / `metric_registered`), so
//! that badges awarded by the registry follow the registry.
