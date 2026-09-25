#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Identity rows beyond the bootstrap: token listing and revocation,
//! namespace info, organisations and their members.

mod common;

use evalhub_store::auth::{self, NamespaceKind, Scope};
use evalhub_store::error::StoreError;
use evalhub_store::records::{self, Actor, IngestFacts, NewVersion, RecordType, Visibility};
use serde_json::json;
use uuid::Uuid;

fn no_badges(_: &IngestFacts) -> Vec<String> {
    Vec::new()
}

#[tokio::test]
async fn tokens_list_with_a_prefix_and_revoke_once() {
    let db = common::db().await;
    let user_id = auth::create_user(&db.pool, "erin").await.unwrap();
    let other = auth::create_user(&db.pool, "frank").await.unwrap();
    assert_eq!(
        auth::user_by_login(&db.pool, "erin").await.unwrap(),
        Some((user_id, "erin".to_owned()))
    );
    assert!(
        auth::user_by_login(&db.pool, "nobody")
            .await
            .unwrap()
            .is_none()
    );

    let h1 = [0xabu8; 32];
    let h2 = [0xcdu8; 32];
    let t1 = auth::create_token(&db.pool, user_id, Scope::Write, &["erin".to_owned()], &h1)
        .await
        .unwrap();
    let t2 = auth::create_token(&db.pool, user_id, Scope::Read, &["erin".to_owned()], &h2)
        .await
        .unwrap();

    let tokens = auth::list_tokens(&db.pool, user_id).await.unwrap();
    assert_eq!(tokens.len(), 2);
    assert_eq!(tokens[0].token_id, t1);
    assert_eq!(tokens[0].prefix, "abababab");
    assert_eq!(tokens[0].scope, Scope::Write);
    assert_eq!(tokens[1].token_id, t2);
    assert_eq!(tokens[1].prefix, "cdcdcdcd");
    assert!(tokens.iter().all(|t| t.revoked_at.is_none()));
    assert!(auth::list_tokens(&db.pool, other).await.unwrap().is_empty());

    let actor = Actor {
        user_id: Some(user_id),
        token_id: Some(t1),
    };
    // Not the owner: nothing happens.
    assert!(
        !auth::revoke_token(&db.pool, other, t1, actor)
            .await
            .unwrap()
    );
    assert!(
        auth::revoke_token(&db.pool, user_id, t1, actor)
            .await
            .unwrap()
    );
    assert!(
        !auth::revoke_token(&db.pool, user_id, t1, actor)
            .await
            .unwrap(),
        "second revoke is a no-op"
    );
    let found = auth::find_token(&db.pool, &h1).await.unwrap().unwrap();
    assert!(found.revoked_at.is_some());
    let tokens = auth::list_tokens(&db.pool, user_id).await.unwrap();
    assert!(tokens[0].revoked_at.is_some());
    assert!(tokens[1].revoked_at.is_none());

    let actions: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT action, subject, ns FROM audit WHERE action LIKE 'token.%' ORDER BY id",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        actions,
        vec![
            (
                "token.create".to_owned(),
                Some(t1.to_string()),
                Some("erin".to_owned())
            ),
            (
                "token.create".to_owned(),
                Some(t2.to_string()),
                Some("erin".to_owned())
            ),
            (
                "token.revoke".to_owned(),
                Some(t1.to_string()),
                Some("erin".to_owned())
            ),
        ]
    );
}

