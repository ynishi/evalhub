#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Relations read side against a real Postgres: resolution (eager and
//! lazy), privacy reduction, traversal, `add`, and the comparison view.
//! Rows are inserted with raw SQL so this file does not depend on the
//! ingest path. Needs Docker.

mod common;

use serde_json::json;
use uuid::Uuid;

use evalhub_store::PgPool;
use evalhub_store::relations::{
    self, Direction, EdgeEnd, RelationTarget, ResolvedTarget, TraverseParams, USES_EVAL,
};

async fn edge(pool: &PgPool, from: Uuid, ty: &str, to: Option<Uuid>, external: Option<&str>) {
    sqlx::query(
        "INSERT INTO relations (from_version_id, type, to_version_id, to_external, attrs) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(from)
    .bind(ty)
    .bind(to)
    .bind(external)
    .bind(json!({"runs": ["r1"]}))
    .execute(pool)
    .await
    .unwrap();
}

async fn fingerprint(pool: &PgPool, version: Uuid, facet: &str, value: &[u8]) {
    sqlx::query("INSERT INTO fingerprints (version_id, facet, fingerprint) VALUES ($1, $2, $3)")
        .bind(version)
        .bind(facet)
        .bind(value)
        .execute(pool)
        .await
        .unwrap();
}

#[test]
fn target_parsing() {
    assert_eq!(
        RelationTarget::parse("alice/single2-k4@3"),
        Some(RelationTarget::Version {
            ns: "alice",
            name: "single2-k4",
            seq: 3
        })
    );
    assert_eq!(
        RelationTarget::parse("external:https://x/y"),
        Some(RelationTarget::External("external:https://x/y"))
    );
    assert_eq!(
        RelationTarget::parse("hf:org/repo@sha"),
        Some(RelationTarget::External("hf:org/repo@sha"))
    );
    assert_eq!(RelationTarget::parse("alice/x"), None);
    assert_eq!(RelationTarget::parse("alice/x@0"), None);
    assert_eq!(RelationTarget::parse("alice/x@v1"), None);
    assert_eq!(RelationTarget::parse("x@1"), None);
}

#[tokio::test]
async fn outgoing_incoming_and_privacy() {
    let db = common::db().await;
    let pool = &db.pool;
    let (_, eval) = common::seed_version(pool, "eval", "alice", "e", 1, "private", "eval").await;
    let (_, card) = common::seed_version(pool, "card", "bob", "c", 1, "public", "card").await;
    edge(pool, card, USES_EVAL, Some(eval), None).await;
    edge(
        pool,
        card,
        "core/baseline_of",
        None,
        Some("hf:org/model@abc"),
    )
    .await;

    // Alice sees the eval endpoint; an anonymous reader sees a commitment.
    let out = relations::outgoing(pool, card, None, &["alice".to_string()])
        .await
        .unwrap();
    assert_eq!(out.len(), 2);
    let uses = out.iter().find(|r| r.relation_type == USES_EVAL).unwrap();
    assert!(
        matches!(&uses.to, ResolvedTarget::Version { version_id, visible: true, record_type, .. } if *version_id == eval && record_type == "eval")
    );
    assert_eq!(uses.attrs, Some(json!({"runs": ["r1"]})));
    let base = out
        .iter()
        .find(|r| r.relation_type == "core/baseline_of")
        .unwrap();
    assert_eq!(base.to, ResolvedTarget::External("hf:org/model@abc".into()));

    let out = relations::outgoing(pool, card, None, &[]).await.unwrap();
    let uses = out.iter().find(|r| r.relation_type == USES_EVAL).unwrap();
    assert!(
        matches!(&uses.to, ResolvedTarget::Version { visible: false, content_hash, .. } if !content_hash.is_empty())
    );

    // Type filter.
    let only = relations::outgoing(pool, card, Some(&[USES_EVAL.to_string()]), &[])
        .await
        .unwrap();
    assert_eq!(only.len(), 1);

    // Incoming on the eval finds the card.
    let inc = relations::incoming(pool, eval, None, &[]).await.unwrap();
    assert_eq!(inc.len(), 1);
    assert!(
        matches!(&inc[0].from, ResolvedTarget::Version { version_id, visible: true, .. } if *version_id == card)
    );
}

#[tokio::test]
async fn lazy_resolution_of_textual_reference() {
    let db = common::db().await;
    let pool = &db.pool;
    let (_, card) = common::seed_version(pool, "card", "bob", "c", 1, "public", "card").await;
    edge(pool, card, USES_EVAL, None, Some("alice/later@1")).await;

    let out = relations::outgoing(pool, card, None, &[]).await.unwrap();
    assert_eq!(
        out[0].to,
        ResolvedTarget::Unresolved("alice/later@1".into())
    );
    assert!(
        relations::incoming(pool, card, None, &[])
            .await
            .unwrap()
            .is_empty()
    );

    // The Eval appears: the same stored edge now resolves, and it is found
    // from the Eval's side too, without any write.
    let (_, later) =
        common::seed_version(pool, "eval", "alice", "later", 1, "public", "later").await;
    let out = relations::outgoing(pool, card, None, &[]).await.unwrap();
    assert!(
        matches!(&out[0].to, ResolvedTarget::Version { version_id, .. } if *version_id == later)
    );
    let inc = relations::incoming(pool, later, None, &[]).await.unwrap();
    assert_eq!(inc.len(), 1);
    assert_eq!(inc[0].from.version_id(), Some(card));
    let stored: (Option<Uuid>,) =
        sqlx::query_as("SELECT to_version_id FROM relations WHERE from_version_id = $1")
            .bind(card)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(stored.0, None, "read side does not write");
}

#[tokio::test]
async fn traverse_depth_direction_and_follow_latest() {
    let db = common::db().await;
    let pool = &db.pool;
    let (_, e_full) =
        common::seed_version(pool, "eval", "alice", "full", 1, "public", "full").await;
    let (_, e_sub) = common::seed_version(pool, "eval", "alice", "sub", 1, "public", "sub").await;
    let (c_rec, c1) = common::seed_version(pool, "card", "bob", "c", 1, "public", "c v1").await;
    edge(pool, e_sub, "core/subset_of", Some(e_full), None).await;
    edge(pool, c1, USES_EVAL, Some(e_sub), None).await;
    edge(
        pool,
        c1,
        "core/baseline_of",
        None,
        Some("external:https://paper"),
    )
    .await;

    let params = |direction, depth, follow_latest| TraverseParams {
        direction,
        types: None,
        depth,
        follow_latest,
    };

    // Out, depth 1: the card, the sub eval, and the external text edge.
    let g = relations::traverse(pool, c1, params(Direction::Out, 1, false), &[])
        .await
        .unwrap();
    let ids: Vec<Uuid> = g.nodes.iter().map(|n| n.version_id).collect();
    assert_eq!(ids, vec![c1, e_sub]);
    assert_eq!(g.edges.len(), 2);
    assert!(
        g.edges
            .iter()
            .any(|e| e.to == EdgeEnd::Text("external:https://paper".into()))
    );

    // Out, depth 2 reaches the full eval; depth is clamped at 5.
    let g = relations::traverse(pool, c1, params(Direction::Out, 9, false), &[])
        .await
        .unwrap();
    let ids: Vec<Uuid> = g.nodes.iter().map(|n| n.version_id).collect();
    assert_eq!(ids, vec![c1, e_sub, e_full]);

    // In from the full eval: sub, then the card.
    let g = relations::traverse(pool, e_full, params(Direction::In, 2, false), &[])
        .await
        .unwrap();
    let ids: Vec<Uuid> = g.nodes.iter().map(|n| n.version_id).collect();
    assert_eq!(ids, vec![e_full, e_sub, c1]);
    assert!(g.edges.iter().all(|e| matches!(e.to, EdgeEnd::Version(_))));

    // Both from sub.
    let g = relations::traverse(pool, e_sub, params(Direction::Both, 1, false), &[])
        .await
        .unwrap();
    assert_eq!(g.nodes.len(), 3);

    // A new card version has no edges; follow_latest reports the name's
    // current state, which is "no edges".
    let c2 = Uuid::new_v4();
    sqlx::query("INSERT INTO versions (version_id, record_id, seq, content_hash, body) VALUES ($1, $2, 2, $3, $4)")
        .bind(c2)
        .bind(c_rec)
        .bind(&[9u8; 32][..])
        .bind(json!({"title": "c v2"}))
        .execute(pool)
        .await
        .unwrap();
    let g = relations::traverse(pool, c1, params(Direction::Out, 2, true), &[])
        .await
        .unwrap();
    let ids: Vec<Uuid> = g.nodes.iter().map(|n| n.version_id).collect();
    assert_eq!(ids, vec![c2]);
    assert!(g.edges.is_empty());
    // Without follow_latest the old version's edges are still there.
    let g = relations::traverse(pool, c1, params(Direction::Out, 2, false), &[])
        .await
        .unwrap();
    assert_eq!(g.nodes.len(), 3);

    // Unknown start: empty graph.
    let g = relations::traverse(pool, Uuid::new_v4(), params(Direction::Both, 1, false), &[])
        .await
        .unwrap();
    assert!(g.nodes.is_empty());
}

#[tokio::test]
async fn add_resolves_and_audits() {
    let db = common::db().await;
    let pool = &db.pool;
    let (eval_record, eval) =
        common::seed_version(pool, "eval", "alice", "e", 1, "public", "e").await;
    let (_, card) = common::seed_version(pool, "card", "alice", "c", 1, "public", "c").await;
    let actor = (Some(Uuid::new_v4()), Some(Uuid::new_v4()));
    // The run `attrs.runs` names must have a row: a used set is fixed now.
    sqlx::query(
        "INSERT INTO runs (record_id, run_id, status, body, content_hash)
         VALUES ($1, 'r9', 'ok', '{}', $2)",
    )
    .bind(eval_record)
    .bind(&[9u8; 32][..])
    .execute(pool)
    .await
    .unwrap();

    let rel = relations::add(
        pool,
        card,
        USES_EVAL,
        RelationTarget::parse("alice/e@1").unwrap(),
        Some(&json!({"runs": ["r9"]})),
        actor,
        &[],
    )
    .await
    .unwrap();
    assert_eq!(rel.to.version_id(), Some(eval));
    assert_eq!(rel.from.version_id(), Some(card));
    let used: Vec<(String, Vec<u8>)> = sqlx::query_as(
        "SELECT run_id, content_hash FROM card_eval_runs WHERE card_version_id = $1",
    )
    .bind(card)
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(used, vec![("r9".to_string(), vec![9u8; 32])]);

    // Without a row the edge is refused, and nothing is written.
    let (_, other) = common::seed_version(pool, "card", "alice", "c-other", 1, "public", "o").await;
    let err = relations::add(
        pool,
        other,
        USES_EVAL,
        RelationTarget::parse("alice/e@1").unwrap(),
        Some(&json!({"runs": ["r9", "r10"]})),
        actor,
        &[],
    )
    .await
    .unwrap_err();
    let evalhub_store::error::StoreError::CardRunsRejected(errors) = err else {
        panic!("{err}");
    };
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].path, "/attrs/runs/1");
    assert_eq!(errors[0].code, evalhub_schema::error::ErrorCode::RunUnknown);
    let edges: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM relations WHERE from_version_id = $1")
            .bind(other)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(edges, 0);

    let rel = relations::add(
        pool,
        card,
        "core/retry_of",
        RelationTarget::parse("alice/c@7").unwrap(),
        None,
        actor,
        &[],
    )
    .await
    .unwrap();
    assert_eq!(rel.to, ResolvedTarget::Unresolved("alice/c@7".into()));

    let err = relations::add(
        pool,
        Uuid::new_v4(),
        USES_EVAL,
        RelationTarget::External("hf:x/y"),
        None,
        actor,
        &[],
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            err,
            evalhub_store::error::StoreError::SourceVersionUnknown(_)
        ),
        "{err}"
    );

    let audit: Vec<(String, Option<String>, Option<Uuid>)> =
        sqlx::query_as("SELECT action, subject, actor_user_id FROM audit ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap();
    assert_eq!(audit.len(), 2);
    assert_eq!(audit[0].0, "relation.add");
    assert_eq!(audit[0].1.as_deref(), Some("card/alice/c@1"));
    assert_eq!(audit[0].2, actor.0);
}

