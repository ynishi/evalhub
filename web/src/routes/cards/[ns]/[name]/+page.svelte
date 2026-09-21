<script lang="ts">
	// One Card: what the producer claimed, and the facts the hub added
	// around it. Results are shown as reported — value, n, aggregation,
	// uncertainty — because a score without its denominator compares to
	// nothing, and the hub has no opinion to add.
	import { page } from '$app/state';
	import {
		attachmentHref,
		getRecord,
		getRelations,
		listVersions,
		setLabel,
		setVisibility,
		type VersionEnvelope
	} from '$lib/api/client';
	import Badges from '$lib/components/Badges.svelte';
	import Errors from '$lib/components/Errors.svelte';
	import Facets from '$lib/components/Facets.svelte';
	import Relations from '$lib/components/Relations.svelte';
	import { session } from '$lib/session.svelte';

	const ns = $derived(page.params.ns ?? '');
	const name = $derived(page.params.name ?? '');
	/** `?v=3` reads one version; without it, the latest live one. */
	const at = $derived(page.url.searchParams.get('v'));
	const address = $derived(at ? `${name}@${at}` : name);

	let version = $state<VersionEnvelope | null>(null);
	let versions = $state<VersionEnvelope[]>([]);
	let graph = $state<Awaited<ReturnType<typeof getRelations>> | null>(null);
	let error = $state<unknown>(null);
	let notice = $state<string | null>(null);

	const record = $derived((version?.record ?? null) as Record<string, any> | null);
	const results = $derived((record?.results ?? []) as Record<string, any>[]);
	const attachments = $derived((record?.attachments ?? []) as Record<string, any>[]);
	const counts = $derived(record?.counts as Record<string, number> | undefined);
	const mayWrite = $derived(session.mayWrite(ns));

	async function load() {
		error = null;
		try {
			version = await getRecord('cards', ns, address, 'fingerprints,badges,changed');
			versions = await listVersions('cards', ns, name);
			graph = await getRelations('cards', ns, address, { direction: 'both', depth: 1 });
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
			await setVisibility('cards', ns, name, next);
			notice = `This Card is now ${next}.`;
		} catch (e) {
			error = e;
		}
	}

	async function moveLabel(seq: number) {
		const label = prompt('Label for this version (never purely numeric):');
		if (!label) return;
		try {
			await setLabel('cards', ns, `${name}@${seq}`, label);
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
			{#if version?.label}<span class="tag">{version.label}</span>{/if}
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
		<p class="small muted">
			The body is gone; the version id and the content hash remain, so anything citing it still
			resolves and shows what happened.
		</p>
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

	{#if record?.producer}
		<p class="muted small">
			Produced by <code>{record.producer.name}</code>
			{record.producer.version ?? ''}.
			{#if record.provenance}
				Source: <code>{record.provenance.source_type}</code>, relationship
				<code>{record.provenance.evaluator_relationship}</code>.
			{/if}
		</p>
	{/if}

	{#if results.length > 0}
		<h2>Results</h2>
		<p class="muted small">
			These are the producer's numbers, kept as submitted. The hub checked the arithmetic of the
			counts and nothing else.
		</p>
		<div class="scroll-x">
			<table>
				<thead>
					<tr>
						<th scope="col">Metric</th>
						<th scope="col">Value</th>
						<th scope="col">n</th>
						<th scope="col">Aggregation</th>
						<th scope="col">Uncertainty</th>
						<th scope="col">By</th>
						<th scope="col">Samples</th>
					</tr>
				</thead>
				<tbody>
					{#each results as result (result.metric + (result.by ? JSON.stringify(result.by) : ''))}
						<tr>
							<td class="mono">{result.metric}</td>
							<td class="num">{result.value}</td>
							<td class="num">{result.n ?? ''}</td>
							<td>{result.aggregation ?? ''}</td>
							<td class="small">
								{#if result.uncertainty?.stderr != null}± {result.uncertainty.stderr}{/if}
								{#if result.uncertainty?.ci}
									<span class="faint">
										CI {result.uncertainty.ci.level}: {result.uncertainty.ci.low}–{result
											.uncertainty.ci.high}
									</span>
								{/if}
							</td>
							<td class="small">{result.by ? JSON.stringify(result.by) : ''}</td>
							<td class="small mono">{result.samples_ref ?? ''}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}

	{#if counts}
		<h2>Counts</h2>
		<dl class="kv">
			{#each Object.entries(counts) as [key, value] (key)}
				<dt>{key}</dt>
				<dd>{value}</dd>
			{/each}
		</dl>
	{/if}

	{#if record}
		<h2>Facets</h2>
		<p class="muted small">
			Equal fingerprints mean two records agree on every core key of that facet. The hub does not
			combine them into a verdict.
		</p>
		<Facets {record} fingerprints={version.fingerprints} />
	{/if}

	{#if record?.redaction?.applied}
		<h2>Redaction</h2>
		<p class="muted small">
			The producer states that something was removed before publishing. The hub does not redact and
			does not verify this.
		</p>
		<dl class="kv">
			<dt>method</dt>
			<dd>{record.redaction.method ?? 'unstated'}</dd>
			<dt>fields</dt>
			<dd>{(record.redaction.fields ?? []).join(', ') || 'unstated'}</dd>
		</dl>
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
						<th scope="col">sha256</th>
					</tr>
				</thead>
				<tbody>
					{#each attachments as attachment (attachment.sha256 + attachment.path)}
						<tr>
							<td><a href={attachmentHref(attachment.sha256)}>{attachment.path}</a></td>
							<td class="small">{attachment.media_type ?? ''}</td>
							<td class="num">{attachment.size}</td>
							<td class="mono faint small">{String(attachment.sha256).slice(0, 16)}…</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}

	<h2>Relations</h2>
	<Relations {graph} kind="cards" />

	<h2>Version history</h2>
	<div class="scroll-x">
		<table>
			<caption>What changed from one version to the next, as the hub recorded it.</caption>
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
						<td class="num"><a href="/cards/{ns}/{name}?v={entry.seq}">{entry.seq}</a></td>
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
							<td>
								<button class="link" onclick={() => moveLabel(entry.seq)}>label…</button>
							</td>
						{/if}
					</tr>
				{/each}
			</tbody>
		</table>
	</div>

	<h2>Export</h2>
	<p class="muted small">
		<a href="/api/v1/cards/{ns}/{address}/export?format=hf-model-index">
			Hugging Face <code>model-index</code> (YAML)
		</a>
		— the projection a model card embeds.
	</p>
{/if}
