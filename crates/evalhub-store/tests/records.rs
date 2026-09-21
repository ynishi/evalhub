#![allow(clippy::unwrap_used, clippy::expect_used, clippy::type_complexity)]

//! Records repository against a real Postgres: create, idempotent re-post,
//! append, visibility, concurrency, labels, namespaces, audit; plus the
//! side tables, relation resolution, attachment gating, tombstones and
//! listing of M2.

mod common;

use std::collections::BTreeMap;

use evalhub_store::auth::{self, NamespaceKind, Scope};
use evalhub_store::error::StoreError;
use evalhub_store::records::{
    self, Actor, CreateOutcome, IngestFacts, ListCursor, ListParams, ListSort, NewAttachmentRef,
    NewRelation, NewResult, NewVersion, RecordType, RelationTarget, TombstoneReason,
    TombstoneRequest, Visibility,
};
use serde_json::{Value, json};
use uuid::Uuid;

fn ids() -> (Uuid, Uuid) {
    (Uuid::new_v4(), Uuid::new_v4())
}

fn no_badges(_: &IngestFacts) -> Vec<String> {
    Vec::new()
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
        fingerprints: &[],
        results: &[],
        relations: &[],
        attachments: &[],
    }
}

async fn alice(pool: &evalhub_store::PgPool) {
    auth::ensure_namespace(pool, "alice", NamespaceKind::User)
        .await
        .unwrap();
}

async fn attachment_row(pool: &evalhub_store::PgPool, sha: &[u8; 32], ready: bool) {
    sqlx::query("INSERT INTO attachments (sha256, size, state) VALUES ($1, 1, $2)")
        .bind(sha.as_slice())
        .bind(if ready { "ready" } else { "pending" })
        .execute(pool)
        .await
        .unwrap();
}

async fn post(
    pool: &evalhub_store::PgPool,
    ns: &str,
    name: &str,
    body: &Value,
    hash: &[u8; 32],
) -> CreateOutcome {
    records::create_or_append(pool, new_version(ns, name, body, hash), ids, &no_badges)
        .await
        .unwrap()
}

fn alice_ns() -> Vec<String> {
    vec!["alice".to_owned()]
}

#[tokio::test]
async fn create_then_get_round_trips() {
    let db = common::db().await;
    alice(&db.pool).await;
    let body = json!({"schema": "evalhub.card/1.0", "title": "t", "results": []});
    let hash = common::sha256(body.to_string().as_bytes());

    let out = post(&db.pool, "alice", "x", &body, &hash).await;
    let CreateOutcome::Created { meta, facts } = out else {
        panic!("first post must create");
    };
    assert_eq!(meta.seq, 1);
    assert_eq!(meta.content_hash, hash.to_vec());
    assert_eq!(meta.changed, vec!["results", "schema", "title"]);
    assert!(meta.badges.is_empty());
    assert_eq!(meta.visibility, Visibility::Private);
    assert!(facts.all_refs_resolved, "vacuously true with no relations");
    assert!(!facts.harness_registered);
    assert!(
        facts.all_metrics_registered,
        "vacuously true with no results"
    );

    let got = records::get_latest(&db.pool, RecordType::Card, "alice", "x", &alice_ns())
        .await
        .unwrap()
        .expect("owner sees it");
    assert_eq!(got.meta, meta);
    assert_eq!(got.body, Some(body.clone()));
    assert!(got.tombstone.is_none());

    let by_seq = records::get_by_seq(&db.pool, RecordType::Card, "alice", "x", 1, &alice_ns())
        .await
        .unwrap()
        .expect("seq 1 exists");
    assert_eq!(by_seq, got);

    // An Eval of the same name is a different record.
    let none = records::get_latest(&db.pool, RecordType::Eval, "alice", "x", &alice_ns())
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
    let first = post(&db.pool, "alice", "x", &body, &hash).await;

    let again = post(&db.pool, "alice", "x", &body, &hash).await;
    let CreateOutcome::Existing(meta) = again else {
        panic!("same hash must be an idempotent hit");
    };
    assert_eq!(meta.version_id, first.meta().version_id);
    assert_eq!(meta.seq, 1);

    let body2 = json!({"a": 1, "b": {"x": 2}, "c": true});
    let hash2 = common::sha256(b"two");
    let second = post(&db.pool, "alice", "x", &body2, &hash2).await;
    let CreateOutcome::Created { meta: meta2, .. } = second else {
        panic!("different hash must append");
    };
    assert_eq!(meta2.seq, 2);
    assert_eq!(meta2.record_id, first.meta().record_id);
    assert_ne!(meta2.version_id, first.meta().version_id);
    assert_eq!(meta2.changed, vec!["b", "c"]);

    let latest = records::get_latest(&db.pool, RecordType::Card, "alice", "x", &alice_ns())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.meta.seq, 2);
    assert_eq!(latest.body, Some(body2));
    let v1 = records::get_by_seq(&db.pool, RecordType::Card, "alice", "x", 1, &alice_ns())
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
    post(&db.pool, "alice", "x", &body, &hash).await;

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

    records::set_visibility(
        &db.pool,
        RecordType::Card,
        "alice",
        "x",
        Visibility::Public,
        Actor::default(),
    )
    .await
    .unwrap();
    let got = records::get_latest(&db.pool, RecordType::Card, "alice", "x", anon)
        .await
        .unwrap()
        .expect("public is visible to anonymous");
    assert_eq!(got.meta.visibility, Visibility::Public);

    records::set_visibility(
        &db.pool,
        RecordType::Card,
        "alice",
        "x",
        Visibility::Private,
        Actor::default(),
    )
    .await
    .unwrap();
    assert!(
        records::get_latest(&db.pool, RecordType::Card, "alice", "x", anon)
            .await
            .unwrap()
            .is_none()
    );

    let err = records::set_visibility(
        &db.pool,
        RecordType::Card,
        "alice",
        "nope",
        Visibility::Public,
        Actor::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, StoreError::RecordNotFound), "got {err:?}");
}

