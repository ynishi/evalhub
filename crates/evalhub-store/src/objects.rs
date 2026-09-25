//! Attachment lifecycle: presign, confirm, reference count, garbage collect.
//!
//! ```text
//!  client                         hub                              object store
//!    │  POST /attachments {sha256,size,media_type}                     │
//!    │────────────────────────────▶│  exists & ready? → 200, done       │
//!    │                             │  else insert (pending), presign PUT │
//!    │◀──── { upload_url } ────────│                                    │
//!    │  PUT bytes ─────────────────┼───────────────────────────────────▶│
//!    │  POST /attachments/{sha256}/complete                            │
//!    │────────────────────────────▶│  HEAD object; size matches? ──────▶│
//!    │                             │  (optionally stream & hash)        │
//!    │◀──── 200 (state = ready) ───│                                    │
//! ```
//!
//! Objects are keyed by sha256, so the same file attached to two records is
//! stored once, and a record can only be posted against objects that are
//! `ready` (`409 attachment_missing` otherwise). The hub never receives the
//! bytes on the request path: upload and download are presigned URLs
//! (`GET /attachments/{sha256}` answers `302`), which is what lets the server
//! stay stateless and lets storage with free egress do the serving.
//!
//! # Verification on `complete`
//!
//! `complete` checks the object exists and its size matches the declared
//! size. Hashing the object to confirm the sha256 requires streaming it
//! through the hub once; v0 does this for objects below a configured size
//! and trusts the client's hash above it, recording which
//! (`attachments.hashed_by_hub`). The hub does not look inside the file in
//! either case — no row counts, no JSONL schema.
//!
//! # Reference counting and GC
//!
//! `attachment_refs (version_id, sha256, path)` is written in the record's
//! transaction, and `run_attachment_refs (record_id, run_id, path,
//! sha256)` in a run write's ([`crate::runs`]); an object's reference
//! count is the sum over both tables. A version tombstone deletes the
//! version's refs and a run delete the run's; the GC job (see
//! `evalhub_server::jobs`) deletes objects with zero refs in both tables
//! older than a grace period, so an upload that is `ready` but not yet referenced is not
//! collected under the client. `pending` objects older than the grace
//! period are also collected. GC deletes the object before the row: a row
//! without an object is a harmless `ObjectNotUploaded` on the next
//! `complete`, an object without a row would leak for ever.
//!
//! # Private records
//!
//! A presigned GET for an attachment of a private record is issued only to
//! a caller with access to that record. The URL itself is time-limited;
//! the hub does not try to revoke it. [`referencing`] lists the records an
//! object is attached to, through a version or through a run, which is
//! what the download handler authorises against. A reference from an
//! archived run carries `run_archived`: an archived run is visible to
//! members of its namespace only, so that reference counts for a member
//! only, even on a public Eval.
//!
//! # Backend
//!
//! `object_store` with the `aws` feature, configured with a custom endpoint
//! and path-style addressing so MinIO, R2 and Tigris all work; presigning
//! is `object_store::signer::Signer`. The full AWS SDK was rejected as
//! dependency weight for a service that only ever signs URLs.
//!
//! Two clients are built from one configuration. The first, on
//! `endpoint`, is what the hub itself talks to (`HEAD`, the streaming read
//! for hashing, `DELETE`). The second, on `public_endpoint` when set, only
//! signs URLs: SigV4 signs the `Host` header, so a URL must be signed for
//! the host the client will actually send it to, and that host (a public
//! DNS name, a port mapped from a container) is often not the one the hub
//! reaches the store on. Without `public_endpoint` the two are the same.
//!
//! The bucket must already exist; `object_store` has no create-bucket call
//! and the hub does not try to provision storage. Keys are
//! `sha256/<64 hex>`.

use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::StreamExt;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path as ObjectPath;
use object_store::signer::{Method, Signer};
use object_store::{ObjectStoreExt, PutPayload};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use url::Url;
use uuid::Uuid;

