<script lang="ts">
	// One Eval: the material, and the Cards measured on it.
	//
	// The comparison view is the only screen that puts records side by
	// side, and it does exactly what the hub does — line them up and label
	// agreement on an axis. `same_model` means the model fingerprints
	// match. It is not a ranking, and there is no winner column.
	//
	// The runs belong to the record, not to a version: the header line
	// counts them from the envelope's `runs` summary, and the Runs table
	// reads `GET …/runs` one page at a time, joined with the judgements of
	// the Card chosen above it.
	import { page } from '$app/state';
	import { untrack } from 'svelte';
	import {
		attachmentHref,
		comparison,
		getRecord,
		getRelations,
		listRuns,
		listVersions,
		setLabel,
		setVisibility,
		type Comparison,
		type RunPage,
		type RunRow,
		type VersionEnvelope
	} from '$lib/api/client';
	import Badges from '$lib/components/Badges.svelte';
	import Errors from '$lib/components/Errors.svelte';
	import Facets from '$lib/components/Facets.svelte';
	import Relations from '$lib/components/Relations.svelte';
	import Withheld from '$lib/components/Withheld.svelte';
	import { FACETS } from '$lib/paths';
	import { session } from '$lib/session.svelte';

	const ns = $derived(page.params.ns ?? '');
	const name = $derived(page.params.name ?? '');
	const at = $derived(page.url.searchParams.get('v'));
	const address = $derived(at ? `${name}@${at}` : name);

	let version = $state<VersionEnvelope | null>(null);
	let versions = $state<VersionEnvelope[]>([]);
	let graph = $state<Awaited<ReturnType<typeof getRelations>> | null>(null);
	let view = $state<Comparison | null>(null);
	let groupBy = $state('');
	let error = $state<unknown>(null);
	let notice = $state<string | null>(null);

	const record = $derived((version?.record ?? null) as Record<string, any> | null);
	const summary = $derived(version?.runs ?? null);
	const attachments = $derived((record?.attachments ?? []) as Record<string, any>[]);
	const mayWrite = $derived(session.mayWrite(ns));

	/** Flat list or groups, reduced to one shape for the table. */
	const groups = $derived(view?.groups ?? (view?.items ? [{ key: null, items: view.items }] : []));

	/** Runs shown per page. */
	const RUNS_PAGE = 50;

	let runPage = $state<RunPage | null>(null);
	/** The cursors of the pages before this one; empty on the first page. */
	let runCursors = $state<string[]>([]);
	let runsBusy = $state(false);
	let runsError = $state<unknown>(null);
	/** `{ns}/{name}` of the Card whose judgements are joined, or ''. */
	let judgeCard = $state('');

	/** The Cards the comparison lists, once each, as `cards=` spells them. */
	const cardChoices = $derived(
		[...new Set(groups.flatMap((g) => g.items.map((row) => `${row.ns}/${row.name}`)))].sort()
	);
	const runRows = $derived(runPage?.items ?? []);
	/** One column per metric id present on this page, in id order. */
	const metricIds = $derived(
		[...new Set(runRows.flatMap((row) => Object.keys(row.metrics ?? {})))].sort()
	);
	/** Per joined Card, one column per metric it judged on this page. */
	const judgementColumns = $derived(
		(runPage?.cards ?? []).flatMap((card) =>
			[
				...new Set(
					runRows.flatMap((row) => (row.cards[card.card]?.results ?? []).map((r) => r.metric))
				)
			]
				.sort()
				.map((metric) => ({ card: card.card, metric }))
		)
	);

	/** What one Card said about one run for one metric: value and label of
	 * every entry, in the Card's order. */
	function judgement(row: RunRow, card: string, metric: string): string {
		return (row.cards[card]?.results ?? [])
			.filter((r) => r.metric === metric)
			.map((r) => [r.value, r.label].filter((x) => x !== undefined && x !== null).join(' '))
			.join(', ');
	}

	function when(stamp: string | null | undefined): string {
		return stamp ? stamp.slice(0, 19).replace('T', ' ') : '';
	}

	async function loadRuns(cursor?: string) {
		runsBusy = true;
		runsError = null;
		try {
			runPage = await listRuns(ns, name, {
				cards: judgeCard ? [judgeCard] : undefined,
				limit: RUNS_PAGE,
				cursor
			});
		} catch (e) {
			runsError = e;
			runPage = null;
		} finally {
			runsBusy = false;
		}
	}

	function firstRuns() {
		runCursors = [];
		void loadRuns();
	}

	function nextRuns() {
		const cursor = runPage?.next_cursor;
		if (!cursor) return;
		runCursors = [...runCursors, cursor];
		void loadRuns(cursor);
	}

	function previousRuns() {
		const back = runCursors.slice(0, -1);
		runCursors = back;
		void loadRuns(back[back.length - 1]);
	}

	async function load() {
		error = null;
		try {
			version = await getRecord('evals', ns, address, 'fingerprints,badges,changed');
			versions = await listVersions('evals', ns, name);
			graph = await getRelations('evals', ns, address, { direction: 'both', depth: 1 });
			await loadComparison();
		} catch (e) {
			error = e;
		}
	}

	$effect(() => {
		// Runs belong to the record, so they reload when the name changes,
		// not when `?v=` moves between versions.
		// `untrack`: the load reads `judgeCard` and the cursors, and a change
		// of Card is handled by the select, not by re-running this.
		void ns;
		void name;
		untrack(() => {
			judgeCard = '';
			firstRuns();
		});
	});

	async function loadComparison() {
		try {
			view = await comparison(ns, address, groupBy || undefined);
		} catch (e) {
			error = e;
		}
	}

	$effect(() => {
		void ns;
		void address;
		void load();
	});

	async function toggleVisibility(next: 'public' | 'private') {
		try {
			await setVisibility('evals', ns, name, next);
			notice = `This Eval is now ${next}.`;
		} catch (e) {
			error = e;
		}
	}

	async function moveLabel(seq: number) {
		const label = prompt('Label for this version (never purely numeric):');
		if (!label) return;
		try {
			await setLabel('evals', ns, `${name}@${seq}`, label);
			notice = `Label “${label}” now points at version ${seq}.`;
			await load();
		} catch (e) {
			error = e;
		}
	}