#[tokio::test]
async fn comparison_view() {
    let db = common::db().await;
    let pool = &db.pool;
    let (_, eval) = common::seed_version(pool, "eval", "alice", "e", 1, "public", "e").await;
    fingerprint(pool, eval, "harness", b"H1").await;
    fingerprint(pool, eval, "model", b"M1").await;

    // Same harness, different model.
    let (_, c1) = common::seed_version(pool, "card", "alice", "c1", 1, "public", "c1").await;
    fingerprint(pool, c1, "harness", b"H1").await;
    fingerprint(pool, c1, "model", b"M2").await;
    edge(pool, c1, USES_EVAL, Some(eval), None).await;

    // Same model via a textual edge; private to bob.
    let (_, c2) = common::seed_version(pool, "card", "bob", "c2", 1, "private", "c2").await;
    fingerprint(pool, c2, "harness", b"H9").await;
    fingerprint(pool, c2, "model", b"M1").await;
    edge(pool, c2, USES_EVAL, None, Some("alice/e@1")).await;

    // Edge on an old version only: the name's latest version does not use the eval.
    let (c3_rec, c3v1) = common::seed_version(pool, "card", "alice", "c3", 1, "public", "c3").await;
    edge(pool, c3v1, USES_EVAL, Some(eval), None).await;
    sqlx::query("INSERT INTO versions (version_id, record_id, seq, content_hash, body) VALUES ($1, $2, 2, $3, $4)")
        .bind(Uuid::new_v4())
        .bind(c3_rec)
        .bind(&[3u8; 32][..])
        .bind(json!({"title": "c3 v2"}))
        .execute(pool)
        .await
        .unwrap();

    // Another eval's card is not listed.
    let (_, other) =
        common::seed_version(pool, "eval", "alice", "other", 1, "public", "other").await;
    let (_, c4) = common::seed_version(pool, "card", "alice", "c4", 1, "public", "c4").await;
    edge(pool, c4, USES_EVAL, Some(other), None).await;

    let rows = relations::cards_using_eval(pool, eval, &[]).await.unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].name, "c1");
    assert_eq!(rows[0].title.as_deref(), Some("c1"));
    assert!(rows[0].same_harness);
    assert!(!rows[0].same_model);
    assert_eq!(
        rows[0].fingerprints.get("model").map(Vec::as_slice),
        Some(&b"M2"[..])
    );

    let rows = relations::cards_using_eval(pool, eval, &["bob".to_string()])
        .await
        .unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    let c2row = rows.iter().find(|r| r.name == "c2").unwrap();
    assert!(!c2row.same_harness);
    assert!(c2row.same_model);
    assert_eq!(c2row.version_id, c2);

    assert!(
        relations::cards_using_eval(pool, Uuid::new_v4(), &[])
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn resolution_never_reveals_a_private_version() {
    let db = common::db().await;
    let pool = &db.pool;
    let (_, secret) =
        common::seed_version(pool, "eval", "alice", "secret", 1, "private", "s").await;
    let (_, card) =
        common::seed_version(pool, "card", "alice", "public-card", 1, "public", "c").await;
    let (_, probe) = common::seed_version(pool, "card", "mallory", "probe", 1, "public", "p").await;
    let actor = (Some(Uuid::new_v4()), Some(Uuid::new_v4()));
    let alice = vec!["alice".to_owned()];

    // The owner's edge resolves: writing in alice implies reading there.
    let rel = relations::add(
        pool,
        card,
        USES_EVAL,
        RelationTarget::parse("alice/secret@1").unwrap(),
        None,
        actor,
        &[],
    )
    .await
    .unwrap();
    assert_eq!(rel.to.version_id(), Some(secret));

    // A body citing it withholds the element from outsiders only.
    let hidden = relations::hidden_targets(pool, &[card, probe], &[])
        .await
        .unwrap();
    assert_eq!(hidden.len(), 1, "{hidden:?}");
    let h = &hidden[&card][0];
    assert_eq!(
        (
            h.relation_type.as_str(),
            h.ns.as_str(),
            h.name.as_str(),
            h.seq
        ),
        (USES_EVAL, "alice", "secret", 1)
    );
    assert_eq!(h.version_id, secret);
    assert_eq!(h.content_hash.len(), 32);
    assert!(
        relations::hidden_targets(pool, &[card], &alice)
            .await
            .unwrap()
            .is_empty()
    );

    // Someone without access probing the name gets what a missing version
    // gives: the text back, unresolved, now and on every later read.
    let rel = relations::add(
        pool,
        probe,
        USES_EVAL,
        RelationTarget::parse("alice/secret@1").unwrap(),
        None,
        actor,
        &[],
    )
    .await
    .unwrap();
    assert_eq!(rel.to, ResolvedTarget::Unresolved("alice/secret@1".into()));
    let missing = relations::add(
        pool,
        probe,
        USES_EVAL,
        RelationTarget::parse("alice/nothing@1").unwrap(),
        None,
        actor,
        &[],
    )
    .await
    .unwrap();
    assert_eq!(
        missing.to,
        ResolvedTarget::Unresolved("alice/nothing@1".into())
    );
    let outsider = relations::outgoing(pool, probe, None, &[]).await.unwrap();
    assert!(
        outsider
            .iter()
            .all(|r| matches!(r.to, ResolvedTarget::Unresolved(_))),
        "{outsider:?}"
    );
    assert!(
        relations::hidden_targets(pool, &[probe], &[])
            .await
            .unwrap()
            .is_empty(),
        "an unresolved edge withholds nothing: it holds only the writer's text"
    );

    // The owner reading the probe sees it link up lazily.
    let owner = relations::outgoing(pool, probe, None, &alice)
        .await
        .unwrap();
    assert!(
        owner.iter().any(|r| r.to.version_id() == Some(secret)),
        "{owner:?}"
    );

    // Once public, it links up for everyone.
    sqlx::query("UPDATE records SET visibility = 'public' WHERE ns = 'alice' AND name = 'secret'")
        .execute(pool)
        .await
        .unwrap();
    let outsider = relations::outgoing(pool, probe, None, &[]).await.unwrap();
    assert!(
        outsider.iter().any(|r| r.to.version_id() == Some(secret)),
        "{outsider:?}"
    );
}
