#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The runs of an Eval over HTTP: writing one or a batch, the projection
//! with Cards' judgements, the summary on the Eval, archive and delete,
//! the 0.1.x body, the limits, and the privacy boundary around runs and
//! judgements. Needs Docker (Postgres, and MinIO for the download test).

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use common::{CARD, EVAL, Hub};
use evalhub_store::auth::Scope;

/// The Eval every test writes runs to.
const EVAL_NAME: &str = "alice/single2-k4";
/// The Card that judges them.
const CARD_NAME: &str = "alice/verdicts";

/// alice with a `write` token and her Eval `alice/single2-k4` (private, as
/// every record starts), attachments seeded.
async fn hub_with_eval(hub: Hub) -> (Hub, String) {
    let alice = hub.user("alice", Scope::Admin).await;
    hub.ready_attachments(EVAL).await;
    hub.ready_attachments(CARD).await;
    let (status, env) = hub
        .call(
            Method::POST,
            &format!("/api/v1/evals/{EVAL_NAME}"),
            Some(&alice),
            Some(EVAL),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{env}");
    (hub, alice)
}

/// The fixture Card with run_results is accepted once its runs exist.
#[tokio::test]
async fn card_with_run_results_is_posted_after_its_runs() {
    let (hub, alice) = hub_with_eval(Hub::start().await).await;
    let (status, env) = hub.post_card_with_runs(&alice, EVAL_NAME, CARD_NAME).await;
    assert_eq!(status, StatusCode::CREATED, "{env}");
}

async fn make_public(hub: &Hub, token: &str, kind: &str, name: &str) {
    set_visibility(hub, token, kind, name, "public").await;
}

async fn set_visibility(hub: &Hub, token: &str, kind: &str, name: &str, visibility: &str) {
    let (status, body) = hub
        .call(
            Method::PATCH,
            &format!("/api/v1/{kind}/{name}/settings"),
            Some(token),
            Some(&json!({ "visibility": visibility }).to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn get(hub: &Hub, path: &str, token: Option<&str>) -> (StatusCode, Value) {
    hub.call(Method::GET, path, token, None).await
}

fn run_ids(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["run_id"].as_str().unwrap().to_string())
        .collect()
}

/// `PUT` a run → `GET …/runs` returns it with its metrics → a Card with
/// `run_results` for it appears under `cards=` → overwriting the run puts
/// it in that Card's `changed_since_card`, in the projection and in the
/// comparison view.
#[tokio::test]
async fn a_run_its_judgement_and_what_changed_since() {
    let (hub, alice) = hub_with_eval(Hub::start().await).await;

    let (status, put) = hub
        .put_run(&alice, EVAL_NAME, "r1", json!({"core/tokens_out": 1834.0}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{put}");
    assert_eq!(put["run_id"], "r1");
    assert_eq!(put["status"], "ok");
    assert_eq!(put["result"], "created");
    assert_eq!(put["content_hash"].as_str().unwrap().len(), 64);
    assert_eq!(put["runs_hash"].as_str().unwrap().len(), 64);

    // The same content again: 200, nothing written, same hashes.
    let (status, again) = hub
        .put_run(&alice, EVAL_NAME, "r1", json!({"core/tokens_out": 1834.0}))
        .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["result"], "unchanged");
    assert_eq!(again["content_hash"], put["content_hash"]);
    assert_eq!(again["runs_hash"], put["runs_hash"]);

    let (status, page) = get(
        &hub,
        &format!("/api/v1/evals/{EVAL_NAME}/runs"),
        Some(&alice),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(run_ids(&page), ["r1"]);
    let row = &page["items"][0];
    assert_eq!(row["state"], "live");
    assert_eq!(row["status"], "ok");
    assert_eq!(row["metrics"]["core/tokens_out"], 1834.0);
    assert_eq!(row["content_hash"], put["content_hash"]);
    assert_eq!(row["fingerprints"]["model"].as_str().unwrap().len(), 64);
    assert_eq!(page["next_cursor"], Value::Null);

    // The Card judges r1 and r2 (the helper PUTs both, overwriting r1).
    let (status, card) = hub.post_card_with_runs(&alice, EVAL_NAME, CARD_NAME).await;
    assert_eq!(status, StatusCode::CREATED, "{card}");

    let url = format!("/api/v1/evals/{EVAL_NAME}/runs?cards={CARD_NAME}");
    let (status, page) = get(&hub, &url, Some(&alice)).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(run_ids(&page), ["r1", "r2"]);
    let c = &page["cards"][0];
    assert_eq!(c["card"], CARD_NAME);
    assert_eq!(c["seq"], 1);
    assert_eq!(c["runs_used"], 2);
    assert_eq!(c["changed_since_card"], json!([]));
    assert_eq!(c["used_set_hash"], c["posted_used_set_hash"]);
    let cell = &page["items"][0]["cards"][CARD_NAME];
    assert_eq!(cell["used"], true);
    assert_eq!(cell["changed"], false);
    assert_eq!(cell["results"][0]["metric"], "core/pass");
    assert_eq!(cell["results"][0]["label"], "pass");
    assert_eq!(cell["results"][0]["value"], 1.0);
    assert_eq!(
        page["items"][1]["cards"][CARD_NAME]["results"][0]["label"],
        "fail"
    );

    // Filter and sort on a judgement and a metric.
    let filter = r#"{"path":"results[alice/verdicts][core/pass].label","op":"eq","value":"fail"}"#;
    let (status, page) = get(
        &hub,
        &format!(
            "{url}&where={}&sort=metrics[core/tokens_out]:desc",
            encode(filter)
        ),
        Some(&alice),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(run_ids(&page), ["r2"]);

    // Overwrite r1: it is now in the Card's changed_since_card.
    let (status, updated) = hub
        .put_run(&alice, EVAL_NAME, "r1", json!({"core/tokens_out": 9.0}))
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["result"], "updated");
    assert_ne!(updated["content_hash"], put["content_hash"]);

    let (_, page) = get(&hub, &url, Some(&alice)).await;
    let c = &page["cards"][0];
    assert_eq!(c["changed_since_card"], json!(["r1"]));
    assert_ne!(c["used_set_hash"], c["posted_used_set_hash"]);
    assert_eq!(page["items"][0]["cards"][CARD_NAME]["changed"], true);
    assert_eq!(page["items"][1]["cards"][CARD_NAME]["changed"], false);

    // The comparison view reports the same used set.
    let (status, view) = get(
        &hub,
        &format!("/api/v1/evals/{EVAL_NAME}/cards"),
        Some(&alice),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let row = &view["items"][0];
    assert_eq!(row["name"], "verdicts");
    assert_eq!(row["runs_used"], 2);
    assert_eq!(row["changed_since_card"], json!(["r1"]));
    assert_eq!(row["used_set_hash"], c["used_set_hash"]);
}

/// `PUT` with a `run_id` in the body that differs from the path is `422
/// run_id_mismatch`, and a body with no `run_id` is refused by the schema.
#[tokio::test]
async fn put_checks_the_run_id_against_the_path() {
    let (hub, alice) = hub_with_eval(Hub::start().await).await;
    hub.ready_attachments(common::RUN).await;
    let (status, body) = hub
        .call(
            Method::PUT,
            &format!("/api/v1/evals/{EVAL_NAME}/runs/other"),
            Some(&alice),
            Some(common::RUN),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let codes: Vec<&str> = body["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["run_id_mismatch"], "{body}");
    assert_eq!(body["errors"][0]["path"], "/run_id");

    let mut run: Value = serde_json::from_str(common::RUN).unwrap();
    run.as_object_mut().unwrap().remove("run_id");
    let (status, body) = hub
        .call(
            Method::PUT,
            &format!("/api/v1/evals/{EVAL_NAME}/runs/r1"),
            Some(&alice),
            Some(&run.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    // The schema requires `run_id`; the validator reports it at the root.
    assert_eq!(body["errors"].as_array().unwrap().len(), 1, "{body}");
    assert_eq!(body["errors"][0]["code"], "schema", "{body}");
    assert_eq!(body["errors"][0]["path"], "", "{body}");

    // No live Eval: 404.
    let (status, _) = hub
        .call(
            Method::PUT,
            "/api/v1/evals/alice/nothing/runs/r1",
            Some(&alice),
            Some(common::RUN),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A batch with invalid elements writes nothing and lists every failing
/// element by index; a valid batch writes all.
#[tokio::test]
async fn a_batch_is_all_or_nothing() {
    let (hub, alice) = hub_with_eval(Hub::start().await).await;
    let ok = |id: &str| json!({"run_id": id, "status": "ok", "metrics": {"core/x": 1.0}});
    let body = json!({"runs": [
        ok("b1"),
        {"run_id": "b2", "status": "error"},
        ok("b3"),
        ok("b1"),
    ]});
    let (status, err) = hub
        .call(
            Method::POST,
            &format!("/api/v1/evals/{EVAL_NAME}/runs:batch"),
            Some(&alice),
            Some(&body.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    let errors = err["errors"].as_array().unwrap();
    assert!(
        errors
            .iter()
            .any(|e| e["path"] == "/runs/1/error" && e["code"] == "run_status_detail"),
        "{err}"
    );
    assert!(
        errors
            .iter()
            .any(|e| e["path"] == "/runs/3/run_id" && e["code"] == "batch_duplicate_run_id"),
        "{err}"
    );
    assert!(
        errors
            .iter()
            .all(|e| e["path"].as_str().unwrap().starts_with("/runs/1")
                || e["path"].as_str().unwrap().starts_with("/runs/3")),
        "{err}"
    );
    let (_, page) = get(
        &hub,
        &format!("/api/v1/evals/{EVAL_NAME}/runs"),
        Some(&alice),
    )
    .await;
    assert_eq!(page["items"], json!([]), "nothing was written: {page}");

    let body = json!({"runs": [ok("b1"), ok("b2")]});
    let (status, done) = hub
        .call(
            Method::POST,
            &format!("/api/v1/evals/{EVAL_NAME}/runs:batch"),
            Some(&alice),
            Some(&body.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["runs"][0]["run_id"], "b1");
    assert_eq!(done["runs"][0]["result"], "created");
    assert_eq!(done["runs"][1]["result"], "created");
    let (_, page) = get(
        &hub,
        &format!("/api/v1/evals/{EVAL_NAME}/runs"),
        Some(&alice),
    )
    .await;
    assert_eq!(run_ids(&page), ["b1", "b2"]);
    let (_, eval) = get(&hub, &format!("/api/v1/evals/{EVAL_NAME}"), Some(&alice)).await;
    assert_eq!(eval["runs"]["runs_hash"], done["runs_hash"]);
}

/// The status of a refused batch follows its entries: only `409` codes
/// (a deleted id) is `409`, any validation failure makes it `422`, and
/// every entry is named by its element's index either way.
#[tokio::test]
async fn a_refused_batch_is_409_only_for_conflicts() {
    let (hub, alice) = hub_with_eval(Hub::start().await).await;
    let batch = format!("/api/v1/evals/{EVAL_NAME}/runs:batch");
    let ok = |id: &str| json!({"run_id": id, "status": "ok"});
    let (status, _) = hub
        .call(
            Method::POST,
            &batch,
            Some(&alice),
            Some(&json!({"runs": [ok("gone")]}).to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = hub
        .call(
            Method::DELETE,
            &format!("/api/v1/evals/{EVAL_NAME}/runs/gone"),
            Some(&alice),
            Some(r#"{"reason":"withdrawn"}"#),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, err) = hub
        .call(
            Method::POST,
            &batch,
            Some(&alice),
            Some(&json!({"runs": [ok("fresh"), ok("gone")]}).to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    assert_eq!(
        err["errors"],
        json!([{
            "path": "/runs/1/run_id",
            "code": "run_deleted",
            "hint": err["errors"][0]["hint"],
        }]),
        "{err}"
    );

    let body = json!({"runs": [ok("gone"), ok("fresh"), {"run_id": "bad", "status": "error"}]});
    let (status, err) = hub
        .call(Method::POST, &batch, Some(&alice), Some(&body.to_string()))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    let entries: Vec<(&str, &str)> = err["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["path"].as_str().unwrap(), e["code"].as_str().unwrap()))
        .collect();
    assert_eq!(
        entries,
        [
            ("/runs/0/run_id", "run_deleted"),
            ("/runs/2/error", "run_status_detail"),
        ],
        "{err}"
    );

    // Neither refused batch wrote `fresh`.
    let (_, page) = get(
        &hub,
        &format!("/api/v1/evals/{EVAL_NAME}/runs"),
        Some(&alice),
    )
    .await;
    assert_eq!(page["items"], json!([]), "{page}");
}

/// A run naming an attachment that is not uploaded and confirmed is
/// `409 attachment_missing` at that attachment's `sha256`.
#[tokio::test]
async fn a_run_with_an_unconfirmed_attachment_is_a_conflict() {
    let (hub, alice) = hub_with_eval(Hub::start().await).await;
    hub.ready_attachments(common::RUN).await;
    let mut run: Value = serde_json::from_str(common::RUN).unwrap();
    run["attachments"][1]["sha256"] = json!("ff".repeat(32));
    let (status, err) = hub
        .call(
            Method::PUT,
            &format!("/api/v1/evals/{EVAL_NAME}/runs/r1"),
            Some(&alice),
            Some(&run.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    assert_eq!(err["errors"].as_array().unwrap().len(), 1, "{err}");
    assert_eq!(err["errors"][0]["code"], "attachment_missing");
    assert_eq!(err["errors"][0]["path"], "/attachments/1/sha256");
}

/// A 2.0 header carrying `runs` is `422` with the single `runs_moved`.
#[tokio::test]
async fn a_2_0_header_with_runs_is_runs_moved() {
    let (hub, alice) = hub_with_eval(Hub::start().await).await;
    let mut body: Value = serde_json::from_str(EVAL).unwrap();
    body["runs"] = json!([{"run_id": "r1", "outcome": "pass"}]);
    body["unknown_key"] = json!(true);
    let (status, err) = hub
        .call(
            Method::POST,
            &format!("/api/v1/evals/{EVAL_NAME}"),
            Some(&alice),
            Some(&body.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["errors"].as_array().unwrap().len(), 1, "{err}");
    assert_eq!(err["errors"][0]["code"], "runs_moved");
}

/// A 1.0 body is converted: `201` with `Deprecation`, `converted_from`
/// and the runs; the runs are on `GET …/runs`; the same body again is a
/// no-op; a run the run rules refuse is a `422`, not a `500`.
#[tokio::test]
async fn a_1_0_body_is_converted_for_one_release() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(common::EVAL_V1).await;
    let url = "/api/v1/evals/alice/legacy";
    let (status, headers, env) = hub
        .call_full(Method::POST, url, Some(&alice), Some(common::EVAL_V1))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{env}");
    assert_eq!(
        headers.get("deprecation").map(|v| v.to_str().unwrap()),
        Some("@1790294400")
    );
    assert_eq!(env["converted_from"], "evalhub.eval/1.0");
    assert_eq!(env["converted_runs"]["runs"][0]["run_id"], "r1");
    assert_eq!(env["converted_runs"]["runs"][0]["result"], "created");

    // The stored version is the 2.0 header, and content_hash is its hash.
    let (_, read) = get(&hub, url, Some(&alice)).await;
    assert_eq!(read["record"]["schema"], "evalhub.eval/2.0");
    assert!(read["record"].get("runs").is_none(), "{read}");
    assert_eq!(read["content_hash"], env["content_hash"]);
    assert_eq!(read["runs"]["count"], 1);
    assert_eq!(
        read["runs"]["runs_hash"],
        env["converted_runs"]["runs_hash"]
    );

    let (_, page) = get(&hub, &format!("{url}/runs"), Some(&alice)).await;
    assert_eq!(run_ids(&page), ["r1"]);
    assert_eq!(page["items"][0]["status"], "ok");

    let (status, headers, again) = hub
        .call_full(Method::POST, url, Some(&alice), Some(common::EVAL_V1))
        .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert!(headers.contains_key("deprecation"));
    assert_eq!(again["seq"], env["seq"]);
    assert_eq!(again["converted_runs"]["runs"][0]["result"], "unchanged");

    let mut bad: Value = serde_json::from_str(common::EVAL_V1).unwrap();
    bad["runs"][0]["run_id"] = json!("a/b");
    let (status, err) = hub
        .call(
            Method::POST,
            "/api/v1/evals/alice/legacy-bad",
            Some(&alice),
            Some(&bad.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert!(
        err["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["code"] == "run_id_invalid" && e["path"] == "/runs/0/run_id"),
        "{err}"
    );
}

/// A public Card whose run_results judge runs of a private Eval shows an
/// outsider no run id and no judgement, on `GET` and on
/// `POST /cards/query`, and says how many were withheld.
#[tokio::test]
async fn judgements_of_a_private_eval_are_withheld() {
    let (hub, alice) = hub_with_eval(Hub::start().await).await;
    let bob = hub.user("bob", Scope::Write).await;
    let (status, card) = hub.post_card_with_runs(&alice, EVAL_NAME, CARD_NAME).await;
    assert_eq!(status, StatusCode::CREATED, "{card}");
    make_public(&hub, &alice, "cards", CARD_NAME).await;

    let query = json!({"where": {"path": "title", "op": "contains", "value": "verdicts"}});
    for reader in [Some(bob.as_str()), None] {
        let (status, env) = get(&hub, &format!("/api/v1/cards/{CARD_NAME}"), reader).await;
        assert_eq!(status, StatusCode::OK, "{env}");
        assert_eq!(env["record"]["run_results"], json!([]), "{env}");
        assert_eq!(env["withheld"]["run_results"], 2, "{env}");
        assert_eq!(env["withheld"]["relations"].as_array().unwrap().len(), 1);
        let text = env.to_string();
        assert!(!text.contains("single2-k4"), "{text}");
        assert!(!text.contains("\"r1\""), "{text}");
        assert!(!text.contains("\"pass\""), "{text}");

        let (status, page) = hub
            .call(
                Method::POST,
                "/api/v1/cards/query",
                reader,
                Some(&query.to_string()),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        let hit = &page["items"][0];
        assert_eq!(hit["name"], "verdicts", "{page}");
        assert_eq!(hit["record"]["run_results"], json!([]), "{page}");
        assert_eq!(hit["withheld"]["run_results"], 2, "{page}");
        assert!(!page.to_string().contains("single2-k4"), "{page}");
    }

    // Export projects results, model and task only: no run id, no verdict,
    // no Eval name, for anyone.
    for reader in [Some(bob.as_str()), Some(alice.as_str())] {
        let (status, yaml) = hub
            .call_text(
                Method::GET,
                &format!("/api/v1/cards/{CARD_NAME}/export?format=hf-model-index"),
                reader,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{yaml}");
        assert!(!yaml.contains("run_results"), "{yaml}");
        assert!(!yaml.contains("single2-k4"), "{yaml}");
    }

    // The owner reads the stored body.
    let (_, env) = get(&hub, &format!("/api/v1/cards/{CARD_NAME}"), Some(&alice)).await;
    assert!(env.get("withheld").is_none(), "{env}");
    assert_eq!(env["record"]["run_results"].as_array().unwrap().len(), 2);

    // Once the Eval is public, nothing is withheld from bob either.
    make_public(&hub, &alice, "evals", EVAL_NAME).await;
    let (_, env) = get(&hub, &format!("/api/v1/cards/{CARD_NAME}"), Some(&bob)).await;
    assert!(env.get("withheld").is_none(), "{env}");
    assert_eq!(env["record"]["run_results"].as_array().unwrap().len(), 2);
}

/// Every run route of a private Eval is 404 for an outsider, including
/// after `PATCH …/settings` makes a public one private again.
#[tokio::test]
async fn runs_of_a_private_eval_are_not_found() {
    let (hub, alice) = hub_with_eval(Hub::start().await).await;
    let bob = hub.user("bob", Scope::Write).await;
    let (status, _) = hub
        .put_run(&alice, EVAL_NAME, "r1", json!({"core/tokens_out": 1.0}))
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let runs = format!("/api/v1/evals/{EVAL_NAME}/runs");
    let one = format!("{runs}/r1");
    let outsider_sees_nothing = |label: &'static str| {
        let (hub, bob, runs, one) = (&hub, &bob, &runs, &one);
        async move {
            for reader in [Some(bob.as_str()), None] {
                assert_eq!(
                    get(hub, runs, reader).await.0,
                    StatusCode::NOT_FOUND,
                    "{label}"
                );
                assert_eq!(
                    get(hub, one, reader).await.0,
                    StatusCode::NOT_FOUND,
                    "{label}"
                );
            }
            // Writes by someone without write on alice: 404, not 403.
            let (status, _) = hub
                .put_run(bob, EVAL_NAME, "r9", json!({"core/tokens_out": 1.0}))
                .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{label}");
            let (status, body) = hub
                .call(Method::PATCH, one, Some(bob), Some(r#"{"archived":true}"#))
                .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{label}");
            assert_eq!(body, Value::Null, "{label}: empty 404");
            let (status, body) = hub
                .call(
                    Method::DELETE,
                    one,
                    Some(bob),
                    Some(r#"{"reason":"withdrawn"}"#),
                )
                .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{label}");
            assert_eq!(body, Value::Null, "{label}: empty 404");
            let (status, body) = hub
                .call(
                    Method::POST,
                    &format!("{runs}:batch"),
                    Some(bob),
                    Some(r#"{"runs":[{"run_id":"r9","status":"ok"}]}"#),
                )
                .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{label}");
            assert_eq!(body, Value::Null, "{label}: empty 404");
        }
    };
    outsider_sees_nothing("private").await;

    make_public(&hub, &alice, "evals", EVAL_NAME).await;
    assert_eq!(get(&hub, &runs, Some(&bob)).await.0, StatusCode::OK);
    assert_eq!(get(&hub, &one, None).await.0, StatusCode::OK);
    // A public Eval: bob may read but not write, and is told so.
    let (status, _) = hub
        .put_run(&bob, EVAL_NAME, "r9", json!({"core/tokens_out": 1.0}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    set_visibility(&hub, &alice, "evals", EVAL_NAME, "private").await;
    outsider_sees_nothing("private again").await;
}

/// A Card named in `cards=` that the caller may not see, that does not
/// exist, or that does not use the Eval, is the same 404.
#[tokio::test]
async fn cards_the_caller_cannot_use_are_not_found() {
    let (hub, alice) = hub_with_eval(Hub::start().await).await;
    let bob = hub.user("bob", Scope::Write).await;
    let (status, _) = hub.post_card_with_runs(&alice, EVAL_NAME, CARD_NAME).await;
    assert_eq!(status, StatusCode::CREATED);
    make_public(&hub, &alice, "evals", EVAL_NAME).await;
    // A Card that exists, is public, and does not use this Eval.
    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/unrelated",
            Some(&alice),
            Some(CARD),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    make_public(&hub, &alice, "cards", "alice/unrelated").await;

    let runs = format!("/api/v1/evals/{EVAL_NAME}/runs");
    let (status, _) = get(&hub, &format!("{runs}?cards={CARD_NAME}"), Some(&alice)).await;
    assert_eq!(status, StatusCode::OK);
    for (who, card) in [
        (Some(bob.as_str()), CARD_NAME),
        (None, CARD_NAME),
        (Some(bob.as_str()), "alice/no-such-card"),
        (Some(bob.as_str()), "alice/unrelated"),
        (Some(bob.as_str()), "not-a-card-ref"),
    ] {
        let (status, body) = get(&hub, &format!("{runs}?cards={card}"), who).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{card}");
        assert_eq!(body, Value::Null, "one shape: no body ({card})");
    }
}

/// Above `limits.body_bytes` and above `limits.batch_runs`: 413 with the
/// error envelope; above `limits.run_results`: 422 too_many_run_results.
#[tokio::test]
async fn limits_answer_with_the_envelope() {
    let hub = Hub::start_with_config(|c| {
        c.limits.body_bytes = 4096;
        c.limits.batch_runs = 2;
        c.limits.run_results = 1;
    })
    .await;
    let (hub, alice) = hub_with_eval(hub).await;

    let big = json!({"runs": [{"run_id": "x", "status": "ok", "meta": "a".repeat(8192)}]});
    let (status, err) = hub
        .call(
            Method::POST,
            &format!("/api/v1/evals/{EVAL_NAME}/runs:batch"),
            Some(&alice),
            Some(&big.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{err}");
    assert_eq!(err["errors"][0]["code"], "body_too_large", "{err}");

    let ok = |id: &str| json!({"run_id": id, "status": "ok"});
    let three = json!({"runs": [ok("a"), ok("b"), ok("c")]});
    let (status, err) = hub
        .call(
            Method::POST,
            &format!("/api/v1/evals/{EVAL_NAME}/runs:batch"),
            Some(&alice),
            Some(&three.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{err}");
    assert_eq!(err["errors"][0]["code"], "batch_too_large", "{err}");
    assert_eq!(err["errors"][0]["path"], "/runs");

    // The Card fixture carries two run_results; the limit is one.
    let (status, err) = hub.post_card_with_runs(&alice, EVAL_NAME, CARD_NAME).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["errors"][0]["code"], "too_many_run_results", "{err}");
    assert_eq!(err["errors"].as_array().unwrap().len(), 1);
}

/// An archived run is absent from the projection and 404 on its own for a
/// non-member, and present for a member who asks with `include=archived`.
#[tokio::test]
async fn archived_runs_are_for_members() {
    let (hub, alice) = hub_with_eval(Hub::start().await).await;
    let bob = hub.user("bob", Scope::Write).await;
    make_public(&hub, &alice, "evals", EVAL_NAME).await;
    for id in ["r1", "r2"] {
        let (status, _) = hub
            .put_run(&alice, EVAL_NAME, id, json!({"core/tokens_out": 1.0}))
            .await;
        assert_eq!(status, StatusCode::CREATED);
    }
    let one = format!("/api/v1/evals/{EVAL_NAME}/runs/r1");
    let (status, run) = hub
        .call(
            Method::PATCH,
            &one,
            Some(&alice),
            Some(r#"{"archived":true}"#),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{run}");
    assert_eq!(run["archived"], true);

    let runs = format!("/api/v1/evals/{EVAL_NAME}/runs");
    for reader in [Some(bob.as_str()), None] {
        let (_, page) = get(&hub, &runs, reader).await;
        assert_eq!(run_ids(&page), ["r2"]);
        // Asking changes nothing for a non-member.
        let (_, page) = get(&hub, &format!("{runs}?include=archived,deleted"), reader).await;
        assert_eq!(run_ids(&page), ["r2"]);
        assert_eq!(get(&hub, &one, reader).await.0, StatusCode::NOT_FOUND);
    }

    let (_, page) = get(&hub, &runs, Some(&alice)).await;
    assert_eq!(run_ids(&page), ["r2"]);
    let (_, page) = get(&hub, &format!("{runs}?include=archived"), Some(&alice)).await;
    assert_eq!(run_ids(&page), ["r1", "r2"]);
    assert_eq!(page["items"][0]["state"], "archived");
    assert_eq!(page["items"][0]["metrics"]["core/tokens_out"], 1.0);
    let (status, run) = get(&hub, &one, Some(&alice)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(run["archived"], true);

    let (status, run) = hub
        .call(
            Method::PATCH,
            &one,
            Some(&alice),
            Some(r#"{"archived":false}"#),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{run}");
    assert_eq!(get(&hub, &one, Some(&bob)).await.0, StatusCode::OK);
}

/// An archived run's attachment is downloadable by a member only; a live
/// run of a public Eval lets anyone download it.
#[tokio::test]
async fn an_archived_runs_attachment_is_for_members() {
    let (hub, alice) = hub_with_eval(Hub::start_with_storage().await).await;
    let bob = hub.user("bob", Scope::Write).await;
    make_public(&hub, &alice, "evals", EVAL_NAME).await;
    let (status, _) = hub
        .put_run(&alice, EVAL_NAME, "r1", json!({"core/tokens_out": 1.0}))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    // The diff is referenced by the run only (the header references the
    // other object of the fixture as well).
    let run: Value = serde_json::from_str(common::RUN).unwrap();
    let sha = run["attachments"][1]["sha256"].as_str().unwrap();
    let download = format!("/api/v1/attachments/{sha}");

    for reader in [Some(bob.as_str()), None, Some(alice.as_str())] {
        assert_eq!(get(&hub, &download, reader).await.0, StatusCode::FOUND);
    }
    let (status, _) = hub
        .call(
            Method::PATCH,
            &format!("/api/v1/evals/{EVAL_NAME}/runs/r1"),
            Some(&alice),
            Some(r#"{"archived":true}"#),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        get(&hub, &download, Some(&bob)).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(get(&hub, &download, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        get(&hub, &download, Some(&alice)).await.0,
        StatusCode::FOUND
    );
}

/// A deleted run reads as `{ run_id, content_hash, tombstone }`; it is not
/// deleted twice, archived or written again.
#[tokio::test]
async fn a_deleted_run_keeps_its_commitment() {
    let (hub, alice) = hub_with_eval(Hub::start().await).await;
    let (_, put) = hub
        .put_run(&alice, EVAL_NAME, "r1", json!({"core/tokens_out": 1.0}))
        .await;
    let one = format!("/api/v1/evals/{EVAL_NAME}/runs/r1");
    let (status, run) = hub
        .call(
            Method::DELETE,
            &one,
            Some(&alice),
            Some(r#"{"reason":"withdrawn","note":"bad seed"}"#),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{run}");
    let (status, read) = get(&hub, &one, Some(&alice)).await;
    assert_eq!(status, StatusCode::OK);
    for body in [&run, &read] {
        let keys: Vec<&str> = body
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["content_hash", "run_id", "tombstone"], "{body}");
        assert_eq!(body["content_hash"], put["content_hash"]);
        assert_eq!(body["tombstone"]["reason"], "withdrawn");
        assert_eq!(body["tombstone"]["note"], "bad seed");
    }

    let (status, err) = hub
        .call(
            Method::DELETE,
            &one,
            Some(&alice),
            Some(r#"{"reason":"withdrawn"}"#),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    assert_eq!(err["errors"][0]["code"], "run_deleted");
    let (status, err) = hub
        .call(
            Method::PATCH,
            &one,
            Some(&alice),
            Some(r#"{"archived":true}"#),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    let (status, err) = hub
        .put_run(&alice, EVAL_NAME, "r1", json!({"core/tokens_out": 2.0}))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    assert_eq!(err["errors"][0]["code"], "run_deleted");
    assert_eq!(err["errors"][0]["path"], "/run_id");

    // The projection lists it only when a member asks.
    let runs = format!("/api/v1/evals/{EVAL_NAME}/runs");
    let (_, page) = get(&hub, &runs, Some(&alice)).await;
    assert_eq!(page["items"], json!([]));
    let (_, page) = get(&hub, &format!("{runs}?include=deleted"), Some(&alice)).await;
    assert_eq!(run_ids(&page), ["r1"]);
    assert_eq!(page["items"][0]["state"], "deleted");
    assert!(page["items"][0].get("status").is_none(), "{page}");

    // Unknown run: 404.
    let (status, _) = hub
        .call(
            Method::DELETE,
            &format!("{runs}/nope"),
            Some(&alice),
            Some(r#"{"reason":"withdrawn"}"#),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// The runs summary on `GET /evals/{ns}/{name}[@seq]`: counts of live
/// runs by status, the one `runs_hash`, and archived / deleted counts for
/// members only; the same at every `@seq`.
#[tokio::test]
async fn the_eval_carries_a_runs_summary() {
    let (hub, alice) = hub_with_eval(Hub::start().await).await;
    let bob = hub.user("bob", Scope::Write).await;
    make_public(&hub, &alice, "evals", EVAL_NAME).await;
    let url = format!("/api/v1/evals/{EVAL_NAME}");

    let (_, env) = get(&hub, &url, Some(&alice)).await;
    assert_eq!(env["runs"]["count"], 0, "{env}");
    let empty_hash = env["runs"]["runs_hash"].clone();
    assert_eq!(empty_hash.as_str().unwrap().len(), 64);

    let body = json!({"runs": [
        {"run_id": "a", "status": "ok"},
        {"run_id": "b", "status": "ok"},
        {"run_id": "c", "status": "error", "error": {"kind": "timeout"}},
        {"run_id": "d", "status": "skipped", "skip_reason": "flaky"},
        {"run_id": "e", "status": "ok"},
    ]});
    let (status, batch) = hub
        .call(
            Method::POST,
            &format!("{url}/runs:batch"),
            Some(&alice),
            Some(&body.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{batch}");
    hub.call(
        Method::PATCH,
        &format!("{url}/runs/b"),
        Some(&alice),
        Some(r#"{"archived":true}"#),
    )
    .await;
    hub.call(
        Method::DELETE,
        &format!("{url}/runs/e"),
        Some(&alice),
        Some(r#"{"reason":"duplicate"}"#),
    )
    .await;

    // A second header version: the summary is the record's, at any @seq.
    let (status, _) = hub
        .call(
            Method::POST,
            &url,
            Some(&alice),
            Some(&common::with_title(EVAL, "second")),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);

    for addr in [url.clone(), format!("{url}@1"), format!("{url}@2")] {
        let (_, env) = get(&hub, &addr, Some(&alice)).await;
        let runs = &env["runs"];
        assert_eq!(runs["count"], 3, "{addr}: {env}");
        assert_eq!(
            runs["by_status"],
            json!({"ok": 1, "error": 1, "skipped": 1})
        );
        assert_eq!(runs["archived"], 1);
        assert_eq!(runs["deleted"], 1);
        assert_eq!(runs["runs_hash"], batch["runs_hash"]);
        assert_ne!(runs["runs_hash"], empty_hash);

        let (_, env) = get(&hub, &addr, Some(&bob)).await;
        let runs = &env["runs"];
        assert_eq!(runs["count"], 3, "{addr}: {env}");
        assert!(runs.get("archived").is_none(), "{env}");
        assert!(runs.get("deleted").is_none(), "{env}");
        assert_eq!(runs["runs_hash"], batch["runs_hash"]);
    }

    // A Card has no runs block.
    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/c",
            Some(&alice),
            Some(CARD),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, env) = get(&hub, "/api/v1/cards/alice/c", Some(&alice)).await;
    assert!(env.get("runs").is_none(), "{env}");
}

/// Pages of the projection chain through a signed cursor bound to the Eval
/// and the sort.
#[tokio::test]
async fn the_projection_pages_by_cursor() {
    let (hub, alice) = hub_with_eval(Hub::start().await).await;
    let runs: Vec<Value> = (0..5)
        .map(|i| json!({"run_id": format!("p{i}"), "status": "ok", "metrics": {"core/x": f64::from(i % 2)}}))
        .collect();
    let url = format!("/api/v1/evals/{EVAL_NAME}/runs");
    let (status, _) = hub
        .call(
            Method::POST,
            &format!("{url}:batch"),
            Some(&alice),
            Some(&json!({ "runs": runs }).to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let sort = "sort=metrics[core/x]:desc";
    let mut seen = Vec::new();
    let mut next = format!("{url}?{sort}&limit=2");
    loop {
        let (status, page) = get(&hub, &next, Some(&alice)).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        seen.extend(run_ids(&page));
        match page["next_cursor"].as_str() {
            Some(c) => next = format!("{url}?{sort}&limit=2&cursor={c}"),
            None => break,
        }
    }
    // `run_id` breaks ties in the last key's direction (descending here).
    assert_eq!(seen, ["p3", "p1", "p4", "p2", "p0"]);

    // A cursor for another sort is refused.
    let (_, page) = get(&hub, &format!("{url}?{sort}&limit=2"), Some(&alice)).await;
    let c = page["next_cursor"].as_str().unwrap().to_string();
    let (status, body) = get(&hub, &format!("{url}?limit=2&cursor={c}"), Some(&alice)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, Value::Null, "a 400 carries no body");

    // A cursor minted on this Eval is refused on another one, even with
    // the same sort and runs that would page the same way.
    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/evals/alice/other",
            Some(&alice),
            Some(EVAL),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/evals/alice/other/runs:batch",
            Some(&alice),
            Some(&json!({ "runs": runs }).to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let other = "/api/v1/evals/alice/other/runs";
    let (status, _) = get(&hub, &format!("{other}?{sort}&limit=2"), Some(&alice)).await;
    assert_eq!(status, StatusCode::OK, "the other Eval pages on its own");
    let (status, body) = get(
        &hub,
        &format!("{other}?{sort}&limit=2&cursor={c}"),
        Some(&alice),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, Value::Null);

    // A record list cursor is not a run cursor, and the reverse.
    let (_, list) = get(&hub, "/api/v1/evals?limit=1", Some(&alice)).await;
    let record_cursor = list["next_cursor"].as_str().unwrap().to_string();
    let (status, body) = get(&hub, &format!("{url}?cursor={record_cursor}"), Some(&alice)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, Value::Null);
    let (status, body) = get(
        &hub,
        &format!("/api/v1/evals?limit=1&cursor={c}"),
        Some(&alice),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, Value::Null);
    // Nor is a record query cursor.
    let (status, page) = hub
        .call(
            Method::POST,
            "/api/v1/evals/query",
            Some(&alice),
            Some(r#"{"limit":1}"#),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let query_cursor = page["next_cursor"].as_str().unwrap().to_string();
    let (status, body) = get(&hub, &format!("{url}?cursor={query_cursor}"), Some(&alice)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, Value::Null);
    // A typo in a path is a 422, not an empty page.
    let (status, err) = get(&hub, &format!("{url}?sort=metricz"), Some(&alice)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["errors"][0]["code"], "unknown_path");
    // An unknown query key is a 400.
    let (status, _) = get(&hub, &format!("{url}?card={CARD_NAME}"), Some(&alice)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// `run.*` and `migration.*` rows are in `GET /audit`.
#[tokio::test]
async fn run_and_migration_actions_are_audited() {
    let (hub, alice) = hub_with_eval(Hub::start().await).await;
    let one = format!("/api/v1/evals/{EVAL_NAME}/runs/r1");
    hub.put_run(&alice, EVAL_NAME, "r1", json!({"core/tokens_out": 1.0}))
        .await;
    hub.put_run(&alice, EVAL_NAME, "r1", json!({"core/tokens_out": 2.0}))
        .await;
    hub.call(
        Method::PATCH,
        &one,
        Some(&alice),
        Some(r#"{"archived":true}"#),
    )
    .await;
    hub.call(
        Method::PATCH,
        &one,
        Some(&alice),
        Some(r#"{"archived":false}"#),
    )
    .await;
    hub.call(
        Method::DELETE,
        &one,
        Some(&alice),
        Some(r#"{"reason":"withdrawn"}"#),
    )
    .await;

    // A 0.1.x row, converted by the data migration as `evalhub migrate`
    // does on an old database: its audit row lands in alice's log.
    seed_v1_eval(&hub, "old").await;
    sqlx::query("DELETE FROM data_migrations WHERE name = $1")
        .bind(evalhub_store::data_migrations::RUNS_SPLIT)
        .execute(&hub.pool)
        .await
        .unwrap();
    evalhub_store::pool::migrate(&hub.pool).await.unwrap();

    let (status, page) = get(&hub, "/api/v1/audit?ns=alice&limit=200", Some(&alice)).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let actions: Vec<(&str, &str)> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["action"].as_str().unwrap(),
                e["subject"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    let subject = "eval/alice/single2-k4/runs/r1";
    for action in [
        "run.create",
        "run.update",
        "run.archive",
        "run.unarchive",
        "run.delete",
    ] {
        assert!(
            actions.contains(&(action, subject)),
            "{action} missing: {actions:?}"
        );
    }
    assert!(
        actions.contains(&("migration.runs_split", "eval/alice/old@1")),
        "{actions:?}"
    );
}

/// Insert `eval/alice/{name}` with one live `evalhub.eval/1.0` version, as
/// 0.1.x stored it, with plain SQL: the 0.2.0 write path converts such a
/// body on the way in and can no longer produce the row.
async fn seed_v1_eval(hub: &Hub, name: &str) {
    let body: Value = serde_json::from_str(common::EVAL_V1).unwrap();
    let (bytes, hash) = evalhub_core::canonical::hash_value(&body).unwrap();
    let canonical: Value = serde_json::from_slice(&bytes).unwrap();
    let record_id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO records (id, type, ns, name) VALUES ($1, 'eval', 'alice', $2)")
        .bind(record_id)
        .bind(name)
        .execute(&hub.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO versions (version_id, record_id, seq, content_hash, body)
         VALUES ($1, $2, 1, $3, $4)",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(record_id)
    .bind(hash.as_bytes().as_slice())
    .bind(canonical)
    .execute(&hub.pool)
    .await
    .unwrap();
}

/// Percent-encode a query-string value.
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