#[tokio::test]
async fn concurrent_appends_to_one_name_are_gap_free() {
    let db = common::db().await;
    alice(&db.pool).await;
    let body0 = json!({"v": 0});
    let h0 = common::sha256(b"v0");
    post(&db.pool, "alice", "x", &body0, &h0).await;

    let body1 = json!({"v": 1});
    let h1 = common::sha256(b"v1");
    let body2 = json!({"v": 2});
    let h2 = common::sha256(b"v2");
    let (a, b) = tokio::join!(
        records::create_or_append(
            &db.pool,
            new_version("alice", "x", &body1, &h1),
            ids,
            &no_badges
        ),
        records::create_or_append(
            &db.pool,
            new_version("alice", "x", &body2, &h2),
            ids,
            &no_badges
        ),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    let mut seqs = vec![a.meta().seq, b.meta().seq];
    seqs.sort();
    assert_eq!(seqs, vec![2, 3]);
    assert!(matches!(a, CreateOutcome::Created { .. }));
    assert!(matches!(b, CreateOutcome::Created { .. }));
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
        records::create_or_append(
            &db.pool,
            new_version("alice", "fresh", &body1, &h1),
            ids,
            &no_badges
        ),
        records::create_or_append(
            &db.pool,
            new_version("alice", "fresh", &body2, &h2),
            ids,
            &no_badges
        ),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.meta().record_id, b.meta().record_id);
    let mut seqs = vec![a.meta().seq, b.meta().seq];
    seqs.sort();
    assert_eq!(seqs, vec![1, 2]);
}

#[tokio::test]
async fn label_on_post_is_stored_and_unique_within_a_name() {
    let db = common::db().await;
    alice(&db.pool).await;
    let body = json!({"v": 1});
    let h = common::sha256(b"l1");
    let mut nv = new_version("alice", "x", &body, &h);
    nv.label = Some("stable");
    let out = records::create_or_append(&db.pool, nv, ids, &no_badges)
        .await
        .unwrap();
    assert_eq!(out.meta().label.as_deref(), Some("stable"));

    let body2 = json!({"v": 2});
    let h2 = common::sha256(b"l2");
    let mut nv2 = new_version("alice", "x", &body2, &h2);
    nv2.label = Some("stable");
    let err = records::create_or_append(&db.pool, nv2, ids, &no_badges)
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::LabelInUse), "got {err:?}");

    // The failed append wrote nothing: still one version.
    let latest = records::get_latest(&db.pool, RecordType::Card, "alice", "x", &alice_ns())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.meta.seq, 1);

    // The same label on another name is fine.
    let mut nv3 = new_version("alice", "y", &body2, &h2);
    nv3.label = Some("stable");
    records::create_or_append(&db.pool, nv3, ids, &no_badges)
        .await
        .unwrap();
}

