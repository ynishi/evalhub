#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Attachment lifecycle against a real Postgres and MinIO: announce,
//! presigned upload, complete (size and hash checks), presigned download,
//! reference lookup and garbage collection. Needs Docker.

mod common;

use std::time::Duration;

use sha2::{Digest, Sha256};

use evalhub_store::error::StoreError;
use evalhub_store::objects::{self, AttachmentState, UploadState};

fn real_sha(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

async fn upload(url: &url::Url, bytes: &[u8]) {
    let res = reqwest::Client::new()
        .put(url.clone())
        .body(bytes.to_vec())
        .send()
        .await
        .expect("PUT");
    assert!(res.status().is_success(), "PUT status {}", res.status());
}

#[tokio::test]
async fn announce_upload_complete_download() {
    let db = common::db().await;
    let s3 = common::s3().await;
    let bytes = b"hello evalhub, these are attachment bytes";
    let sha = real_sha(bytes);

    let state = objects::begin_upload(&db.pool, &sha, bytes.len() as i64, Some("text/plain"))
        .await
        .unwrap();
    assert_eq!(state, UploadState::Pending);
    let row = objects::state(&db.pool, &sha).await.unwrap().unwrap();
    assert_eq!(row.state, AttachmentState::Pending);
    assert_eq!(row.media_type.as_deref(), Some("text/plain"));

    // Not uploaded yet.
    let err = objects::complete(&db.pool, &s3.objects, &sha, 1 << 20)
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::ObjectNotUploaded(_)), "{err}");

    let put = s3.objects.presign_put(&sha, None).await.unwrap();
    assert!(put.as_str().starts_with(&s3.endpoint), "{put}");
    assert!(
        put.path()
            .ends_with(&format!("sha256/{}", hex::encode(sha))),
        "{put}"
    );
    upload(&put, bytes).await;

    let done = objects::complete(&db.pool, &s3.objects, &sha, 1 << 20)
        .await
        .unwrap();
    assert_eq!(done.size, bytes.len() as u64);
    assert!(done.hashed_by_hub);
    let row = objects::state(&db.pool, &sha).await.unwrap().unwrap();
    assert_eq!(row.state, AttachmentState::Ready);
    assert!(row.hashed_by_hub);

    // Announcing again reports ready and does not touch the row.
    let again = objects::begin_upload(&db.pool, &sha, 1, None)
        .await
        .unwrap();
    assert_eq!(again, UploadState::Ready);
    let row = objects::state(&db.pool, &sha).await.unwrap().unwrap();
    assert_eq!(row.size, bytes.len() as i64);

    // Download through a presigned GET.
    let get = s3
        .objects
        .presign_get(&sha, Some(Duration::from_secs(60)))
        .await
        .unwrap();
    let body = reqwest::get(get).await.unwrap().bytes().await.unwrap();
    assert_eq!(&body[..], &bytes[..]);

    assert!(objects::missing(&db.pool, &[sha]).await.unwrap().is_empty());
    let other = real_sha(b"never announced");
    assert_eq!(
        objects::missing(&db.pool, &[sha, other]).await.unwrap(),
        vec![other]
    );
}

