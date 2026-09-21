//! Registry entries and the `applying → applied` transition.
//!
//! `registry (kind, ns, id, version, body jsonb, state)`, unique on
//! `(kind, ns, id, version)`. Entries are immutable; a change is a new
//! version. `core/` entries are seeded from `evalhub_core::registry` by the
//! second migration and rejected on `PUT`.
//!
//! `PUT /registry/{kind}/{ns}/{id}@{version}` requires `write` on `ns`.
//! For `metrics`, `harnesses` and `relation_types` it is a plain insert and
//! returns `201`. For `ext_schemas` it returns `202` with `state:
//! applying`, and [`crate::index`] is asked to create the expression
//! indexes for the schema's typed paths; the state becomes `applied` when
//! every index reports valid. The read side reports the state so the UI
//! and the query type check can tell the caller why a path is not yet
//! sortable.
//!
//! Registering a harness or metric also enqueues a badge recomputation for
//! versions that cite it (`harness_registered` / `metric_registered`), so
//! that badges awarded by the registry follow the registry.
//! [`versions_citing_harness`] and [`versions_citing_metric`] are what that
//! job asks; the badge rules themselves stay in `evalhub_core::badge`,
//! because the store has no opinion about what a badge means.
//!
//! # Typed paths of an `ext_schema`
//!
//! An `ext_schema` body is `{ ns, schema, fingerprint }` where `schema` is
//! a JSON Schema for one namespace's extension object. The indexable paths
//! are the leaves of its `properties` whose `type` is `number`, `string` or
//! `boolean`; a nested object recurses, and anything else (an array, a
//! union, a `$ref`) is recorded as not indexable rather than guessed at.
//! Each path is rooted at `{ext, {ns}/{id}, …}`, which is where the record
//! carries it.

use chrono::{DateTime, Utc};
use evalhub_query::ir::ValueType;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::audit::{NewAudit, append};
use crate::error::{SQLSTATE_UNIQUE, StoreError, violated_constraint};
use crate::records::Actor;

/// The namespace whose entries ship with the hub and cannot be written
/// over the API.
pub const CORE_NS: &str = "core";

/// What a registry entry defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `{ id, lower_is_better, description }` — the `metric_registered`
    /// badge and the sort direction.
    Metrics,
    /// `{ name, version, homepage }` — the `harness_registered` badge.
    Harnesses,
    /// `{ from, to, inverse }` — relation validation and traversal.
    RelationTypes,
    /// `{ ns, schema, fingerprint }` — typed `ext` indexing.
    ExtSchemas,
}

impl Kind {
    /// The `kind` column's value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Metrics => "metrics",
            Self::Harnesses => "harnesses",
            Self::RelationTypes => "relation_types",
            Self::ExtSchemas => "ext_schemas",
        }
    }

    /// Parse a `kind` from the column or a URL segment.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "metrics" => Some(Self::Metrics),
            "harnesses" => Some(Self::Harnesses),
            "relation_types" => Some(Self::RelationTypes),
            "ext_schemas" => Some(Self::ExtSchemas),
            _ => None,
        }
    }
}

/// Whether an entry's indexes are in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// The entry exists and its indexes are still being built. Its paths
    /// behave as unregistered meanwhile.
    Applying,
    /// Every index the entry implies exists and is valid.
    Applied,
}

impl State {
    /// The `state` column's value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Applying => "applying",
            Self::Applied => "applied",
        }
    }

    /// Parse a `state` from the column. An unknown value reads as
    /// `applying`, the state that grants nothing.
    pub fn parse(s: &str) -> Self {
        if s == "applied" {
            Self::Applied
        } else {
            Self::Applying
        }
    }
}

/// One registry entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// What it defines.
    pub kind: Kind,
    /// Namespace that published it.
    pub ns: String,
    /// Its id within the namespace.
    pub id: String,
    /// Its version. Entries are immutable; a change is a new version.
    pub version: String,
    /// The definition.
    pub body: Value,
    /// Whether its indexes are in place.
    pub state: State,
    /// When it was published.
    pub created_at: DateTime<Utc>,
}

