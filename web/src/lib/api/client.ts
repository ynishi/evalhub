/**
 * The one place this application talks to the hub.
 *
 * Every request goes through `api` (the generated, typed client) or through
 * one of the helpers below; no component calls `fetch`. Two reasons: the
 * session cookie needs `credentials: 'include'` on every request, and a
 * `401` anywhere has to send the reader back to the login screen, which is
 * one middleware here rather than a habit everyone has to remember.
 *
 * The UI is served from the hub's own origin, so paths are relative and
 * there is no CORS to configure.
 */
import createClient, { type Middleware } from 'openapi-fetch';
import type { components, paths } from './schema';

/** What the hub says went wrong, one entry per violation. */
export type ErrorEntry = components['schemas']['ErrorEntry'];
/** The identity behind the current session, or an anonymous one. */
export type Whoami = components['schemas']['Whoami'];
/** One row of a record listing. */
export type ListItem = components['schemas']['ListItem'];
/** A stored version and the facts the hub adds around it. */
export type VersionEnvelope = components['schemas']['VersionEnvelope'];
/** One hit of a query, which is a version plus its name. */
export type QueryHit = components['schemas']['QueryHitDto'];
/** The body of `POST /{cards|evals}/query`. */
export type QueryRequest = components['schemas']['QueryRequest'];
/** A filter: a connective (`and`/`or`/`not`) or a leaf. This is the query
 * grammar's own schema, spliced into the contract by the hub. */
export type Filter = components['schemas']['QueryFilter'];
/** An edge between two versions. */
export type Relation = components['schemas']['RelationDto'];
/** The Cards measured on one Eval version. */
export type Comparison = components['schemas']['ComparisonDto'];
/** One Card in the comparison view. */
export type ComparisonRow = components['schemas']['ComparisonRowDto'];
/** A registry entry. */
export type RegistryEntry = components['schemas']['EntryDto'];
/** One of the caller's tokens, without its secret. */
export type Token = components['schemas']['TokenDto'];
/** A namespace and what it holds. */
export type Namespace = components['schemas']['NamespaceDto'];
/** A member of an organisation. */
export type Member = components['schemas']['MemberDto'];
/** What a token may do. */
export type Scope = components['schemas']['ScopeDto'];
/** Whether a record is public or private. */
export type Visibility = components['schemas']['VisibilityParam'];
/** What a read removed from a record body because the reader may not see it. */
export type Withheld = components['schemas']['Withheld'];
/** An Eval's runs, counted, as its `GET` envelope carries them. */
export type RunsSummary = components['schemas']['RunsSummaryDto'];
/** One page of an Eval's runs, joined with the requested Cards' judgements. */
export type RunPage = components['schemas']['RunPageDto'];
/** One row of that page. */
export type RunRow = components['schemas']['RunRowDto'];
/** A Card of the request, as the run page joined it. */
export type RunPageCard = components['schemas']['RunPageCardDto'];
/** One run as stored, or its tombstone. */
export type RunEnvelope = components['schemas']['RunEnvelope'];
/** What `PUT …/runs/{run_id}` did. */
export type RunWritten = components['schemas']['RunWrittenDto'];
/** What `POST …/runs:batch` did, per run. */
export type RunsWritten = components['schemas']['BatchResponse'];
/** Why a run is deleted. */
export type TombstoneReason = components['schemas']['TombstoneReasonParam'];

/** The two kinds of record, as they appear in a URL. */
export type Kind = 'cards' | 'evals';

/** A failed call, carrying whatever the hub said about it. */
export class ApiError extends Error {
	/** HTTP status. */
	readonly status: number;
	/** The `errors[]` envelope, when the status carries one. */
	readonly entries: ErrorEntry[];

	constructor(status: number, entries: ErrorEntry[], message?: string) {
		super(message ?? entries[0]?.hint ?? `the hub answered ${status}`);
		this.name = 'ApiError';
		this.status = status;
		this.entries = entries;
	}

	/** The entries that concern one JSON pointer, for showing a message
	 * beside the field that caused it rather than in a heap at the top. */
	at(pointer: string): ErrorEntry[] {
		return this.entries.filter((e) => e.path === pointer);
	}
}

/** Called when the hub refuses the session. The layout installs a handler
 * that routes to the login screen; until then, failures still throw. */