#[tokio::test]
async fn set_label_points_repoints_and_refuses_numeric() {
    let db = common::db().await;
    alice(&db.pool).await;
    let b1 = json!({"v": 1});
    let h1 = common::sha256(b"sl1");
    let b2 = json!({"v": 2});
    let h2 = common::sha256(b"sl2");
    post(&db.pool, "alice", "x", &b1, &h1).await;
    post(&db.pool, "alice", "x", &b2, &h2).await;

    let meta = records::set_label(
        &db.pool,
        RecordType::Card,
        "alice",
        "x",
        1,
        "baseline",
        Actor::default(),
    )
    .await
    .unwrap();
    assert_eq!(meta.seq, 1);
    assert_eq!(meta.label.as_deref(), Some("baseline"));
    let by_label = records::get_by_label(
        &db.pool,
        RecordType::Card,
        "alice",
        "x",
        "baseline",
        &alice_ns(),
    )
    .await
    .unwrap()
    .expect("label resolves");
    assert_eq!(by_label.meta.seq, 1);
    assert_eq!(by_label.body, Some(b1.clone()));

    // Re-point: the label moves, it is not duplicated.
    let meta = records::set_label(
        &db.pool,
        RecordType::Card,
        "alice",
        "x",
        2,
        "baseline",
        Actor::default(),
    )
    .await
    .unwrap();
    assert_eq!(meta.seq, 2);
    let v1 = records::get_by_seq(&db.pool, RecordType::Card, "alice", "x", 1, &alice_ns())
        .await
        .unwrap()
        .unwrap();
    assert!(v1.meta.label.is_none());
    let by_label = records::get_by_label(
        &db.pool,
        RecordType::Card,
        "alice",
        "x",
        "baseline",
        &alice_ns(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(by_label.meta.seq, 2);

    // Anonymous does not see the private record by label either.
    assert!(
        records::get_by_label(&db.pool, RecordType::Card, "alice", "x", "baseline", &[])
            .await
            .unwrap()
            .is_none()
    );

    let err = records::set_label(
        &db.pool,
        RecordType::Card,
        "alice",
        "x",
        1,
        "123",
        Actor::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, StoreError::LabelInvalid), "got {err:?}");
    let err = records::set_label(
        &db.pool,
        RecordType::Card,
        "alice",
        "x",
        9,
        "nine",
        Actor::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, StoreError::VersionNotFound), "got {err:?}");
    let err = records::set_label(
        &db.pool,
        RecordType::Card,
        "alice",
        "nope",
        1,
        "x",
        Actor::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, StoreError::RecordNotFound), "got {err:?}");
}

#[tokio::test]
async fn unknown_namespace_is_a_typed_error() {
    let db = common::db().await;
    let body = json!({"v": 1});
    let h = common::sha256(b"n");
    let err = records::create_or_append(
        &db.pool,
        new_version("nobody", "x", &body, &h),
        ids,
        &no_badges,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, StoreError::NamespaceUnknown(ref ns) if ns == "nobody"),
        "got {err:?}"
    );
}

