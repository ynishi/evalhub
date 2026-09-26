//! The HTTP handlers, one module per resource group, all under `/api/v1`.
//!
//! | Group                    | Endpoints                                                                                                   |
//! | ------------------------ | ----------------------------------------------------------------------------------------------------------- |
//! | [`meta`]                 | `GET /openapi.json`, `GET /schemas/{name}`, `GET /whoami`, `GET /healthz`                                   |
//! | [`auth`]                 | `POST,DELETE /session`, `GET,POST,DELETE /tokens`, `GET /namespaces/{ns}`, `GET,POST,DELETE /orgs/{org}/members` |
//! | [`records`]              | `POST /{cards\|evals}/{ns}/{name}?label=`, `GET …/{name}[@{seq}\|@{label}]?expand=`, `GET …/versions`, `DELETE …@{…}`, `PATCH …@{…}/label`, `PATCH …/settings`, `GET /{cards\|evals}?ns&search&sort&cursor` |
//! | [`runs`]                 | `PUT,GET,PATCH,DELETE /evals/{ns}/{name}/runs/{run_id}`, `POST /evals/{ns}/{name}/runs:batch`, `GET /evals/{ns}/{name}/runs?cards&where&sort&limit&cursor&include` |
//! | [`attachments`]          | `POST /attachments`, `POST /attachments/{sha256}/complete`, `HEAD,GET /attachments/{sha256}` (GET → 302)    |
//! | [`query`]                | `POST /{cards\|evals}/query`                                                                                |
//! | [`relations`]            | `GET …/{name}/relations?direction&types&depth&follow_latest&version`, `POST …@{…}/relations`, `GET /evals/{ns}/{name}/cards?version&group_by` |
//! | [`registry`]             | `GET /registry/{kind}?ns`, `GET,PUT /registry/{harnesses\|metrics\|relation_types\|ext_schemas}/{ns}/{id}@{version}` (`core/` read-only) |
//! | [`export`]               | `GET …/{name}[@…]/export?format=hf-model-index` (`bundle` is `501`, see [`export`]) |
//! | [`audit`]                | `GET /audit?ns&cursor`                                                                                      |
//!
//! Statuses: `201` created, `202` accepted (an `ext_schema` whose indexes
//! are building), `200` idempotent hit or read, `409`
//! (`attachment_missing`, `label_in_use`, `registry_entry_exists`,
//! `run_deleted`), `413` (`body_too_large`, `batch_too_large`), `422`
//! (`errors[]`), `404` (including private), `403` (scope), `401` (token),
//! `400` (not JSON, bad cursor), `501` (a format the hub has not decided
//! on), `503` (a capability this deployment lacks). See [`crate::error`].
//!
//! # Request size
//!
//! [`router`] puts axum's `DefaultBodyLimit` at `limits.body_bytes` (16
//! MiB by default; axum's own default, 2 MiB, was the effective cap before
//! 0.2.0) on every route, so every extractor that buffers a body refuses a
//! larger one. axum answers that refusal with `413` and a plain-text body;
//! [`body_too_large`] rewrites it into the error envelope with
//! `body_too_large`, so every `413` the hub sends has the one shape. The
//! handlers' own `413` (`batch_too_large`) is already an envelope and
//! passes through. The other limits are the handlers': the batch count in
//! [`runs`], the `run_results` count in [`records`].
//!
//! Handlers are thin: extract, authorise, call the store or core, map the
//! result. Rules live in the library crates so that they are the same for
//! every caller and testable without HTTP.
//!
//! [`router`] builds the whole tree. Routes registered through `aide`'s
//! `ApiRouter` appear in `openapi.json`; the document itself and the SPA
//! fallback are plain axum routes and do not.

use std::sync::Arc;

use aide::axum::ApiRouter;
use aide::axum::routing::{delete_with, get_with, patch_with, post_with, put_with};
use aide::transform::TransformOperation;
use axum::Json;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower_http::trace::TraceLayer;

