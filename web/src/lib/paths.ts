/**
 * The query vocabulary, walked out of the served JSON Schemas.
 *
 * The hub's type check builds its path table from the committed record
 * schema; this does the same walk in the browser so the query builder
 * offers the paths that exist and the operators that fit their type. It is
 * deliberately not a hand-written list: a key added to the record schema
 * shows up here in the same release.
 *
 * What this cannot know is which paths are *indexed* — that follows from
 * the migration, not the schema. So the builder offers every operator the
 * type allows and lets the hub answer `422 not_indexed` for the ones it
 * will not serve; the error lands against the field that caused it.
 */

/** The scalar types the query language distinguishes. */
export type PathType = 'number' | 'string' | 'boolean';

/** One entry of the vocabulary. */
export interface QueryPath {
	/** The path as the query language spells it, `model.id`. */
	path: string;
	/** Its type, or `array` for a path that only accepts `any`. */
	type: PathType | 'array';
	/** The facet or group it belongs to, for grouping the picker. */
	group: string;
	/** The schema's description, shown as help text. */
	description?: string;
}

/** The operators the language defines, in the order the type table lists
 * them. */
export const OPERATORS = [
	'eq',
	'ne',
	'gt',
	'gte',
	'lt',
	'lte',
	'in',
	'prefix',
	'contains',
	'exists',
	'any'
] as const;

/** One operator of the query language. */
export type Operator = (typeof OPERATORS)[number];

/**
 * Which operators suit a path, per the type table in the query crate:
 * `eq`/`ne`/`in` take any scalar, the comparisons take numbers and strings,
 * `prefix`/`contains` take strings, `exists` takes anything, and `any` is
 * for arrays.
 */
export function operatorsFor(type: QueryPath['type']): Operator[] {
	if (type === 'array') return ['any', 'exists'];
	const common: Operator[] = ['eq', 'ne', 'in', 'exists'];
	if (type === 'number') return ['eq', 'ne', 'gt', 'gte', 'lt', 'lte', 'in', 'exists'];
	if (type === 'string')
		return ['eq', 'ne', 'gt', 'gte', 'lt', 'lte', 'in', 'prefix', 'contains', 'exists'];
	return common;
}

/** The seven facets, in the order the record docs list them. An Eval has
 * the first six; `grading` belongs to the Card. */
export const FACETS = [
	'model',
	'task',
	'harness',
	'generation',
	'trial',
	'grading',
	'env'
] as const;

/** The side tables an `any` can range over. `runs` is deliberately absent:
 * the hub has no side table for it, so `any` would have nothing to walk. */
const ARRAY_PATHS = ['results', 'relations', 'attachments'] as const;

type Schema = Record<string, any>;

/** Follow a local `$ref` (`#/$defs/Model`) inside one document. */
function deref(root: Schema, node: Schema): Schema {
	let current = node;
	for (let hops = 0; current?.$ref && hops < 10; hops += 1) {
		const ref: string = current.$ref;
		if (!ref.startsWith('#/')) return current;
		let target: any = root;
		for (const segment of ref.slice(2).split('/')) {
			target = target?.[segment.replace(/~1/g, '/').replace(/~0/g, '~')];
		}
		current = {
			...target,
			...Object.fromEntries(Object.entries(current).filter(([k]) => k !== '$ref'))
		};
	}
	return current;
}

/** The scalar type of a schema node, if it has exactly one. JSON Schema
 * writes an optional key as `["string", "null"]`, which is still a string
 * as far as a query is concerned. */
function scalarType(node: Schema): PathType | 'array' | null {
	const flat = node.allOf?.length === 1 ? { ...node, ...node.allOf[0] } : node;
	const raw = flat.type;
	const names: string[] = Array.isArray(raw) ? raw : raw ? [raw] : [];
	const meaningful = names.filter((n) => n !== 'null');
	if (meaningful.includes('array')) return 'array';
	if (meaningful.includes('object')) return null;
	if (meaningful.includes('number') || meaningful.includes('integer')) return 'number';
	if (meaningful.includes('boolean')) return 'boolean';
	if (meaningful.includes('string')) return 'string';
	// A closed enum of strings, which schemars writes as `oneOf` of consts.
	if (Array.isArray(flat.oneOf) && flat.oneOf.every((v: Schema) => typeof v.const === 'string'))
		return 'string';
	return null;
}

