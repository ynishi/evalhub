<script lang="ts">
	// One Eval: the material, and the Cards measured on it.
	//
	// The comparison view is the only screen that puts records side by
	// side, and it does exactly what the hub does — line them up and label
	// agreement on an axis. `same_model` means the model fingerprints
	// match. It is not a ranking, and there is no winner column.
	import { page } from '$app/state';
	import {
		attachmentHref,
		comparison,
		getRecord,
		getRelations,
		listVersions,
		setLabel,
		setVisibility,
		type Comparison,
		type VersionEnvelope
	} from '$lib/api/client';
	import Badges from '$lib/components/Badges.svelte';
	import Errors from '$lib/components/Errors.svelte';
	import Facets from '$lib/components/Facets.svelte';
	import Relations from '$lib/components/Relations.svelte';
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
	const runs = $derived((record?.runs ?? []) as Record<string, any>[]);
	const attachments = $derived((record?.attachments ?? []) as Record<string, any>[]);
	const mayWrite = $derived(session.mayWrite(ns));

	/** Flat list or groups, reduced to one shape for the table. */
	const groups = $derived(view?.groups ?? (view?.items ? [{ key: null, items: view.items }] : []));

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

	{#if runs.length > 0}
		<h2>Runs</h2>
		<div class="scroll-x">
			<table>
				<caption>{runs.length} runs in this version.</caption>
				<thead>
					<tr>
						<th scope="col">Run</th>
						<th scope="col">Outcome</th>
						<th scope="col">Started</th>
						<th scope="col">Ended</th>
						<th scope="col">Calls</th>
						<th scope="col">Artifacts</th>
					</tr>
				</thead>
				<tbody>
					{#each runs as run (run.run_id)}
						<tr>
							<td class="mono">{run.run_id}</td>
							<td>{run.outcome ?? ''}</td>
							<td class="faint small">{run.started_at ?? ''}</td>
							<td class="faint small">{run.ended_at ?? ''}</td>
							<td class="small mono">{run.calls ?? ''}</td>
							<td class="small mono">{(run.artifacts ?? []).join(', ')}</td>
						</tr>
					{/each}
				</tbody>
			</table>
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