#[tokio::test]
async fn side_tables_are_written_and_read_back() {
    let db = common::db().await;
    alice(&db.pool).await;
    let sha = [3u8; 32];
    attachment_row(&db.pool, &sha, true).await;

    let body = json!({"title": "with sides"});
    let h = common::sha256(b"sides");
    let by = json!({"grader": "exact"});
    let attrs = json!({"runs": ["r1"]});
    let nv = NewVersion {
        fingerprints: &[("model", [1u8; 32]), ("task", [2u8; 32])],
        results: &[
            NewResult {
                ordinal: 0,
                metric: "core/pass_rate",
                aggregation: "mean",
                value: 0.5,
                n: Some(33),
                by: Some(&by),
            },
            NewResult {
                ordinal: 1,
                metric: "alice/custom",
                aggregation: "custom",
                value: 1.0,
                n: None,
                by: None,
            },
        ],
        relations: &[NewRelation {
            relation_type: "core/uses_eval",
            target: RelationTarget::External("hf:org/repo@abc"),
            attrs: Some(&attrs),
        }],
        attachments: &[NewAttachmentRef {
            path: "samples.jsonl",
            sha256: sha,
        }],
        ..new_version("alice", "x", &body, &h)
    };
    let out = records::create_or_append(&db.pool, nv, ids, &no_badges)
        .await
        .unwrap();
    let version_id = out.meta().version_id;

    let fps = records::load_fingerprints(&db.pool, version_id)
        .await
        .unwrap();
    let expected: BTreeMap<String, Vec<u8>> = [
        ("model".to_owned(), vec![1u8; 32]),
        ("task".to_owned(), vec![2u8; 32]),
    ]
    .into_iter()
    .collect();
    assert_eq!(fps, expected);
    assert!(
        records::load_fingerprints(&db.pool, Uuid::new_v4())
            .await
            .unwrap()
            .is_empty()
    );

    let results: Vec<(i32, String, String, f64, Option<i32>, Option<Value>)> = sqlx::query_as(
        "SELECT ordinal, metric, aggregation, value, n, by FROM results WHERE version_id = $1 ORDER BY ordinal",
    )
    .bind(version_id)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].1, "core/pass_rate");
    assert_eq!(results[0].3, 0.5);
    assert_eq!(results[0].4, Some(33));
    assert_eq!(results[0].5, Some(by));
    assert_eq!(results[1].4, None);

    let rels: Vec<(String, Option<Uuid>, Option<String>, Option<Value>)> = sqlx::query_as(
        "SELECT type, to_version_id, to_external, attrs FROM relations WHERE from_version_id = $1",
    )
    .bind(version_id)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(rels.len(), 1);
    assert_eq!(rels[0].0, "core/uses_eval");
    assert!(rels[0].1.is_none());
    assert_eq!(rels[0].2.as_deref(), Some("hf:org/repo@abc"));
    assert_eq!(rels[0].3, Some(attrs));

    let refs: Vec<(Vec<u8>, String)> =
        sqlx::query_as("SELECT sha256, path FROM attachment_refs WHERE version_id = $1")
            .bind(version_id)
            .fetch_all(&db.pool)
            .await
            .unwrap();
    assert_eq!(refs, vec![(sha.to_vec(), "samples.jsonl".to_owned())]);
}

#[tokio::test]
async fn unready_attachment_blocks_the_write() {
    let db = common::db().await;
    alice(&db.pool).await;
    let ready = [1u8; 32];
    let pending = [2u8; 32];
    let unknown = [3u8; 32];
    attachment_row(&db.pool, &ready, true).await;
    attachment_row(&db.pool, &pending, false).await;

    let body = json!({"title": "blocked"});
    let h = common::sha256(b"blocked");
    let nv = NewVersion {
        attachments: &[
            NewAttachmentRef {
                path: "a",
                sha256: ready,
            },
            NewAttachmentRef {
                path: "b",
                sha256: pending,
            },
            NewAttachmentRef {
                path: "c",
                sha256: unknown,
            },
            NewAttachmentRef {
                path: "d",
                sha256: unknown,
            },
        ],
        ..new_version("alice", "x", &body, &h)
    };
    let err = records::create_or_append(&db.pool, nv, ids, &no_badges)
        .await
        .unwrap_err();
    let StoreError::AttachmentMissing(missing) = err else {
        panic!("got {err:?}");
    };
    assert_eq!(missing, vec![pending, unknown], "input order, deduplicated");

    // Nothing was written, not even the record row.
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM records")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count.0, 0);
}

