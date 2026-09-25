//! Relations: edges between versions, their resolution, traversal, and the
//! comparison view.
//!
//! # Storage
//!
//! `relations (from_version_id, type, to_version_id, to_external, attrs)`.
//! Edges are written from `relations[]` on ingest (in `records`, inside
//! the version's transaction) and can be added later with
//! `POST .../{name}@{seq}/relations` ([`add`]). A `core/uses_eval` edge
//! from a Card into an Eval also fixes the Card version's *used set* for
//! that Eval record (`card_eval_runs`), on ingest and on [`add`] alike;
//! see "A Card's runs" in [`crate::records`]. `to_version_id` is the
//! resolved target, or `NULL` with `to_external` set for targets outside
//! the hub and for references that did not resolve at write time. `type`
//! is a registry id; the inverse name is looked up from the registry at
//! read time, not stored.
//!
//! # Resolution
//!
//! `{ns}/{name}@{seq}` resolves to a `version_id` at ingest. A reference to
//! a name that does not exist yet, or a version that does not exist yet, is
//! stored unresolved (`to_version_id NULL`, `to_external` holding the
//! textual reference). The read side re-attempts resolution lazily
//! ([`outgoing`], [`incoming`], [`traverse`]) without writing, so a Card
//! published before its Eval links up once the Eval exists, while the
//! `refs_resolved` badge stays as recorded (a fact about ingest).
//!
//! Resolution only ever reaches a version the resolving party may see
//! ([`visible_to`]): the writer at write time, the reader at read time. A
//! version the writer may not see is stored unresolved, exactly like one
//! that does not exist, so neither the response nor the `refs_resolved`
//! badge tells the writer whether someone else's private version exists.
//! The lazy re-attempt on the read side resolves a textual target only
//! for a reader who may see it, so once the target is made public the edge
//! links up for everyone, and until then a reader without access sees the
//! text the writer typed and nothing more.
//!
//! An edge that did resolve at write time keeps its target. A reader who
//! may not see that target gets the commitment (`visible == false`: the
//! server reports `{ private: true, version_id, content_hash }`), and the
//! body of the version the edge leaves has the element removed for that
//! reader ([`hidden_targets`]). Tombstoned targets resolve too and are
//! flagged.
//!
//! # Traversal
//!
//! `GET .../{name}/relations?direction=out|in|both&types=&depth=&follow_latest=&version=`
//! walks the graph from a version, breadth-first, up to `depth` edges
//! (clamped to 1..=5). Edges attach to versions, so by default a new
//! version of a Card has no edges; `follow_latest=true` reports edges by
//! *name*: every node reached (the start included) is replaced by the
//! latest non-tombstoned version of its record before its edges are read,
//! so the graph shows what the names currently say about each other.
//! Private endpoints the caller cannot see are still nodes, with
//! `visible == false` and only `version_id` / `content_hash` meaningful.
//!
//! The judgements of a Card (`run_results[]`) are withheld the same way
//! when they name an Eval the reader may not see: [`hidden_evals`] says,
//! per Card version, which Evals and how many elements, so the read path
//! can remove them and report the count. Neither the Eval's name nor a run
//! id nor a verdict reaches such a reader.
//!
//! # The comparison view
//!
//! `GET /evals/{ns}/{name}/cards?version=&group_by=fingerprint.{facet}`
//! lists every Card whose `core/uses_eval` edge points at the given Eval
//! version ([`cards_using_eval`]): the latest live version of each such
//! Card, visible to the caller, with its facet fingerprints and
//! `same_harness` / `same_model`. Grouping by a facet is the caller's fold
//! over `fingerprints`. That is the whole of the hub's comparison logic: it
//! lines records up and labels agreement on axes; it does not rank,
//! average, or declare a winner.
//!
//! Conditions are per run, so agreement is judged against the runs the
//! Card used: `same_harness` / `same_model` is true when the Card's
//! fingerprint equals that of *every* run in its used set for the Eval
//! record, and false when any used run's differs or is missing, or the
//! Card has none. A Card with an empty used set (the Eval had no runs when
//! it was posted, or its edge did not resolve then, so nothing was
//! recorded) is compared with the Eval version's header instead, which is
//! what the view did before runs were rows and what an Eval without runs
//! still means. Each row also carries the used set's summary: `runs_used`,
//! `used_set_hash` (the `evalhub_core::run::runs_hash` formula over the
//! used runs' current content hashes) and `changed_since_card` (the used
//! runs overwritten since the Card used them).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::StoreError;

/// The relation type of the edge from a Card to the Eval it was measured on.
pub const USES_EVAL: &str = "core/uses_eval";

