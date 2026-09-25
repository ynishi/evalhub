#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The committed `schemas/*.json` are generated from the Rust types. This
//! test regenerates them and fails if the committed file differs.
//!
//! To update after an intentional type change:
//!
//! ```text
//! EVALHUB_UPDATE_SCHEMAS=1 cargo test -p evalhub-schema --test schemas
//! ```
//!
//! and commit the result together with the type change.

use std::path::PathBuf;

fn schemas_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schemas")
}

fn render(schema: &schemars::Schema) -> String {
    let mut s = serde_json::to_string_pretty(schema).unwrap();
    s.push('\n');
    s
}

#[test]
fn committed_schemas_are_current() {
    let update = std::env::var_os("EVALHUB_UPDATE_SCHEMAS").is_some();
    let dir = schemas_dir();
    let mut stale = Vec::new();

    for (name, schema) in evalhub_schema::all_schemas() {
        let path = dir.join(format!("{name}.json"));
        let generated = render(&schema);
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        if generated == committed {
            continue;
        }
        if update {
            std::fs::write(&path, &generated).unwrap();
            eprintln!("updated {}", path.display());
        } else {
            let new = dir.join(format!("{name}.json.new"));
            std::fs::write(&new, &generated).unwrap();
            stale.push(format!(
                "{} is stale; the current schema was written to {}. Review it, then \
                 rerun with EVALHUB_UPDATE_SCHEMAS=1 to replace the committed file.",
                path.display(),
                new.display()
            ));
        }
    }

    assert!(stale.is_empty(), "{}", stale.join("\n"));
}

#[test]
fn schema_names_match_all_schemas() {
    let names: Vec<&str> = evalhub_schema::all_schemas()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(names, evalhub_schema::SCHEMA_NAMES);
}

#[test]
fn schemas_are_draft_2020_12_and_closed() {
    for (name, schema) in evalhub_schema::all_schemas() {
        let v = schema.as_value();
        assert_eq!(
            v["$schema"], "https://json-schema.org/draft/2020-12/schema",
            "{name}: $schema"
        );
        assert!(v.get("$id").is_none(), "{name}: $id is the server's to set");
        assert_eq!(
            v["additionalProperties"], false,
            "{name}: the root object must be closed"
        );
    }
}

#[test]
fn fingerprint_exclusions_are_in_the_schema() {
    let card = evalhub_schema::schema_for_card();
    let v = card.as_value();
    let defs = &v["$defs"];
    assert_eq!(
        defs["Model"]["properties"]["context_window"]["x-fingerprint"],
        false
    );
    assert_eq!(defs["Env"]["properties"]["os"]["x-fingerprint"], false);
    assert_eq!(
        defs["Env"]["properties"]["hardware"]["x-fingerprint"],
        false
    );
    // A core key carries no marker at all.
    assert!(
        defs["Model"]["properties"]["id"]
            .get("x-fingerprint")
            .is_none()
    );
}

/// The identifiers a schema document's `schema` key accepts: its `const`,
/// or every member of its `enum`, sorted.
fn accepted_ids(schema: &schemars::Schema) -> Vec<String> {
    let prop = &schema.as_value()["properties"]["schema"];
    if let Some(c) = prop.get("const") {
        return vec![c.as_str().unwrap().to_string()];
    }
    let mut ids: Vec<String> = prop["enum"]
        .as_array()
        .unwrap_or_else(|| panic!("`schema` has neither const nor enum: {prop}"))
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    ids.sort();
    ids
}

#[test]
fn schema_identifier_constants_match_the_schema_const() {
    use evalhub_schema::RecordKind;

    assert_eq!(RecordKind::Card.schema_id(), evalhub_schema::CARD_SCHEMA);
    assert_eq!(RecordKind::Eval.schema_id(), evalhub_schema::EVAL_SCHEMA);

    for kind in [RecordKind::Card, RecordKind::Eval] {
        let ids = kind.schema_ids();
        // The current identifier is listed, and listed first.
        assert_eq!(ids[0], kind.schema_id(), "{kind:?}");
        // `schema()` is the document for the current identifier.
        assert!(
            accepted_ids(&kind.schema()).contains(&kind.schema_id().to_string()),
            "{kind:?}: schema() does not accept {}",
            kind.schema_id()
        );
        // Every accepted identifier has a document, and that document's
        // `schema` constraint accepts only identifiers `schema_ids()` lists.
        for id in ids {
            let doc = kind
                .schema_for(id)
                .unwrap_or_else(|| panic!("{kind:?}: no document for {id}"));
            let accepted = accepted_ids(&doc);
            assert!(accepted.contains(&id.to_string()), "{id}: {accepted:?}");
            for other in &accepted {
                assert!(
                    ids.contains(&other.as_str()),
                    "{id}'s document accepts {other}, which schema_ids() does not list"
                );
            }
        }
        assert!(kind.schema_for("evalhub.nope/1.0").is_none());
    }

    // The Card's two minors share one document; the Eval's majors do not.
    assert_eq!(
        accepted_ids(&evalhub_schema::schema_for_card()),
        ["evalhub.card/1.0", "evalhub.card/1.1"]
    );
    assert_eq!(
        RecordKind::Card.schema_for("evalhub.card/1.0"),
        RecordKind::Card.schema_for("evalhub.card/1.1")
    );
    assert_eq!(
        accepted_ids(&evalhub_schema::schema_for_eval()),
        [evalhub_schema::EVAL_SCHEMA]
    );
    assert_eq!(
        accepted_ids(&evalhub_schema::schema_for_eval_v1()),
        ["evalhub.eval/1.0"]
    );
    assert!(RecordKind::Eval.schema_for("evalhub.card/1.1").is_none());
    assert!(
        RecordKind::Card
            .schema_for(evalhub_schema::EVAL_SCHEMA)
            .is_none()
    );
}