use evalhub_schema::openapi::API_PREFIX;
use evalhub_store::PgPool;

use crate::config::Config;
use crate::state::{AppState, PathTableHandle};

pub mod attachments;
pub mod audit;
pub mod auth;
pub mod export;
pub mod meta;
pub mod query;
pub mod records;
pub mod registry;
pub mod relations;
pub mod runs;

/// Build the application router and the OpenAPI document it describes.
///
/// Every route is mounted whether or not `db` and `objects` are present,
/// so the document is the same in every configuration; `serve` refuses to
/// start without a database, a router built with `db: None` (the boot
/// tests) answers `500` on any route that needs one, and one built
/// without an object store answers `503` on the attachment routes.
pub fn router(
    config: Arc<Config>,
    db: Option<PgPool>,
    objects: Option<Arc<evalhub_store::objects::Objects>>,
    path_tables: PathTableHandle,
) -> Router {
    let mut openapi = crate::openapi::skeleton();

    let api = ApiRouter::new()
        .api_route(
            "/healthz",
            get_with(meta::healthz, |op| {
                op.id("healthz")
                    .summary("Liveness")
                    .description("Answers as long as the process is up.")
            }),
        )
        .api_route(
            "/whoami",
            get_with(meta::whoami, |op| {
                op.id("whoami")
                    .summary("Who the hub thinks the caller is")
                    .description("Identity and scope of the presented token, or anonymous.")
            }),
        )
        .api_route(
            "/session",
            post_with(auth::create_session, |op| {
                op.id("create_session")
                    .summary("Open a UI session")
                    .description(
                        "Exchanges a token for a session cookie. The hub has no \
                         passwords, so the credential is a token; the cookie carries a \
                         second token minted for the same user with the same scope and \
                         namespaces, which `DELETE /session` revokes. The presented \
                         token is left alone.",
                    )
                    .response_with::<200, Json<meta::Whoami>, _>(|r| {
                        r.description("The session's identity, as `whoami` reports it.")
                    })
            })
            .delete_with(auth::delete_session, |op| {
                op.id("delete_session")
                    .summary("End a UI session")
                    .description(
                        "Revokes the token the cookie carries and clears the cookie. \
                         Answers `204` whether or not a session was open.",
                    )
                    .response_with::<204, (), _>(|r| {
                        r.description("The session is closed and the cookie cleared.")
                    })
            }),
        )
        .api_route(
            "/tokens",
            get_with(auth::list_tokens, |op| {
                op.id("list_tokens")
                    .summary("The caller's tokens")
                    .description("Secrets are not included; they are shown once, at issue.")
            })
            .post_with(auth::create_token, |op| {
                op.id("create_token").summary("Issue a token").description(
                    "The new token may name the caller's own login and organisations \
                         where the caller is `admin`, and its scope may not exceed the \
                         presenting token's. The secret is in the response and nowhere else.",
                )
            }),
        )
        .api_route(
            "/tokens/{token_id}",
            delete_with(auth::revoke_token, |op| {
                op.id("revoke_token")
                    .summary("Revoke a token")
                    .description("Takes effect on the next request; there is no cache.")
            }),
        )
        .api_route(
            "/namespaces/{ns}",
            get_with(auth::namespace, |op| {
                op.id("get_namespace")
                    .summary("A namespace and what it holds")
                    .description("Counts only the records the caller may see.")
            }),
        )
        .api_route(
            "/orgs",
            post_with(auth::create_org, |op| {
                op.id("create_org")
                    .summary("Create an organisation")
                    .description("The caller becomes its first `admin`.")
            }),
        )
        .api_route(
            "/orgs/{org}/members",
            get_with(auth::list_members, |op| {
                op.id("list_members")
                    .summary("Members of an organisation")
                    .description(
                        "Any role is enough; the presented token must name the organisation.",
                    )
            })
            .post_with(auth::add_member, |op| {
                op.id("add_member")
                    .summary("Add a member or change their role")
                    .description("`admin` on the organisation.")
            }),
        )
        .api_route(
            "/orgs/{org}/members/{user}",
            delete_with(auth::remove_member, |op| {
                op.id("remove_member")
                    .summary("Remove a member")
                    .description("`admin` on the organisation.")
            }),
        )
        .api_route(
            "/audit",
            get_with(audit::list, |op| {
                op.id("list_audit")
                    .summary("Audit log of a namespace")
                    .description(
                        "Append-only: who changed what, when. Newest first, paged by cursor. \
                         Requires `admin` on the namespace.",
                    )
            }),
        )
        .api_route(
            "/attachments",
            post_with(attachments::announce, |op| {
                op.id("announce_attachment")
                    .summary("Announce an upload")
                    .description(
                        "Returns `200 { state: \"ready\" }` when the object is already \
                         confirmed, otherwise `201` with a presigned `PUT` URL. The hub \
                         never receives the bytes: they go straight to the object store. \
                         Any valid token may announce; using the object needs `write` on \
                         the record's namespace.",
                    )
            }),
        )
        .api_route(
            "/attachments/{sha256}/complete",
            post_with(attachments::complete, |op| {
                op.id("complete_attachment")
                    .summary("Confirm an upload")
                    .description(
                        "Checks the object exists with the announced size and, below \
                         `attachments.hash_verify_max_bytes`, that its bytes hash to the \
                         announced sha256. A mismatch is `422` and the object stays \
                         pending.",
                    )
            }),
        )
        .api_route(
            "/attachments/{sha256}",
            get_with(attachments::download, |op| {
                op.id("download_attachment")
                    .summary("Download an attachment")
                    .description(
                        "`302` to a presigned, time-limited URL. Open to everyone when a \
                         public record references the object, to the covering token when \
                         only private ones do, and to any token while the object is not \
                         referenced at all.",
                    )
            })
            .head_with(attachments::head, |op| {
                op.id("head_attachment")
                    .summary("Size and media type of an attachment")
                    .description("The same authorisation as the download, without the redirect.")
            }),
        )
        .api_route(
            "/cards/{ns}/{name}/export",
            get_with(export::export_card, |op| {
                export_docs(op.id("export_card").summary("Export a Card"))
            }),
        )
        .api_route(
            "/evals/{ns}/{name}/export",
            get_with(export::export_eval, |op| {
                export_docs(op.id("export_eval").summary("Export an Eval"))
            }),
        )
        .api_route(
            "/registry/{kind}",
            get_with(registry::list, |op| {
                op.id("list_registry")
                    .summary("The vocabulary of one kind")
                    .description(
                        "Metrics, harnesses, relation types or extension schemas. Public: \
                         the vocabulary is what a client needs to read a record.",
                    )
            }),
        )
        .api_route(
            "/registry/{kind}/{ns}/{id}",
            get_with(registry::get, |op| {
                op.id("get_registry_entry")
                    .summary("One registry entry")
                    .description("`{id}` carries the version: `pass_rate@1`.")
            })
            .put_with(registry::put, |op| {
                op.id("put_registry_entry")
                    .summary("Register a definition")
                    .description(
                        "Needs `write` on `ns`; `core/` is read-only. Entries are \
                         immutable, so a second write of the same address is `409` and a \
                         correction is a new version. An `ext_schemas` entry answers \
                         `202`: its expression indexes are built in the background, and \
                         until they are valid its paths accept `eq` and `exists` only.",
                    )
            }),
        )
        .api_route(
            "/evals/{ns}/{name}/cards",
            get_with(relations::eval_cards, |op| {
                op.id("eval_cards")
                    .summary("Cards measured on this Eval")
                    .description(
                        "Every visible Card whose latest live version has a \
                         `core/uses_eval` edge to this Eval version, with its per-facet \
                         fingerprints, whether its harness and model match those of every \
                         run it used, and its used set: `runs_used`, `used_set_hash` and \
                         `changed_since_card`. `group_by=fingerprint.{facet}` groups them \
                         by that fingerprint. The hub lines the Cards up; it does not rank \
                         them.",
                    )
            }),
        )
        .api_route(
            "/evals/{ns}/{name}/runs",
            get_with(runs::list_runs, |op| {
                op.id("list_runs")
                    .summary("The runs of an Eval, with Cards' judgements")
                    .description(
                        "One page of the Eval's runs: status, error kind, times, metrics and \
                         fingerprints, and for each Card named in `cards=` its judgements of \
                         each run and its used set (`runs_used`, `used_set_hash`, \
                         `changed_since_card`). `where` is the query grammar as JSON text \
                         over the run paths; `sort` is `{path}[:asc|:desc]`, repeatable. \
                         Archived and deleted runs are left out unless a member asks with \
                         `include=archived,deleted`. A Card that is unknown, invisible or \
                         does not use this Eval is `404`, as is a private Eval.",
                    )
            }),
        )
        .api_route(
            "/evals/{ns}/{name}/runs:batch",
            post_with(runs::batch_runs, |op| {
                op.id("batch_runs")
                    .summary("Write several runs, all or nothing")
                    .description(
                        "`{ runs: [Run, …] }`, each carrying its own `run_id`. Every \
                         element is checked as by `PUT …/runs/{run_id}`, in one \
                         transaction: if any fails, nothing is written and every failing \
                         element is listed, its entries' paths prefixed with \
                         `/runs/{index}` (`422`, or `409` when every entry is \
                         `run_deleted` / `attachment_missing`). More than \
                         `limits.batch_runs` runs is `413 batch_too_large`.",
                    )
            }),
        )
        .api_route(
            "/evals/{ns}/{name}/runs/{run_id}",
            put_with(runs::put_run, |op| {
                op.id("put_run")
                    .summary("Write one run")
                    .description(
                        "Creates (`201`) or overwrites (`200`, `result: updated`) the run; \
                         identical content is `200` with `result: unchanged` and writes \
                         nothing. Facets the run omits are copied from the latest header. \
                         The body's `run_id` must equal the path's (`run_id_mismatch`). A \
                         deleted run id is `409 run_deleted`; an attachment not uploaded \
                         and confirmed is `409 attachment_missing`. Needs `write` on `ns`.",
                    )
                    .response_with::<201, Json<runs::RunWrittenDto>, _>(|r| {
                        r.description("The run was created.")
                    })
                    .response_with::<200, Json<runs::RunWrittenDto>, _>(|r| {
                        r.description("The run was overwritten, or already had this content.")
                    })
            })
            .get_with(runs::get_run, |op| {
                op.id("get_run").summary("Read one run").description(
                    "The stored run. An archived run is `404` to anyone who is not a member \
                         of the namespace; a deleted run is `{ run_id, content_hash, \
                         tombstone }`.",
                )
            })
            .patch_with(runs::patch_run, |op| {
                op.id("patch_run")
                    .summary("Archive or unarchive a run")
                    .description(
                        "`{ archived }`. An archived run is kept whole and counted in \
                         `runs_hash`, and hidden from everyone but members. No hash changes.",
                    )
            })
            .delete_with(runs::delete_run, |op| {
                op.id("delete_run").summary("Delete a run").description(
                    "Tombstones the run with `{ reason, note }`: the body and its \
                         attachment references go, the run id, content hash and metrics stay, \
                         and the id is never written again. Deleting twice is `409 run_deleted`.",
                )
            }),
        )
        .api_route(
            "/cards/query",
            post_with(query::query_cards, |op| {
                query_docs(op.id("query_cards").summary("Search Cards"))
            }),
        )
        .api_route(
            "/evals/query",
            post_with(query::query_evals, |op| {
                query_docs(op.id("query_evals").summary("Search Evals"))
            }),
        )
        .merge(relation_routes!(
            "cards",
            "card",
            relations::add_card_relation,
            relations::card_relations
        ))
        .merge(relation_routes!(
            "evals",
            "eval",
            relations::add_eval_relation,
            relations::eval_relations
        ))
        .merge(record_routes!(
            "cards",
            "card",
            "Card",
            records::list_cards,
            records::post_card,
            records::get_card,
            records::delete_card,
            records::versions_card,
            records::label_card,
            records::settings_card
        ))
        .merge(record_routes!(
            "evals",
            "eval",
            "Eval",
            records::list_evals,
            records::post_eval,
            records::get_eval,
            records::delete_eval,
            records::versions_eval,
            records::label_eval,
            records::settings_eval
        ));

    let app = ApiRouter::new()
        .nest(API_PREFIX, api)
        .finish_api(&mut openapi);
    tighten_query_schema(&mut openapi);

    let cursors = crate::auth::CursorSigner::new(
        config
            .auth
            .cursor_key
            .as_ref()
            .map(secrecy::ExposeSecret::expose_secret),
    );
    let cookie_key = crate::state::CookieKey::new(
        config
            .auth
            .cookie_key
            .as_ref()
            .map(secrecy::ExposeSecret::expose_secret),
        &config.bind,
    );
    let state = AppState {
        config,
        db,
        objects,
        openapi: Arc::new(openapi),
        cursors,
        cookie_key,
        path_tables,
    };

    let body_limit = state.config.limits.body_bytes;
    app.route("/openapi.json", get(meta::openapi))
        .route("/schemas/{name}", get(meta::schema))
        .fallback(crate::embed::fallback)
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(axum::middleware::map_response(body_too_large))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Rewrite axum's plain-text `413` (a body above `DefaultBodyLimit`) into
/// the error envelope with `body_too_large`. A `413` that already is JSON
/// (the handlers' `batch_too_large`) is left alone, as is every other
/// response.
pub async fn body_too_large(response: Response) -> Response {
    if response.status() != StatusCode::PAYLOAD_TOO_LARGE {
        return response;
    }
    let is_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if is_json {
        return response;
    }
    crate::error::ApiError::TooLarge(vec![evalhub_schema::error::ErrorEntry {
        path: String::new(),
        code: evalhub_schema::error::ErrorCode::BodyTooLarge,
        hint: Some("the request body is larger than limits.body_bytes".to_string()),
    }])
    .into_response()
}

/// The relation routes both record groups have.
macro_rules! relation_routes {
    ($noun:literal, $id:literal, $add:path, $get:path) => {
        ApiRouter::new().api_route(
            concat!("/", $noun, "/{ns}/{name}/relations"),
            post_with($add, |op| {
                op.id(concat!("add_", $id, "_relation"))
                    .summary("Add an edge")
                    .description(
                        "Addresses the source version with `@{seq}`. `to` is \
                         `{ns}/{name}@{seq}`, `external:<url>` or `hf:<repo>@<sha>`; a \
                         hub reference that does not exist yet is stored textually and \
                         resolves on its own once it does.",
                    )
            })
            .get_with($get, |op| {
                op.id(concat!($id, "_relations"))
                    .summary("Walk the relation graph")
                    .description(
                        "Breadth-first from the addressed version. `direction` is `out` \
                         (default), `in` or `both`; `depth` is 1–5; `types` filters by \
                         relation type. Endpoints the caller may not see are reduced to \
                         their version id and content hash.",
                    )
            }),
        )
    };
}
use relation_routes;

/// The five routes every record group has, so that Cards and Evals cannot
/// drift apart: the two differ only in the handlers they name and in the
/// nouns that appear in the documentation.
macro_rules! record_routes {
    ($noun:literal, $id:literal, $one:literal, $list:path, $post:path, $get:path, $delete:path,
     $versions:path, $label:path, $settings:path) => {
        ApiRouter::new()
            .api_route(
                concat!("/", $noun),
                get_with($list, |op| {
                    op.id(concat!("list_", $noun))
                        .summary(concat!("List ", $noun))
                        .description(
                            "Names with at least one live version, newest first by default. \
                             Private records appear only for a caller whose token covers \
                             their namespace. Paging is by opaque cursor.",
                        )
                }),
            )
            .api_route(
                concat!("/", $noun, "/{ns}/{name}"),
                post_with($post, |op| {
                    post_record_docs(
                        op.id(concat!("post_", $id))
                            .summary(concat!("Append ", $one, " version")),
                    )
                })
                .get_with($get, |op| {
                    get_record_docs(
                        op.id(concat!("get_", $id))
                            .summary(concat!("Read ", $one, " version")),
                    )
                })
                .delete_with($delete, |op| {
                    op.id(concat!("delete_", $id))
                        .summary(concat!("Tombstone ", $one, " version"))
                        .description(
                            "Addresses one version with `@{seq}` and replaces its body with \
                             a tombstone carrying a reason. The version id, the content hash \
                             and `changed[]` remain, so relations pointing at it still \
                             resolve and show what happened.",
                        )
                }),
            )
            .api_route(
                concat!("/", $noun, "/{ns}/{name}/versions"),
                get_with($versions, |op| {
                    op.id(concat!("versions_", $id))
                        .summary("Version history")
                        .description(
                            "Every version of the name, oldest first, tombstones included \
                             and without bodies.",
                        )
                }),
            )
            .api_route(
                concat!("/", $noun, "/{ns}/{name}/label"),
                patch_with($label, |op| {
                    op.id(concat!("label_", $id))
                        .summary("Point a label at a version")
                        .description(
                            "Addresses the target version with `@{seq}`. A label is unique \
                             within the name and never purely numeric; re-pointing moves it \
                             off whichever version held it.",
                        )
                }),
            )
            .api_route(
                concat!("/", $noun, "/{ns}/{name}/settings"),
                patch_with($settings, |op| {
                    op.id(concat!("settings_", $id))
                        .summary("Change visibility")
                        .description("Makes the name public or private. Every version follows.")
                }),
            )
    };
}
use record_routes;

/// Replace the open `where` of the published `QueryRequest` with the
/// filter grammar's own schema.
///
/// The envelope carries `where` as an arbitrary value, because the record
/// crate must not depend on the query language. The document a client
/// generates from should still describe the filter, and the grammar can
/// describe itself, so the two are joined here — the one place that knows
/// both.
///
/// # Why the definitions move
///
/// The grammar's schema is a standalone JSON Schema document: its
/// recursive parts live in a root `$defs` and refer to each other as
/// `#/$defs/filter`. Pasted into `components/schemas/QueryRequest`, those
/// pointers still resolve from the *document* root, where there is no
/// `$defs` — every generator then fails, and the ones that do not produce
/// an unusable type. So each definition is hoisted into
/// `components/schemas` under a prefixed name and every pointer is
/// rewritten to match. A generated client ends up with a real `QueryFilter`
/// type it can name.
fn tighten_query_schema(openapi: &mut aide::openapi::OpenApi) {
    use aide::openapi::SchemaObject;

    let Some(components) = openapi.components.as_mut() else {
        return;
    };
    if !components.schemas.contains_key("QueryRequest") {
        return;
    }

    let mut filter = evalhub_query::request_schema().to_value();
    let defs = filter
        .as_object_mut()
        .and_then(|o| o.remove("$defs"))
        .and_then(|d| match d {
            serde_json::Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default();

    // `filter` becomes `QueryFilter`, so the name says which document it
    // belongs to when it sits beside the record schemas.
    let component_name = |def: &str| {
        let mut chars = def.chars();
        match chars.next() {
            Some(first) => format!("Query{}{}", first.to_uppercase(), chars.as_str()),
            None => "Query".to_string(),
        }
    };

    for (name, mut schema) in defs {
        repoint_defs(&mut schema, &component_name);
        components.schemas.insert(
            component_name(&name),
            SchemaObject {
                json_schema: schemars::Schema::try_from(schema).unwrap_or_default(),
                example: None,
                external_docs: None,
            },
        );
    }
    repoint_defs(&mut filter, &component_name);

    if let Some(request) = components.schemas.get_mut("QueryRequest")
        && let Some(object) = request.json_schema.as_object_mut()
        && let Some(properties) = object.get_mut("properties").and_then(|p| p.as_object_mut())
    {
        properties.insert("where".to_string(), filter);
    }
}

/// Rewrite every `#/$defs/<name>` pointer to the component it was hoisted
/// to, anywhere in the value.
fn repoint_defs(value: &mut serde_json::Value, component_name: &dyn Fn(&str) -> String) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(reference)) = map.get("$ref")
                && let Some(def) = reference.strip_prefix("#/$defs/")
            {
                let target = format!("#/components/schemas/{}", component_name(def));
                map.insert("$ref".to_string(), serde_json::Value::String(target));
            }
            for child in map.values_mut() {
                repoint_defs(child, component_name);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                repoint_defs(item, component_name);
            }
        }
        _ => {}
    }
}

