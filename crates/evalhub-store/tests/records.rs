#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Records repository against a real Postgres: create, idempotent re-post,
//! append, visibility, concurrency, labels, namespaces, audit; plus the
//! identity rows the server's bootstrap and token extractor rely on.

mod common;

use evalhub_store::auth::{self, NamespaceKind, Scope};
use evalhub_store::error::StoreError;
use evalhub_store::records::{self, Actor, CreateOutcome, NewVersion, RecordType, Visibility};
use serde_json::{Value, json};
use uuid::Uuid;

fn ids() -> (Uuid, Uuid) {
    (Uuid::new_v4(), Uuid::new_v4())
}

fn new_version<'a>(
    ns: &'a str,
    name: &'a str,
    body: &'a Value,
    hash: &'a [u8; 32],
) -> NewVersion<'a> {
    NewVersion {
        record_type: RecordType::Card,
        ns,
        name,
        body,
        content_hash: hash,
        label: None,
        actor: Actor::default(),
    }
}

async fn alice(pool: &evalhub_store::PgPool) {
    auth::ensure_namespace(pool, "alice", NamespaceKind::User)
        .await
        .unwrap();
}

#[tokio::test]
async fn create_then_get_round_trips() {
    let db = common::db().await;
    alice(&db.pool).await;
    let body = json!({"schema": "evalhub.card/1.0", "title": "t", "results": []});
    let hash = common::sha256(body.to_string().as_bytes());

    let out = records::create_or_append(&db.pool, new_version("alice", "x", &body, &hash), ids)
        .await
        .unwrap();
    let CreateOutcome::Created(meta) = out else {
        panic!("first post must create");
    };
    assert_eq!(meta.seq, 1);
    assert_eq!(meta.content_hash, hash.to_vec());
    assert_eq!(meta.changed, vec!["results", "schema", "title"]);
    assert!(meta.badges.is_empty());
    assert_eq!(meta.visibility, Visibility::Private);

    let got = records::get_latest(
        &db.pool,
        RecordType::Card,
        "alice",
        "x",
        &["alice".to_owned()],
    )
    .await
    .unwrap()
    .expect("owner sees it");
    assert_eq!(got.meta, meta);
    assert_eq!(got.body, Some(body.clone()));

    let by_seq = records::get_by_seq(
        &db.pool,
        RecordType::Card,
        "alice",
        "x",
        1,
        &["alice".to_owned()],
    )
    .await
    .unwrap()
    .expect("seq 1 exists");
    assert_eq!(by_seq, got);

    // An Eval of the same name is a different record.
    let none = records::get_latest(
        &db.pool,
        RecordType::Eval,
        "alice",
        "x",
        &["alice".to_owned()],
    )
    .await
    .unwrap();
    assert!(none.is_none());
}

