<script lang="ts">
	// A form over the query language, not a JSON editor with help.
	//
	// The path list is walked out of the record's served JSON Schema, so it
	// is whatever the hub actually accepts; picking a path narrows the
	// operator list to the ones its type allows. What the browser cannot
	// know is which paths are indexed — that follows from the migration —
	// so an operator the hub will not serve comes back as `not_indexed`
	// against that row rather than being hidden here.
	import { ApiError, getSchema, type Filter } from '$lib/api/client';
	import {
		ANY_TERMS,
		operatorsFor,
		parseLiteral,
		pathsFromSchema,
		type Operator,
		type QueryPath
	} from '$lib/paths';

	let {
		schemaName,
		error = null,
		onrun
	}: {
		schemaName: 'card' | 'eval';
		error?: unknown;
		onrun: (where: Filter | undefined) => void;
	} = $props();

	interface Row {
		id: number;
		path: string;
		op: Operator;
		value: string;
		/** For `any`: one term per column of the side table. */
		terms: Record<string, string>;
	}

	let nextId = 1;
	let rows = $state<Row[]>([{ id: 0, path: '', op: 'eq', value: '', terms: {} }]);
	let vocabulary = $state<QueryPath[]>([]);
	let loadError = $state<unknown>(null);

	$effect(() => {
		getSchema(schemaName)
			.then((schema) => {
				vocabulary = pathsFromSchema(schema);
			})
			.catch((e) => {
				loadError = e;
			});
	});

	const groups = $derived(
		[...new Set(vocabulary.map((p) => p.group))].map((group) => ({
			group,
			paths: vocabulary.filter((p) => p.group === group)
		}))
	);

	function info(path: string): QueryPath | undefined {
		return vocabulary.find((p) => p.path === path);
	}

	/** The `422` entries that belong to row `index`, so the message sits
	 * beside the control that caused it. */
	function entriesFor(index: number) {
		if (!(error instanceof ApiError)) return [];
		const prefix = `/where/and/${index}`;
		return error.entries.filter((e) => e.path === prefix || e.path.startsWith(`${prefix}/`));
	}

	function addRow() {
		rows = [...rows, { id: nextId++, path: '', op: 'eq', value: '', terms: {} }];
	}

	function removeRow(id: number) {
		rows = rows.filter((r) => r.id !== id);
		if (rows.length === 0) addRow();
	}

	function onPathChange(row: Row) {
		const allowed = operatorsFor(info(row.path)?.type ?? 'string');
		if (!allowed.includes(row.op)) row.op = allowed[0];
		row.terms = {};
	}

	/** The rows as the filter grammar spells them. A single term goes up
	 * bare; several are joined with `and`, which is what the row list means
	 * and what the error pointers (`/where/and/{index}`) assume. */
	function build(): Filter | undefined {
		const leaves: Filter[] = rows
			.filter((row) => row.path !== '')
			.map((row) => {
				const type = info(row.path)?.type ?? 'string';
				if (row.op === 'exists') return { path: row.path, op: 'exists' } as Filter;
				if (row.op === 'any') {
					const terms = ANY_TERMS[row.path] ?? [];
					const match: Record<string, unknown> = {};
					for (const { term, type: termType } of terms) {
						const raw = row.terms[term];
						if (raw !== undefined && raw !== '') match[term] = parseLiteral(raw, termType, 'eq');
					}
					return { path: row.path, op: 'any', match } as Filter;
				}
				return {
					path: row.path,
					op: row.op,
					value: parseLiteral(row.value, type, row.op)
				} as Filter;
			});
		if (leaves.length === 0) return undefined;
		if (leaves.length === 1) return leaves[0];
		return { and: leaves };
	}
</script>

<fieldset>
	<legend>Filter</legend>

	{#if loadError}
		<p class="notice error">
			The path vocabulary could not be loaded, so this builder cannot complete paths. The schemas
			are served at <code>/schemas/{schemaName}</code>.
		</p>
	{/if}

	{#each rows as row, index (row.id)}
		{@const type = info(row.path)?.type ?? 'string'}
		{@const allowed = operatorsFor(type)}
		{@const problems = entriesFor(index)}
		<div class="term">
			<div class="grow">
				<label for="path-{row.id}">Path</label>
				<select
					id="path-{row.id}"
					bind:value={row.path}
					onchange={() => onPathChange(row)}
					aria-describedby={problems.length ? `problem-${row.id}` : undefined}
				>
					<option value="">choose a path…</option>
					{#each groups as { group, paths } (group)}
						<optgroup label={group}>
							{#each paths as path (path.path)}
								<option value={path.path}>{path.path}</option>
							{/each}
						</optgroup>
					{/each}
				</select>
			</div>

			<div>
				<label for="op-{row.id}">Operator</label>
				<select id="op-{row.id}" bind:value={row.op}>
					{#each allowed as op (op)}
						<option value={op}>{op}</option>
					{/each}
				</select>
			</div>

			{#if row.op === 'any'}
				{#each ANY_TERMS[row.path] ?? [] as term (term.term)}
					<div>
						<label for="term-{row.id}-{term.term}">{term.term}</label>
						<input
							id="term-{row.id}-{term.term}"
							type="text"
							bind:value={row.terms[term.term]}
							placeholder={term.type}
						/>
					</div>
				{/each}
			{:else if row.op !== 'exists'}
				<div class="grow">
					<label for="value-{row.id}">
						Value {#if row.op === 'in'}<span class="faint">(comma separated)</span>{/if}
					</label>
					<input id="value-{row.id}" type="text" bind:value={row.value} placeholder={type} />
				</div>
			{/if}

			<button
				type="button"
				class="link"
				onclick={() => removeRow(row.id)}
				aria-label="Remove this term"
			>
				remove
			</button>
		</div>

		{#if info(row.path)?.description}
			<p class="faint small hint">{info(row.path)?.description}</p>
		{/if}
		{#if problems.length > 0}
			<p class="notice error small" id="problem-{row.id}">
				{#each problems as problem (problem.code + problem.path)}
					<span class="tag">{problem.code}</span>
					{problem.hint ?? problem.path}
				{/each}
			</p>
		{/if}
	{/each}

	<div class="row actions">
		<button type="button" onclick={addRow}>Add term</button>
		<button type="button" class="primary" onclick={() => onrun(build())}>Run query</button>
		<span class="faint small">Terms are combined with <code>and</code>.</span>
	</div>
</fieldset>

<style>
	fieldset {
		border: 1px solid var(--line);
		border-radius: var(--radius);
		padding: 0.75rem 1rem 1rem;
		margin: 0 0 1rem;
	}
	legend {
		color: var(--fg-soft);
		font-size: 0.85rem;
		padding: 0 0.3rem;
	}
	.term {
		display: flex;
		gap: 0.6rem;
		align-items: flex-end;
		flex-wrap: wrap;
		margin-bottom: 0.5rem;
	}
	.term > div {
		min-width: 8rem;
	}
	.term .grow {
		flex: 1;
		min-width: 12rem;
	}
	.hint {
		margin: -0.25rem 0 0.6rem;
	}
	.actions {
		margin-top: 0.5rem;
	}
</style>