#[tokio::test]
async fn relations_resolve_to_versions_or_stay_textual() {
    let db = common::db().await;
    alice(&db.pool).await;
    let eval_body = json!({"eval_kind": "run_set"});
    let eh = common::sha256(b"eval");
    let eval = records::create_or_append(
        &db.pool,
        NewVersion {
            record_type: RecordType::Eval,
            ..new_version("alice", "e", &eval_body, &eh)
        },
        ids,
        &no_badges,
    )
    .await
    .unwrap();
    let eval_version = eval.meta().version_id;

    let body = json!({"title": "card"});
    let h = common::sha256(b"card1");
    let nv = NewVersion {
        relations: &[
            NewRelation {
                relation_type: "core/uses_eval",
                target: RelationTarget::Version {
                    ns: "alice",
                    name: "e",
                    seq: 1,
                    record_type: Some(RecordType::Eval),
                },
                attrs: None,
            },
            NewRelation {
                relation_type: "core/uses_eval",
                target: RelationTarget::Version {
                    ns: "alice",
                    name: "missing",
                    seq: 1,
                    record_type: None,
                },
                attrs: None,
            },
            NewRelation {
                relation_type: "core/baseline_of",
                target: RelationTarget::External("external:https://example.org/x"),
                attrs: None,
            },
        ],
        ..new_version("alice", "c", &body, &h)
    };
    let out = records::create_or_append(&db.pool, nv, ids, &no_badges)
        .await
        .unwrap();
    let CreateOutcome::Created { meta, facts } = out else {
        panic!("created");
    };
    assert!(!facts.all_refs_resolved, "one hub target did not resolve");
    let rels: Vec<(String, Option<Uuid>, Option<String>)> = sqlx::query_as(
        "SELECT type, to_version_id, to_external FROM relations WHERE from_version_id = $1 ORDER BY to_external NULLS FIRST",
    )
    .bind(meta.version_id)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(rels.len(), 3);
    assert_eq!(rels[0].1, Some(eval_version));
    assert!(rels[0].2.is_none());
    assert_eq!(rels[1].2.as_deref(), Some("alice/missing@1"));
    assert!(rels[1].1.is_none());
    assert_eq!(rels[2].2.as_deref(), Some("external:https://example.org/x"));

    // Only resolvable hub targets: the fact is true. A kind-less target
    // resolves while the name is unambiguous.
    let h2 = common::sha256(b"card2");
    let body2 = json!({"title": "card 2"});
    let nv2 = NewVersion {
        relations: &[NewRelation {
            relation_type: "core/uses_eval",
            target: RelationTarget::Version {
                ns: "alice",
                name: "e",
                seq: 1,
                record_type: None,
            },
            attrs: None,
        }],
        ..new_version("alice", "c", &body2, &h2)
    };
    let CreateOutcome::Created { facts, .. } =
        records::create_or_append(&db.pool, nv2, ids, &no_badges)
            .await
            .unwrap()
    else {
        panic!("created");
    };
    assert!(facts.all_refs_resolved);

    // A Card named alice/e makes the kind-less reference ambiguous.
    let h3 = common::sha256(b"card-e");
    let body3 = json!({"title": "a card called e"});
    post(&db.pool, "alice", "e", &body3, &h3).await;
    let h4 = common::sha256(b"card3");
    let body4 = json!({"title": "card 3"});
    let nv4 = NewVersion {
        relations: &[NewRelation {
            relation_type: "core/uses_eval",
            target: RelationTarget::Version {
                ns: "alice",
                name: "e",
                seq: 1,
                record_type: None,
            },
            attrs: None,
        }],
        ..new_version("alice", "c", &body4, &h4)
    };
    let CreateOutcome::Created { facts, .. } =
        records::create_or_append(&db.pool, nv4, ids, &no_badges)
            .await
            .unwrap()
    else {
        panic!("created");
    };
    assert!(!facts.all_refs_resolved, "ambiguous name stays unresolved");
    let h5 = common::sha256(b"card4");
    let body5 = json!({"title": "card 4"});
    let nv5 = NewVersion {
        relations: &[NewRelation {
            relation_type: "core/uses_eval",
            target: RelationTarget::Version {
                ns: "alice",
                name: "e",
                seq: 1,
                record_type: Some(RecordType::Eval),
            },
            attrs: None,
        }],
        ..new_version("alice", "c", &body5, &h5)
    };
    let CreateOutcome::Created { facts, .. } =
        records::create_or_append(&db.pool, nv5, ids, &no_badges)
            .await
            .unwrap()
    else {
        panic!("created");
    };
    assert!(facts.all_refs_resolved, "the kind disambiguates");
}

