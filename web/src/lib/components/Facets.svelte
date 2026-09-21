<script lang="ts">
	// The facets of a record, each with its fingerprint when one was asked
	// for. A fingerprint is agreement on an axis: two records with the same
	// model fingerprint were measured on the same model, and that is the
	// whole claim.
	import { FACETS } from '$lib/paths';

	let {
		record,
		fingerprints
	}: { record: Record<string, unknown>; fingerprints?: Record<string, string> | null } = $props();

	function entries(value: unknown, prefix = ''): [string, string][] {
		if (value === null || typeof value !== 'object') return [];
		const out: [string, string][] = [];
		for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
			const path = prefix ? `${prefix}.${key}` : key;
			if (child !== null && typeof child === 'object' && !Array.isArray(child)) {
				out.push(...entries(child, path));
			} else {
				out.push([path, Array.isArray(child) ? JSON.stringify(child) : String(child)]);
			}
		}
		return out;
	}

	const present = $derived(FACETS.filter((f) => record?.[f] != null));
</script>

{#if present.length === 0}
	<p class="empty">This record declares no facets.</p>
{:else}
	<div class="stack">
		{#each present as facet (facet)}
			{@const fp = fingerprints?.[facet]}
			<section>
				<div class="between">
					<h3 id="facet-{facet}">{facet}</h3>
					{#if fp}
						<span
							class="tag"
							title="Records whose {facet} fingerprint matches agree on every core key of this facet."
						>
							{fp.slice(0, 12)}…
						</span>
					{/if}
				</div>
				<dl class="kv">
					{#each entries(record[facet]) as [key, value] (key)}
						<dt>{key}</dt>
						<dd>{value}</dd>
					{/each}
				</dl>
			</section>
		{/each}
	</div>
{/if}
