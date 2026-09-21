//! The HTTP handlers, one module per resource group, all under `/api/v1`.
//!
//! | Group                    | Endpoints                                                                                                   |
//! | ------------------------ | ----------------------------------------------------------------------------------------------------------- |
//! | [`meta`]                 | `GET /openapi.json`, `GET /schemas/{name}`, `GET /whoami`, `GET /healthz`                                   |
//! | [`auth`]                 | `GET,POST,DELETE /tokens`, `GET /namespaces/{ns}`, `GET,POST,DELETE /orgs/{org}/members`                    |
//! | [`records`]              | `POST /{cards\|evals}/{ns}/{name}?label=`, `GET …/{name}[@{seq}\|@{label}]?expand=`, `GET …/versions`, `DELETE …@{…}`, `PATCH …@{…}/label`, `PATCH …/settings`, `GET /{cards\|evals}?ns&search&sort&cursor` |
//! | [`attachments`]          | `POST /attachments`, `POST /attachments/{sha256}/complete`, `HEAD,GET /attachments/{sha256}` (GET → 302)    |
//! | [`query`]                | `POST /{cards\|evals}/query`                                                                                |
//! | [`relations`]            | `GET …/{name}/relations?direction&types&depth&follow_latest&version`, `POST …@{…}/relations`, `GET /evals/{ns}/{name}/cards?version&group_by` |
//! | [`registry`]             | `GET,PUT /registry/{harnesses\|metrics\|relation_types\|ext_schemas}/{ns}/{id}@{version}` (`core/` read-only) |
//! | [`export`]               | `GET …/{name}/export?format=hf-model-index\|bundle&version=`                                                |
//! | [`audit`]                | `GET /audit?ns&cursor`                                                                                      |
//!
//! Statuses: `201` created, `200` idempotent hit or read, `409`
//! (`attachment_missing`, `label_in_use`), `422` (`errors[]`), `404`
//! (including private), `403` (scope), `401` (token), `400` (not JSON, bad
//! cursor). See [`crate::error`].
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
use aide::axum::routing::{delete_with, get_with, patch_with, post_with};
use aide::transform::TransformOperation;
use axum::Json;
use axum::Router;
use axum::routing::get;
use tower_http::trace::TraceLayer;

use evalhub_schema::openapi::API_PREFIX;
use evalhub_store::PgPool;

use crate::config::Config;
use crate::state::AppState;

pub mod attachments;
pub mod audit;
pub mod auth;
pub mod export;
pub mod meta;
pub mod query;
pub mod records;
pub mod registry;
pub mod relations;

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
                    .description("Any member may read the roster.")
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
            "/evals/{ns}/{name}/cards",
            get_with(relations::eval_cards, |op| {
                op.id("eval_cards")
                    .summary("Cards measured on this Eval")
                    .description(
                        "Every visible Card whose latest live version has a \
                         `core/uses_eval` edge to this Eval version, with its per-facet \
                         fingerprints and whether its harness and model match the Eval's. \
                         `group_by=fingerprint.{facet}` groups them by that fingerprint. \
                         The hub lines the Cards up; it does not rank them.",
                    )
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

    let cursors = crate::auth::CursorSigner::new(
        config
            .auth
            .cursor_key
            .as_ref()
            .map(secrecy::ExposeSecret::expose_secret),
    );
    let state = AppState {
        config,
        db,
        objects,
        openapi: Arc::new(openapi),
        cursors,
    };

    app.route("/openapi.json", get(meta::openapi))
        .route("/schemas/{name}", get(meta::schema))
        .fallback(crate::embed::fallback)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
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

fn post_record_docs(op: TransformOperation<'_>) -> TransformOperation<'_> {
    op.description(
        "Validates the body, canonicalises it and stores it as the next version \
         of `{ns}/{name}`, creating the name if needed. Requires a token with \
         `write` on `ns`. If the canonical body equals the latest version's, \
         that version is returned with `200` and nothing is written.",
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