#[tokio::test]
async fn same_body_is_idempotent_and_different_body_appends() {
    let db = common::db().await;
    alice(&db.pool).await;
    let body = json!({"a": 1, "b": {"x": 1}});
    let hash = common::sha256(b"one");
    let first = records::create_or_append(&db.pool, new_version("alice", "x", &body, &hash), ids)
        .await
        .unwrap();

    let again = records::create_or_append(&db.pool, new_version("alice", "x", &body, &hash), ids)
        .await
        .unwrap();
    let CreateOutcome::Existing(meta) = again else {
        panic!("same hash must be an idempotent hit");
    };
    assert_eq!(meta.version_id, first.meta().version_id);
    assert_eq!(meta.seq, 1);

    let body2 = json!({"a": 1, "b": {"x": 2}, "c": true});
    let hash2 = common::sha256(b"two");
    let second =
        records::create_or_append(&db.pool, new_version("alice", "x", &body2, &hash2), ids)
            .await
            .unwrap();
    let CreateOutcome::Created(meta2) = second else {
        panic!("different hash must append");
    };
    assert_eq!(meta2.seq, 2);
    assert_eq!(meta2.record_id, first.meta().record_id);
    assert_ne!(meta2.version_id, first.meta().version_id);
    assert_eq!(meta2.changed, vec!["b", "c"]);

    let latest = records::get_latest(
        &db.pool,
        RecordType::Card,
        "alice",
        "x",
        &["alice".to_owned()],
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(latest.meta.seq, 2);
    assert_eq!(latest.body, Some(body2));
    let v1 = records::get_by_seq(
        &db.pool,
        RecordType::Card,
        "alice",
        "x",
        1,
        &["alice".to_owned()],
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(v1.body, Some(body));
}

#[tokio::test]
async fn private_record_is_invisible_without_the_namespace() {
    let db = common::db().await;
    alice(&db.pool).await;
    let body = json!({"a": 1});
    let hash = common::sha256(b"p");
    records::create_or_append(&db.pool, new_version("alice", "x", &body, &hash), ids)
        .await
        .unwrap();

    let anon: &[String] = &[];
    assert!(
        records::get_latest(&db.pool, RecordType::Card, "alice", "x", anon)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        records::get_by_seq(
            &db.pool,
            RecordType::Card,
            "alice",
            "x",
            1,
            &["bob".to_owned()]
        )
        .await
        .unwrap()
        .is_none()
    );
    assert!(
        records::get_latest(
            &db.pool,
            RecordType::Card,
            "alice",
            "x",
            &["bob".to_owned(), "alice".to_owned()]
        )
        .await
        .unwrap()
        .is_some()
    );

    // Flip to public directly (the settings endpoint is M2) and it is visible to anyone.
    sqlx::query("UPDATE records SET visibility = 'public' WHERE ns = 'alice' AND name = 'x'")
        .execute(&db.pool)
        .await
        .unwrap();
    let got = records::get_latest(&db.pool, RecordType::Card, "alice", "x", anon)
        .await
        .unwrap()
        .expect("public is visible to anonymous");
    assert_eq!(got.meta.visibility, Visibility::Public);
}

#[tokio::test]
async fn concurrent_appends_to_one_name_are_gap_free() {
    let db = common::db().await;
    alice(&db.pool).await;
    let body0 = json!({"v": 0});
    let h0 = common::sha256(b"v0");
    records::create_or_append(&db.pool, new_version("alice", "x", &body0, &h0), ids)
        .await
        .unwrap();

    let body1 = json!({"v": 1});
    let h1 = common::sha256(b"v1");
    let body2 = json!({"v": 2});
    let h2 = common::sha256(b"v2");
    let (a, b) = tokio::join!(
        records::create_or_append(&db.pool, new_version("alice", "x", &body1, &h1), ids),
        records::create_or_append(&db.pool, new_version("alice", "x", &body2, &h2), ids),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    let mut seqs = vec![a.meta().seq, b.meta().seq];
    seqs.sort();
    assert_eq!(seqs, vec![2, 3]);
    assert!(matches!(a, CreateOutcome::Created(_)));
    assert!(matches!(b, CreateOutcome::Created(_)));
}

#[tokio::test]
async fn concurrent_creates_of_a_new_name_yield_one_record() {
    let db = common::db().await;
    alice(&db.pool).await;
    let body1 = json!({"v": 1});
    let h1 = common::sha256(b"c1");
    let body2 = json!({"v": 2});
    let h2 = common::sha256(b"c2");
    let (a, b) = tokio::join!(
        records::create_or_append(&db.pool, new_version("alice", "fresh", &body1, &h1), ids),
        records::create_or_append(&db.pool, new_version("alice", "fresh", &body2, &h2), ids),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.meta().record_id, b.meta().record_id);
    let mut seqs = vec![a.meta().seq, b.meta().seq];
    seqs.sort();
    assert_eq!(seqs, vec![1, 2]);
}

#[tokio::test]
async fn label_is_stored_and_unique_within_a_name() {
    let db = common::db().await;
    alice(&db.pool).await;
    let body = json!({"v": 1});
    let h = common::sha256(b"l1");
    let mut nv = new_version("alice", "x", &body, &h);
    nv.label = Some("stable");
    let out = records::create_or_append(&db.pool, nv, ids).await.unwrap();
    assert_eq!(out.meta().label.as_deref(), Some("stable"));

    let body2 = json!({"v": 2});
    let h2 = common::sha256(b"l2");
    let mut nv2 = new_version("alice", "x", &body2, &h2);
    nv2.label = Some("stable");
    let err = records::create_or_append(&db.pool, nv2, ids)
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::LabelInUse), "got {err:?}");

    // The failed append wrote nothing: still one version.
    let latest = records::get_latest(
        &db.pool,
        RecordType::Card,
        "alice",
        "x",
        &["alice".to_owned()],
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(latest.meta.seq, 1);

    // The same label on another name is fine.
    let mut nv3 = new_version("alice", "y", &body2, &h2);
    nv3.label = Some("stable");
    records::create_or_append(&db.pool, nv3, ids).await.unwrap();
}

#[tokio::test]
async fn unknown_namespace_is_a_typed_error() {
    let db = common::db().await;
    let body = json!({"v": 1});
    let h = common::sha256(b"n");
    let err = records::create_or_append(&db.pool, new_version("nobody", "x", &body, &h), ids)
        .await
        .unwrap_err();
    assert!(
        matches!(err, StoreError::NamespaceUnknown(ref ns) if ns == "nobody"),
        "got {err:?}"
    );
}

#[tokio::test]
async fn every_write_leaves_an_audit_row() {
    let db = common::db().await;
    alice(&db.pool).await;
    let user_id = auth::create_user(&db.pool, "carol").await.unwrap();
    let token_id = auth::create_token(
        &db.pool,
        user_id,
        Scope::Write,
        &["alice".to_owned()],
        &[7u8; 32],
    )
    .await
    .unwrap();
    let actor = Actor {
        user_id: Some(user_id),
        token_id: Some(token_id),
    };

    let body = json!({"v": 1});
    let h = common::sha256(b"a1");
    let mut nv = new_version("alice", "x", &body, &h);
    nv.actor = actor;
    records::create_or_append(&db.pool, nv, ids).await.unwrap();
    // idempotent hit: no audit row
    records::create_or_append(&db.pool, nv, ids).await.unwrap();
    let body2 = json!({"v": 2});
    let h2 = common::sha256(b"a2");
    let mut nv2 = new_version("alice", "x", &body2, &h2);
    nv2.actor = actor;
    records::create_or_append(&db.pool, nv2, ids).await.unwrap();

    type AuditRow = (
        String,
        Option<String>,
        Option<Uuid>,
        Option<Uuid>,
        Option<String>,
    );
    let rows: Vec<AuditRow> = sqlx::query_as(
        "SELECT action, subject, actor_user_id, actor_token_id, ns FROM audit ORDER BY id",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, "record.create");
    assert_eq!(rows[0].1.as_deref(), Some("card/alice/x@1"));
    assert_eq!(rows[0].2, Some(user_id));
    assert_eq!(rows[0].3, Some(token_id));
    assert_eq!(rows[0].4.as_deref(), Some("alice"));
    assert_eq!(rows[1].0, "version.append");
    assert_eq!(rows[1].1.as_deref(), Some("card/alice/x@2"));
}

#[tokio::test]
async fn users_namespaces_and_tokens() {
    let db = common::db().await;
    let user_id = auth::create_user(&db.pool, "dave").await.unwrap();
    let dup = auth::create_user(&db.pool, "dave").await.unwrap_err();
    assert!(
        matches!(dup, StoreError::LoginInUse(ref l) if l == "dave"),
        "got {dup:?}"
    );

    // The personal namespace exists, so a record can be written to it.
    let body = json!({"v": 1});
    let h = common::sha256(b"d");
    records::create_or_append(&db.pool, new_version("dave", "x", &body, &h), ids)
        .await
        .unwrap();

    // Org namespace, idempotent.
    auth::ensure_namespace(&db.pool, "acme", NamespaceKind::Org)
        .await
        .unwrap();
    auth::ensure_namespace(&db.pool, "acme", NamespaceKind::Org)
        .await
        .unwrap();
    let kinds: Vec<(String, String)> =
        sqlx::query_as("SELECT ns, kind FROM namespaces ORDER BY ns")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        kinds,
        vec![
            ("acme".to_owned(), "org".to_owned()),
            ("dave".to_owned(), "user".to_owned())
        ]
    );

    let hash = [9u8; 32];
    let token_id = auth::create_token(
        &db.pool,
        user_id,
        Scope::Admin,
        &["dave".to_owned(), "acme".to_owned()],
        &hash,
    )
    .await
    .unwrap();
    let row = auth::find_token(&db.pool, &hash)
        .await
        .unwrap()
        .expect("token found");
    assert_eq!(row.token_id, token_id);
    assert_eq!(row.user_id, user_id);
    assert_eq!(row.login, "dave");
    assert_eq!(row.scope, Scope::Admin);
    assert_eq!(row.namespaces, vec!["dave", "acme"]);
    assert!(row.revoked_at.is_none());

    assert!(
        auth::find_token(&db.pool, &[0u8; 32])
            .await
            .unwrap()
            .is_none()
    );

    sqlx::query("UPDATE tokens SET revoked_at = now() WHERE token_id = $1")
        .bind(token_id)
        .execute(&db.pool)
        .await
        .unwrap();
    let revoked = auth::find_token(&db.pool, &hash).await.unwrap().unwrap();
    assert!(revoked.revoked_at.is_some());
}