impl Entry {
    /// `{kind}/{ns}/{id}@{version}`, which is how an entry is addressed
    /// and how errors name it.
    pub fn address(&self) -> String {
        format!(
            "{}/{}/{}@{}",
            self.kind.as_str(),
            self.ns,
            self.id,
            self.version
        )
    }
}

/// A typed path an `ext_schema` makes indexable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtPath {
    /// Segments from the document root: `["ext", "{ns}/{id}", …]`.
    pub path: Vec<String>,
    /// The type the schema declares, which picks the cast helper.
    pub ty: ValueType,
}

/// An applied `ext_schema`, in the shape the query crate's path table
/// wants.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtSchema {
    /// The `ext` key these paths hang under: `{ns}/{id}`.
    pub key: String,
    /// The registry version they were derived from.
    pub version: String,
    /// Whether the namespace asked for its keys to join the facet
    /// fingerprint.
    pub fingerprint: bool,
    /// The paths, in declaration order.
    pub paths: Vec<ExtPath>,
}

/// One entry as the row reads.
fn row_to_entry(
    kind: String,
    ns: String,
    id: String,
    version: String,
    body: Value,
    state: String,
    created_at: DateTime<Utc>,
) -> Result<Entry, StoreError> {
    Ok(Entry {
        kind: Kind::parse(&kind)
            .ok_or_else(|| StoreError::QueryUnsupported(format!("unknown registry kind {kind}")))?,
        ns,
        id,
        version,
        body,
        state: State::parse(&state),
        created_at,
    })
}

/// One entry by address, or `None`.
pub async fn get(
    pool: &PgPool,
    kind: Kind,
    ns: &str,
    id: &str,
    version: &str,
) -> Result<Option<Entry>, StoreError> {
    let row = sqlx::query!(
        "SELECT kind, ns, id, version, body, state, created_at
         FROM registry
         WHERE kind = $1 AND ns = $2 AND id = $3 AND version = $4",
        kind.as_str(),
        ns,
        id,
        version,
    )
    .fetch_optional(pool)
    .await?;
    row.map(|r| row_to_entry(r.kind, r.ns, r.id, r.version, r.body, r.state, r.created_at))
        .transpose()
}

/// Entries of `kind`, optionally of one namespace, newest first.
///
/// Paging is by offset: the registry is a catalogue of definitions, not a
/// growing log, so there is no keyset here.
pub async fn list(
    pool: &PgPool,
    kind: Option<Kind>,
    ns: Option<&str>,
    limit: u32,
    offset: u32,
) -> Result<Vec<Entry>, StoreError> {
    let rows = sqlx::query!(
        "SELECT kind, ns, id, version, body, state, created_at
         FROM registry
         WHERE ($1::text IS NULL OR kind = $1)
           AND ($2::text IS NULL OR ns = $2)
         ORDER BY kind, ns, id, version
         LIMIT $3 OFFSET $4",
        kind.map(Kind::as_str),
        ns,
        i64::from(limit.max(1)),
        i64::from(offset),
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| row_to_entry(r.kind, r.ns, r.id, r.version, r.body, r.state, r.created_at))
        .collect()
}