#[tokio::test]
async fn badges_come_from_the_callers_rule_over_the_stores_facts() {
    let db = common::db().await;
    alice(&db.pool).await;
    // `core/pass_rate` is seeded by the core-registry migration; only the
    // harness needs registering here.
    sqlx::query(
        "INSERT INTO registry (kind, ns, id, version, body) VALUES
         ('harnesses', 'acme', 'bench', '1.0', '{}')",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let body = json!({"harness": {"name": "acme/bench", "version": "1.0"}});
    let h = common::sha256(b"badged");
    let nv = NewVersion {
        results: &[NewResult {
            ordinal: 0,
            metric: "core/pass_rate",
            aggregation: "mean",
            value: 0.1,
            n: None,
            by: None,
        }],
        ..new_version("alice", "x", &body, &h)
    };
    let rule = |f: &IngestFacts| {
        let mut b = Vec::new();
        if f.harness_registered {
            b.push("harness_registered".to_owned());
        }
        if f.all_metrics_registered {
            b.push("metric_registered".to_owned());
        }
        if f.all_refs_resolved {
            b.push("refs_resolved".to_owned());
        }
        b
    };
    let out = records::create_or_append(&db.pool, nv, ids, &rule)
        .await
        .unwrap();
    let CreateOutcome::Created { meta, facts } = out else {
        panic!("created");
    };
    assert!(facts.harness_registered);
    assert!(facts.all_metrics_registered);
    assert_eq!(
        meta.badges,
        vec!["harness_registered", "metric_registered", "refs_resolved"]
    );
    let got = records::get_latest(&db.pool, RecordType::Card, "alice", "x", &alice_ns())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.meta.badges, meta.badges);

    // An unregistered metric or a bare harness name: not registered.
    let body2 = json!({"harness": {"name": "bench", "version": "1.0"}});
    let h2 = common::sha256(b"unbadged");
    let nv2 = NewVersion {
        results: &[NewResult {
            ordinal: 0,
            metric: "alice/mine",
            aggregation: "mean",
            value: 0.1,
            n: None,
            by: None,
        }],
        ..new_version("alice", "y", &body2, &h2)
    };
    let CreateOutcome::Created { facts, .. } = records::create_or_append(&db.pool, nv2, ids, &rule)
        .await
        .unwrap()
    else {
        panic!("created");
    };
    assert!(!facts.harness_registered);
    assert!(!facts.all_metrics_registered);
}

#[tokio::test]
async fn tombstone_keeps_the_facts_and_drops_the_body() {
    let db = common::db().await;
    alice(&db.pool).await;
    let sha = [5u8; 32];
    attachment_row(&db.pool, &sha, true).await;
    let b1 = json!({"v": 1});
    let h1 = common::sha256(b"t1");
    let b2 = json!({"v": 2});
    let h2 = common::sha256(b"t2");
    post(&db.pool, "alice", "x", &b1, &h1).await;
    let v2 = records::create_or_append(
        &db.pool,
        NewVersion {
            fingerprints: &[("model", [9u8; 32])],
            attachments: &[NewAttachmentRef {
                path: "f",
                sha256: sha,
            }],
            ..new_version("alice", "x", &b2, &h2)
        },
        ids,
        &no_badges,
    )
    .await
    .unwrap();
    let v2_id = v2.meta().version_id;

    let meta = records::tombstone(
        &db.pool,
        RecordType::Card,
        "alice",
        "x",
        2,
        TombstoneRequest {
            reason: TombstoneReason::Withdrawn,
            note: Some("wrong seed"),
        },
        Actor::default(),
    )
    .await
    .unwrap();
    assert_eq!(meta.version_id, v2_id);
    assert_eq!(meta.content_hash, h2.to_vec());
    assert_eq!(meta.changed, vec!["v"]);

    // Latest skips it; @2 shows the tombstone and no body.
    let latest = records::get_latest(&db.pool, RecordType::Card, "alice", "x", &alice_ns())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.meta.seq, 1);
    let v2 = records::get_by_seq(&db.pool, RecordType::Card, "alice", "x", 2, &alice_ns())
        .await
        .unwrap()
        .unwrap();
    assert!(v2.body.is_none());
    let ts = v2.tombstone.expect("tombstoned");
    assert_eq!(ts.reason, TombstoneReason::Withdrawn);
    assert_eq!(ts.note.as_deref(), Some("wrong seed"));
    assert_eq!(v2.meta.content_hash, h2.to_vec());

    // Fingerprints stay, attachment refs go.
    assert_eq!(
        records::load_fingerprints(&db.pool, v2_id)
            .await
            .unwrap()
            .len(),
        1
    );
    let refs: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM attachment_refs WHERE version_id = $1")
        .bind(v2_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(refs.0, 0);

    // The version list shows both, with the tombstone on the second.
    let versions = records::list_versions(&db.pool, RecordType::Card, "alice", "x", &alice_ns())
        .await
        .unwrap()
        .expect("visible");
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].meta.seq, 1);
    assert!(versions[0].tombstone.is_none());
    assert_eq!(versions[1].meta.seq, 2);
    assert!(versions[1].tombstone.is_some());
    assert!(
        records::list_versions(&db.pool, RecordType::Card, "alice", "x", &[])
            .await
            .unwrap()
            .is_none(),
        "private: the list is not even known to exist"
    );

    // seq is not reused after a tombstone.
    let b3 = json!({"v": 3});
    let h3 = common::sha256(b"t3");
    assert_eq!(post(&db.pool, "alice", "x", &b3, &h3).await.meta().seq, 3);

    let err = records::tombstone(
        &db.pool,
        RecordType::Card,
        "alice",
        "x",
        2,
        TombstoneRequest {
            reason: TombstoneReason::Other,
            note: None,
        },
        Actor::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, StoreError::AlreadyTombstoned), "got {err:?}");
    let err = records::tombstone(
        &db.pool,
        RecordType::Card,
        "alice",
        "x",
        9,
        TombstoneRequest {
            reason: TombstoneReason::Other,
            note: None,
        },
        Actor::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, StoreError::VersionNotFound), "got {err:?}");
}

