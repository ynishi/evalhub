#!/usr/bin/env node
// Regenerate the typed client from the OpenAPI document.
//
//   node scripts/generate-client.mjs                       # a running hub
//   node scripts/generate-client.mjs ../path/openapi.json  # a file
//
// A contributor without a database still needs to regenerate, which is why
// a file argument works: the server crate's snapshot test writes the same
// document to `crates/evalhub-server/tests/snapshots/boot__openapi.snap`.
//
// The result, `src/lib/api/schema.d.ts`, is committed. It is a contract
// artifact like `.sqlx/` and `crates/evalhub-schema/schemas/*.json`:
// regenerate it in the same pull request as the handler change that moved
// the contract.

import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import openapiTS, { astToString } from 'openapi-typescript';

const here = dirname(fileURLToPath(import.meta.url));
const out = resolve(here, '../src/lib/api/schema.d.ts');
const source = process.argv[2] ?? 'http://127.0.0.1:8080/openapi.json';

/**
 * Fail loudly on a `$ref` the document cannot resolve.
 *
 * The hub hoists the query grammar's definitions into
 * `components/schemas` as it splices them, so a well-formed document has
 * no `#/$defs/…` pointer left. One appearing again would mean the served
 * contract regressed, and generating from it would produce a client that
 * silently loses the filter type — better to stop.
 *
 * @param {any} doc parsed OpenAPI document
 */
function refuseDanglingDefs(doc) {
	const dangling = new Set();
	const walk = (node) => {
		if (node === null || typeof node !== 'object') return;
		if (Array.isArray(node)) return node.forEach(walk);
		if (typeof node.$ref === 'string' && node.$ref.startsWith('#/$defs/')) {
			dangling.add(node.$ref);
		}
		Object.values(node).forEach(walk);
	};
	walk(doc);
	if (dangling.size > 0) {
		throw new Error(
			`the OpenAPI document has ${dangling.size} reference(s) that do not resolve ` +
				`from its root: ${[...dangling].join(', ')}. The hub must hoist embedded ` +
				`definitions into components/schemas as it splices them.`
		);
	}
}

const banner = `// Generated from the hub's OpenAPI document by scripts/generate-client.mjs.
// Do not edit by hand; regenerate with \`pnpm run generate-client\`.
`;

const raw = /^https?:\/\//.test(source)
	? await fetch(source).then((r) => {
			if (!r.ok) throw new Error(`${source} answered ${r.status}`);
			return r.json();
		})
	: JSON.parse(await readFile(resolve(process.cwd(), source), 'utf8'));

refuseDanglingDefs(raw);

const ast = await openapiTS(raw, { alphabetize: true });
await mkdir(dirname(out), { recursive: true });
await writeFile(out, banner + astToString(ast));
console.log(`wrote ${out} from ${source}`);
