//! Attachment handlers: presigned upload, completion, presigned download.
//!
//! The two-step upload and the `ready` gate are described in
//! `evalhub_store::objects`. `GET /attachments/{sha256}` answers `302` to a
//! presigned URL and is authorised against the records that reference the
//! object: if any referencing record is public, anyone may download; else
//! the caller needs access to at least one of them.
//!
//! ```text
//! POST /attachments {sha256,size,media_type}
//!   already ready?                        → 200 { state: "ready" }
//!   else                                  → 201 { state: "pending", upload_url, expires_in_secs }
//! PUT <upload_url> (direct to the store, the hub never sees the bytes)
//! POST /attachments/{sha256}/complete     → 200 { size, hashed_by_hub }
//! GET  /attachments/{sha256}              → 302 <presigned GET>
//! ```
//!
//! # Who may upload
//!
//! Any valid token. An upload is content-addressed and not yet attached to
//! anything, so there is no namespace to authorise against; what a caller
//! cannot do is *use* it, because posting a record needs `write` on its
//! namespace. Announcing a sha does not reveal whether someone else has
//! uploaded the same bytes: the `ready` answer is the same one the caller
//! would get for their own completed upload.
//!
//! # Who may download
//!
//! In order:
//!
//! 1. any referencing record is public → anyone, no token needed;
//! 2. otherwise a referencing record's namespace is one the caller's token
//!    covers → that caller;
//! 3. an object with no references at all → any valid token. It is an
//!    upload in flight, and the uploader is the only party that knows its
//!    sha before it is attached to something. This is the one case where
//!    the hub cannot name an owner, so it settles for "authenticated";
//! 4. anything else → `404`, the same answer as for an object that was
//!    never announced, so a probe cannot tell the two apart.
//!
//! # Verification on `complete`
//!
//! The store checks that the object exists and that its size matches what
//! was announced, and re-hashes it when it is below
//! `attachments.hash_verify_max_bytes`. A size or hash that does not match
//! is `422`: the claim the client made about the bytes is wrong, and the
//! row stays `pending` so they can upload again.

use aide::axum::IntoApiResponse;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use evalhub_schema::error::{ErrorCode, ErrorEntry};
use evalhub_store::error::StoreError;
use evalhub_store::objects::{self, UploadState};

use crate::auth::{Auth, MaybeAuth};
use crate::error::ApiError;
use crate::state::AppState;

/// Body of `POST /attachments`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnnounceBody {
    /// sha256 of the bytes, 64 lower-case hexadecimal characters. This is
    /// the object's only name.
    pub sha256: String,
    /// Size of the bytes. `complete` refuses an object of another size.
    pub size: i64,
    /// Media type hint, for display and download. Not interpreted.
    pub media_type: Option<String>,
}

/// Where an announced object stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StateDto {
    /// Announced; the hub is waiting for the bytes and a `complete` call.
    Pending,
    /// Confirmed: the object exists with the announced size.
    Ready,
}

/// Response of `POST /attachments`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AnnouncedDto {
    /// `ready` when the object was already confirmed, `pending` otherwise.
    pub state: StateDto,
    /// Where to `PUT` the bytes. Absent when the object is already ready.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_url: Option<String>,
    /// How long that URL is valid for. Absent with `upload_url`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in_secs: Option<u64>,
}

/// Response of `POST /attachments/{sha256}/complete`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CompletedDto {
    /// Size of the object as the store reports it.
    pub size: u64,
    /// Whether the hub re-hashed the bytes. `false` means the object was
    /// above `attachments.hash_verify_max_bytes` and the client's sha256
    /// was taken on trust; the record says which.
    pub hashed_by_hub: bool,
}

/// A path segment that must be a sha256.
fn sha_from_path(hex_str: &str) -> Result<[u8; 32], ApiError> {
    let mut out = [0u8; 32];
    if hex_str.len() == 64 && hex::decode_to_slice(hex_str, &mut out).is_ok() {
        Ok(out)
    } else {
        Err(ApiError::BadRequest)
    }
}

fn sha_from_body(hex_str: &str) -> Result<[u8; 32], ApiError> {
    let mut out = [0u8; 32];
    if hex_str.len() == 64 && hex::decode_to_slice(hex_str, &mut out).is_ok() {
        Ok(out)
    } else {
        Err(ApiError::Validation(vec![ErrorEntry {
            path: "/sha256".to_string(),
            code: ErrorCode::Schema,
            hint: Some("sha256 must be 64 hexadecimal characters".to_string()),
        }]))
    }
}

/// `POST /api/v1/attachments` — announce an upload and get a presigned
/// `PUT` URL, or learn that the object is already there.
pub async fn announce(
    State(state): State<AppState>,
    _caller: Auth,
    Json(body): Json<AnnounceBody>,
) -> Result<(StatusCode, Json<AnnouncedDto>), ApiError> {
    let sha = sha_from_body(&body.sha256)?;
    if body.size < 0 {
        return Err(ApiError::Validation(vec![ErrorEntry {
            path: "/size".to_string(),
            code: ErrorCode::Schema,
            hint: Some("size must not be negative".to_string()),
        }]));
    }
    let objects = state.objects()?;
    let pool = state.db()?;
    match objects::begin_upload(pool, &sha, body.size, body.media_type.as_deref()).await? {
        UploadState::Ready => Ok((
            StatusCode::OK,
            Json(AnnouncedDto {
                state: StateDto::Ready,
                upload_url: None,
                expires_in_secs: None,
            }),
        )),
        UploadState::Pending => {
            let url = objects.presign_put(&sha, None).await?;
            Ok((
                StatusCode::CREATED,
                Json(AnnouncedDto {
                    state: StateDto::Pending,
                    upload_url: Some(url.to_string()),
                    expires_in_secs: Some(state.config.s3.presign_ttl_secs),
                }),
            ))
        }
    }
}