let onUnauthorized: (() => void) | null = null;

/** Register the "the session is gone" handler. */
export function setUnauthorizedHandler(handler: () => void): void {
	onUnauthorized = handler;
}

const unauthorized: Middleware = {
	async onResponse({ response }) {
		if (response.status === 401) onUnauthorized?.();
		return response;
	}
};

/** The generated client. Prefer the helpers below; reach for this when a
 * call has no helper yet. */
export const api = createClient<paths>({
	baseUrl: '/',
	credentials: 'include'
});
api.use(unauthorized);

/** Unwrap a generated-client result, raising {@link ApiError} on failure. */
export function unwrap<T>(
	result: { data?: T; error?: unknown; response: Response },
	what: string
): T {
	if (result.data !== undefined) return result.data;
	const body = result.error as { errors?: ErrorEntry[] } | undefined;
	throw new ApiError(result.response.status, body?.errors ?? [], `${what} failed`);
}

/** A raw call for what the generated client cannot express: the served
 * JSON Schemas, which are documents rather than API operations. Keeping it
 * here rather than in a component holds the "one module talks to the hub"
 * line. */
async function raw(path: string, init: RequestInit): Promise<Response> {
	const response = await fetch(path, { credentials: 'include', ...init });
	if (response.status === 401) onUnauthorized?.();
	return response;
}

/** Exchange a bearer token for a session cookie. The token is sent once
 * and never stored in the browser; the hub mints a UI token and puts it in
 * a cookie the script cannot read. */
export async function login(token: string): Promise<Whoami> {
	const params = { body: { token } } as never;
	return unwrap(await api.POST('/api/v1/session', params), 'signing in');
}

/** End the session: the hub revokes the cookie's token and clears it.
 * Answers `204` whether or not one was open, so there is nothing to
 * unwrap. */
export async function logout(): Promise<void> {
	await api.DELETE('/api/v1/session', {});
}

/** Who the hub thinks the caller is. Never throws: an anonymous caller is
 * a legitimate answer, and so is a hub that has no session yet. */
export async function whoami(): Promise<Whoami> {
	const { data } = await api.GET('/api/v1/whoami', {});
	return data ?? { user: null, namespaces: [], scope: null };
}

/** List records of one kind. */
export async function listRecords(
	kind: Kind,
	query: { ns?: string; search?: string; sort?: string; cursor?: string; limit?: number }
): Promise<{ items: ListItem[]; next_cursor?: string | null }> {
	const params = { query: query as never };
	const result =
		kind === 'cards'
			? await api.GET('/api/v1/cards', params)
			: await api.GET('/api/v1/evals', params);
	return unwrap(result, 'listing records');
}

/** Run a typed query against one kind. */
export async function runQuery(
	kind: Kind,
	body: QueryRequest
): Promise<{ items: QueryHit[]; next_cursor?: string | null }> {
	const params = { body } as never;
	const result =
		kind === 'cards'
			? await api.POST('/api/v1/cards/query', params)
			: await api.POST('/api/v1/evals/query', params);
	return unwrap(result, 'the query');
}

/** Read one version: the latest, or `@{seq}` / `@{label}` when `name`
 * carries a suffix. `expand` asks for the hub's derived facts. */
export async function getRecord(
	kind: Kind,
	ns: string,
	name: string,
	expand?: string
): Promise<VersionEnvelope> {
	const params = { params: { path: { ns, name }, query: expand ? { expand } : {} } } as never;
	const result =
		kind === 'cards'
			? await api.GET('/api/v1/cards/{ns}/{name}', params)
			: await api.GET('/api/v1/evals/{ns}/{name}', params);
	return unwrap(result, 'reading the record');
}

/** Every version of a name, oldest first, tombstones included. */
export async function listVersions(
	kind: Kind,
	ns: string,
	name: string
): Promise<VersionEnvelope[]> {
	const params = { params: { path: { ns, name } } } as never;
	const result =
		kind === 'cards'
			? await api.GET('/api/v1/cards/{ns}/{name}/versions', params)
			: await api.GET('/api/v1/evals/{ns}/{name}/versions', params);
	return unwrap<{ items: VersionEnvelope[] }>(result, 'reading the version history').items;
}

