//! `ApiError`: the one error type the handlers return, and its HTTP mapping.
//!
//! | Variant                         | Status | Body                                  |
//! | ------------------------------- | ------ | ------------------------------------- |
//! | `Validation(Vec<error::Error>)` | 422    | `{ errors: [{ path, code, hint }] }`  |
//! | `AttachmentMissing`             | 409    | `{ errors: [{ code: attachment_missing }] }` |
//! | `LabelInUse`                    | 409    | `{ errors: [{ code: label_in_use }] }`|
//! | `NotFound`                      | 404    | empty                                 |
//! | `Forbidden`                     | 403    | empty                                 |
//! | `Unauthorized`                  | 401    | empty                                 |
//! | `BadRequest`                    | 400    | empty                                 |
//! | `Internal(anyhow::Error)`       | 500    | empty; logged with `error!`           |
//!
//! Domain errors from the library crates (`thiserror` enums) convert into
//! these variants at the handler boundary; `anyhow` is used only for the
//! `Internal` case and in `main`. Nothing internal (SQL text, paths,
//! panics) reaches a response body.
