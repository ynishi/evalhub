#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The registry against a real Postgres: the seeded `core/` vocabulary,
//! immutable entries, and the `applying → applied` transition that builds
//! an expression index.

mod common;

use evalhub_core::registry as core_registry;
use evalhub_query::ir::ValueType;
use evalhub_store::records::Actor;
use evalhub_store::registry::{self, Kind, State};
use evalhub_store::{error::StoreError, index};
use serde_json::json;

/// The migration and the Rust constants describe the same vocabulary.
/// This is the check that keeps them from drifting: the migration cannot
/// call Rust, so the test compares what it wrote with what ships.
#[tokio::test]
async fn migration_seeds_exactly_the_core_vocabulary() {
    let db = common::db().await;

    let metrics = registry::list(&db.pool, Some(Kind::Metrics), Some("core"), 100, 0)
        .await
        .unwrap();
    assert_eq!(metrics.len(), core_registry::CORE_METRICS.len());
    for def in core_registry::CORE_METRICS {
        let name = def.id.strip_prefix("core/").unwrap();
        let entry = metrics
            .iter()
            .find(|e| e.id == name)
            .unwrap_or_else(|| panic!("{} was not seeded", def.id));
        assert_eq!(entry.version, core_registry::CORE_VERSION);
        assert_eq!(entry.state, State::Applied);
        assert_eq!(entry.body["id"], def.id);
        assert_eq!(entry.body["lower_is_better"], def.lower_is_better);
        assert_eq!(entry.body["description"], def.description);
    }

    let relations = registry::list(&db.pool, Some(Kind::RelationTypes), Some("core"), 100, 0)
        .await
        .unwrap();
    assert_eq!(relations.len(), core_registry::CORE_RELATION_TYPES.len());
    for def in core_registry::CORE_RELATION_TYPES {
        let name = def.id.strip_prefix("core/").unwrap();
        let entry = relations
            .iter()
            .find(|e| e.id == name)
            .unwrap_or_else(|| panic!("{} was not seeded", def.id));
        assert_eq!(entry.body["id"], def.id);
        assert_eq!(entry.body["from"], def.from.as_str());
        assert_eq!(entry.body["to"], def.to.as_str());
        assert_eq!(entry.body["inverse"], def.inverse);
    }

    // And nothing else was seeded under `core`.
    let all = registry::list(&db.pool, None, Some("core"), 200, 0)
        .await
        .unwrap();
    assert_eq!(
        all.len(),
        core_registry::CORE_METRICS.len() + core_registry::CORE_RELATION_TYPES.len()
    );
}