/// A target as written in `relations[].to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationTarget<'a> {
    /// `{ns}/{name}@{seq}`: a version on this hub.
    Version {
        /// Namespace.
        ns: &'a str,
        /// Record name.
        name: &'a str,
        /// Version sequence number.
        seq: i32,
    },
    /// `external:…` or `hf:…`: outside the hub, stored textually.
    External(&'a str),
}

impl<'a> RelationTarget<'a> {
    /// Parse the textual form. `None` when it is neither a pinned hub
    /// reference nor an `external:` / `hf:` target.
    pub fn parse(s: &'a str) -> Option<Self> {
        if s.starts_with("external:") || s.starts_with("hf:") {
            return Some(Self::External(s));
        }
        let (path, seq) = s.rsplit_once('@')?;
        let seq: i32 = seq.parse().ok().filter(|n| *n >= 1)?;
        let (ns, name) = path.split_once('/')?;
        if ns.is_empty() || name.is_empty() || name.contains('/') {
            return None;
        }
        Some(Self::Version { ns, name, seq })
    }

    /// The textual form, as stored in `to_external` when unresolved.
    pub fn text(&self) -> String {
        match self {
            Self::Version { ns, name, seq } => format!("{ns}/{name}@{seq}"),
            Self::External(s) => (*s).to_string(),
        }
    }
}

/// One endpoint of an edge, as the reader sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedTarget {
    /// A version on this hub.
    Version {
        /// The version.
        version_id: Uuid,
        /// Namespace of its record.
        ns: String,
        /// Name of its record.
        name: String,
        /// Sequence number.
        seq: i32,
        /// `card` or `eval`.
        record_type: String,
        /// Whether the version has been tombstoned.
        tombstoned: bool,
        /// Whether the caller may see the record. When `false`, only
        /// `version_id` and `content_hash` may be shown.
        visible: bool,
        /// sha256 of the canonical body; the commitment shown for private endpoints.
        content_hash: Vec<u8>,
    },
    /// A `{ns}/{name}@{seq}` reference that does not exist (yet).
    Unresolved(String),
    /// An `external:` / `hf:` target.
    External(String),
}

impl ResolvedTarget {
    /// The version id, when the endpoint is a hub version.
    pub fn version_id(&self) -> Option<Uuid> {
        match self {
            Self::Version { version_id, .. } => Some(*version_id),
            _ => None,
        }
    }
}

/// An edge with both endpoints resolved for a reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRelation {
    /// Source version.
    pub from: ResolvedTarget,
    /// Registry id of the relation type.
    pub relation_type: String,
    /// Target.
    pub to: ResolvedTarget,
    /// Free-form attributes.
    pub attrs: Option<Value>,
}

/// Which edges a traversal follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Edges leaving the node.
    Out,
    /// Edges entering the node.
    In,
    /// Both.
    Both,
}

/// Parameters of [`traverse`].
#[derive(Debug, Clone)]
pub struct TraverseParams {
    /// Which edges to follow.
    pub direction: Direction,
    /// Restrict to these relation types; `None` for all.
    pub types: Option<Vec<String>>,
    /// Maximum number of edges from the start; clamped to `1..=5`.
    pub depth: u32,
    /// Replace every node by the latest live version of its record before
    /// reading its edges.
    pub follow_latest: bool,
}

/// A node of a traversal result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// The version.
    pub version_id: Uuid,
    /// Namespace.
    pub ns: String,
    /// Record name.
    pub name: String,
    /// Sequence number.
    pub seq: i32,
    /// `card` or `eval`.
    pub record_type: String,
    /// Tombstoned?
    pub tombstoned: bool,
    /// Visible to the caller?
    pub visible: bool,
    /// Content hash (the commitment).
    pub content_hash: Vec<u8>,
}

/// The far end of a traversal edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeEnd {
    /// A node in the graph.
    Version(Uuid),
    /// An unresolved or external target, textual.
    Text(String),
}

/// An edge of a traversal result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    /// Source version.
    pub from: Uuid,
    /// Relation type.
    pub relation_type: String,
    /// Target.
    pub to: EdgeEnd,
    /// Attributes.
    pub attrs: Option<Value>,
}

/// What a traversal found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Graph {
    /// Every version reached, the start first.
    pub nodes: Vec<Node>,
    /// Every edge followed.
    pub edges: Vec<Edge>,
}

