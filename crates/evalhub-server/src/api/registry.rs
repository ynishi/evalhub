//! Registry handlers. `core/` is read-only; `PUT` of an `ext_schema`
//! returns `202` and starts the index job. See `evalhub_store::registry`.
