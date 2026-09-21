<script lang="ts">
	// The vocabulary the hub knows: metrics, harnesses, relation types and
	// the extension schemas that make a producer's `ext` keys typed and
	// queryable. An `ext_schema` sits in `applying` while its index is
	// building, which is why the state column is here rather than implied.
	import { listRegistry, type RegistryEntry } from '$lib/api/client';
	import Errors from '$lib/components/Errors.svelte';

	const KINDS = [
		{ kind: 'metrics', about: 'What a number means, and which way is up.' },
		{ kind: 'harnesses', about: 'The software that produced a record.' },
		{ kind: 'relation_types', about: 'The edges a record may declare.' },
		{ kind: 'ext_schemas', about: 'Typed extension keys, indexed once applied.' }
	] as const;

	let kind = $state<string>('metrics');
	let ns = $state('');
	let entries = $state<RegistryEntry[]>([]);
	let error = $state<unknown>(null);
	let busy = $state(false);

	const about = $derived(KINDS.find((k) => k.kind === kind)?.about ?? '');

	async function load() {
		busy = true;
		error = null;
		try {
			entries = await listRegistry(kind, { ns: ns || undefined, limit: 200 });
		} catch (e) {
			error = e;
			entries = [];
		} finally {
			busy = false;
		}
	}

	$effect(() => {
		void kind;
		void load();
	});

	/** The one or two fields worth showing per kind, so the table says
	 * something instead of printing a JSON blob in every row. */
	function summary(entry: RegistryEntry): string {
		const body = (entry.body ?? {}) as Record<string, unknown>;
		if (entry.kind === 'metrics') {
			const direction = body.lower_is_better ? 'lower is better' : 'higher is better';
			return [direction, body.description].filter(Boolean).join(' — ');
		}
		if (entry.kind === 'relation_types') {
			return [
				body.from && body.to ? `${body.from} → ${body.to}` : null,
				body.inverse ? `inverse: ${body.inverse}` : null
			]
				.filter(Boolean)
				.join(', ');
		}
		if (entry.kind === 'harnesses') return String(body.homepage ?? '');
		if (entry.kind === 'ext_schemas') {
			const paths = (body.paths ?? body.properties) as unknown;
			return paths ? `${Object.keys(paths as object).length} typed paths` : '';
		}
		return '';
	}
</script>

<svelte:head><title>Registry · evalhub</title></svelte:head>

<h1>Registry</h1>
<p class="muted">
	Entries are immutable: a change is a new version. Anything under <code>core/</code> ships with the hub
	and is read-only.
</p>

<div class="controls">
	<div>
		<label for="kind">Kind</label>
		<select id="kind" bind:value={kind}>
			{#each KINDS as entry (entry.kind)}
				<option value={entry.kind}>{entry.kind}</option>
			{/each}
		</select>
	</div>
	<div>
		<label for="ns">Namespace</label>
		<input id="ns" type="text" bind:value={ns} placeholder="any" />
	</div>
	<button onclick={load}>Apply</button>
	<span class="faint small grow">{about}</span>
</div>

<Errors {error} />

{#if busy}
	<p class="empty">Loading…</p>
{:else if entries.length === 0}
	<p class="empty">No entries of this kind.</p>
{:else}
	<div class="scroll-x">
		<table>
			<caption>{entries.length} entries</caption>
			<thead>
				<tr>
					<th scope="col">Id</th>
					<th scope="col">Version</th>
					<th scope="col">State</th>
					<th scope="col">Summary</th>
					<th scope="col">Registered</th>
				</tr>
			</thead>
			<tbody>
				{#each entries as entry (entry.kind + entry.ns + entry.id + entry.version)}
					<tr>
						<td class="mono">{entry.ns}/{entry.id}</td>
						<td class="mono">{entry.version}</td>
						<td>
							{#if entry.state === 'applying'}
								<span
									class="tag"
									title="Its index is still building; the paths answer eq and exists meanwhile."
								>
									applying
								</span>
							{:else}
								<span class="small muted">{entry.state}</span>
							{/if}
						</td>
						<td class="small">{summary(entry)}</td>
						<td class="faint small">{entry.created_at.slice(0, 10)}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
{/if}

<p class="faint small">
	Registering an entry is a <code>PUT</code> with <code>write</code> on the namespace. This UI browses;
	a harness registers.
</p>