/// One Card in the comparison view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparisonRow {
    /// The Card's latest live version.
    pub version_id: Uuid,
    /// Namespace.
    pub ns: String,
    /// Name.
    pub name: String,
    /// Sequence number of that version.
    pub seq: i32,
    /// The record's title.
    pub title: Option<String>,
    /// Content hash.
    pub content_hash: Vec<u8>,
    /// Facet fingerprints of the Card, keyed by facet name.
    pub fingerprints: BTreeMap<String, Vec<u8>>,
    /// The Card's harness fingerprint equals that of every run in its used
    /// set for this Eval; `false` when any used run's differs or is
    /// missing, or the Card has none. With an empty used set, the Eval
    /// version's header fingerprint is compared instead (see the module
    /// doc).
    pub same_harness: bool,
    /// As `same_harness`, for the model fingerprint.
    pub same_model: bool,
    /// How many runs the Card's latest version used from this Eval record
    /// (its `card_eval_runs` rows).
    pub runs_used: i64,
    /// `evalhub_core::run::runs_hash` over the used runs' *current*
    /// content hashes. Equal to the hash over the posting-time hashes
    /// exactly when `changed_since_card` is empty.
    pub used_set_hash: Vec<u8>,
    /// The used runs whose content hash differs from the one recorded when
    /// the Card used them (overwritten since), by `run_id` ascending.
    pub changed_since_card: Vec<String>,
}

/// Whether a reader whose token covers `caller_ns` may see a record in
/// namespace `ns` with the given `visibility` (`public` / `private`).
///
/// The one definition of visibility across a relation: the traversal, the
/// comparison view, relation resolution and the redaction of bodies all
/// ask this. The SQL read paths in [`crate::records`] apply the same
/// predicate as `visibility = 'public' OR ns = ANY(caller_ns)`.
pub fn visible_to(visibility: &str, ns: &str, caller_ns: &[String]) -> bool {
    visibility == "public" || caller_ns.iter().any(|n| n == ns)
}

/// A relation target a reader may not see, as recorded on the edge that
/// points at it: which `relations[]` elements of the source body to
/// withhold, and the commitment to report instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HiddenTarget {
    /// Registry id of the relation type of the edge.
    pub relation_type: String,
    /// Namespace of the target record.
    pub ns: String,
    /// Name of the target record.
    pub name: String,
    /// Sequence number of the target version.
    pub seq: i32,
    /// The target version.
    pub version_id: Uuid,
    /// Its content hash: the commitment.
    pub content_hash: Vec<u8>,
}

/// What the read side needs to know about a version.
#[derive(Debug, Clone)]
struct VersionInfo {
    version_id: Uuid,
    record_id: Uuid,
    ns: String,
    name: String,
    seq: i32,
    record_type: String,
    tombstoned: bool,
    visibility: String,
    content_hash: Vec<u8>,
}

impl VersionInfo {
    fn text(&self) -> String {
        format!("{}/{}@{}", self.ns, self.name, self.seq)
    }

    fn visible(&self, caller_ns: &[String]) -> bool {
        visible_to(&self.visibility, &self.ns, caller_ns)
    }

    fn target(&self, caller_ns: &[String]) -> ResolvedTarget {
        ResolvedTarget::Version {
            version_id: self.version_id,
            ns: self.ns.clone(),
            name: self.name.clone(),
            seq: self.seq,
            record_type: self.record_type.clone(),
            tombstoned: self.tombstoned,
            visible: self.visible(caller_ns),
            content_hash: self.content_hash.clone(),
        }
    }

    fn node(&self, caller_ns: &[String]) -> Node {
        Node {
            version_id: self.version_id,
            ns: self.ns.clone(),
            name: self.name.clone(),
            seq: self.seq,
            record_type: self.record_type.clone(),
            tombstoned: self.tombstoned,
            visible: self.visible(caller_ns),
            content_hash: self.content_hash.clone(),
        }
    }
}

async fn version_info(pool: &PgPool, version_id: Uuid) -> Result<Option<VersionInfo>, StoreError> {
    let row = sqlx::query!(
        "SELECT v.version_id, v.record_id, r.ns, r.name, v.seq, r.type AS record_type,
                (v.tombstoned_at IS NOT NULL) AS \"tombstoned!\", r.visibility, v.content_hash
         FROM versions v JOIN records r ON r.id = v.record_id
         WHERE v.version_id = $1",
        version_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| VersionInfo {
        version_id: r.version_id,
        record_id: r.record_id,
        ns: r.ns,
        name: r.name,
        seq: r.seq,
        record_type: r.record_type,
        tombstoned: r.tombstoned,
        visibility: r.visibility,
        content_hash: r.content_hash,
    }))
}