/// Publish an entry.
///
/// `ext_schemas` land in [`State::Applying`] and every other kind in
/// [`State::Applied`], because only an `ext_schema` implies an index. The
/// write is audited under `ns`.
///
/// Errors: [`StoreError::RegistryCoreReadOnly`] for the `core/` namespace,
/// [`StoreError::RegistryEntryExists`] when the address is taken.
pub async fn put(
    pool: &PgPool,
    kind: Kind,
    ns: &str,
    id: &str,
    version: &str,
    body: &Value,
    actor: Actor,
) -> Result<Entry, StoreError> {
    if ns == CORE_NS {
        return Err(StoreError::RegistryCoreReadOnly);
    }
    let state = match kind {
        Kind::ExtSchemas => State::Applying,
        _ => State::Applied,
    };
    let mut tx = pool.begin().await?;
    let inserted = sqlx::query!(
        "INSERT INTO registry (kind, ns, id, version, body, state)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING created_at",
        kind.as_str(),
        ns,
        id,
        version,
        body,
        state.as_str(),
    )
    .fetch_one(&mut *tx)
    .await;
    let created_at = match inserted {
        Ok(row) => row.created_at,
        Err(e) => {
            if let Some((code, _)) = violated_constraint(&e)
                && code == SQLSTATE_UNIQUE
            {
                return Err(StoreError::RegistryEntryExists(format!(
                    "{}/{ns}/{id}@{version}",
                    kind.as_str()
                )));
            }
            return Err(e.into());
        }
    };
    let subject = format!("{}/{ns}/{id}@{version}", kind.as_str());
    append(
        &mut tx,
        NewAudit {
            actor,
            ns: Some(ns),
            action: "registry.put",
            subject: Some(&subject),
            detail: Some(serde_json::json!({ "state": state.as_str() })),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Entry {
        kind,
        ns: ns.to_owned(),
        id: id.to_owned(),
        version: version.to_owned(),
        body: body.clone(),
        state,
        created_at,
    })
}

/// Move an entry's state. Used by the index job when every index it
/// implies reports valid.
pub async fn set_state(
    pool: &PgPool,
    kind: Kind,
    ns: &str,
    id: &str,
    version: &str,
    state: State,
) -> Result<(), StoreError> {
    let done = sqlx::query!(
        "UPDATE registry SET state = $5
         WHERE kind = $1 AND ns = $2 AND id = $3 AND version = $4",
        kind.as_str(),
        ns,
        id,
        version,
        state.as_str(),
    )
    .execute(pool)
    .await?;
    if done.rows_affected() == 0 {
        return Err(StoreError::RegistryEntryNotFound(format!(
            "{}/{ns}/{id}@{version}",
            kind.as_str()
        )));
    }
    Ok(())
}

/// Every `ext_schema` in a given state, as typed paths.
///
/// The query crate builds its path table from the [`State::Applied`] ones;
/// the index job asks for the [`State::Applying`] ones.
pub async fn ext_schemas(pool: &PgPool, state: State) -> Result<Vec<ExtSchema>, StoreError> {
    let rows = sqlx::query!(
        "SELECT ns, id, version, body FROM registry
         WHERE kind = 'ext_schemas' AND state = $1
         ORDER BY ns, id, version",
        state.as_str(),
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| {
            let key = format!("{}/{}", r.ns, r.id);
            let address = format!("ext_schemas/{}/{}@{}", r.ns, r.id, r.version);
            let paths =
                typed_paths(&key, &r.body).map_err(|e| StoreError::ExtSchemaInvalid(address, e))?;
            Ok(ExtSchema {
                key,
                version: r.version,
                fingerprint: r.body.get("fingerprint").and_then(Value::as_bool) == Some(true),
                paths,
            })
        })
        .collect()
}

/// The applied `ext_schemas`, which is what the query type check reads.
pub async fn ext_schemas_applied(pool: &PgPool) -> Result<Vec<ExtSchema>, StoreError> {
    ext_schemas(pool, State::Applied).await
}

/// Derive the indexable paths of one `ext_schema` body.
///
/// Walks `schema.properties`, taking `number` / `string` / `boolean`
/// leaves and recursing into `object`. Everything else is skipped: an
/// array or a union has no single expression index, and guessing one would
/// mean a query that silently scans.
pub fn typed_paths(key: &str, body: &Value) -> Result<Vec<ExtPath>, String> {
    let schema = body
        .get("schema")
        .ok_or_else(|| "body has no `schema`".to_string())?;
    let mut out = Vec::new();
    walk(
        schema,
        &mut vec!["ext".to_string(), key.to_string()],
        &mut out,
    );
    Ok(out)
}

fn walk(schema: &Value, prefix: &mut Vec<String>, out: &mut Vec<ExtPath>) {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return;
    };
    for (name, property) in properties {
        let ty = property.get("type").and_then(Value::as_str);
        prefix.push(name.clone());
        match ty {
            Some("number") | Some("integer") => out.push(ExtPath {
                path: prefix.clone(),
                ty: ValueType::Number,
            }),
            Some("string") => out.push(ExtPath {
                path: prefix.clone(),
                ty: ValueType::String,
            }),
            Some("boolean") => out.push(ExtPath {
                path: prefix.clone(),
                ty: ValueType::Boolean,
            }),
            Some("object") => walk(property, prefix, out),
            _ => {}
        }
        prefix.pop();
    }
}