use crate::error::StoreError;

/// Everything needed to reach the S3-compatible store. Secrets arrive
/// already exposed; the server's config layer is where they are redacted.
#[derive(Debug, Clone)]
pub struct ObjectConfig {
    /// Endpoint the hub reaches the store on, e.g. `http://127.0.0.1:9000`.
    pub endpoint: String,
    /// Endpoint clients reach the store on, when it differs from
    /// `endpoint`. Presigned URLs are signed for this host.
    pub public_endpoint: Option<String>,
    /// Bucket holding the attachments. Must exist.
    pub bucket: String,
    /// Access key id.
    pub access_key: String,
    /// Secret access key.
    pub secret_key: String,
    /// Region for SigV4; S3-compatible stores accept any, `us-east-1` is
    /// the conventional choice.
    pub region: String,
    /// Path-style addressing (`host/bucket/key`); required by MinIO,
    /// harmless elsewhere. `false` selects virtual-hosted style, in which
    /// case `endpoint` must already carry the bucket in its host.
    pub path_style: bool,
    /// Allow `http://` endpoints. Only for local development.
    pub allow_http: bool,
    /// Lifetime of a presigned URL.
    pub presign_ttl: Duration,
}

impl ObjectConfig {
    /// The region S3-compatible stores conventionally accept.
    pub const DEFAULT_REGION: &'static str = "us-east-1";
}

/// The store client pair: one for the hub's own I/O, one for signing.
#[derive(Debug, Clone)]
pub struct Objects {
    io: AmazonS3,
    signer: AmazonS3,
    ttl: Duration,
}

/// Object key for an attachment: `sha256/<hex>`.
pub fn key(sha: &[u8; 32]) -> ObjectPath {
    ObjectPath::from(format!("sha256/{}", hex::encode(sha)))
}

fn build(cfg: &ObjectConfig, endpoint: &str) -> Result<AmazonS3, StoreError> {
    Ok(AmazonS3Builder::new()
        .with_endpoint(endpoint)
        .with_bucket_name(&cfg.bucket)
        .with_access_key_id(&cfg.access_key)
        .with_secret_access_key(&cfg.secret_key)
        .with_region(&cfg.region)
        .with_allow_http(cfg.allow_http)
        .with_virtual_hosted_style_request(!cfg.path_style)
        .build()?)
}

impl Objects {
    /// Build the clients. Fails only on a malformed configuration; the
    /// store is not contacted.
    pub fn new(cfg: ObjectConfig) -> Result<Self, StoreError> {
        let io = build(&cfg, &cfg.endpoint)?;
        let signer = match &cfg.public_endpoint {
            Some(public) if public != &cfg.endpoint => build(&cfg, public)?,
            _ => io.clone(),
        };
        Ok(Self {
            io,
            signer,
            ttl: cfg.presign_ttl,
        })
    }

    /// A URL a client may `PUT` the bytes to. `expires` defaults to the
    /// configured TTL.
    pub async fn presign_put(
        &self,
        sha: &[u8; 32],
        expires: Option<Duration>,
    ) -> Result<Url, StoreError> {
        Ok(self
            .signer
            .signed_url(Method::PUT, &key(sha), expires.unwrap_or(self.ttl))
            .await?)
    }

    /// A URL a client may `GET` the bytes from.
    pub async fn presign_get(
        &self,
        sha: &[u8; 32],
        expires: Option<Duration>,
    ) -> Result<Url, StoreError> {
        Ok(self
            .signer
            .signed_url(Method::GET, &key(sha), expires.unwrap_or(self.ttl))
            .await?)
    }