/// `{ns}/{name}@{seq}` → the version id, tombstoned or not, when a party
/// covering `caller_ns` may see it. A version that exists but is not
/// visible is `None`, the same answer as one that does not exist.
async fn resolve_ref(
    pool: &PgPool,
    ns: &str,
    name: &str,
    seq: i32,
    caller_ns: &[String],
) -> Result<Option<Uuid>, StoreError> {
    let rows = sqlx::query!(
        "SELECT v.version_id, r.visibility FROM versions v JOIN records r ON r.id = v.record_id
         WHERE r.ns = $1 AND r.name = $2 AND v.seq = $3
         ORDER BY r.type",
        ns,
        name,
        seq,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .find(|r| visible_to(&r.visibility, ns, caller_ns))
        .map(|r| r.version_id))
}

/// The latest live version of the record owning `version_id`.
async fn latest_of_record(pool: &PgPool, record_id: Uuid) -> Result<Option<Uuid>, StoreError> {
    let row = sqlx::query!(
        "SELECT version_id FROM versions
         WHERE record_id = $1 AND tombstoned_at IS NULL
         ORDER BY seq DESC LIMIT 1",
        record_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.version_id))
}

/// Resolve a stored `(to_version_id, to_external)` pair for a reader,
/// re-attempting textual references. A textual reference resolves only to
/// a version the reader may see; otherwise it stays [`ResolvedTarget::Unresolved`],
/// so the lazy path is never a way to learn that a private version exists.
async fn resolve_stored(
    pool: &PgPool,
    to_version_id: Option<Uuid>,
    to_external: Option<String>,
    caller_ns: &[String],
) -> Result<ResolvedTarget, StoreError> {
    if let Some(id) = to_version_id
        && let Some(info) = version_info(pool, id).await?
    {
        return Ok(info.target(caller_ns));
    }
    let text = to_external.unwrap_or_default();
    match RelationTarget::parse(&text) {
        Some(RelationTarget::Version { ns, name, seq }) => {
            if let Some(id) = resolve_ref(pool, ns, name, seq, caller_ns).await?
                && let Some(info) = version_info(pool, id).await?
            {
                Ok(info.target(caller_ns))
            } else {
                Ok(ResolvedTarget::Unresolved(text))
            }
        }
        Some(RelationTarget::External(_)) => Ok(ResolvedTarget::External(text)),
        None => Ok(ResolvedTarget::Unresolved(text)),
    }
}