#[tokio::test]
async fn namespace_info_counts_what_the_caller_can_see() {
    let db = common::db().await;
    auth::create_user(&db.pool, "gina").await.unwrap();
    assert!(
        auth::namespace(&db.pool, "nobody", &[])
            .await
            .unwrap()
            .is_none()
    );

    let mk = |rt: RecordType, name: &'static str, seed: &'static [u8]| {
        let pool = db.pool.clone();
        async move {
            let body = json!({"title": name});
            let hash = common::sha256(seed);
            records::create_or_append(
                &pool,
                NewVersion {
                    record_type: rt,
                    ns: "gina",
                    name,
                    body: &body,
                    content_hash: &hash,
                    label: None,
                    actor: Actor::default(),
                    readable_ns: &[],
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
    };
    mk(RecordType::Card, "c1", b"c1").await;
    mk(RecordType::Card, "c2", b"c2").await;
    mk(RecordType::Eval, "e1", b"e1").await;
    records::set_visibility(
        &db.pool,
        RecordType::Card,
        "gina",
        "c1",
        Visibility::Public,
        Actor::default(),
    )
    .await
    .unwrap();

    let own = auth::namespace(&db.pool, "gina", &["gina".to_owned()])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(own.ns, "gina");
    assert_eq!(own.kind, NamespaceKind::User);
    assert_eq!((own.cards, own.evals), (2, 1));

    let anon = auth::namespace(&db.pool, "gina", &[])
        .await
        .unwrap()
        .unwrap();
    assert_eq!((anon.cards, anon.evals), (1, 0));
}

#[tokio::test]
async fn organisations_and_members() {
    let db = common::db().await;
    let hank = auth::create_user(&db.pool, "hank").await.unwrap();
    let ivy = auth::create_user(&db.pool, "ivy").await.unwrap();
    let actor = Actor {
        user_id: Some(hank),
        token_id: None,
    };

    auth::create_org(&db.pool, "acme", actor).await.unwrap();
    let dup = auth::create_org(&db.pool, "acme", actor).await.unwrap_err();
    assert!(
        matches!(dup, StoreError::NamespaceInUse(ref ns) if ns == "acme"),
        "got {dup:?}"
    );
    let clash = auth::create_org(&db.pool, "hank", actor).await.unwrap_err();
    assert!(
        matches!(clash, StoreError::NamespaceInUse(_)),
        "a user's namespace cannot become an org: {clash:?}"
    );
    let info = auth::namespace(&db.pool, "acme", &[])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(info.kind, NamespaceKind::Org);

    assert!(
        auth::org_members(&db.pool, "acme")
            .await
            .unwrap()
            .is_empty()
    );
    auth::add_org_member(&db.pool, "acme", hank, Scope::Admin, actor)
        .await
        .unwrap();
    auth::add_org_member(&db.pool, "acme", ivy, Scope::Read, actor)
        .await
        .unwrap();
    // Upsert changes the role.
    auth::add_org_member(&db.pool, "acme", ivy, Scope::Write, actor)
        .await
        .unwrap();
    let members = auth::org_members(&db.pool, "acme").await.unwrap();
    let logins: Vec<(&str, Scope)> = members.iter().map(|m| (m.login.as_str(), m.role)).collect();
    assert_eq!(logins, vec![("hank", Scope::Admin), ("ivy", Scope::Write)]);
    assert_eq!(
        auth::org_role(&db.pool, "acme", ivy).await.unwrap(),
        Some(Scope::Write)
    );
    assert_eq!(
        auth::org_role(&db.pool, "acme", Uuid::new_v4())
            .await
            .unwrap(),
        None
    );

    // A user namespace has no members.
    let err = auth::add_org_member(&db.pool, "hank", ivy, Scope::Read, actor)
        .await
        .unwrap_err();
    assert!(
        matches!(err, StoreError::NotAnOrganisation(ref ns) if ns == "hank"),
        "got {err:?}"
    );
    // An unknown user cannot be added.
    let err = auth::add_org_member(&db.pool, "acme", Uuid::new_v4(), Scope::Read, actor)
        .await
        .unwrap_err();
    assert!(
        matches!(err, StoreError::NamespaceUnknown(_)),
        "got {err:?}"
    );

    assert!(
        auth::remove_org_member(&db.pool, "acme", ivy, actor)
            .await
            .unwrap()
    );
    assert!(
        !auth::remove_org_member(&db.pool, "acme", ivy, actor)
            .await
            .unwrap()
    );
    assert_eq!(auth::org_members(&db.pool, "acme").await.unwrap().len(), 1);

    let actions: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT action, subject FROM audit WHERE ns = 'acme' ORDER BY id")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    let actions: Vec<(&str, Option<&str>)> = actions
        .iter()
        .map(|(a, s)| (a.as_str(), s.as_deref()))
        .collect();
    assert_eq!(
        actions,
        vec![
            ("org.create", Some("acme")),
            ("org.member.add", Some("hank")),
            ("org.member.add", Some("ivy")),
            ("org.member.add", Some("ivy")),
            ("org.member.remove", Some("ivy")),
        ]
    );
}
