// Generated from the hub's OpenAPI document by scripts/generate-client.mjs.
// Do not edit by hand; regenerate with `pnpm run generate-client`.
export interface paths {
    "/api/v1/attachments": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Announce an upload
         * @description Returns `200 { state: "ready" }` when the object is already confirmed, otherwise `201` with a presigned `PUT` URL. The hub never receives the bytes: they go straight to the object store. Any valid token may announce; using the object needs `write` on the record's namespace.
         */
        post: operations["announce_attachment"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/attachments/{sha256}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Download an attachment
         * @description `302` to a presigned, time-limited URL. Open to everyone when a public record references the object, to the covering token when only private ones do, and to any token while the object is not referenced at all.
         */
        get: operations["download_attachment"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        /**
         * Size and media type of an attachment
         * @description The same authorisation as the download, without the redirect.
         */
        head: operations["head_attachment"];
        patch?: never;
        trace?: never;
    };
    "/api/v1/attachments/{sha256}/complete": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Confirm an upload
         * @description Checks the object exists with the announced size and, below `attachments.hash_verify_max_bytes`, that its bytes hash to the announced sha256. A mismatch is `422` and the object stays pending.
         */
        post: operations["complete_attachment"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/audit": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Audit log of a namespace
         * @description Append-only: who changed what, when. Newest first, paged by cursor. Requires `admin` on the namespace.
         */
        get: operations["list_audit"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/cards": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * List cards
         * @description Names with at least one live version, newest first by default. Private records appear only for a caller whose token covers their namespace. Paging is by opaque cursor.
         */
        get: operations["list_cards"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/cards/{ns}/{name}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Read Card version
         * @description The latest live version of `{ns}/{name}`, or the version `@{seq}`. Private records are `404` to a caller whose token does not cover `ns`.
         */
        get: operations["get_card"];
        put?: never;
        /**
         * Append Card version
         * @description Validates the body, canonicalises it and stores it as the next version of `{ns}/{name}`, creating the name if needed. Requires a token with `write` on `ns`. If the canonical body equals the latest version's, that version is returned with `200` and nothing is written.
         */
        post: operations["post_card"];
        /**
         * Tombstone Card version
         * @description Addresses one version with `@{seq}` and replaces its body with a tombstone carrying a reason. The version id, the content hash and `changed[]` remain, so relations pointing at it still resolve and show what happened.
         */
        delete: operations["delete_card"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/cards/{ns}/{name}/export": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Export a Card
         * @description `format=hf-model-index` projects a Card's model, task and results onto the YAML a Hugging Face model card embeds, with a `source` link back to the version. The projection is lossy and one-way: the hub reads and writes converters rather than asking anyone to adopt its record format. An Eval has no results, so that format is `400` on one. `format=bundle` is `501` while its design is open.
         */
        get: operations["export_card"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/cards/{ns}/{name}/label": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        /**
         * Point a label at a version
         * @description Addresses the target version with `@{seq}`. A label is unique within the name and never purely numeric; re-pointing moves it off whichever version held it.
         */
        patch: operations["label_card"];
        trace?: never;
    };
    "/api/v1/cards/{ns}/{name}/relations": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Walk the relation graph
         * @description Breadth-first from the addressed version. `direction` is `out` (default), `in` or `both`; `depth` is 1–5; `types` filters by relation type. Endpoints the caller may not see are reduced to their version id and content hash.
         */
        get: operations["card_relations"];
        put?: never;
        /**
         * Add an edge
         * @description Addresses the source version with `@{seq}`. `to` is `{ns}/{name}@{seq}`, `external:<url>` or `hf:<repo>@<sha>`; a hub reference that does not exist yet is stored textually and resolves on its own once it does.
         */
        post: operations["add_card_relation"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/cards/{ns}/{name}/settings": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        /**
         * Change visibility
         * @description Makes the name public or private. Every version follows.
         */
        patch: operations["settings_card"];
        trace?: never;
    };
    "/api/v1/cards/{ns}/{name}/versions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Version history
         * @description Every version of the name, oldest first, tombstones included and without bodies.
         */
        get: operations["versions_card"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/cards/query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Search Cards
         * @description A typed filter over the record schema. Every path is checked against the schema and against what the indexes can serve, so a typo is `422 unknown_path` rather than an empty page, and an operator no index answers is `422 not_indexed` rather than a sequential scan. Paging is by opaque cursor; `expand` adds fingerprints or relations to each hit.
         */
        post: operations["query_cards"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/evals": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * List evals
         * @description Names with at least one live version, newest first by default. Private records appear only for a caller whose token covers their namespace. Paging is by opaque cursor.
         */
        get: operations["list_evals"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/evals/{ns}/{name}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Read Eval version
         * @description The latest live version of `{ns}/{name}`, or the version `@{seq}`. Private records are `404` to a caller whose token does not cover `ns`.
         */
        get: operations["get_eval"];
        put?: never;
        /**
         * Append Eval version
         * @description Validates the body, canonicalises it and stores it as the next version of `{ns}/{name}`, creating the name if needed. Requires a token with `write` on `ns`. If the canonical body equals the latest version's, that version is returned with `200` and nothing is written.
         */
        post: operations["post_eval"];
        /**
         * Tombstone Eval version
         * @description Addresses one version with `@{seq}` and replaces its body with a tombstone carrying a reason. The version id, the content hash and `changed[]` remain, so relations pointing at it still resolve and show what happened.
         */
        delete: operations["delete_eval"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/evals/{ns}/{name}/cards": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Cards measured on this Eval
         * @description Every visible Card whose latest live version has a `core/uses_eval` edge to this Eval version, with its per-facet fingerprints and whether its harness and model match the Eval's. `group_by=fingerprint.{facet}` groups them by that fingerprint. The hub lines the Cards up; it does not rank them.
         */
        get: operations["eval_cards"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/evals/{ns}/{name}/export": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Export an Eval
         * @description `format=hf-model-index` projects a Card's model, task and results onto the YAML a Hugging Face model card embeds, with a `source` link back to the version. The projection is lossy and one-way: the hub reads and writes converters rather than asking anyone to adopt its record format. An Eval has no results, so that format is `400` on one. `format=bundle` is `501` while its design is open.
         */
        get: operations["export_eval"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/evals/{ns}/{name}/label": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        /**
         * Point a label at a version
         * @description Addresses the target version with `@{seq}`. A label is unique within the name and never purely numeric; re-pointing moves it off whichever version held it.
         */
        patch: operations["label_eval"];
        trace?: never;
    };
    "/api/v1/evals/{ns}/{name}/relations": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Walk the relation graph
         * @description Breadth-first from the addressed version. `direction` is `out` (default), `in` or `both`; `depth` is 1–5; `types` filters by relation type. Endpoints the caller may not see are reduced to their version id and content hash.
         */
        get: operations["eval_relations"];
        put?: never;
        /**
         * Add an edge
         * @description Addresses the source version with `@{seq}`. `to` is `{ns}/{name}@{seq}`, `external:<url>` or `hf:<repo>@<sha>`; a hub reference that does not exist yet is stored textually and resolves on its own once it does.
         */
        post: operations["add_eval_relation"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/evals/{ns}/{name}/settings": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        /**
         * Change visibility
         * @description Makes the name public or private. Every version follows.
         */
        patch: operations["settings_eval"];
        trace?: never;
    };
    "/api/v1/evals/{ns}/{name}/versions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Version history
         * @description Every version of the name, oldest first, tombstones included and without bodies.
         */
        get: operations["versions_eval"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/evals/query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Search Evals
         * @description A typed filter over the record schema. Every path is checked against the schema and against what the indexes can serve, so a typo is `422 unknown_path` rather than an empty page, and an operator no index answers is `422 not_indexed` rather than a sequential scan. Paging is by opaque cursor; `expand` adds fingerprints or relations to each hit.
         */
        post: operations["query_evals"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/healthz": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Liveness
         * @description Answers as long as the process is up.
         */
        get: operations["healthz"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/namespaces/{ns}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * A namespace and what it holds
         * @description Counts only the records the caller may see.
         */
        get: operations["get_namespace"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/orgs": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Create an organisation
         * @description The caller becomes its first `admin`.
         */
        post: operations["create_org"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/orgs/{org}/members": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Members of an organisation
         * @description Any role is enough; the presented token must name the organisation.
         */
        get: operations["list_members"];
        put?: never;
        /**
         * Add a member or change their role
         * @description `admin` on the organisation.
         */
        post: operations["add_member"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/orgs/{org}/members/{user}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        /**
         * Remove a member
         * @description `admin` on the organisation.
         */
        delete: operations["remove_member"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/registry/{kind}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * The vocabulary of one kind
         * @description Metrics, harnesses, relation types or extension schemas. Public: the vocabulary is what a client needs to read a record.
         */
        get: operations["list_registry"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/registry/{kind}/{ns}/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * One registry entry
         * @description `{id}` carries the version: `pass_rate@1`.
         */
        get: operations["get_registry_entry"];
        /**
         * Register a definition
         * @description Needs `write` on `ns`; `core/` is read-only. Entries are immutable, so a second write of the same address is `409` and a correction is a new version. An `ext_schemas` entry answers `202`: its expression indexes are built in the background, and until they are valid its paths accept `eq` and `exists` only.
         */
        put: operations["put_registry_entry"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/session": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Open a UI session
         * @description Exchanges a token for a session cookie. The hub has no passwords, so the credential is a token; the cookie carries a second token minted for the same user with the same scope and namespaces, which `DELETE /session` revokes. The presented token is left alone.
         */
        post: operations["create_session"];
        /**
         * End a UI session
         * @description Revokes the token the cookie carries and clears the cookie. Answers `204` whether or not a session was open.
         */
        delete: operations["delete_session"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/tokens": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * The caller's tokens
         * @description Secrets are not included; they are shown once, at issue.
         */
        get: operations["list_tokens"];
        put?: never;
        /**
         * Issue a token
         * @description The new token may name the caller's own login and organisations where the caller is `admin`, and its scope may not exceed the presenting token's. The secret is in the response and nowhere else.
         */
        post: operations["create_token"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/tokens/{token_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        /**
         * Revoke a token
         * @description Takes effect on the next request; there is no cache.
         */
        delete: operations["revoke_token"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/whoami": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Who the hub thinks the caller is
         * @description Identity and scope of the presented token, or anonymous.
         */
        get: operations["whoami"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
}
export type webhooks = Record<string, never>;
export interface components {
    schemas: {
        /** @description Body of `POST /attachments`. */
        AnnounceBody: {
            /** @description Media type hint, for display and download. Not interpreted. */
            media_type?: string | null;
            /**
             * @description sha256 of the bytes, 64 lower-case hexadecimal characters. This is
             *     the object's only name.
             */
            sha256: string;
            /**
             * Format: int64
             * @description Size of the bytes. `complete` refuses an object of another size.
             */
            size: number;
        };
        /** @description One entry of the log. */
        AuditEntry: {
            /** @description What happened, such as `version.append` or `org.member.add`. */
            action: string;
            /** @description Login of the user behind the action, if there was one. */
            actor?: string | null;
            /**
             * Format: date-time
             * @description When it happened.
             */
            at: string;
            /** @description Action-specific facts. */
            detail?: unknown;
            /** @description The namespace the action concerns. */
            ns?: string | null;
            /** @description What it happened to, such as `card/alice/single2@3`. */
            subject?: string | null;
            /** @description Identifier of the token presented, if one was. */
            token_id?: string | null;
        };
        /** @description Query string of `GET /audit`. */
        AuditQuery: {
            /** @description Cursor from a previous page's `next_cursor`. */
            cursor?: string | null;
            /**
             * Format: uint32
             * @description Page size, 1–200; default 50.
             */
            limit?: number | null;
            /** @description Namespace to read the log of. The caller needs `admin` on it. */
            ns: string;
        };
        /** @description The comparison view: Cards measured on one Eval version. */
        ComparisonDto: {
            /** @description The Cards grouped by the requested facet fingerprint. */
            groups?: components["schemas"]["ComparisonGroupDto"][] | null;
            /** @description Every Card, when `group_by` was not given. */
            items?: components["schemas"]["ComparisonRowDto"][] | null;
        };
        /** @description Cards sharing one fingerprint of the facet named in `group_by`. */
        ComparisonGroupDto: {
            /** @description The Cards in the group. */
            items: components["schemas"]["ComparisonRowDto"][];
            /** @description The shared fingerprint in hex, or `null` for Cards with none. */
            key?: string | null;
        };
        /** @description Query string of the comparison view. */
        ComparisonQuery: {
            /**
             * @description `fingerprint.{facet}` for one of the seven facets. Groups the
             *     Cards by that fingerprint; omitted, the list is flat.
             */
            group_by?: string | null;
        };
        /** @description One Card in the comparison view. */
        ComparisonRowDto: {
            /** @description sha256 of the canonical body. */
            content_hash: string;
            /** @description Per-facet fingerprints, hex, keyed by facet name. */
            fingerprints: {
                [key: string]: string;
            };
            /** @description Record name. */
            name: string;
            /** @description Namespace. */
            ns: string;
            /** @description The Card's harness fingerprint equals the Eval's. */
            same_harness: boolean;
            /** @description The Card's model fingerprint equals the Eval's. */
            same_model: boolean;
            /**
             * Format: int32
             * @description Sequence number of that version.
             */
            seq: number;
            /** @description The record's title. */
            title?: string | null;
            /** @description The Card's latest live version. */
            version_id: string;
        };
        /** @description Response of `POST /attachments/{sha256}/complete`. */
        CompletedDto: {
            /**
             * @description Whether the hub re-hashed the bytes. `false` means the object was
             *     above `attachments.hash_verify_max_bytes` and the client's sha256
             *     was taken on trust; the record says which.
             */
            hashed_by_hub: boolean;
            /**
             * Format: uint64
             * @description Size of the object as the store reports it.
             */
            size: number;
        };
        /** @description Sort direction. */
        Dir: "asc" | "desc";
        /** @description Which edges a traversal follows. */
        DirectionParam: "out" | "in" | "both";
        /** @description An edge of the relation graph. */
        EdgeDto: {
            /** @description Free-form attributes. */
            attrs?: unknown;
            /** @description Version the edge leaves. */
            from: string;
            /** @description Version it points at, when the target is on this hub. */
            to?: string | null;
            /** @description Textual target, for an unresolved or external reference. */
            to_text?: string | null;
            /** @description Registry id of the relation type. */
            type: string;
        };
        /** @description One registry entry. */
        EntryDto: {
            /** @description The definition, whose shape depends on the kind. */
            body: unknown;
            /**
             * Format: date-time
             * @description When it was registered.
             */
            created_at: string;
            /** @description Identifier within the namespace. */
            id: string;
            /** @description `metrics`, `harnesses`, `relation_types` or `ext_schemas`. */
            kind: string;
            /** @description Namespace that owns it. */
            ns: string;
            /** @description `applied`, or `applying` while its indexes are being built. */
            state: string;
            /** @description Version of the entry. Entries are immutable; a change is a new one. */
            version: string;
        };
        /**
         * @description A page of entries. The registry is small, so this pages by offset
         *     rather than by cursor.
         */
        EntryPage: {
            /** @description The entries, ordered by kind, namespace, id and version. */
            items: components["schemas"]["EntryDto"][];
        };
        /**
         * @description Path of one entry: `/registry/{kind}/{ns}/{id}` where `id` carries
         *     `@{version}`.
         */
        EntryPath: {
            /** @description Identifier followed by `@{version}`. */
            id: string;
            /** @description `metrics`, `harnesses`, `relation_types` or `ext_schemas`. */
            kind: string;
            /** @description Namespace that owns the entry. */
            ns: string;
        };
        /** @description The closed set of error codes. */
        ErrorCode: "schema" | "number_too_large" | "counts_missing" | "counts_inconsistent" | "attachment_ref_unknown" | "attachment_path_invalid" | "metric_id_invalid" | "type_mismatch" | "not_indexed" | "unknown_path" | "attachment_missing" | "label_in_use" | "namespace_in_use" | "registry_entry_exists";
        /** @description One rejection. */
        ErrorEntry: {
            /** @description The code. */
            code: components["schemas"]["ErrorCode"];
            /** @description Prose for a human. Not part of the contract. */
            hint?: string | null;
            /** @description Location in the submitted document, JSON-pointer-like (`results[0].metric`). */
            path: string;
        };
        /** @description The body of every `422` and `409` response. */
        ErrorEnvelope: {
            /** @description Every rejection found, in document order. */
            errors: components["schemas"]["ErrorEntry"][];
        };
        /** @description Optional parts of a version that a read can include. */
        Expand: "relations" | "badges" | "fingerprints" | "changed";
        /** @description Query string of the export endpoints. */
        ExportQuery: {
            /** @description `hf-model-index` or `bundle`. */
            format: string;
        };
        /** @description Query string of `GET …/{name}`. */
        GetQuery: {
            /** @description Comma-separated: `fingerprints`, `badges`, `changed`, `relations`. */
            expand?: string | null;
        };
        /** @description What a traversal found. */
        GraphDto: {
            /** @description Every edge followed. */
            edges: components["schemas"]["EdgeDto"][];
            /** @description Every version reached, the start first. */
            nodes: components["schemas"]["NodeDto"][];
        };
        /** @description `GET /api/v1/healthz` */
        Health: {
            /** @description `connected` when a database pool is open, `not_configured` otherwise. */
            database: string;
            /** @description Always `ok` when the process answers. */
            status: string;
        };
        /**
         * @description A list of things, without paging: these are all small.
         *
         *     The schema name carries the item type, so a generated client gets one
         *     list type per item type instead of `Items` and `Items2`.
         */
        Items_for_MemberDto: {
            /** @description The items. */
            items: components["schemas"]["MemberDto"][];
        };
        /**
         * @description A list of things, without paging: these are all small.
         *
         *     The schema name carries the item type, so a generated client gets one
         *     list type per item type instead of `Items` and `Items2`.
         */
        Items_for_TokenDto: {
            /** @description The items. */
            items: components["schemas"]["TokenDto"][];
        };
        /** @description Path of `GET /registry/{kind}`. */
        KindPath: {
            /** @description `metrics`, `harnesses`, `relation_types` or `ext_schemas`. */
            kind: string;
        };
        /** @description Body of `PATCH …/{name}@{seq}/label`. */
        LabelBody: {
            /** @description The label to point at this version. Moved if it named another. */
            label: string;
        };
        /** @description One row of `GET /{cards|evals}`. */
        ListItem: {
            /**
             * Format: date-time
             * @description When the name was created.
             */
            created_at: string;
            /** @description The latest live version, without its record. */
            latest: components["schemas"]["VersionEnvelope"];
            /** @description Name of the record. */
            name: string;
            /** @description Namespace of the record. */
            ns: string;
            /** @description `title` of the latest live version. */
            title?: string | null;
            /** @description `public` or `private`. */
            visibility: components["schemas"]["VisibilityParam"];
        };
        /** @description Query string of `GET /{cards|evals}`. */
        ListQuery: {
            /** @description Cursor from a previous page's `next_cursor`. */
            cursor?: string | null;
            /**
             * Format: uint32
             * @description Page size, 1–200; default 50.
             */
            limit?: number | null;
            /** @description Only records in this namespace. */
            ns?: string | null;
            /** @description Case-insensitive substring of the name or the latest title. */
            search?: string | null;
            /** @description Sort order; default `created_desc`. */
            sort?: components["schemas"]["SortParam"] | null;
        };
        /** @description A member of an organisation. */
        MemberDto: {
            /** @description Their role in the organisation. */
            role: components["schemas"]["ScopeDto"];
            /** @description The member's login. */
            user: string;
        };
        /** @description A namespace and what it holds. */
        NamespaceDto: {
            /**
             * Format: int64
             * @description Cards in it that the caller may see.
             */
            cards: number;
            /**
             * Format: date-time
             * @description When it was created.
             */
            created_at: string;
            /**
             * Format: int64
             * @description Evals in it that the caller may see.
             */
            evals: number;
            /** @description `user` or `org`. */
            kind: string;
            /** @description The namespace: a user login or an organisation slug. */
            ns: string;
        };
        /** @description Body of `POST /orgs/{org}/members`. */
        NewMemberBody: {
            /** @description The role they get. */
            role: components["schemas"]["ScopeDto"];
            /** @description Login of the user to add. */
            user: string;
        };
        /** @description Body of `POST /orgs`. */
        NewOrgBody: {
            /** @description Slug of the organisation, which is also its namespace. */
            ns: string;
        };
        /** @description Body of `POST …/relations`. */
        NewRelationBody: {
            /** @description Free-form attributes for this relation type. */
            attrs?: unknown;
            /** @description Target: `{ns}/{name}@{seq}`, `external:…` or `hf:…`. */
            to: string;
            /** @description Registry id of the relation type, `{ns}/{name}`. */
            type: string;
        };
        /** @description Body of `POST /session`. */
        NewSessionBody: {
            /**
             * @description A token secret, the same string a client would send as
             *     `Authorization: Bearer …`.
             */
            token: string;
        };
        /** @description Body of `POST /tokens`. */
        NewTokenBody: {
            /**
             * @description Namespaces it covers: the caller's login, and organisations where
             *     the caller is `admin`.
             */
            namespaces: string[];
            /** @description Scope of the new token; may not exceed the presenting token's. */
            scope: components["schemas"]["ScopeDto"];
        };
        /** @description A node of the relation graph. */
        NodeDto: {
            /** @description sha256 of the canonical body: the commitment, always present. */
            content_hash: string;
            /** @description Record name, when the caller may see it. */
            name?: string | null;
            /** @description Namespace, when the caller may see the record. */
            ns?: string | null;
            /**
             * @description `true` on a node the caller may not see; then only `version_id`
             *     and `content_hash` are present.
             */
            private: boolean;
            /** @description `card` or `eval`, when the caller may see it. */
            record_type?: string | null;
            /**
             * Format: int32
             * @description Sequence number, when the caller may see it.
             */
            seq?: number | null;
            /** @description Whether the version has been tombstoned, when the caller may see it. */
            tombstoned?: boolean | null;
            /** @description The version. */
            version_id: string;
        };
        /**
         * @description One page of results.
         *
         *     The schema name carries the item type (`Page_for_VersionEnvelope`), so a
         *     generated client gets one page type per item type instead of `Page` and
         *     `Page2`.
         */
        Page_for_AuditEntry: {
            /** @description The items on this page. */
            items: components["schemas"]["AuditEntry"][];
            /**
             * @description Cursor for the next page, or `null` on the last page.
             * @default null
             */
            next_cursor: string | null;
        };
        /**
         * @description One page of results.
         *
         *     The schema name carries the item type (`Page_for_VersionEnvelope`), so a
         *     generated client gets one page type per item type instead of `Page` and
         *     `Page2`.
         */
        Page_for_ListItem: {
            /** @description The items on this page. */
            items: components["schemas"]["ListItem"][];
            /**
             * @description Cursor for the next page, or `null` on the last page.
             * @default null
             */
            next_cursor: string | null;
        };
        /**
         * @description One page of results.
         *
         *     The schema name carries the item type (`Page_for_VersionEnvelope`), so a
         *     generated client gets one page type per item type instead of `Page` and
         *     `Page2`.
         */
        Page_for_QueryHitDto: {
            /** @description The items on this page. */
            items: components["schemas"]["QueryHitDto"][];
            /**
             * @description Cursor for the next page, or `null` on the last page.
             * @default null
             */
            next_cursor: string | null;
        };
        /**
         * @description One page of results.
         *
         *     The schema name carries the item type (`Page_for_VersionEnvelope`), so a
         *     generated client gets one page type per item type instead of `Page` and
         *     `Page2`.
         */
        Page_for_VersionEnvelope: {
            /** @description The items on this page. */
            items: components["schemas"]["VersionEnvelope"][];
            /**
             * @description Cursor for the next page, or `null` on the last page.
             * @default null
             */
            next_cursor: string | null;
        };
        /** @description Query string of `POST`. */
        PostQuery: {
            /** @description Label to attach to the new version. Never purely numeric. */
            label?: string | null;
        };
        /** @description A filter: a connective or a leaf. */
        QueryFilter: {
            and: components["schemas"]["QueryFilter"][];
        } | {
            or: components["schemas"]["QueryFilter"][];
        } | {
            not: components["schemas"]["QueryFilter"];
        } | {
            /** @enum {unknown} */
            op: "eq" | "ne" | "gt" | "gte" | "lt" | "lte" | "in" | "prefix" | "contains";
            path: string;
            value: unknown;
        } | {
            /** @constant */
            op: "exists";
            path: string;
        } | {
            match: {
                [key: string]: (string | number | boolean | null) | {
                    /** @enum {unknown} */
                    op?: "eq" | "ne" | "gt" | "gte" | "lt" | "lte" | "in" | "prefix" | "contains";
                    value: unknown;
                };
            };
            /** @constant */
            op: "any";
            path: string;
        };
        /** @description One matched version, with what the hub knows about it. */
        QueryHitDto: {
            /** @description Badges awarded at ingest. */
            badges: string[];
            /** @description Top-level keys whose value differs from the previous version. */
            changed: string[];
            /** @description Hex sha256 of the canonical record. */
            content_hash: string;
            /**
             * Format: date-time
             * @description When the hub stored this version.
             */
            created_at: string;
            /** @description Per-facet fingerprints as hex, with `expand=fingerprints`. */
            fingerprints?: {
                [key: string]: string;
            } | null;
            /** @description Identifier of the named record, stable across versions (ULID). */
            id: string;
            /** @description Client-chosen label of this version, if it has one. */
            label?: string | null;
            /** @description Name of the record within the namespace. */
            name: string;
            /** @description Namespace the record lives in. */
            ns: string;
            /** @description The canonical record. */
            record?: unknown;
            /** @description Outgoing edges, with `expand=relations`. */
            relations?: components["schemas"]["RelationDto"][] | null;
            /**
             * Format: int32
             * @description Sequence number within the name.
             */
            seq: number;
            /** @description Identifier of this version (ULID). */
            version_id: string;
        };
        /** @description The body of `POST /cards/query` and `POST /evals/query`. */
        QueryRequest: {
            /** @description Opaque cursor from a previous page's `next_cursor`. */
            cursor?: string | null;
            /** @description Extra fields to include on each item. */
            expand?: components["schemas"]["Expand"][];
            /**
             * Format: uint32
             * @description Page size. The server clamps it to its maximum.
             */
            limit?: number | null;
            /** @description Sort keys, applied in order. */
            sort?: components["schemas"]["Sort"][];
            /** @description Which versions to search. Defaults to `latest`. */
            version?: components["schemas"]["VersionSelector"] | null;
            where?: components["schemas"]["QueryFilter"];
        };
        /** @description Path of a record: `/{cards|evals}/{ns}/{name}`. */
        RecordPath: {
            /**
             * @description Record name, optionally followed by `@{seq}` or `@{label}` to
             *     address one version.
             */
            name: string;
            /** @description Namespace: a user login or an organisation slug. */
            ns: string;
        };
        /**
         * @description Query string of `GET /registry/{kind}`.
         *
         *     Named for the registry rather than `ListQuery`, so that a generated
         *     client gets one type per listing instead of `ListQuery` and
         *     `ListQuery2`.
         */
        RegistryListQuery: {
            /**
             * Format: uint32
             * @description Page size, 1–200; default 50.
             */
            limit?: number | null;
            /** @description Only entries owned by this namespace. */
            ns?: string | null;
            /**
             * Format: uint32
             * @description How many entries to skip.
             */
            offset?: number | null;
        };
        /** @description An edge as returned by `POST …/relations`. */
        RelationDto: {
            /** @description Free-form attributes. */
            attrs?: unknown;
            /** @description The version the edge leaves. */
            from: components["schemas"]["TargetDto"];
            /** @description The version or external reference it points at. */
            to: components["schemas"]["TargetDto"];
            /** @description Registry id of the relation type. */
            type: string;
        };
        /**
         * @description Path of the record an edge leaves or is read from:
         *     `/{cards|evals}/{ns}/{name}`. Named apart from the records module's
         *     path type so the two do not collide in the generated schema.
         */
        RelationPath: {
            /** @description Record name, optionally followed by `@{seq}` or `@{label}`. */
            name: string;
            /** @description Namespace: a user login or an organisation slug. */
            ns: string;
        };
        /** @description A scope as it crosses the wire. */
        ScopeDto: "read" | "write" | "admin";
        /** @description Body of `PATCH …/{name}/settings` and its response. */
        Settings: {
            /** @description `public` or `private`. */
            visibility: components["schemas"]["VisibilityParam"];
        };
        /** @description One sort key. */
        Sort: {
            /** @description Direction. */
            dir: components["schemas"]["Dir"];
            /** @description A query path (for example `results[core/pass_rate].value`). */
            path: string;
        };
        /** @description Sort orders of the list endpoints. */
        SortParam: "created_desc" | "created_asc" | "name_asc";
        /** @description One endpoint of an edge. */
        TargetDto: {
            /** @description Record name. */
            name: string;
            /** @description Namespace of its record. */
            ns: string;
            /** @description `card` or `eval`. */
            record_type: string;
            /**
             * Format: int32
             * @description Sequence number.
             */
            seq: number;
            /** @description Whether the version has been tombstoned. */
            tombstoned: boolean;
            /** @description The version. */
            version_id: string;
        } | {
            /** @description sha256 of the canonical body, so the reference is still pinned. */
            content_hash: string;
            /** @description Always `true`; marks the reduced form. */
            private: boolean;
            /** @description The version it points at. */
            version_id: string;
        } | {
            /** @description The reference as written. */
            unresolved: string;
        } | {
            /** @description The reference as written. */
            external: string;
        };
        /** @description One of the caller's tokens. The secret is not here; it was shown once. */
        TokenDto: {
            /**
             * Format: date-time
             * @description When it was issued.
             */
            created_at: string;
            /** @description Namespaces it covers. */
            namespaces: string[];
            /** @description First eight hex characters of the stored hash, to tell tokens apart. */
            prefix: string;
            /**
             * Format: date-time
             * @description When it was revoked, if it was. A revoked token is refused.
             */
            revoked_at?: string | null;
            /** @description What the token may do. */
            scope: components["schemas"]["ScopeDto"];
            /** @description Identifier of the token, used to revoke it. */
            token_id: string;
        };
        /** @description Body of `DELETE …/{name}@{seq}`. */
        TombstoneBody: {
            /** @description Free text shown with the tombstone. */
            note?: string | null;
            /** @description Why the version is withdrawn. */
            reason: components["schemas"]["TombstoneReasonParam"];
        };
        /** @description The tombstone of a withdrawn version. */
        TombstoneDto: {
            /**
             * Format: date-time
             * @description When it was withdrawn.
             */
            at: string;
            /** @description Free text from the producer, if any. */
            note?: string | null;
            /** @description Why. */
            reason: components["schemas"]["TombstoneReasonParam"];
        };
        /** @description Why a version was tombstoned. */
        TombstoneReasonParam: "withdrawn" | "duplicate" | "takedown" | "other";
        /** @description Query string of `GET …/relations`. */
        TraverseQuery: {
            /**
             * Format: uint32
             * @description How many edges from the start, 1–5; default 1.
             */
            depth?: number | null;
            /** @description `out` (default), `in` or `both`. */
            direction?: components["schemas"]["DirectionParam"] | null;
            /**
             * @description Walk from the latest live version of each record instead of the
             *     version the edge names.
             */
            follow_latest?: boolean | null;
            /** @description Comma-separated relation type ids to follow; all by default. */
            types?: string | null;
        };
        /** @description What the hub adds around a stored version. */
        VersionEnvelope: {
            /** @description Badges awarded at ingest. */
            badges: string[];
            /** @description Top-level keys whose value differs from the previous version. */
            changed: string[];
            /** @description Hex sha256 of the canonical record. */
            content_hash: string;
            /**
             * Format: date-time
             * @description When the hub stored this version.
             */
            created_at: string;
            /** @description Per-facet fingerprints as hex, with `expand=fingerprints`. */
            fingerprints?: {
                [key: string]: string;
            } | null;
            /** @description Identifier of the named record, stable across versions (ULID). */
            id: string;
            /** @description Client-chosen label of this version, if any. */
            label?: string | null;
            /**
             * @description The canonical record. Absent in `POST` responses, in version lists,
             *     and for tombstones.
             */
            record?: unknown;
            /** @description The version's outgoing edges, with `expand=relations`. */
            relations?: components["schemas"]["RelationDto"][] | null;
            /**
             * Format: int32
             * @description Sequence number within the name, from 1, gap-free.
             */
            seq: number;
            /** @description Present when the version was withdrawn; `record` is then absent. */
            tombstone?: components["schemas"]["TombstoneDto"] | null;
            /** @description Identifier of this version (ULID). Relations point at these. */
            version_id: string;
        };
        /** @description Which versions a query considers. */
        VersionSelector: "latest" | "all";
        /** @description A record's visibility. */
        VisibilityParam: "public" | "private";
        /** @description `GET /api/v1/whoami` */
        Whoami: {
            /** @description Namespaces the presented token covers. Empty when anonymous. */
            namespaces: string[];
            /** @description Scope of the presented token, or `null` when anonymous. */
            scope?: string | null;
            /** @description Login of the authenticated user, or `null` for an anonymous caller. */
            user?: string | null;
        };
    };
    responses: never;
    parameters: never;
    requestBodies: never;
    headers: never;
    pathItems: never;
}
export type $defs = Record<string, never>;
export interface operations {
    announce_attachment: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** @description Body of `POST /attachments`. */
        requestBody: {
            content: {
                "application/json": components["schemas"]["AnnounceBody"];
            };
        };
        responses: {
            /** @description Failed to parse the request body as JSON */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description Expected request with `Content-Type: application/json` */
            415: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    download_attachment: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    head_attachment: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    complete_attachment: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Response of `POST /attachments/{sha256}/complete`. */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["CompletedDto"];
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    list_audit: {
        parameters: {
            query: {
                /** @description Cursor from a previous page's `next_cursor`. */
                cursor?: string;
                /** @description Page size, 1–200; default 50. */
                limit?: number;
                /** @description Namespace to read the log of. The caller needs `admin` on it. */
                ns: string;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /**
             * @description One page of results.
             *
             *     The schema name carries the item type (`Page_for_VersionEnvelope`), so a
             *     generated client gets one page type per item type instead of `Page` and
             *     `Page2`.
             */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Page_for_AuditEntry"];
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    list_cards: {
        parameters: {
            query?: {
                /** @description Cursor from a previous page's `next_cursor`. */
                cursor?: string;
                /** @description Page size, 1–200; default 50. */
                limit?: number;
                /** @description Only records in this namespace. */
                ns?: string;
                /** @description Case-insensitive substring of the name or the latest title. */
                search?: string;
                /** @description Sort order; default `created_desc`. */
                sort?: components["schemas"]["SortParam"];
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /**
             * @description One page of results.
             *
             *     The schema name carries the item type (`Page_for_VersionEnvelope`), so a
             *     generated client gets one page type per item type instead of `Page` and
             *     `Page2`.
             */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Page_for_ListItem"];
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    get_card: {
        parameters: {
            query?: {
                /** @description Comma-separated: `fingerprints`, `badges`, `changed`, `relations`. */
                expand?: string;
            };
            header?: never;
            path: {
                /**
                 * @description Record name, optionally followed by `@{seq}` or `@{label}` to
                 *     address one version.
                 */
                name: string;
                /** @description Namespace: a user login or an organisation slug. */
                ns: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description The version and the hub's facts about it. */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["VersionEnvelope"];
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    post_card: {
        parameters: {
            query?: {
                /** @description Label to attach to the new version. Never purely numeric. */
                label?: string;
            };
            header?: never;
            path: {
                /**
                 * @description Record name, optionally followed by `@{seq}` or `@{label}` to
                 *     address one version.
                 */
                name: string;
                /** @description Namespace: a user login or an organisation slug. */
                ns: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": unknown;
            };
        };
        responses: {
            /** @description The body equals the latest version; that version is returned. */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["VersionEnvelope"];
                };
            };
            /** @description A new version was stored. */
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["VersionEnvelope"];
                };
            };
            /** @description Failed to parse the request body as JSON */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description Expected request with `Content-Type: application/json` */
            415: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    delete_card: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /**
                 * @description Record name, optionally followed by `@{seq}` or `@{label}` to
                 *     address one version.
                 */
                name: string;
                /** @description Namespace: a user login or an organisation slug. */
                ns: string;
            };
            cookie?: never;
        };
        /** @description Body of `DELETE …/{name}@{seq}`. */
        requestBody: {
            content: {
                "application/json": components["schemas"]["TombstoneBody"];
            };
        };
        responses: {
            /** @description What the hub adds around a stored version. */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["VersionEnvelope"];
                };
            };
            /** @description Failed to parse the request body as JSON */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description Expected request with `Content-Type: application/json` */
            415: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    export_card: {
        parameters: {
            query: {
                /** @description `hf-model-index` or `bundle`. */
                format: string;
            };
            header?: never;
            path: {
                /**
                 * @description Record name, optionally followed by `@{seq}` or `@{label}` to
                 *     address one version.
                 */
                name: string;
                /** @description Namespace: a user login or an organisation slug. */
                ns: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    label_card: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /**
                 * @description Record name, optionally followed by `@{seq}` or `@{label}` to
                 *     address one version.
                 */
                name: string;
                /** @description Namespace: a user login or an organisation slug. */
                ns: string;
            };
            cookie?: never;
        };
        /** @description Body of `PATCH …/{name}@{seq}/label`. */
        requestBody: {
            content: {
                "application/json": components["schemas"]["LabelBody"];
            };
        };
        responses: {
            /** @description What the hub adds around a stored version. */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["VersionEnvelope"];
                };
            };
            /** @description Failed to parse the request body as JSON */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description Expected request with `Content-Type: application/json` */
            415: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    card_relations: {
        parameters: {
            query?: {
                /** @description How many edges from the start, 1–5; default 1. */
                depth?: number;
                /** @description `out` (default), `in` or `both`. */
                direction?: components["schemas"]["DirectionParam"];
                /**
                 * @description Walk from the latest live version of each record instead of the
                 *     version the edge names.
                 */
                follow_latest?: boolean;
                /** @description Comma-separated relation type ids to follow; all by default. */
                types?: string;
            };
            header?: never;
            path: {
                /** @description Record name, optionally followed by `@{seq}` or `@{label}`. */
                name: string;
                /** @description Namespace: a user login or an organisation slug. */
                ns: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description What a traversal found. */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["GraphDto"];
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    add_card_relation: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Record name, optionally followed by `@{seq}` or `@{label}`. */
                name: string;
                /** @description Namespace: a user login or an organisation slug. */
                ns: string;
            };
            cookie?: never;
        };
        /** @description Body of `POST …/relations`. */
        requestBody: {
            content: {
                "application/json": components["schemas"]["NewRelationBody"];
            };
        };
        responses: {
            /** @description Failed to parse the request body as JSON */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description Expected request with `Content-Type: application/json` */
            415: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    settings_card: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /**
                 * @description Record name, optionally followed by `@{seq}` or `@{label}` to
                 *     address one version.
                 */
                name: string;
                /** @description Namespace: a user login or an organisation slug. */
                ns: string;
            };
            cookie?: never;
        };
        /** @description Body of `PATCH …/{name}/settings` and its response. */
        requestBody: {
            content: {
                "application/json": components["schemas"]["Settings"];
            };
        };
        responses: {
            /** @description Body of `PATCH …/{name}/settings` and its response. */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Settings"];
                };
            };
            /** @description Failed to parse the request body as JSON */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description Expected request with `Content-Type: application/json` */
            415: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    versions_card: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /**
                 * @description Record name, optionally followed by `@{seq}` or `@{label}` to
                 *     address one version.
                 */
                name: string;
                /** @description Namespace: a user login or an organisation slug. */
                ns: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /**
             * @description One page of results.
             *
             *     The schema name carries the item type (`Page_for_VersionEnvelope`), so a
             *     generated client gets one page type per item type instead of `Page` and
             *     `Page2`.
             */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Page_for_VersionEnvelope"];
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    query_cards: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** @description The body of `POST /cards/query` and `POST /evals/query`. */
        requestBody: {
            content: {
                "application/json": components["schemas"]["QueryRequest"];
            };
        };
        responses: {
            /** @description One page of matching versions. */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Page_for_QueryHitDto"];
                };
            };
            /** @description Failed to parse the request body as JSON */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description Expected request with `Content-Type: application/json` */
            415: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    list_evals: {
        parameters: {
            query?: {
                /** @description Cursor from a previous page's `next_cursor`. */
                cursor?: string;
                /** @description Page size, 1–200; default 50. */
                limit?: number;
                /** @description Only records in this namespace. */
                ns?: string;
                /** @description Case-insensitive substring of the name or the latest title. */
                search?: string;
                /** @description Sort order; default `created_desc`. */
                sort?: components["schemas"]["SortParam"];
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /**
             * @description One page of results.
             *
             *     The schema name carries the item type (`Page_for_VersionEnvelope`), so a
             *     generated client gets one page type per item type instead of `Page` and
             *     `Page2`.
             */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Page_for_ListItem"];
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    get_eval: {
        parameters: {
            query?: {
                /** @description Comma-separated: `fingerprints`, `badges`, `changed`, `relations`. */
                expand?: string;
            };
            header?: never;
            path: {
                /**
                 * @description Record name, optionally followed by `@{seq}` or `@{label}` to
                 *     address one version.
                 */
                name: string;
                /** @description Namespace: a user login or an organisation slug. */
                ns: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description The version and the hub's facts about it. */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["VersionEnvelope"];
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    post_eval: {
        parameters: {
            query?: {
                /** @description Label to attach to the new version. Never purely numeric. */
                label?: string;
            };
            header?: never;
            path: {
                /**
                 * @description Record name, optionally followed by `@{seq}` or `@{label}` to
                 *     address one version.
                 */
                name: string;
                /** @description Namespace: a user login or an organisation slug. */
                ns: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": unknown;
            };
        };
        responses: {
            /** @description The body equals the latest version; that version is returned. */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["VersionEnvelope"];
                };
            };
            /** @description A new version was stored. */
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["VersionEnvelope"];
                };
            };
            /** @description Failed to parse the request body as JSON */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description Expected request with `Content-Type: application/json` */
            415: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    delete_eval: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /**
                 * @description Record name, optionally followed by `@{seq}` or `@{label}` to
                 *     address one version.
                 */
                name: string;
                /** @description Namespace: a user login or an organisation slug. */
                ns: string;
            };
            cookie?: never;
        };
        /** @description Body of `DELETE …/{name}@{seq}`. */
        requestBody: {
            content: {
                "application/json": components["schemas"]["TombstoneBody"];
            };
        };
        responses: {
            /** @description What the hub adds around a stored version. */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["VersionEnvelope"];
                };
            };
            /** @description Failed to parse the request body as JSON */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description Expected request with `Content-Type: application/json` */
            415: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    eval_cards: {
        parameters: {
            query?: {
                /**
                 * @description `fingerprint.{facet}` for one of the seven facets. Groups the
                 *     Cards by that fingerprint; omitted, the list is flat.
                 */
                group_by?: string;
            };
            header?: never;
            path: {
                /** @description Record name, optionally followed by `@{seq}` or `@{label}`. */
                name: string;
                /** @description Namespace: a user login or an organisation slug. */
                ns: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description The comparison view: Cards measured on one Eval version. */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ComparisonDto"];
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    export_eval: {
        parameters: {
            query: {
                /** @description `hf-model-index` or `bundle`. */
                format: string;
            };
            header?: never;
            path: {
                /**
                 * @description Record name, optionally followed by `@{seq}` or `@{label}` to
                 *     address one version.
                 */
                name: string;
                /** @description Namespace: a user login or an organisation slug. */
                ns: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    label_eval: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /**
                 * @description Record name, optionally followed by `@{seq}` or `@{label}` to
                 *     address one version.
                 */
                name: string;
                /** @description Namespace: a user login or an organisation slug. */
                ns: string;
            };
            cookie?: never;
        };
        /** @description Body of `PATCH …/{name}@{seq}/label`. */
        requestBody: {
            content: {
                "application/json": components["schemas"]["LabelBody"];
            };
        };
        responses: {
            /** @description What the hub adds around a stored version. */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["VersionEnvelope"];
                };
            };
            /** @description Failed to parse the request body as JSON */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description Expected request with `Content-Type: application/json` */
            415: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    eval_relations: {
        parameters: {
            query?: {
                /** @description How many edges from the start, 1–5; default 1. */
                depth?: number;
                /** @description `out` (default), `in` or `both`. */
                direction?: components["schemas"]["DirectionParam"];
                /**
                 * @description Walk from the latest live version of each record instead of the
                 *     version the edge names.
                 */
                follow_latest?: boolean;
                /** @description Comma-separated relation type ids to follow; all by default. */
                types?: string;
            };
            header?: never;
            path: {
                /** @description Record name, optionally followed by `@{seq}` or `@{label}`. */
                name: string;
                /** @description Namespace: a user login or an organisation slug. */
                ns: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description What a traversal found. */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["GraphDto"];
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    add_eval_relation: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Record name, optionally followed by `@{seq}` or `@{label}`. */
                name: string;
                /** @description Namespace: a user login or an organisation slug. */
                ns: string;
            };
            cookie?: never;
        };
        /** @description Body of `POST …/relations`. */
        requestBody: {
            content: {
                "application/json": components["schemas"]["NewRelationBody"];
            };
        };
        responses: {
            /** @description Failed to parse the request body as JSON */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description Expected request with `Content-Type: application/json` */
            415: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    settings_eval: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /**
                 * @description Record name, optionally followed by `@{seq}` or `@{label}` to
                 *     address one version.
                 */
                name: string;
                /** @description Namespace: a user login or an organisation slug. */
                ns: string;
            };
            cookie?: never;
        };
        /** @description Body of `PATCH …/{name}/settings` and its response. */
        requestBody: {
            content: {
                "application/json": components["schemas"]["Settings"];
            };
        };
        responses: {
            /** @description Body of `PATCH …/{name}/settings` and its response. */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Settings"];
                };
            };
            /** @description Failed to parse the request body as JSON */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description Expected request with `Content-Type: application/json` */
            415: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    versions_eval: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /**
                 * @description Record name, optionally followed by `@{seq}` or `@{label}` to
                 *     address one version.
                 */
                name: string;
                /** @description Namespace: a user login or an organisation slug. */
                ns: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /**
             * @description One page of results.
             *
             *     The schema name carries the item type (`Page_for_VersionEnvelope`), so a
             *     generated client gets one page type per item type instead of `Page` and
             *     `Page2`.
             */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Page_for_VersionEnvelope"];
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    query_evals: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** @description The body of `POST /cards/query` and `POST /evals/query`. */
        requestBody: {
            content: {
                "application/json": components["schemas"]["QueryRequest"];
            };
        };
        responses: {
            /** @description One page of matching versions. */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Page_for_QueryHitDto"];
                };
            };
            /** @description Failed to parse the request body as JSON */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description Expected request with `Content-Type: application/json` */
            415: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    healthz: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description `GET /api/v1/healthz` */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Health"];
                };
            };
        };
    };
    get_namespace: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description A namespace and what it holds. */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["NamespaceDto"];
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    create_org: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** @description Body of `POST /orgs`. */
        requestBody: {
            content: {
                "application/json": components["schemas"]["NewOrgBody"];
            };
        };
        responses: {
            /** @description Failed to parse the request body as JSON */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description Expected request with `Content-Type: application/json` */
            415: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    list_members: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /**
             * @description A list of things, without paging: these are all small.
             *
             *     The schema name carries the item type, so a generated client gets one
             *     list type per item type instead of `Items` and `Items2`.
             */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Items_for_MemberDto"];
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    add_member: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** @description Body of `POST /orgs/{org}/members`. */
        requestBody: {
            content: {
                "application/json": components["schemas"]["NewMemberBody"];
            };
        };
        responses: {
            /** @description Failed to parse the request body as JSON */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description Expected request with `Content-Type: application/json` */
            415: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    remove_member: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    list_registry: {
        parameters: {
            query?: {
                /** @description Page size, 1–200; default 50. */
                limit?: number;
                /** @description Only entries owned by this namespace. */
                ns?: string;
                /** @description How many entries to skip. */
                offset?: number;
            };
            header?: never;
            path: {
                /** @description `metrics`, `harnesses`, `relation_types` or `ext_schemas`. */
                kind: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /**
             * @description A page of entries. The registry is small, so this pages by offset
             *     rather than by cursor.
             */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["EntryPage"];
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    get_registry_entry: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Identifier followed by `@{version}`. */
                id: string;
                /** @description `metrics`, `harnesses`, `relation_types` or `ext_schemas`. */
                kind: string;
                /** @description Namespace that owns the entry. */
                ns: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description One registry entry. */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["EntryDto"];
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    put_registry_entry: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Identifier followed by `@{version}`. */
                id: string;
                /** @description `metrics`, `harnesses`, `relation_types` or `ext_schemas`. */
                kind: string;
                /** @description Namespace that owns the entry. */
                ns: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": unknown;
            };
        };
        responses: {
            /** @description Failed to parse the request body as JSON */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description Expected request with `Content-Type: application/json` */
            415: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    create_session: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** @description Body of `POST /session`. */
        requestBody: {
            content: {
                "application/json": components["schemas"]["NewSessionBody"];
            };
        };
        responses: {
            /** @description The session's identity, as `whoami` reports it. */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Whoami"];
                };
            };
            /** @description Failed to parse the request body as JSON */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description Expected request with `Content-Type: application/json` */
            415: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    delete_session: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description The session is closed and the cookie cleared. */
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    list_tokens: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /**
             * @description A list of things, without paging: these are all small.
             *
             *     The schema name carries the item type, so a generated client gets one
             *     list type per item type instead of `Items` and `Items2`.
             */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Items_for_TokenDto"];
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    create_token: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** @description Body of `POST /tokens`. */
        requestBody: {
            content: {
                "application/json": components["schemas"]["NewTokenBody"];
            };
        };
        responses: {
            /** @description Failed to parse the request body as JSON */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description Expected request with `Content-Type: application/json` */
            415: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/plain": string;
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    revoke_token: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description No token, or an unrecognised one. */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description The token lacks the scope or namespace. */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Not found, or private to a namespace the caller cannot see. */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description State conflict: an attachment is not ready, or the label is taken. */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
            /** @description The record is invalid; every violation is listed. */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorEnvelope"];
                };
            };
        };
    };
    whoami: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description `GET /api/v1/whoami` */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Whoami"];
                };
            };
        };
    };
}