/// Add an edge after ingest. The target is resolved like at ingest: only
/// to a version the writer may see, where the writer covers `writer_ns`
/// plus the source's own namespace (the caller has checked `write` on
/// it). An unresolvable or invisible hub reference is stored textually.
/// Writes an audit row. `actor` is `(user_id, token_id)`.
///
/// A `core/uses_eval` edge from a Card version that resolves to an Eval's
/// version fixes that version's used set for the Eval record in the same
/// transaction, as the ingest does (this path has no `run_results`), with
/// one difference: here the used set only grows. Runs already recorded
/// for the version stay recorded with their posting-time hashes, and the
/// new edge can only add runs, because the version's `run_results[]` were
/// checked against the set as it was. `extend_used_set` below has the
/// detail.
///
/// Errors: [`StoreError::SourceVersionUnknown`];
/// [`StoreError::CardRunsRejected`] when `attrs.runs` names a run with no
/// row in the Eval (`run_unknown` at `/attrs/runs/{k}`), in which case no
/// edge is written.
pub async fn add(
    pool: &PgPool,
    from_version_id: Uuid,
    relation_type: &str,
    target: RelationTarget<'_>,
    attrs: Option<&Value>,
    actor: (Option<Uuid>, Option<Uuid>),
    writer_ns: &[String],
) -> Result<StoredRelation, StoreError> {
    let from = version_info(pool, from_version_id)
        .await?
        .ok_or(StoreError::SourceVersionUnknown(from_version_id))?;
    let mut writer_ns = writer_ns.to_vec();
    if !writer_ns.contains(&from.ns) {
        writer_ns.push(from.ns.clone());
    }
    let (mut to_version_id, mut to_external) = match target {
        RelationTarget::Version { ns, name, seq } => {
            match resolve_ref(pool, ns, name, seq, &writer_ns).await? {
                Some(id) => (Some(id), None),
                None => (None, Some(target.text())),
            }
        }
        RelationTarget::External(s) => (None, Some(s.to_string())),
    };
    let mut tx = pool.begin().await?;
    if relation_type == USES_EVAL
        && from.record_type == "card"
        && let Some(to) = to_version_id
        && !extend_used_set(&mut tx, &from, to, attrs, &writer_ns).await?
    {
        // The Eval turned private between resolution and the lock: store
        // the edge as text, as any reference to a version the writer may
        // not see is stored.
        to_version_id = None;
        to_external = Some(target.text());
    }
    sqlx::query!(
        "INSERT INTO relations (from_version_id, type, to_version_id, to_external, attrs)
         VALUES ($1, $2, $3, $4, $5)",
        from_version_id,
        relation_type,
        to_version_id,
        to_external,
        attrs,
    )
    .execute(&mut *tx)
    .await?;
    let subject = format!("{}/{}@{}", from.record_type, from.text(), "");
    let subject = subject.trim_end_matches('@').to_string();
    let detail = serde_json::json!({
        "type": relation_type,
        "to": to_version_id.map(|id| id.to_string()).unwrap_or_else(|| to_external.clone().unwrap_or_default()),
    });
    sqlx::query!(
        "INSERT INTO audit (actor_user_id, actor_token_id, ns, action, subject, detail)
         VALUES ($1, $2, $3, 'relation.add', $4, $5)",
        actor.0,
        actor.1,
        from.ns,
        subject,
        detail,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let to = resolve_stored(pool, to_version_id, to_external, &writer_ns).await?;
    Ok(StoredRelation {
        from: from.target(&writer_ns),
        relation_type: relation_type.to_string(),
        to,
        attrs: attrs.cloned(),
    })
}

/// A `core/uses_eval` edge from the Card version `from` to the version
/// `to` is being added: when `to` is an Eval's, fix the used set it
/// contributes and record it, inside the edge's transaction. Returns
/// `false` when the Eval is no longer visible to `writer_ns` under the
/// lock, having written nothing; the caller then stores the edge
/// unresolved.
///
/// Locks as the ingest does: the Card record `FOR UPDATE`, then the Eval
/// record `FOR SHARE`, re-reading its visibility under that lock
/// ([`crate::used_set`]: the resolution before the transaction read it
/// unlocked, and a `set_visibility` may have committed since).
///
/// **The used set only grows.** It is recomputed over every
/// `core/uses_eval` edge of `from` into the same Eval record, the new one
/// included, and the runs not yet recorded are inserted with their hashes
/// now; rows already in `card_eval_runs` stay, with the hash recorded when
/// they were added. This is not the ingest's rule applied from scratch:
/// under it, a Card posted with an edge carrying no `attrs.runs` (every
/// live run) that later gains an edge with `attrs.runs: ["rX"]` would have
/// a used set of `{rX}`, and every run it was recorded as using, and may
/// have judged in its `run_results[]`, would fall outside its own used
/// set. The version's `run_results[]` were checked against the set as it
/// was, and a version is write-once, so the set may gain runs but never
/// lose one. Only the new edge's `attrs.runs` are checked
/// (`/attrs/runs/{k}`); the stored edges' runs had rows when they were
/// written, and a run row is never deleted.
async fn extend_used_set(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    from: &VersionInfo,
    to: Uuid,
    attrs: Option<&Value>,
    writer_ns: &[String],
) -> Result<bool, StoreError> {
    let target = sqlx::query!(
        "SELECT v.record_id, r.type AS record_type
         FROM versions v JOIN records r ON r.id = v.record_id
         WHERE v.version_id = $1",
        to,
    )
    .fetch_optional(&mut **tx)
    .await?;
    let Some(target) = target.filter(|t| t.record_type == "eval") else {
        return Ok(true);
    };
    sqlx::query!(
        "SELECT id FROM records WHERE id = $1 FOR UPDATE",
        from.record_id
    )
    .fetch_one(&mut **tx)
    .await?;
    let visible =
        crate::used_set::lock_evals(tx, &std::iter::once(target.record_id).collect(), writer_ns)
            .await?;
    if !visible.contains(&target.record_id) {
        return Ok(false);
    }

    let stored = sqlx::query_scalar!(
        "SELECT rel.attrs FROM relations rel
         JOIN versions tv ON tv.version_id = rel.to_version_id
         WHERE rel.from_version_id = $1 AND rel.type = $2 AND tv.record_id = $3",
        from.version_id,
        USES_EVAL,
        target.record_id,
    )
    .fetch_all(&mut **tx)
    .await?;
    let mut uses: Vec<crate::used_set::UsesEval> = stored
        .iter()
        .map(|attrs| crate::used_set::UsesEval {
            eval_record_id: target.record_id,
            runs: crate::used_set::UsesEval::runs_of(attrs.as_ref()),
            runs_path: None,
        })
        .collect();
    uses.push(crate::used_set::UsesEval {
        eval_record_id: target.record_id,
        runs: crate::used_set::UsesEval::runs_of(attrs),
        runs_path: Some("/attrs/runs".to_string()),
    });
    let mut errors = Vec::new();
    let used = crate::used_set::fix(tx, &uses, &mut errors).await?;
    if !errors.is_empty() {
        crate::used_set::sort_entries(&mut errors);
        return Err(StoreError::CardRunsRejected(errors));
    }
    crate::used_set::insert(tx, from.version_id, &used).await?;
    Ok(true)
}

/// An Eval whose runs a Card version judges (`run_results[].eval`) and
/// that a reader may not see: what a read path needs to withhold those
/// `run_results[]` elements and say how many it withheld.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HiddenEval {
    /// The Eval record (`run_results.eval_record_id`).
    pub eval_record_id: Uuid,
    /// Its namespace: `run_results[].eval` is `{ns}/{name}`.
    pub ns: String,
    /// Its name.
    pub name: String,
    /// How many `run_results[]` elements of the version name it.
    pub run_results: i64,
}

