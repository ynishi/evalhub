//! Validation: everything the hub checks before it stores a record.
//!
//! Validation runs in two passes and collects every failure from both into
//! one list, so a producer sees all problems in one round trip.
//!
//! # Pass 1: structural
//!
//! The record is checked against the JSON Schema for its declared `schema`
//! (`evalhub.card/1.0` or `evalhub.eval/1.0`) using `jsonschema` with draft
//! 2020-12. The core is closed, so an unknown key anywhere outside `ext` is
//! a `schema` error. In the same pass every number is inspected: an integer
//! outside ±2^53 is `number_too_large`.
//!
//! The schema is the committed `schemas/*.json` from `evalhub_schema`, loaded
//! once. `$ref` resolution is confined to those embedded documents; the
//! validator never fetches over the network.
//!
//! # Pass 2: semantic
//!
//! Rules the schema language cannot express:
//!
//! | Rule                                                                                   | Code                       |
//! | -------------------------------------------------------------------------------------- | -------------------------- |
//! | `results` non-empty ⇒ `counts` present                                                 | `counts_missing`           |
//! | `counts.attempted >= completed + failed + skipped + errored`                           | `counts_inconsistent`      |
//! | every `results[].samples_ref`, `runs[].calls`, `runs[].artifacts[]` is an `attachments[].path` | `attachment_ref_unknown` |
//! | `attachments[].path` unique, relative, contains no `..`                                | `attachment_path_invalid`  |
//! | `results[].metric` matches `{ns}/{name}`                                               | `metric_id_invalid`        |
//! | `relations[].to` is `{ns}/{name}@{seq}`, `external:…`, or `hf:…`                       | `schema`                   |
//!
//! # What is deliberately not an error
//!
//! - A `relations[].to` that does not resolve. The record is accepted; the
//!   `refs_resolved` badge is withheld. A Card may legitimately be published
//!   before the Eval it cites.
//! - A `harness` or `metric` the registry does not know. Accepted; the
//!   `harness_registered` / `metric_registered` badges are withheld. The hub
//!   does not gate on vocabulary.
//! - An `attachments[].sha256` that has not been uploaded. This is not a
//!   validation failure but a state conflict, `409 attachment_missing`,
//!   raised by the store, because only the store knows.
//!
//! # Output
//!
//! `Vec<Error { path, code, hint }>`, empty on success. `path` is a JSON
//! pointer into the submitted record. `hint` is prose and not part of the
//! contract.