    /// Size of the object, or `None` when there is no such object.
    pub async fn head(&self, sha: &[u8; 32]) -> Result<Option<u64>, StoreError> {
        match self.io.head(&key(sha)).await {
            Ok(meta) => Ok(Some(meta.size)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// sha256 of the object's bytes, computed by streaming it through the
    /// hub once. Costs one full read of the object; the caller bounds the
    /// size before asking.
    pub async fn sha256_of(&self, sha: &[u8; 32]) -> Result<[u8; 32], StoreError> {
        let result = self.io.get(&key(sha)).await?;
        let mut stream = result.into_stream();
        let mut hasher = Sha256::new();
        while let Some(chunk) = stream.next().await {
            hasher.update(&chunk?);
        }
        Ok(hasher.finalize().into())
    }

    /// Delete the object. Deleting a missing object is not an error.
    pub async fn delete(&self, sha: &[u8; 32]) -> Result<(), StoreError> {
        match self.io.delete(&key(sha)).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Write bytes under the key directly, bypassing the presigned flow.
    /// For tests and tooling; the API never proxies bytes.
    pub async fn put_direct(&self, sha: &[u8; 32], bytes: Vec<u8>) -> Result<(), StoreError> {
        self.io.put(&key(sha), PutPayload::from(bytes)).await?;
        Ok(())
    }
}

/// State of an `attachments` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentState {
    /// Announced; the bytes may or may not be in the store yet.
    Pending,
    /// Confirmed: the object exists with the announced size.
    Ready,
}

impl AttachmentState {
    fn parse(s: &str) -> Self {
        if s == "ready" {
            Self::Ready
        } else {
            Self::Pending
        }
    }
}

/// What `begin_upload` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadState {
    /// The object is already confirmed; nothing to upload.
    Ready,
    /// A `pending` row exists (created now or earlier); the client uploads.
    Pending,
}

/// An `attachments` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentRow {
    /// Content address.
    pub sha256: [u8; 32],
    /// Announced size in bytes.
    pub size: i64,
    /// Media type hint, if announced.
    pub media_type: Option<String>,
    /// Pending or ready.
    pub state: AttachmentState,
    /// Whether `complete` re-hashed the bytes (`false` above the size cap).
    pub hashed_by_hub: bool,
    /// When the row was announced.
    pub created_at: DateTime<Utc>,
}

/// What `complete` confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Completed {
    /// Size of the object in the store.
    pub size: u64,
    /// Whether the bytes were re-hashed by the hub.
    pub hashed_by_hub: bool,
}

/// A record referencing an attachment: the namespace, its visibility
/// (`public` / `private`), and the version or the run holding the
/// reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Referencing {
    /// Namespace of the referencing record.
    pub ns: String,
    /// `public` or `private`.
    pub visibility: String,
    /// The version whose `attachments[]` names the object; `None` when the
    /// reference is a run's (a run has no version).
    pub version_id: Option<Uuid>,
    /// The run whose `attachments[]` names the object, as `(record_id,
    /// run_id)`; `None` for a version's reference.
    pub run: Option<(Uuid, String)>,
    /// The reference is a run's and the run is archived. An archived run
    /// is visible to members of `ns` only, so the download check counts
    /// this reference only for a member, whatever `visibility` says.
    /// Always `false` for a version's reference.
    pub run_archived: bool,
}

fn sha_from(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let n = bytes.len().min(32);
    out[..n].copy_from_slice(&bytes[..n]);
    out
}

/// Announce an upload. Returns `Ready` when the object is already
/// confirmed (the client skips the upload), `Pending` otherwise, having
/// created the row or refreshed the announced size / media type of an
/// existing pending one.
pub async fn begin_upload(
    pool: &PgPool,
    sha: &[u8; 32],
    size: i64,
    media_type: Option<&str>,
) -> Result<UploadState, StoreError> {
    let sha_bytes: &[u8] = sha;
    let row = sqlx::query!(
        "INSERT INTO attachments (sha256, size, media_type)
         VALUES ($1, $2, $3)
         ON CONFLICT (sha256) DO UPDATE
           SET size = CASE WHEN attachments.state = 'pending' THEN EXCLUDED.size ELSE attachments.size END,
               media_type = CASE WHEN attachments.state = 'pending'
                                 THEN COALESCE(EXCLUDED.media_type, attachments.media_type)
                                 ELSE attachments.media_type END
         RETURNING state",
        sha_bytes,
        size,
        media_type,
    )
    .fetch_one(pool)
    .await?;
    Ok(match AttachmentState::parse(&row.state) {
        AttachmentState::Ready => UploadState::Ready,
        AttachmentState::Pending => UploadState::Pending,
    })
}