/// For each of `card_version_ids`, the Eval records named by its
/// `run_results[]` that a reader covering `caller_ns` may not see, by
/// `(ns, name)`. Versions with none are absent from the map.
///
/// The second input of the server's redaction, beside [`hidden_targets`]:
/// that one covers the resolved `relations[]` elements, this one the
/// judgements. A read path removes every `run_results[]` element whose
/// `eval` is `{ns}/{name}` of one of these, and reports the count, never
/// the name, run ids or verdicts. Every stored `run_results` row names an
/// Eval the writer could see when the Card was posted (the ingest refuses
/// anything else), so there is no unresolved case here. Cost: one
/// grouped read over the versions' `run_results`.
pub async fn hidden_evals(
    pool: &PgPool,
    card_version_ids: &[Uuid],
    caller_ns: &[String],
) -> Result<HashMap<Uuid, Vec<HiddenEval>>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT rr.version_id, r.id AS eval_record_id, r.ns, r.name, r.visibility,
                  COUNT(*) AS "run_results!"
           FROM run_results rr JOIN records r ON r.id = rr.eval_record_id
           WHERE rr.version_id = ANY($1)
           GROUP BY rr.version_id, r.id, r.ns, r.name, r.visibility
           ORDER BY rr.version_id, r.ns, r.name"#,
        card_version_ids,
    )
    .fetch_all(pool)
    .await?;
    let mut out: HashMap<Uuid, Vec<HiddenEval>> = HashMap::new();
    for r in rows {
        if visible_to(&r.visibility, &r.ns, caller_ns) {
            continue;
        }
        out.entry(r.version_id).or_default().push(HiddenEval {
            eval_record_id: r.eval_record_id,
            ns: r.ns,
            name: r.name,
            run_results: r.run_results,
        });
    }
    Ok(out)
}

/// For each of `version_ids`, the targets of its resolved edges that a
/// reader covering `caller_ns` may not see. Versions with none are absent
/// from the map.
///
/// This is what a read path needs to withhold `relations[]` elements from
/// a body: an element whose `(type, to)` names one of these targets is
/// removed and the commitment reported instead. Edges stored unresolved
/// are not here: they carry only the text the writer typed, which reveals
/// nothing about the target (see the module doc).
pub async fn hidden_targets(
    pool: &PgPool,
    version_ids: &[Uuid],
    caller_ns: &[String],
) -> Result<HashMap<Uuid, Vec<HiddenTarget>>, StoreError> {
    let rows = sqlx::query!(
        "SELECT rel.from_version_id, rel.type AS relation_type, v.version_id,
                r.ns, r.name, v.seq, r.visibility, v.content_hash
         FROM relations rel
         JOIN versions v ON v.version_id = rel.to_version_id
         JOIN records r ON r.id = v.record_id
         WHERE rel.from_version_id = ANY($1)
         ORDER BY rel.from_version_id, rel.type, v.version_id",
        version_ids,
    )
    .fetch_all(pool)
    .await?;
    let mut out: HashMap<Uuid, Vec<HiddenTarget>> = HashMap::new();
    for r in rows {
        if visible_to(&r.visibility, &r.ns, caller_ns) {
            continue;
        }
        out.entry(r.from_version_id)
            .or_default()
            .push(HiddenTarget {
                relation_type: r.relation_type,
                ns: r.ns,
                name: r.name,
                seq: r.seq,
                version_id: r.version_id,
                content_hash: r.content_hash,
            });
    }
    Ok(out)
}