#[tokio::test]
async fn complete_rejects_unknown_wrong_size_and_wrong_hash() {
    let db = common::db().await;
    let s3 = common::s3().await;

    let err = objects::complete(&db.pool, &s3.objects, &real_sha(b"x"), 1 << 20)
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::AttachmentUnknown(_)), "{err}");

    // Size mismatch: announced one byte more than uploaded.
    let bytes = b"twelve bytes";
    let sha = real_sha(bytes);
    objects::begin_upload(&db.pool, &sha, bytes.len() as i64 + 1, None)
        .await
        .unwrap();
    upload(&s3.objects.presign_put(&sha, None).await.unwrap(), bytes).await;
    let err = objects::complete(&db.pool, &s3.objects, &sha, 1 << 20)
        .await
        .unwrap_err();
    assert!(
        matches!(err, StoreError::ObjectSizeMismatch { declared, actual } if declared == 13 && actual == 12),
        "{err}"
    );
    // Still pending.
    assert_eq!(
        objects::state(&db.pool, &sha).await.unwrap().unwrap().state,
        AttachmentState::Pending
    );

    // Hash mismatch: announced under the sha of other bytes.
    let lie = real_sha(b"something else");
    objects::begin_upload(&db.pool, &lie, bytes.len() as i64, None)
        .await
        .unwrap();
    upload(&s3.objects.presign_put(&lie, None).await.unwrap(), bytes).await;
    let err = objects::complete(&db.pool, &s3.objects, &lie, 1 << 20)
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::ObjectHashMismatch(_)), "{err}");
    // Above the hashing cap the client's hash is trusted and recorded as such.
    let done = objects::complete(&db.pool, &s3.objects, &lie, 0)
        .await
        .unwrap();
    assert!(!done.hashed_by_hub);
    let row = objects::state(&db.pool, &lie).await.unwrap().unwrap();
    assert_eq!(row.state, AttachmentState::Ready);
    assert!(!row.hashed_by_hub);
}

#[tokio::test]
async fn referencing_and_gc() {
    let db = common::db().await;
    let s3 = common::s3().await;
    let pool = &db.pool;

    let referenced = real_sha(b"referenced");
    let orphan = real_sha(b"orphan");
    let fresh = real_sha(b"fresh orphan");
    for (sha, bytes) in [
        (referenced, &b"referenced"[..]),
        (orphan, &b"orphan"[..]),
        (fresh, &b"fresh orphan"[..]),
    ] {
        objects::begin_upload(pool, &sha, bytes.len() as i64, None)
            .await
            .unwrap();
        upload(&s3.objects.presign_put(&sha, None).await.unwrap(), bytes).await;
        objects::complete(pool, &s3.objects, &sha, 1 << 20)
            .await
            .unwrap();
    }

    let (_, v_pub) = common::seed_version(pool, "card", "alice", "c", 1, "public", "c").await;
    let (_, v_priv) = common::seed_version(pool, "eval", "bob", "e", 1, "private", "e").await;
    for v in [v_pub, v_priv] {
        sqlx::query("INSERT INTO attachment_refs (version_id, sha256, path) VALUES ($1, $2, 'samples.jsonl')")
            .bind(v)
            .bind(referenced.as_slice())
            .execute(pool)
            .await
            .unwrap();
    }
    let refs = objects::referencing(pool, &referenced).await.unwrap();
    assert_eq!(refs.len(), 2);
    assert!(
        refs.iter()
            .any(|r| r.ns == "alice" && r.visibility == "public" && r.version_id == v_pub)
    );
    assert!(
        refs.iter()
            .any(|r| r.ns == "bob" && r.visibility == "private" && r.version_id == v_priv)
    );
    assert!(
        objects::referencing(pool, &orphan)
            .await
            .unwrap()
            .is_empty()
    );

    // Backdate two rows past the grace period; `fresh` stays young.
    for sha in [referenced, orphan] {
        sqlx::query(
            "UPDATE attachments SET created_at = now() - interval '2 hours' WHERE sha256 = $1",
        )
        .bind(sha.as_slice())
        .execute(pool)
        .await
        .unwrap();
    }
    let candidates = objects::gc_candidates(pool, Duration::from_secs(3600))
        .await
        .unwrap();
    assert_eq!(candidates, vec![orphan]);

    let collected = objects::gc(pool, &s3.objects, Duration::from_secs(3600))
        .await
        .unwrap();
    assert_eq!(collected, 1);
    assert!(objects::state(pool, &orphan).await.unwrap().is_none());
    assert!(s3.objects.head(&orphan).await.unwrap().is_none());
    assert!(objects::state(pool, &referenced).await.unwrap().is_some());
    assert_eq!(s3.objects.head(&referenced).await.unwrap(), Some(10));
    assert!(objects::state(pool, &fresh).await.unwrap().is_some());

    // Deleting a missing object is not an error.
    s3.objects.delete(&orphan).await.unwrap();
}