/// Confirm an upload: the object must exist with the announced size, and
/// is re-hashed when `size <= hash_verify_max_bytes`. On success the row
/// becomes `ready`. Calling it again on a ready row re-verifies and is
/// otherwise a no-op.
pub async fn complete(
    pool: &PgPool,
    objects: &Objects,
    sha: &[u8; 32],
    hash_verify_max_bytes: u64,
) -> Result<Completed, StoreError> {
    let hex = hex::encode(sha);
    let row = state(pool, sha)
        .await?
        .ok_or_else(|| StoreError::AttachmentUnknown(hex.clone()))?;
    let actual = objects
        .head(sha)
        .await?
        .ok_or_else(|| StoreError::ObjectNotUploaded(hex.clone()))?;
    if i128::from(actual) != i128::from(row.size) {
        return Err(StoreError::ObjectSizeMismatch {
            declared: row.size,
            actual,
        });
    }
    let hashed_by_hub = actual <= hash_verify_max_bytes;
    if hashed_by_hub && objects.sha256_of(sha).await? != *sha {
        return Err(StoreError::ObjectHashMismatch(hex));
    }
    let sha_bytes: &[u8] = sha;
    sqlx::query!(
        "UPDATE attachments SET state = 'ready', hashed_by_hub = $2 WHERE sha256 = $1",
        sha_bytes,
        hashed_by_hub,
    )
    .execute(pool)
    .await?;
    Ok(Completed {
        size: actual,
        hashed_by_hub,
    })
}

/// The row for `sha`, if announced.
pub async fn state(pool: &PgPool, sha: &[u8; 32]) -> Result<Option<AttachmentRow>, StoreError> {
    let sha_bytes: &[u8] = sha;
    let row = sqlx::query!(
        "SELECT sha256, size, media_type, state, hashed_by_hub, created_at
         FROM attachments WHERE sha256 = $1",
        sha_bytes,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| AttachmentRow {
        sha256: sha_from(&r.sha256),
        size: r.size,
        media_type: r.media_type,
        state: AttachmentState::parse(&r.state),
        hashed_by_hub: r.hashed_by_hub,
        created_at: r.created_at,
    }))
}

/// Whether every sha in `shas` is `ready`. Returns the ones that are not
/// (unknown or pending), in the order given; empty means all ready.
pub async fn missing(pool: &PgPool, shas: &[[u8; 32]]) -> Result<Vec<[u8; 32]>, StoreError> {
    if shas.is_empty() {
        return Ok(Vec::new());
    }
    let wanted: Vec<Vec<u8>> = shas.iter().map(|s| s.to_vec()).collect();
    let ready = sqlx::query!(
        "SELECT sha256 FROM attachments WHERE state = 'ready' AND sha256 = ANY($1)",
        &wanted,
    )
    .fetch_all(pool)
    .await?;
    let ready: std::collections::HashSet<Vec<u8>> = ready.into_iter().map(|r| r.sha256).collect();
    Ok(shas
        .iter()
        .filter(|s| !ready.contains(s.as_slice()))
        .copied()
        .collect())
}