/// Edges leaving `version_id`, targets resolved for the caller. The
/// version itself must be visible to the caller (the handler checks; this
/// function does not).
pub async fn outgoing(
    pool: &PgPool,
    version_id: Uuid,
    types: Option<&[String]>,
    caller_ns: &[String],
) -> Result<Vec<StoredRelation>, StoreError> {
    let Some(from) = version_info(pool, version_id).await? else {
        return Ok(Vec::new());
    };
    let types: Option<Vec<String>> = types.map(|t| t.to_vec());
    let rows = sqlx::query!(
        "SELECT type AS relation_type, to_version_id, to_external, attrs
         FROM relations
         WHERE from_version_id = $1 AND ($2::text[] IS NULL OR type = ANY($2))
         ORDER BY type, to_version_id, to_external",
        version_id,
        types.as_deref(),
    )
    .fetch_all(pool)
    .await?;
    let from_target = from.target(caller_ns);
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let to = resolve_stored(pool, r.to_version_id, r.to_external, caller_ns).await?;
        out.push(StoredRelation {
            from: from_target.clone(),
            relation_type: r.relation_type,
            to,
            attrs: r.attrs,
        });
    }
    Ok(out)
}

/// Edges entering `version_id`: those resolved to it, and those whose
/// textual target names it. Sources the caller may not see come back
/// with `visible == false`.
pub async fn incoming(
    pool: &PgPool,
    version_id: Uuid,
    types: Option<&[String]>,
    caller_ns: &[String],
) -> Result<Vec<StoredRelation>, StoreError> {
    let Some(to) = version_info(pool, version_id).await? else {
        return Ok(Vec::new());
    };
    let text = to.text();
    let types: Option<Vec<String>> = types.map(|t| t.to_vec());
    let rows = sqlx::query!(
        "SELECT from_version_id, type AS relation_type, attrs
         FROM relations
         WHERE (to_version_id = $1 OR to_external = $2)
           AND ($3::text[] IS NULL OR type = ANY($3))
         ORDER BY type, from_version_id",
        version_id,
        text,
        types.as_deref(),
    )
    .fetch_all(pool)
    .await?;
    let to_target = to.target(caller_ns);
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let from = match version_info(pool, r.from_version_id).await? {
            Some(info) => info.target(caller_ns),
            None => ResolvedTarget::Unresolved(r.from_version_id.to_string()),
        };
        out.push(StoredRelation {
            from,
            relation_type: r.relation_type,
            to: to_target.clone(),
            attrs: r.attrs,
        });
    }
    Ok(out)
}

/// Breadth-first walk from `start`. See the module doc for `depth` and
/// `follow_latest`. Returns an empty graph when `start` does not exist.
pub async fn traverse(
    pool: &PgPool,
    start: Uuid,
    params: TraverseParams,
    caller_ns: &[String],
) -> Result<Graph, StoreError> {
    let depth = params.depth.clamp(1, 5);
    let types = params.types.as_deref();

    let Some(mut start_info) = version_info(pool, start).await? else {
        return Ok(Graph::default());
    };
    if params.follow_latest
        && let Some(latest) = latest_of_record(pool, start_info.record_id).await?
        && latest != start_info.version_id
        && let Some(info) = version_info(pool, latest).await?
    {
        start_info = info;
    }

    let mut graph = Graph::default();
    let mut seen: HashSet<Uuid> = HashSet::new();
    let mut queue: VecDeque<(VersionInfo, u32)> = VecDeque::new();
    seen.insert(start_info.version_id);
    graph.nodes.push(start_info.node(caller_ns));
    queue.push_back((start_info, 0));

    while let Some((node, dist)) = queue.pop_front() {
        if dist >= depth {
            continue;
        }
        let mut neighbours: Vec<(Edge, Option<Uuid>)> = Vec::new();
        if matches!(params.direction, Direction::Out | Direction::Both) {
            for rel in outgoing(pool, node.version_id, types, caller_ns).await? {
                let (end, next) = match &rel.to {
                    ResolvedTarget::Version { version_id, .. } => {
                        (EdgeEnd::Version(*version_id), Some(*version_id))
                    }
                    ResolvedTarget::Unresolved(t) | ResolvedTarget::External(t) => {
                        (EdgeEnd::Text(t.clone()), None)
                    }
                };
                neighbours.push((
                    Edge {
                        from: node.version_id,
                        relation_type: rel.relation_type,
                        to: end,
                        attrs: rel.attrs,
                    },
                    next,
                ));
            }
        }
        if matches!(params.direction, Direction::In | Direction::Both) {
            for rel in incoming(pool, node.version_id, types, caller_ns).await? {
                let Some(from_id) = rel.from.version_id() else {
                    continue;
                };
                neighbours.push((
                    Edge {
                        from: from_id,
                        relation_type: rel.relation_type,
                        to: EdgeEnd::Version(node.version_id),
                        attrs: rel.attrs,
                    },
                    Some(from_id),
                ));
            }
        }
        for (edge, next) in neighbours {
            if !graph.edges.contains(&edge) {
                graph.edges.push(edge);
            }
            let Some(mut next_id) = next else {
                continue;
            };
            let Some(mut info) = version_info(pool, next_id).await? else {
                continue;
            };
            if params.follow_latest
                && let Some(latest) = latest_of_record(pool, info.record_id).await?
                && latest != next_id
                && let Some(latest_info) = version_info(pool, latest).await?
            {
                next_id = latest;
                info = latest_info;
            }
            if seen.insert(next_id) {
                graph.nodes.push(info.node(caller_ns));
                queue.push_back((info, dist + 1));
            }
        }
    }
    Ok(graph)
}