#[tokio::test]
async fn listing_filters_sorts_and_pages() {
    let db = common::db().await;
    alice(&db.pool).await;
    auth::ensure_namespace(&db.pool, "bob", NamespaceKind::User)
        .await
        .unwrap();
    for (i, name) in ["delta", "alpha", "echo", "bravo", "charlie"]
        .iter()
        .enumerate()
    {
        let body = json!({"title": format!("Title {name}")});
        let h = common::sha256(format!("list{i}").as_bytes());
        post(&db.pool, "alice", name, &body, &h).await;
    }
    let bob_private = json!({"title": "bob private"});
    post(
        &db.pool,
        "bob",
        "secret",
        &bob_private,
        &common::sha256(b"bs"),
    )
    .await;
    let bob_public = json!({"title": "alpha from bob"});
    post(
        &db.pool,
        "bob",
        "shared",
        &bob_public,
        &common::sha256(b"bp"),
    )
    .await;
    records::set_visibility(
        &db.pool,
        RecordType::Card,
        "bob",
        "shared",
        Visibility::Public,
        Actor::default(),
    )
    .await
    .unwrap();
    // A record whose only version is tombstoned does not list.
    post(
        &db.pool,
        "alice",
        "gone",
        &json!({"title": "gone"}),
        &common::sha256(b"gone"),
    )
    .await;
    records::tombstone(
        &db.pool,
        RecordType::Card,
        "alice",
        "gone",
        1,
        TombstoneRequest {
            reason: TombstoneReason::Withdrawn,
            note: None,
        },
        Actor::default(),
    )
    .await
    .unwrap();

    let params = |sort, cursor, limit| ListParams {
        record_type: RecordType::Card,
        ns: None,
        search: None,
        sort,
        cursor,
        limit,
    };

    // Alice sees her five plus bob's public one; bob's private is hidden.
    let (items, next) = records::list(&db.pool, params(ListSort::NameAsc, None, 50), &alice_ns())
        .await
        .unwrap();
    assert!(next.is_none());
    let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["alpha", "bravo", "charlie", "delta", "echo", "shared"]
    );
    assert_eq!(items[0].title.as_deref(), Some("Title alpha"));
    assert_eq!(items[5].ns, "bob");
    assert_eq!(items[5].visibility, Visibility::Public);

    // Anonymous sees only the public one.
    let (items, _) = records::list(&db.pool, params(ListSort::NameAsc, None, 50), &[])
        .await
        .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "shared");

    // Namespace filter and search (name or title, case-insensitive).
    let (items, _) = records::list(
        &db.pool,
        ListParams {
            ns: Some("alice"),
            ..params(ListSort::NameAsc, None, 50)
        },
        &alice_ns(),
    )
    .await
    .unwrap();
    assert_eq!(items.len(), 5);
    let (items, _) = records::list(
        &db.pool,
        ListParams {
            search: Some("ALPHA"),
            ..params(ListSort::NameAsc, None, 50)
        },
        &alice_ns(),
    )
    .await
    .unwrap();
    let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, vec!["alpha", "shared"], "name match and title match");
    let (items, _) = records::list(
        &db.pool,
        ListParams {
            search: Some("%"),
            ..params(ListSort::NameAsc, None, 50)
        },
        &alice_ns(),
    )
    .await
    .unwrap();
    assert!(items.is_empty(), "wildcards are literal");

    // Created order, both directions.
    let (desc, _) = records::list(
        &db.pool,
        params(ListSort::CreatedDesc, None, 50),
        &alice_ns(),
    )
    .await
    .unwrap();
    let (asc, _) = records::list(
        &db.pool,
        params(ListSort::CreatedAsc, None, 50),
        &alice_ns(),
    )
    .await
    .unwrap();
    let mut rev: Vec<String> = asc.iter().map(|i| i.name.clone()).collect();
    rev.reverse();
    let d: Vec<String> = desc.iter().map(|i| i.name.clone()).collect();
    assert_eq!(d, rev);
    assert_eq!(asc[0].name, "delta", "first created");

    // Keyset paging: three pages of two, no repeats, no gaps.
    let mut seen = Vec::new();
    let mut cursor: Option<ListCursor> = None;
    let mut pages = 0;
    loop {
        let (items, next) = records::list(
            &db.pool,
            params(ListSort::CreatedDesc, cursor.clone(), 2),
            &alice_ns(),
        )
        .await
        .unwrap();
        pages += 1;
        seen.extend(items.iter().map(|i| i.name.clone()));
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(seen, d);

    let mut seen = Vec::new();
    let mut cursor: Option<ListCursor> = None;
    loop {
        let (items, next) = records::list(
            &db.pool,
            params(ListSort::NameAsc, cursor.clone(), 4),
            &alice_ns(),
        )
        .await
        .unwrap();
        seen.extend(items.iter().map(|i| i.name.clone()));
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(
        seen,
        vec!["alpha", "bravo", "charlie", "delta", "echo", "shared"]
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
    records::create_or_append(&db.pool, nv, ids, &no_badges)
        .await
        .unwrap();
    // idempotent hit: no audit row
    records::create_or_append(&db.pool, nv, ids, &no_badges)
        .await
        .unwrap();
    let body2 = json!({"v": 2});
    let h2 = common::sha256(b"a2");
    let mut nv2 = new_version("alice", "x", &body2, &h2);
    nv2.actor = actor;
    records::create_or_append(&db.pool, nv2, ids, &no_badges)
        .await
        .unwrap();
    records::set_label(&db.pool, RecordType::Card, "alice", "x", 2, "lbl", actor)
        .await
        .unwrap();
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
    records::tombstone(
        &db.pool,
        RecordType::Card,
        "alice",
        "x",
        1,
        TombstoneRequest {
            reason: TombstoneReason::Duplicate,
            note: None,
        },
        actor,
    )
    .await
    .unwrap();

    let rows: Vec<(String, Option<String>, Option<Uuid>, Option<Uuid>, Option<String>)> =
        sqlx::query_as(
            "SELECT action, subject, actor_user_id, actor_token_id, ns FROM audit WHERE ns = 'alice' ORDER BY id",
        )
        .fetch_all(&db.pool)
        .await
        .unwrap();
    let token_id_text = token_id.to_string();
    let actions: Vec<(&str, Option<&str>)> = rows
        .iter()
        .map(|r| (r.0.as_str(), r.1.as_deref()))
        .collect();
    assert_eq!(
        actions,
        vec![
            ("token.create", Some(token_id_text.as_str())),
            ("record.create", Some("card/alice/x@1")),
            ("version.append", Some("card/alice/x@2")),
            ("label.set", Some("card/alice/x@2")),
            ("settings.visibility", Some("card/alice/x")),
            ("version.tombstone", Some("card/alice/x@1")),
        ]
    );
    for r in &rows[1..] {
        assert_eq!(r.2, Some(user_id));
        assert_eq!(r.3, Some(token_id));
        assert_eq!(r.4.as_deref(), Some("alice"));
    }
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
    post(&db.pool, "dave", "x", &body, &h).await;

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
            // The namespace the core registry is published under.
            ("core".to_owned(), "org".to_owned()),
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