/// Every version and every run that references the object, with what the
/// download handler needs to decide who may fetch it: versions first, then
/// runs, each oldest first.
///
/// A run's reference is `run_attachment_refs → runs → records`. A deleted
/// run has no reference (its rows were dropped with the tombstone); an
/// archived run's is returned with `run_archived`, for the caller to count
/// only for a member of the namespace. A run of an Eval with no live header
/// (every version tombstoned) is not returned either: nobody can read such
/// a run ([`crate::runs::get`] answers `None`), so its reference grants no
/// download. It still counts for the GC ([`gc_candidates`]), because the
/// run row and its reference are kept and a new header version makes the
/// run readable again.
pub async fn referencing(pool: &PgPool, sha: &[u8; 32]) -> Result<Vec<Referencing>, StoreError> {
    let sha_bytes: &[u8] = sha;
    let rows = sqlx::query!(
        "SELECT r.ns, r.visibility, v.version_id
         FROM attachment_refs ar
         JOIN versions v ON v.version_id = ar.version_id
         JOIN records r ON r.id = v.record_id
         WHERE ar.sha256 = $1
         ORDER BY v.created_at",
        sha_bytes,
    )
    .fetch_all(pool)
    .await?;
    let mut out: Vec<Referencing> = rows
        .into_iter()
        .map(|r| Referencing {
            ns: r.ns,
            visibility: r.visibility,
            version_id: Some(r.version_id),
            run: None,
            run_archived: false,
        })
        .collect();
    let runs = sqlx::query!(
        r#"SELECT DISTINCT r.ns, r.visibility, u.record_id, u.run_id, u.created_at,
                  u.archived_at IS NOT NULL AS "archived!"
           FROM run_attachment_refs rr
           JOIN runs u ON u.record_id = rr.record_id AND u.run_id = rr.run_id
           JOIN records r ON r.id = u.record_id
           WHERE rr.sha256 = $1
             AND EXISTS (SELECT 1 FROM versions v
                         WHERE v.record_id = r.id AND v.tombstoned_at IS NULL)
           ORDER BY u.created_at, u.record_id, u.run_id"#,
        sha_bytes,
    )
    .fetch_all(pool)
    .await?;
    out.extend(runs.into_iter().map(|r| Referencing {
        ns: r.ns,
        visibility: r.visibility,
        version_id: None,
        run: Some((r.record_id, r.run_id)),
        run_archived: r.archived,
    }));
    Ok(out)
}

/// Objects with no `attachment_refs` row and no `run_attachment_refs` row
/// that were announced more than `grace` ago, pending or ready.
pub async fn gc_candidates(pool: &PgPool, grace: Duration) -> Result<Vec<[u8; 32]>, StoreError> {
    let cutoff = Utc::now() - chrono::Duration::from_std(grace).unwrap_or(chrono::Duration::MAX);
    let rows = sqlx::query!(
        "SELECT a.sha256 FROM attachments a
         WHERE a.created_at < $1
           AND NOT EXISTS (SELECT 1 FROM attachment_refs ar WHERE ar.sha256 = a.sha256)
           AND NOT EXISTS (SELECT 1 FROM run_attachment_refs rr WHERE rr.sha256 = a.sha256)
         ORDER BY a.created_at",
        cutoff,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| sha_from(&r.sha256)).collect())
}

/// Delete every GC candidate: the object first, then the row. Returns how
/// many were collected. A failure on one object stops the sweep and is
/// returned; the ones already collected stay collected.
pub async fn gc(pool: &PgPool, objects: &Objects, grace: Duration) -> Result<usize, StoreError> {
    let candidates = gc_candidates(pool, grace).await?;
    let mut collected = 0;
    for sha in candidates {
        objects.delete(&sha).await?;
        let sha_bytes: &[u8] = &sha;
        // The row may have gained a reference between the SELECT and now;
        // the DELETE re-checks so a just-published record keeps its object
        // row (the object itself is gone, which the next `complete` reports).
        sqlx::query!(
            "DELETE FROM attachments a WHERE a.sha256 = $1
               AND NOT EXISTS (SELECT 1 FROM attachment_refs ar WHERE ar.sha256 = a.sha256)
               AND NOT EXISTS (SELECT 1 FROM run_attachment_refs rr WHERE rr.sha256 = a.sha256)",
            sha_bytes,
        )
        .execute(pool)
        .await?;
        collected += 1;
    }
    Ok(collected)
}