fn export_docs(op: TransformOperation<'_>) -> TransformOperation<'_> {
    op.description(
        "`format=hf-model-index` projects a Card's model, task and results onto the \
         YAML a Hugging Face model card embeds, with a `source` link back to the \
         version. The projection is lossy and one-way: the hub reads and writes \
         converters rather than asking anyone to adopt its record format. An Eval has \
         no results, so that format is `400` on one. `format=bundle` is `501` while \
         its design is open.",
    )
}

fn query_docs(op: TransformOperation<'_>) -> TransformOperation<'_> {
    op.description(
        "A typed filter over the record schema. Every path is checked against the \
         schema and against what the indexes can serve, so a typo is `422 unknown_path` \
         rather than an empty page, and an operator no index answers is \
         `422 not_indexed` rather than a sequential scan. Paging is by opaque cursor; \
         `expand` adds fingerprints or relations to each hit.",
    )
    .response_with::<200, Json<evalhub_schema::query::Page<query::QueryHitDto>>, _>(|r| {
        r.description("One page of matching versions.")
    })
}

fn post_record_docs(op: TransformOperation<'_>) -> TransformOperation<'_> {
    op.description(
        "Validates the body, canonicalises it and stores it as the next version \
         of `{ns}/{name}`, creating the name if needed. Requires a token with \
         `write` on `ns`. If the canonical body equals the latest version's, \
         that version is returned with `200` and nothing is written. An \
         `evalhub.eval/2.0` body carrying `runs` is `422 runs_moved`: runs are \
         written with `PUT …/runs/{run_id}` or `POST …/runs:batch`. An \
         `evalhub.eval/1.0` body is still accepted until 0.3.0: it is stored as \
         a 2.0 header plus run rows, and the response carries a `Deprecation` \
         header, `converted_from` and `converted_runs`. A Card with more \
         `run_results` than `limits.run_results` is `422 too_many_run_results`.",
    )
    .response_with::<201, Json<records::VersionEnvelope>, _>(|r| {
        r.description("A new version was stored.")
    })
    .response_with::<200, Json<records::VersionEnvelope>, _>(|r| {
        r.description("The body equals the latest version; that version is returned.")
    })
}

fn get_record_docs(op: TransformOperation<'_>) -> TransformOperation<'_> {
    op.description(
        "The latest live version of `{ns}/{name}`, or the version `@{seq}`. \
         Private records are `404` to a caller whose token does not cover `ns`.",
    )
    .response_with::<200, Json<records::VersionEnvelope>, _>(|r| {
        r.description("The version and the hub's facts about it.")
    })
}