/** The graph around a version. */
export async function getRelations(
	kind: Kind,
	ns: string,
	name: string,
	query: { direction?: string; depth?: number; types?: string } = {}
): Promise<components['schemas']['GraphDto']> {
	const params = { params: { path: { ns, name }, query } } as never;
	const result =
		kind === 'cards'
			? await api.GET('/api/v1/cards/{ns}/{name}/relations', params)
			: await api.GET('/api/v1/evals/{ns}/{name}/relations', params);
	return unwrap(result, 'reading relations');
}

/** The Cards measured on one Eval version, optionally grouped by a facet
 * fingerprint (`fingerprint.model`, …). */
export async function comparison(ns: string, name: string, groupBy?: string): Promise<Comparison> {
	const params = {
		params: { path: { ns, name }, query: groupBy ? { group_by: groupBy } : {} }
	} as never;
	return unwrap(await api.GET('/api/v1/evals/{ns}/{name}/cards', params), 'the comparison view');
}

/** The query string of `GET …/runs`. `cards` and `sort` repeat. */
export interface RunsQuery {
	/** `{ns}/{name}` of each Card whose judgements to join, in column order. */
	cards?: string[];
	/** The filter, as the query grammar's JSON text. */
	where?: string;
	/** `{path}`, `{path}:asc` or `{path}:desc`, most significant first. */
	sort?: string[];
	/** Page size, 1–200. */
	limit?: number;
	/** The previous page's `next_cursor`. */
	cursor?: string;
	/** `archived`, `deleted` or both; honoured for members only. */
	include?: string;
}

/** One page of an Eval's runs. Runs belong to the record, so `name` never
 * carries an `@{seq}`. */
export async function listRuns(ns: string, name: string, query: RunsQuery = {}): Promise<RunPage> {
	const params = { params: { path: { ns, name }, query } } as never;
	return unwrap(await api.GET('/api/v1/evals/{ns}/{name}/runs', params), 'listing runs');
}

/** One run as stored; a deleted run comes back as its tombstone. */
export async function getRun(ns: string, name: string, runId: string): Promise<RunEnvelope> {
	const params = { params: { path: { ns, name, run_id: runId } } } as never;
	return unwrap(
		await api.GET('/api/v1/evals/{ns}/{name}/runs/{run_id}', params),
		'reading the run'
	);
}

/** Write one run under the `run_id` it carries. */
export async function putRun(
	ns: string,
	name: string,
	run: Record<string, unknown> & { run_id: string }
): Promise<RunWritten> {
	const params = { params: { path: { ns, name, run_id: run.run_id } }, body: run } as never;
	return unwrap(
		await api.PUT('/api/v1/evals/{ns}/{name}/runs/{run_id}', params),
		'writing the run'
	);
}

/** Write several runs in one transaction: all or nothing. */
export async function putRuns(
	ns: string,
	name: string,
	runs: Record<string, unknown>[]
): Promise<RunsWritten> {
	const params = { params: { path: { ns, name } }, body: { runs } } as never;
	return unwrap(await api.POST('/api/v1/evals/{ns}/{name}/runs:batch', params), 'writing the runs');
}

/** Archive or unarchive a run. */
export async function archiveRun(
	ns: string,
	name: string,
	runId: string,
	archived: boolean
): Promise<RunEnvelope> {
	const params = { params: { path: { ns, name, run_id: runId } }, body: { archived } } as never;
	return unwrap(
		await api.PATCH('/api/v1/evals/{ns}/{name}/runs/{run_id}', params),
		'archiving the run'
	);
}

/** Delete a run. The id, the content hash and the metrics stay. */
export async function deleteRun(
	ns: string,
	name: string,
	runId: string,
	reason: TombstoneReason,
	note?: string
): Promise<RunEnvelope> {
	const params = {
		params: { path: { ns, name, run_id: runId } },
		body: { reason, note: note ?? null }
	} as never;
	return unwrap(
		await api.DELETE('/api/v1/evals/{ns}/{name}/runs/{run_id}', params),
		'deleting the run'
	);
}

/** Make a record public or private. */
export async function setVisibility(
	kind: Kind,
	ns: string,
	name: string,
	visibility: Visibility
): Promise<void> {
	const params = { params: { path: { ns, name } }, body: { visibility } } as never;
	const result =
		kind === 'cards'
			? await api.PATCH('/api/v1/cards/{ns}/{name}/settings', params)
			: await api.PATCH('/api/v1/evals/{ns}/{name}/settings', params);
	unwrap(result, 'changing visibility');
}

