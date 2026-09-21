//! Relation handlers: traversal, adding edges, and the comparison view.
//!
//! Semantics in `evalhub_store::relations`. The comparison view
//! (`GET /evals/{ns}/{name}/cards`) is the only endpoint that combines
//! records, and it combines them by lining them up, not by computing over
//! them.