/// An entry is written once and read back; a second write of the same
/// address is refused, and `core/` is refused outright.
#[tokio::test]
async fn entries_are_immutable_and_core_is_read_only() {
    let db = common::db().await;
    common::seed_version(&db.pool, "card", "alice", "x", 1, "private", "x").await;

    let body =
        json!({"name": "my-harness", "version": "0.4.1", "homepage": "https://example.test"});
    let entry = registry::put(
        &db.pool,
        Kind::Harnesses,
        "alice",
        "my-harness",
        "0.4.1",
        &body,
        Actor::default(),
    )
    .await
    .unwrap();
    assert_eq!(entry.state, State::Applied, "only ext_schemas apply later");
    assert_eq!(entry.address(), "harnesses/alice/my-harness@0.4.1");

    let read = registry::get(&db.pool, Kind::Harnesses, "alice", "my-harness", "0.4.1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.body, body);

    let again = registry::put(
        &db.pool,
        Kind::Harnesses,
        "alice",
        "my-harness",
        "0.4.1",
        &json!({"name": "my-harness", "version": "0.4.1"}),
        Actor::default(),
    )
    .await;
    assert!(
        matches!(again, Err(StoreError::RegistryEntryExists(ref a)) if a == "harnesses/alice/my-harness@0.4.1"),
        "{again:?}"
    );

    // A new version is how a definition changes.
    registry::put(
        &db.pool,
        Kind::Harnesses,
        "alice",
        "my-harness",
        "0.4.2",
        &json!({"name": "my-harness", "version": "0.4.2"}),
        Actor::default(),
    )
    .await
    .unwrap();

    let core = registry::put(
        &db.pool,
        Kind::Metrics,
        "core",
        "pass_rate",
        "2",
        &json!({"id": "core/pass_rate"}),
        Actor::default(),
    )
    .await;
    assert!(
        matches!(core, Err(StoreError::RegistryCoreReadOnly)),
        "{core:?}"
    );

    // The write was audited under the namespace that made it.
    let (rows, _) = evalhub_store::audit::list(&db.pool, "alice", None, 50)
        .await
        .unwrap();
    assert!(
        rows.iter().any(|r| r.action == "registry.put"
            && r.subject.as_deref() == Some("harnesses/alice/my-harness@0.4.1")),
        "{rows:?}"
    );
}

/// Registering an `ext_schema` leaves it `applying`; applying it builds
/// the expression indexes and flips the state, and a second apply has
/// nothing left to do.
#[tokio::test]
async fn ext_schema_applies_and_builds_its_indexes() {
    let db = common::db().await;
    common::seed_version(&db.pool, "card", "alice", "x", 1, "private", "x").await;

    let body = json!({
        "ns": "alice",
        "fingerprint": false,
        "schema": {
            "type": "object",
            "properties": {
                "rung": {"type": "integer"},
                "note": {"type": "string"},
                "samples": {"type": "array"}
            }
        }
    });
    let entry = registry::put(
        &db.pool,
        Kind::ExtSchemas,
        "alice",
        "qwen-loop",
        "1",
        &body,
        Actor::default(),
    )
    .await
    .unwrap();
    assert_eq!(entry.state, State::Applying);

    // While applying, the query side sees nothing.
    assert!(
        registry::ext_schemas_applied(&db.pool)
            .await
            .unwrap()
            .is_empty()
    );

    let pending = registry::ext_schemas(&db.pool, State::Applying)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].key, "alice/qwen-loop");
    assert_eq!(pending[0].paths.len(), 2, "the array is not indexable");

    let applied = index::apply_pending(&db.pool).await.unwrap();
    assert_eq!(applied, Some(1));

    let after = registry::get(&db.pool, Kind::ExtSchemas, "alice", "qwen-loop", "1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.state, State::Applied);

    // Both indexes exist and Postgres considers them usable.
    for path in &pending[0].paths {
        let name = index::index_name(&path.path);
        assert!(
            index::is_valid(&db.pool, &name).await.unwrap(),
            "{name} is not valid"
        );
    }

    // And the query side now has the typed paths.
    let visible = registry::ext_schemas_applied(&db.pool).await.unwrap();
    assert_eq!(visible.len(), 1);
    let rung = visible[0]
        .paths
        .iter()
        .find(|p| p.path.last().unwrap() == "rung")
        .unwrap();
    assert_eq!(rung.ty, ValueType::Number);
    assert_eq!(
        rung.path,
        vec![
            "ext".to_string(),
            "alice/qwen-loop".to_string(),
            "rung".to_string()
        ]
    );

    // A second sweep finds nothing pending.
    assert_eq!(index::apply_pending(&db.pool).await.unwrap(), Some(0));
}

/// The badge-recomputation job asks which versions cite a newly
/// registered harness or metric.
#[tokio::test]
async fn citing_versions_are_found_for_badge_recomputation() {
    let db = common::db().await;
    let (_, version_id) =
        common::seed_version(&db.pool, "card", "alice", "x", 1, "private", "x").await;
    sqlx::query(
        "UPDATE versions SET body = body || '{\"harness\": {\"name\": \"my-harness\", \"version\": \"0.4.1\"}}'::jsonb WHERE version_id = $1",
    )
    .bind(version_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO results (version_id, ordinal, metric, aggregation, value) VALUES ($1, 0, 'alice/custom', 'mean', 0.5)",
    )
    .bind(version_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let citing = registry::versions_citing_harness(&db.pool, "my-harness", "0.4.1")
        .await
        .unwrap();
    assert_eq!(citing, vec![version_id]);
    assert!(
        registry::versions_citing_harness(&db.pool, "my-harness", "9.9.9")
            .await
            .unwrap()
            .is_empty()
    );

    let citing = registry::versions_citing_metric(&db.pool, "alice/custom")
        .await
        .unwrap();
    assert_eq!(citing, vec![version_id]);

    registry::set_badges(&db.pool, version_id, &["metric_registered".to_string()])
        .await
        .unwrap();
    let badges: Vec<String> =
        sqlx::query_scalar("SELECT badges FROM versions WHERE version_id = $1")
            .bind(version_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(badges, vec!["metric_registered".to_string()]);
}
