<script lang="ts">
	// List and search one kind of record, with the query builder beside the
	// plain filters. Two ways in: `GET /{kind}` for name, namespace and
	// free text, `POST /{kind}/query` for the typed filter language. Both
	// page by opaque cursor, so the pager works the same either way.
	import {
		listRecords,
		runQuery,
		type Filter,
		type Kind,
		type ListItem,
		type QueryHit,
		type QueryRequest
	} from '$lib/api/client';
	import Badges from '$lib/components/Badges.svelte';
	import Errors from '$lib/components/Errors.svelte';
	import QueryBuilder from '$lib/components/QueryBuilder.svelte';

	let { kind }: { kind: Kind } = $props();

	const schemaName = $derived(kind === 'cards' ? 'card' : 'eval');
	const singular = $derived(kind === 'cards' ? 'Card' : 'Eval');

	let ns = $state('');
	let search = $state('');
	let sort = $state('created_desc');

	/** A row of either source, reduced to what the table shows. */
	interface Row {
		ns: string;
		name: string;
		title: string | null | undefined;
		seq: number;
		badges: string[];
		created_at: string;
		visibility?: string;
	}

	let rows = $state<Row[]>([]);
	let cursors = $state<string[]>([]);
	let nextCursor = $state<string | null | undefined>(null);
	let busy = $state(false);
	let error = $state<unknown>(null);
	let mode = $state<'list' | 'query'>('list');
	let where = $state<Filter | undefined>(undefined);
	let ran = $state(false);

	function fromList(item: ListItem): Row {
		return {
			ns: item.ns,
			name: item.name,
			title: item.title,
			seq: item.latest.seq,
			badges: item.latest.badges ?? [],
			created_at: item.created_at,
			visibility: item.visibility
		};
	}

	function fromHit(hit: QueryHit): Row {
		return {
			ns: hit.ns,
			name: hit.name,
			title:
				typeof hit.record === 'object' && hit.record !== null
					? ((hit.record as Record<string, unknown>).title as string | undefined)
					: undefined,
			seq: hit.seq,
			badges: hit.badges ?? [],
			created_at: hit.created_at
		};
	}

	async function load(cursor?: string) {
		busy = true;
		error = null;
		try {
			if (mode === 'query') {
				const body: QueryRequest = {
					where,
					sort: [],
					expand: ['badges'],
					limit: 50,
					cursor: cursor ?? null
				};
				const page = await runQuery(kind, body);
				rows = page.items.map(fromHit);
				nextCursor = page.next_cursor;
			} else {
				const page = await listRecords(kind, {
					ns: ns || undefined,
					search: search || undefined,
					sort,
					cursor,
					limit: 50
				});
				rows = page.items.map(fromList);
				nextCursor = page.next_cursor;
			}
			ran = true;
		} catch (e) {
			error = e;
			rows = [];
			nextCursor = null;
		} finally {
			busy = false;
		}
	}

	function first() {
		cursors = [];
		void load();
	}

	function next() {
		if (!nextCursor) return;
		cursors = [...cursors, nextCursor];
		void load(nextCursor);
	}

	function previous() {
		const back = cursors.slice(0, -1);
		cursors = back;
		void load(back[back.length - 1]);
	}

	function runFilter(built: Filter | undefined) {
		where = built;
		mode = 'query';
		first();
	}

	$effect(() => {
		// Re-run from the top whenever the kind changes.
		void kind;
		mode = 'list';
		cursors = [];
		void load();
	});
</script>

<svelte:head><title>{singular}s · evalhub</title></svelte:head>

<div class="between">
	<h1>{singular}s</h1>
	<span class="faint small">
		{#if mode === 'query'}filtered by query{:else}listing{/if}
	</span>
</div>

<form
	class="controls"
	onsubmit={(e) => {
		e.preventDefault();
		mode = 'list';
		first();
	}}
>
	<div>
		<label for="ns">Namespace</label>
		<input id="ns" type="text" bind:value={ns} placeholder="any" />
	</div>
	<div class="grow">
		<label for="search">Search</label>
		<input id="search" type="search" bind:value={search} placeholder="name or title" />
	</div>
	<div>
		<label for="sort">Sort</label>
		<select id="sort" bind:value={sort}>
			<option value="created_desc">newest first</option>
			<option value="created_asc">oldest first</option>
			<option value="name_asc">by name</option>
		</select>
	</div>
	<button type="submit">Apply</button>
</form>

<QueryBuilder {schemaName} {error} onrun={runFilter} />

<Errors {error} />

{#if busy && rows.length === 0}
	<p class="empty">Loading…</p>
{:else if ran && rows.length === 0 && !error}
	<p class="empty">
		Nothing here. A private record stays invisible without a token that covers it.
	</p>
{:else if rows.length > 0}
	<div class="scroll-x">
		<table>
			<caption>{rows.length} {rows.length === 1 ? 'record' : 'records'} on this page</caption>
			<thead>
				<tr>
					<th scope="col">Name</th>
					<th scope="col">Title</th>
					<th scope="col">Version</th>
					<th scope="col">Badges</th>
					<th scope="col">Created</th>
				</tr>
			</thead>
			<tbody>
				{#each rows as row (row.ns + '/' + row.name)}
					<tr>
						<td>
							<a href="/{kind}/{row.ns}/{row.name}">{row.ns}/{row.name}</a>
							{#if row.visibility === 'private'}<span class="tag">private</span>{/if}
						</td>
						<td>{row.title ?? ''}</td>
						<td class="num">{row.seq}</td>
						<td><Badges badges={row.badges} /></td>
						<td class="mono faint">{row.created_at.slice(0, 10)}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>

	<div class="pager">
		<button onclick={previous} disabled={cursors.length === 0 || busy}>Previous</button>
		<button onclick={next} disabled={!nextCursor || busy}>Next</button>
		<span class="faint small">Paging is by cursor, so pages stay stable as records arrive.</span>
	</div>
{/if}