/** Walk one object schema, emitting a path per scalar leaf. Recurses into
 * nested objects (`model.quantization.method`) and stops at `ext`, whose
 * keys are a producer's business and are typed by the registry. */
function walk(root: Schema, node: Schema, prefix: string, group: string, out: QueryPath[]): void {
	const resolved = deref(root, node);
	const properties: Schema = resolved.properties ?? {};
	for (const [key, rawChild] of Object.entries(properties)) {
		if (key === 'ext') continue;
		const child = deref(root, rawChild as Schema);
		const path = prefix ? `${prefix}.${key}` : key;
		const type = scalarType(child);
		if (type === 'array' || type === null) {
			if (child.properties) walk(root, child, path, group, out);
			continue;
		}
		out.push({ path, type, group, description: child.description });
	}
}

/**
 * Build the vocabulary for one record kind from its served JSON Schema.
 *
 * Emits: every scalar leaf of every facet, the top-level scalars the hub
 * materialises (`title`, `producer.name`), `fingerprint.{facet}` for each
 * facet the kind has, and the array paths that accept `any`.
 */
export function pathsFromSchema(schema: Schema): QueryPath[] {
	const out: QueryPath[] = [];
	const properties: Schema = schema.properties ?? {};

	for (const [key, rawChild] of Object.entries(properties)) {
		if (key === 'ext') continue;
		const child = deref(schema, rawChild as Schema);
		const type = scalarType(child);
		const isFacet = (FACETS as readonly string[]).includes(key);
		if (type === null || isFacet) {
			if (child.properties) walk(schema, child, key, isFacet ? key : 'record', out);
			continue;
		}
		if (type === 'array') continue;
		out.push({ path: key, type, group: 'record', description: child.description });
	}

	for (const facet of FACETS) {
		if (!properties[facet]) continue;
		out.push({
			path: `fingerprint.${facet}`,
			type: 'string',
			group: 'fingerprint',
			description: `The ${facet} facet's fingerprint: equal fingerprints mean the two records agree on every core key of that facet.`
		});
	}

	for (const table of ARRAY_PATHS) {
		if (!properties[table]) continue;
		out.push({
			path: table,
			type: 'array',
			group: 'arrays',
			description: `Matches when one entry of ${table} satisfies every term.`
		});
	}

	out.sort((a, b) => a.path.localeCompare(b.path));
	return out;
}

/** The terms an `any` over each side table accepts, with their types.
 * These are columns of the hub's tables rather than keys of the record, so
 * they come from the query language rather than from the schema. */
export const ANY_TERMS: Record<string, { term: string; type: PathType }[]> = {
	results: [
		{ term: 'metric', type: 'string' },
		{ term: 'aggregation', type: 'string' },
		{ term: 'value', type: 'number' },
		{ term: 'n', type: 'number' }
	],
	relations: [
		{ term: 'type', type: 'string' },
		{ term: 'to', type: 'string' }
	],
	attachments: [
		{ term: 'path', type: 'string' },
		{ term: 'sha256', type: 'string' }
	]
};

/** Parse what the reader typed into the JSON the filter expects: a number
 * if it reads as one, a boolean for `true`/`false`, a list for `in`, and a
 * string otherwise. */
export function parseLiteral(raw: string, type: QueryPath['type'], op: Operator): unknown {
	if (op === 'in') {
		return raw
			.split(',')
			.map((part) => part.trim())
			.filter((part) => part.length > 0)
			.map((part) => parseLiteral(part, type, 'eq'));
	}
	if (type === 'number') {
		const n = Number(raw);
		return Number.isFinite(n) ? n : raw;
	}
	if (type === 'boolean') return raw === 'true';
	return raw;
}