/// Versions whose `harness` is `name@version`, so their
/// `harness_registered` badge can be recomputed.
pub async fn versions_citing_harness(
    pool: &PgPool,
    name: &str,
    version: &str,
) -> Result<Vec<Uuid>, StoreError> {
    let rows = sqlx::query_scalar!(
        "SELECT version_id FROM versions
         WHERE tombstoned_at IS NULL AND harness_name = $1 AND harness_version = $2",
        name,
        version,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Versions with a result reporting `metric`, so their
/// `metric_registered` badge can be recomputed.
pub async fn versions_citing_metric(pool: &PgPool, metric: &str) -> Result<Vec<Uuid>, StoreError> {
    let rows = sqlx::query_scalar!(
        "SELECT DISTINCT r.version_id FROM results r
         JOIN versions v ON v.version_id = r.version_id
         WHERE v.tombstoned_at IS NULL AND r.metric = $1",
        metric,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Replace a version's badge set, which is what the recomputation job
/// writes after asking `evalhub_core::badge` for the new one.
pub async fn set_badges(
    pool: &PgPool,
    version_id: Uuid,
    badges: &[String],
) -> Result<(), StoreError> {
    sqlx::query!(
        "UPDATE versions SET badges = $2 WHERE version_id = $1",
        version_id,
        badges,
    )
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn kinds_and_states_round_trip() {
        for k in [
            Kind::Metrics,
            Kind::Harnesses,
            Kind::RelationTypes,
            Kind::ExtSchemas,
        ] {
            assert_eq!(Kind::parse(k.as_str()), Some(k));
        }
        assert_eq!(Kind::parse("nope"), None);
        assert_eq!(State::parse("applied"), State::Applied);
        assert_eq!(State::parse("applying"), State::Applying);
        assert_eq!(
            State::parse("something else"),
            State::Applying,
            "an unknown state grants nothing"
        );
    }

    #[test]
    fn typed_paths_take_scalar_leaves_and_recurse_into_objects() {
        let body = serde_json::json!({
            "ns": "alice",
            "fingerprint": true,
            "schema": {
                "type": "object",
                "properties": {
                    "rung": {"type": "integer"},
                    "label": {"type": "string"},
                    "done": {"type": "boolean"},
                    "samples": {"type": "array"},
                    "nested": {
                        "type": "object",
                        "properties": {"depth": {"type": "number"}}
                    }
                }
            }
        });
        let paths = typed_paths("alice/qwen-loop", &body).unwrap();
        let rendered: Vec<(Vec<String>, ValueType)> =
            paths.into_iter().map(|p| (p.path, p.ty)).collect();
        assert!(rendered.contains(&(
            vec!["ext".into(), "alice/qwen-loop".into(), "rung".into()],
            ValueType::Number
        )));
        assert!(rendered.contains(&(
            vec!["ext".into(), "alice/qwen-loop".into(), "label".into()],
            ValueType::String
        )));
        assert!(rendered.contains(&(
            vec!["ext".into(), "alice/qwen-loop".into(), "done".into()],
            ValueType::Boolean
        )));
        assert!(rendered.contains(&(
            vec![
                "ext".into(),
                "alice/qwen-loop".into(),
                "nested".into(),
                "depth".into()
            ],
            ValueType::Number
        )));
        assert!(
            !rendered
                .iter()
                .any(|(p, _)| p.last().is_some_and(|s| s == "samples")),
            "an array has no expression index"
        );
    }

    #[test]
    fn a_body_without_a_schema_is_refused() {
        assert!(typed_paths("alice/x", &serde_json::json!({"ns": "alice"})).is_err());
    }
}