/// `POST /api/v1/attachments/{sha256}/complete` — confirm the bytes are in
/// the store and match what was announced.
pub async fn complete(
    State(state): State<AppState>,
    _caller: Auth,
    Path(sha_hex): Path<String>,
) -> Result<Json<CompletedDto>, ApiError> {
    let sha = sha_from_path(&sha_hex)?;
    let objects = state.objects()?;
    let pool = state.db()?;
    let max = state.config.attachments.hash_verify_max_bytes;
    match objects::complete(pool, objects, &sha, max).await {
        Ok(done) => Ok(Json(CompletedDto {
            size: done.size,
            hashed_by_hub: done.hashed_by_hub,
        })),
        // Announced but the bytes never arrived: the same state, and the
        // same code, as a record naming an object nobody uploaded.
        Err(StoreError::ObjectNotUploaded(_)) => {
            Err(ApiError::AttachmentMissing(vec![ErrorEntry {
                path: "/sha256".to_string(),
                code: ErrorCode::AttachmentMissing,
                hint: Some("upload the bytes to the presigned URL first".to_string()),
            }]))
        }
        Err(StoreError::AttachmentUnknown(_)) => Err(ApiError::NotFound),
        Err(StoreError::ObjectSizeMismatch { declared, actual }) => {
            Err(ApiError::Validation(vec![ErrorEntry {
                path: "/size".to_string(),
                code: ErrorCode::Schema,
                hint: Some(format!(
                    "announced {declared} bytes, the store holds {actual}"
                )),
            }]))
        }
        Err(StoreError::ObjectHashMismatch(_)) => Err(ApiError::Validation(vec![ErrorEntry {
            path: "/sha256".to_string(),
            code: ErrorCode::Schema,
            hint: Some("the uploaded bytes hash to something else".to_string()),
        }])),
        Err(e) => Err(e.into()),
    }
}

/// Decide whether `caller` may fetch the object, per the module doc.
async fn may_download(
    state: &AppState,
    caller: &crate::auth::Caller,
    sha: &[u8; 32],
) -> Result<bool, ApiError> {
    let pool = state.db()?;
    let refs = objects::referencing(pool, sha).await?;
    if refs.is_empty() {
        return Ok(caller.identity.is_some());
    }
    let namespaces = caller.namespaces();
    Ok(refs
        .iter()
        .any(|r| r.visibility == "public" || namespaces.contains(&r.ns)))
}

/// `GET /api/v1/attachments/{sha256}` — redirect to a presigned download.
pub async fn download(
    State(state): State<AppState>,
    MaybeAuth(caller): MaybeAuth,
    Path(sha_hex): Path<String>,
) -> Result<Response, ApiError> {
    let sha = sha_from_path(&sha_hex)?;
    let pool = state.db()?;
    let row = objects::state(pool, &sha)
        .await?
        .ok_or(ApiError::NotFound)?;
    if row.state != objects::AttachmentState::Ready {
        return Err(ApiError::NotFound);
    }
    if !may_download(&state, &caller, &sha).await? {
        return Err(ApiError::NotFound);
    }
    let url = state.objects()?.presign_get(&sha, None).await?;
    Ok((
        StatusCode::FOUND,
        [(header::LOCATION, url.to_string())],
        // A redirect to a signed URL is only good for its lifetime, and
        // the signature is caller-specific; caches must not keep it.
        [(header::CACHE_CONTROL, "no-store".to_string())],
    )
        .into_response())
}

/// `HEAD /api/v1/attachments/{sha256}` — the size and media type of a
/// downloadable object, without the redirect.
pub async fn head(
    State(state): State<AppState>,
    MaybeAuth(caller): MaybeAuth,
    Path(sha_hex): Path<String>,
) -> Result<Response, ApiError> {
    let sha = sha_from_path(&sha_hex)?;
    let pool = state.db()?;
    let row = objects::state(pool, &sha)
        .await?
        .ok_or(ApiError::NotFound)?;
    if row.state != objects::AttachmentState::Ready {
        return Err(ApiError::NotFound);
    }
    if !may_download(&state, &caller, &sha).await? {
        return Err(ApiError::NotFound);
    }
    let mut headers = HeaderMap::new();
    if let Ok(v) = row.size.to_string().parse() {
        headers.insert(header::CONTENT_LENGTH, v);
    }
    if let Some(v) = row.media_type.as_deref().and_then(|m| m.parse().ok()) {
        headers.insert(header::CONTENT_TYPE, v);
    }
    Ok((StatusCode::OK, headers).into_response())
}

/// `Response` is what the redirect handlers return; this keeps the trait
/// in scope so `aide` documents them.
const _: fn() = || {
    fn assert_into_api_response<T: IntoApiResponse>() {}
    assert_into_api_response::<Response>();
};