/** Point a label at `{name}@{seq}`; `name` must carry the `@{seq}`. */
export async function setLabel(
	kind: Kind,
	ns: string,
	name: string,
	label: string
): Promise<VersionEnvelope> {
	const params = { params: { path: { ns, name } }, body: { label } } as never;
	const result =
		kind === 'cards'
			? await api.PATCH('/api/v1/cards/{ns}/{name}/label', params)
			: await api.PATCH('/api/v1/evals/{ns}/{name}/label', params);
	return unwrap(result, 'moving the label');
}

/** The caller's tokens, by prefix; secrets are shown once, at issue. */
export async function listTokens(): Promise<Token[]> {
	return unwrap<{ items: Token[] }>(await api.GET('/api/v1/tokens', {}), 'listing tokens').items;
}

/** Issue a token. The secret in the response is the only copy. */
export async function createToken(
	scope: Scope,
	namespaces: string[]
): Promise<{ token_id: string; secret: string; scope: Scope; namespaces: string[] }> {
	const params = { body: { scope, namespaces } } as never;
	return unwrap(await api.POST('/api/v1/tokens', params), 'issuing a token');
}

/** Revoke one of the caller's tokens. */
export async function revokeToken(tokenId: string): Promise<void> {
	const params = { params: { path: { token_id: tokenId } } } as never;
	const result = await api.DELETE('/api/v1/tokens/{token_id}', params);
	if (result.response.status >= 400) unwrap(result, 'revoking the token');
}

/** A namespace and the counts the caller may see. */
export async function getNamespace(ns: string): Promise<Namespace> {
	const params = { params: { path: { ns } } } as never;
	return unwrap(await api.GET('/api/v1/namespaces/{ns}', params), 'reading the namespace');
}

/** The members of an organisation. Requires membership. */
export async function listMembers(org: string): Promise<Member[]> {
	const params = { params: { path: { org } } } as never;
	return unwrap<{ items: Member[] }>(
		await api.GET('/api/v1/orgs/{org}/members', params),
		'reading the members'
	).items;
}

/** Create an organisation. The caller becomes its first admin. */
export async function createOrg(ns: string): Promise<Namespace> {
	const params = { body: { ns } } as never;
	return unwrap(await api.POST('/api/v1/orgs', params), 'creating the organisation');
}

/** Add a member to an organisation, or change their role. Requires admin on the org. */
export async function addMember(org: string, user: string, role: Scope): Promise<Member> {
	const params = { params: { path: { org } }, body: { user, role } } as never;
	return unwrap(await api.POST('/api/v1/orgs/{org}/members', params), 'adding the member');
}

/** Remove a member from an organisation. Requires admin on the org. */
export async function removeMember(org: string, user: string): Promise<void> {
	const params = { params: { path: { org, user } } } as never;
	const result = await api.DELETE('/api/v1/orgs/{org}/members/{user}', params);
	if (result.response.status >= 400) unwrap(result, 'removing the member');
}

/** Browse one kind of registry entry. */
export async function listRegistry(
	kind: string,
	query: { ns?: string; limit?: number; offset?: number } = {}
): Promise<RegistryEntry[]> {
	const params = { params: { path: { kind }, query } } as never;
	return unwrap<{ items: RegistryEntry[] }>(
		await api.GET('/api/v1/registry/{kind}', params),
		'reading the registry'
	).items;
}

/** Fetch one of the served JSON Schemas (`card`, `eval` (the 1.0 body,
 * until 0.3.0), `eval-2` (the Eval header), `run`, `error`, `query`). The
 * query builder walks these for its path vocabulary, which
 * is why it stays right when the record types change. */
export async function getSchema(name: string): Promise<Record<string, unknown>> {
	const response = await raw(`/schemas/${name}`, { method: 'GET' });
	if (!response.ok) throw new ApiError(response.status, [], `fetching /schemas/${name}`);
	return (await response.json()) as Record<string, unknown>;
}

/** Where a browser should go to download an attachment. The hub answers
 * `302` to a presigned URL, so a plain link is the whole implementation. */
export function attachmentHref(sha256: string): string {
	return `/api/v1/attachments/${sha256}`;
}