</script>

<svelte:head><title>{ns}/{name} · evalhub</title></svelte:head>

<div class="between">
	<div>
		<h1>{record?.title ?? name}</h1>
		<p class="muted small">
			<a href="/ns/{ns}">{ns}</a>/{name}
			{#if version}<span class="tag">version {version.seq}</span>{/if}
			{#if record?.eval_kind}<span class="tag">{record.eval_kind}</span>{/if}
			{#if record?.origin}<span class="tag">{record.origin}</span>{/if}
			{#if summary}
				<span class="tag" title="Runs neither archived nor deleted, by status.">
					{summary.count}
					{summary.count === 1 ? 'run' : 'runs'}: {summary.by_status.ok} ok, {summary.by_status
						.error} error, {summary.by_status.skipped} skipped{#if summary.archived != null}, {summary.archived}
						archived{/if}{#if summary.deleted != null}, {summary.deleted} deleted{/if}
				</span>
			{/if}
		</p>
	</div>
	{#if mayWrite}
		<div class="row">
			<button onclick={() => toggleVisibility('public')}>Make public</button>
			<button onclick={() => toggleVisibility('private')}>Make private</button>
		</div>
	{/if}
</div>

<Errors {error} />
{#if notice}<p class="notice">{notice}</p>{/if}
<Withheld withheld={version?.withheld} />

{#if version?.tombstone}
	<div class="notice error">
		<strong>This version was withdrawn.</strong>
		{version.tombstone.reason}{#if version.tombstone.note}
			— {version.tombstone.note}{/if}
	</div>
{/if}

{#if version}
	<h2>What the hub recorded</h2>
	<dl class="kv">
		<dt>Version id</dt>
		<dd>{version.version_id}</dd>
		<dt>Content hash</dt>
		<dd>{version.content_hash}</dd>
		<dt>Created</dt>
		<dd>{version.created_at}</dd>
		<dt>Badges</dt>
		<dd><Badges badges={version.badges ?? []} /></dd>
	</dl>

	<h2>Cards measured on this Eval</h2>
	<div class="controls">
		<div>
			<label for="group-by">Group by fingerprint</label>
			<select
				id="group-by"
				bind:value={groupBy}
				onchange={() => {
					void loadComparison();
				}}
			>
				<option value="">no grouping</option>
				{#each FACETS as facet (facet)}
					<option value="fingerprint.{facet}">{facet}</option>
				{/each}
			</select>
		</div>
		<p class="faint small grow">
			Grouping puts Cards that agree on that facet together. Agreement on an axis is all the hub
			claims; reading the comparison is yours.
		</p>
	</div>

	{#if groups.length === 0}
		<p class="empty">No Card cites this Eval version yet.</p>
	{:else}
		{#each groups as group (group.key ?? 'ungrouped')}
			{#if group.key !== null && group.key !== undefined}
				<h3 class="mono">{groupBy.replace('fingerprint.', '')} {group.key.slice(0, 16)}…</h3>
			{/if}
			<div class="scroll-x">
				<table>
					<thead>
						<tr>
							<th scope="col">Card</th>
							<th scope="col">Title</th>
							<th scope="col">Version</th>
							<th scope="col">Same model</th>
							<th scope="col">Same harness</th>
						</tr>
					</thead>
					<tbody>
						{#each group.items as row (row.version_id)}
							<tr>
								<td>
									<a href="/cards/{row.ns}/{row.name}?v={row.seq}">{row.ns}/{row.name}</a>
								</td>
								<td>{row.title ?? ''}</td>
								<td class="num">{row.seq}</td>
								<td>
									{#if row.same_model}<span
											class="badge"
											title="Its model fingerprint equals this Eval's.">yes</span
										>{:else}<span class="faint">no</span>{/if}
								</td>
								<td>
									{#if row.same_harness}<span
											class="badge"
											title="Its harness fingerprint equals this Eval's.">yes</span
										>{:else}<span class="faint">no</span>{/if}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/each}
	{/if}

	<h2>Runs</h2>
	<div class="controls">
		<div>
			<label for="judge-card">Judgements of</label>
			<select
				id="judge-card"
				bind:value={judgeCard}
				onchange={firstRuns}
				disabled={cardChoices.length === 0}
			>
				<option value="">no Card</option>
				{#each cardChoices as card (card)}
					<option value={card}>{card}</option>
				{/each}
			</select>
		</div>
		<p class="faint small grow">
			Runs belong to the Eval, not to one version. Choosing a Card adds what its latest version says
			about each run it used.
		</p>
	</div>

	<Errors error={runsError} />

	{#each runPage?.cards ?? [] as card (card.card)}
		<p class="muted small">
			<a href="/cards/{card.card}?v={card.seq}">{card.card}</a> version {card.seq} used
			{card.runs_used}
			{card.runs_used === 1 ? 'run' : 'runs'}{#if card.changed_since_card.length > 0};
				<span class="tag" title="Overwritten since the Card used them.">
					changed since: {card.changed_since_card.join(', ')}
				</span>{/if}.
		</p>
	{/each}

	{#if runsBusy && runRows.length === 0}
		<p class="empty">Loading…</p>
	{:else if runRows.length === 0 && !runsError}
		<p class="empty">No runs recorded for this Eval.</p>
	{:else if runRows.length > 0}
		<div class="scroll-x">
			<table>
				<thead>
					<tr>
						<th scope="col">Run</th>
						<th scope="col">Status</th>
						<th scope="col">Error</th>
						<th scope="col">Started</th>
						<th scope="col">Ended</th>
						{#each metricIds as metric (metric)}
							<th scope="col" class="mono">{metric}</th>
						{/each}
						{#each judgementColumns as column (column.card + ' ' + column.metric)}
							<th scope="col"
								><span class="mono">{column.metric}</span>
								<span class="faint small">by {column.card}</span></th
							>
						{/each}
					</tr>
				</thead>
				<tbody>
					{#each runRows as row (row.run_id)}
						<tr>
							<td class="mono">{row.run_id}</td>
							<td>
								{#if row.state === 'live'}
									{row.status ?? ''}
								{:else}
									<span class="tag">{row.state}</span>
								{/if}
							</td>
							<td class="small mono">{row.error_kind ?? ''}</td>
							<td class="faint small">{when(row.started_at)}</td>
							<td class="faint small">{when(row.ended_at)}</td>
							{#each metricIds as metric (metric)}
								<td class="num">{row.metrics?.[metric] ?? ''}</td>
							{/each}
							{#each judgementColumns as column (column.card + ' ' + column.metric)}
								{@const cell = row.cards[column.card]}
								<td class="small">
									{#if cell && !cell.used}
										<span class="faint" title="Not in this Card's used set.">not used</span>
									{:else}
										{judgement(row, column.card, column.metric)}
										{#if cell?.changed}<span
												class="tag"
												title="The run was overwritten since the Card used it.">changed</span
											>{/if}
									{/if}
								</td>
							{/each}
						</tr>
					{/each}
				</tbody>
			</table>
		</div>

		<div class="pager">
			<button onclick={previousRuns} disabled={runCursors.length === 0 || runsBusy}>Previous</button
			>
			<button onclick={nextRuns} disabled={!runPage?.next_cursor || runsBusy}>Next</button>
			<span class="faint small">{runRows.length} on this page; paging is by cursor.</span>
		</div>
	{/if}

	{#if attachments.length > 0}
		<h2>Attachments</h2>
		<div class="scroll-x">
			<table>
				<thead>
					<tr>
						<th scope="col">Path</th>
						<th scope="col">Media type</th>
						<th scope="col">Size</th>
					</tr>
				</thead>
				<tbody>
					{#each attachments as attachment (attachment.sha256 + attachment.path)}
						<tr>
							<td><a href={attachmentHref(attachment.sha256)}>{attachment.path}</a></td>
							<td class="small">{attachment.media_type ?? ''}</td>
							<td class="num">{attachment.size}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}

	{#if record}
		<h2>Facets</h2>
		<Facets {record} fingerprints={version.fingerprints} />
	{/if}

	<h2>Relations</h2>
	<Relations {graph} kind="evals" />

	<h2>Version history</h2>
	<div class="scroll-x">
		<table>
			<thead>
				<tr>
					<th scope="col">Seq</th>
					<th scope="col">Label</th>
					<th scope="col">Changed keys</th>
					<th scope="col">Created</th>
					<th scope="col">State</th>
					{#if mayWrite}<th scope="col"></th>{/if}
				</tr>
			</thead>
			<tbody>
				{#each versions as entry (entry.version_id)}
					<tr>
						<td class="num"><a href="/evals/{ns}/{name}?v={entry.seq}">{entry.seq}</a></td>
						<td>{entry.label ?? ''}</td>
						<td class="small">{(entry.changed ?? []).join(', ')}</td>
						<td class="mono faint small">{entry.created_at.slice(0, 19).replace('T', ' ')}</td>
						<td class="small">
							{#if entry.tombstone}
								<span class="tag">withdrawn: {entry.tombstone.reason}</span>
							{:else}
								live
							{/if}
						</td>
						{#if mayWrite}
							<td><button class="link" onclick={() => moveLabel(entry.seq)}>label…</button></td>
						{/if}
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
{/if}
