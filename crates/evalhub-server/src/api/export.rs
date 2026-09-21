//! Export handlers.
//!
//! - `hf-model-index`: projects a Card's `results` / `model` / `task` onto
//!   the `model-index` YAML a Hugging Face model card embeds. Read
//!   compatibility only; the hub does not write to the Hub.
//! - `bundle`: a tar of the record JSON plus its attachments. For a public
//!   Card that references a private Eval, the bundle carries the Eval's
//!   `version_id` and attachment sha256s but not the bytes.