async fn fingerprints_of(
    pool: &PgPool,
    version_id: Uuid,
) -> Result<BTreeMap<String, Vec<u8>>, StoreError> {
    let rows = sqlx::query!(
        "SELECT facet, fingerprint FROM fingerprints WHERE version_id = $1",
        version_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| (r.facet, r.fingerprint)).collect())
}

/// The comparison view: every visible Card whose latest live version has a
/// `core/uses_eval` edge (resolved or textual) to `eval_version_id`, with
/// its fingerprints, the `same_harness` / `same_model` flags over its used
/// set, and the used set's summary. See the module doc.
///
/// An empty used set (the Eval had no runs when the Card was posted, or
/// the Card's `attrs.runs` is an explicit `[]`) compares with the Eval
/// version's header fingerprint, as the view did before 0.2.0.
///
/// Cost: a handful of reads per Card, plus one read of every listed Card's
/// used set for the Eval record.
pub async fn cards_using_eval(
    pool: &PgPool,
    eval_version_id: Uuid,
    caller_ns: &[String],
) -> Result<Vec<ComparisonRow>, StoreError> {
    let Some(eval) = version_info(pool, eval_version_id).await? else {
        return Ok(Vec::new());
    };
    let eval_fp = fingerprints_of(pool, eval_version_id).await?;
    let text = eval.text();
    let rows = sqlx::query!(
        "SELECT DISTINCT rel.from_version_id
         FROM relations rel
         WHERE rel.type = $3 AND (rel.to_version_id = $1 OR rel.to_external = $2)",
        eval_version_id,
        text,
        USES_EVAL,
    )
    .fetch_all(pool)
    .await?;

    let mut by_record: HashMap<Uuid, VersionInfo> = HashMap::new();
    for r in rows {
        let Some(info) = version_info(pool, r.from_version_id).await? else {
            continue;
        };
        if info.record_type != "card" || info.tombstoned || !info.visible(caller_ns) {
            continue;
        }
        // Only the latest live version of each Card is compared.
        match latest_of_record(pool, info.record_id).await? {
            Some(latest) if latest == info.version_id => {}
            _ => continue,
        }
        by_record.insert(info.record_id, info);
    }

    let card_versions: Vec<Uuid> = by_record.values().map(|i| i.version_id).collect();
    // One statement: the flags and the summary of a row both come from it,
    // so they cannot disagree with each other.
    let mut used =
        crate::used_set::read(&mut *pool.acquire().await?, &card_versions, eval.record_id).await?;

    let mut out = Vec::with_capacity(by_record.len());
    for info in by_record.into_values() {
        let fp = fingerprints_of(pool, info.version_id).await?;
        let runs = used.remove(&info.version_id).unwrap_or_default();
        let same = |facet: &str, of_run: fn(&crate::used_set::UsedRun) -> Option<&Vec<u8>>| {
            let Some(card) = fp.get(facet) else {
                return false;
            };
            if runs.is_empty() {
                // No used run to compare with: the Eval had no runs when
                // the Card was posted, or the edge did not resolve then.
                return eval_fp.get(facet) == Some(card);
            }
            runs.iter().all(|r| of_run(r) == Some(card))
        };
        let same_harness = same("harness", |r| r.harness.as_ref());
        let same_model = same("model", |r| r.model.as_ref());
        let summary = crate::used_set::summarise(&runs)?;
        let title = sqlx::query!(
            "SELECT title FROM versions WHERE version_id = $1",
            info.version_id
        )
        .fetch_one(pool)
        .await?
        .title;
        out.push(ComparisonRow {
            version_id: info.version_id,
            ns: info.ns,
            name: info.name,
            seq: info.seq,
            title,
            content_hash: info.content_hash,
            same_harness,
            same_model,
            fingerprints: fp,
            runs_used: summary.runs_used,
            used_set_hash: summary.used_set_hash,
            changed_since_card: summary.changed_since_card,
        });
    }
    out.sort_by(|a, b| (&a.ns, &a.name).cmp(&(&b.ns, &b.name)));
    Ok(out)
}
