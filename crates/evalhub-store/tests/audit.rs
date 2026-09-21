#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The audit read side: per-namespace, newest first, paged by id.

mod common;

use evalhub_store::auth::{self, NamespaceKind};
use evalhub_store::records::{self, Actor, IngestFacts, NewVersion, RecordType};
use evalhub_store::{audit, records::Visibility};
use serde_json::json;
use uuid::Uuid;

fn no_badges(_: &IngestFacts) -> Vec<String> {
    Vec::new()
}

#[tokio::test]
async fn list_pages_newest_first_per_namespace() {
    let db = common::db().await;
    auth::ensure_namespace(&db.pool, "alice", NamespaceKind::User)
        .await
        .unwrap();
    auth::ensure_namespace(&db.pool, "bob", NamespaceKind::User)
        .await
        .unwrap();
    let user_id = auth::create_user(&db.pool, "carol").await.unwrap();
    let actor = Actor {
        user_id: Some(user_id),
        token_id: None,
    };

    for i in 0..5 {
        let body = json!({"v": i});
        let hash = common::sha256(format!("audit{i}").as_bytes());
        records::create_or_append(
            &db.pool,
            NewVersion {
                record_type: RecordType::Card,
                ns: "alice",
                name: "x",
                body: &body,
                content_hash: &hash,
                label: None,
                actor,
                fingerprints: &[],
                results: &[],
                relations: &[],
                attachments: &[],
            },
            || (Uuid::new_v4(), Uuid::new_v4()),
            &no_badges,
        )
        .await
        .unwrap();
    }
    records::set_visibility(
        &db.pool,
        RecordType::Card,
        "alice",
        "x",
        Visibility::Public,
        actor,
    )
    .await
    .unwrap();
    let body = json!({"v": 0});
    records::create_or_append(
        &db.pool,
        NewVersion {
            record_type: RecordType::Card,
            ns: "bob",
            name: "y",
            body: &body,
            content_hash: &common::sha256(b"bob"),
            label: None,
            actor,
            fingerprints: &[],
            results: &[],
            relations: &[],
            attachments: &[],
        },
        || (Uuid::new_v4(), Uuid::new_v4()),
        &no_badges,
    )
    .await
    .unwrap();

    // Alice: 5 record writes + 1 settings change = 6 rows, newest first.
    let (page1, next) = audit::list(&db.pool, "alice", None, 4).await.unwrap();
    assert_eq!(page1.len(), 4);
    assert_eq!(page1[0].action, "settings.visibility");
    assert_eq!(page1[0].subject.as_deref(), Some("card/alice/x"));
    assert_eq!(page1[1].subject.as_deref(), Some("card/alice/x@5"));
    assert!(page1.windows(2).all(|w| w[0].id > w[1].id));
    assert!(page1.iter().all(|r| r.ns.as_deref() == Some("alice")));
    assert!(page1.iter().all(|r| r.actor_user_id == Some(user_id)));
    let next = next.expect("more rows");
    assert_eq!(next, page1[3].id);

    let (page2, next2) = audit::list(&db.pool, "alice", Some(next), 4).await.unwrap();
    assert_eq!(page2.len(), 2);
    assert_eq!(page2[1].action, "record.create");
    assert_eq!(page2[1].subject.as_deref(), Some("card/alice/x@1"));
    assert!(next2.is_none());
    assert!(page2.iter().all(|r| r.id < next));

    // Bob's namespace has its own single row; unknown namespaces are empty.
    let (bob, next) = audit::list(&db.pool, "bob", None, 10).await.unwrap();
    assert_eq!(bob.len(), 1);
    assert_eq!(bob[0].subject.as_deref(), Some("card/bob/y@1"));
    assert!(next.is_none());
    assert!(
        audit::list(&db.pool, "zed", None, 10)
            .await
            .unwrap()
            .0
            .is_empty()
    );

    // Exactly `limit` rows: no cursor.
    let (all, next) = audit::list(&db.pool, "alice", None, 6).await.unwrap();
    assert_eq!(all.len(), 6);
    assert!(next.is_none());
    assert!(
        all[5]
            .detail
            .as_ref()
            .unwrap()
            .get("content_hash")
            .is_some()
    );
}
