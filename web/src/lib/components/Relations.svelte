<script lang="ts">
	// Edges as a table, not a graph drawing: v0 shows what points where and
	// what the hub could resolve. An endpoint the caller may not see is a
	// commitment — a version id and a content hash — which is the design's
	// answer to "a public Card may cite a private Eval".
	import type { components } from '$lib/api/schema';

	let {
		graph,
		kind
	}: { graph: components['schemas']['GraphDto'] | null; kind: 'cards' | 'evals' } = $props();

	const nodes = $derived(new Map((graph?.nodes ?? []).map((n) => [n.version_id, n])));
	const edges = $derived(graph?.edges ?? []);

	function describe(id: string | null | undefined, text: string | null | undefined) {
		if (!id) return { label: text ?? 'unresolved', href: null, private: false };
		const node = nodes.get(id);
		if (!node) return { label: id, href: null, private: false };
		if (node.private)
			return { label: `private (${node.content_hash.slice(0, 12)}…)`, href: null, private: true };
		const path = node.record_type === 'eval' ? 'evals' : 'cards';
		return {
			label: `${node.ns}/${node.name}@${node.seq}`,
			href: `/${path}/${node.ns}/${node.name}?v=${node.seq}`,
			private: false
		};
	}
</script>

{#if edges.length === 0}
	<p class="empty">No edges on this version.</p>
{:else}
	<div class="scroll-x">
		<table>
			<caption>
				Edges attach to versions. A Card published before the Eval it cites keeps an unresolved edge
				until that Eval exists.
			</caption>
			<thead>
				<tr>
					<th scope="col">From</th>
					<th scope="col">Type</th>
					<th scope="col">To</th>
					<th scope="col">Attributes</th>
				</tr>
			</thead>
			<tbody>
				{#each edges as edge, index (edge.from + edge.type + (edge.to ?? edge.to_text ?? index))}
					{@const from = describe(edge.from, null)}
					{@const to = describe(edge.to, edge.to_text)}
					<tr>
						<td>
							{#if from.href}<a href={from.href}>{from.label}</a>{:else}<span
									class={from.private ? 'muted' : 'mono'}>{from.label}</span
								>{/if}
						</td>
						<td class="mono">{edge.type}</td>
						<td>
							{#if to.href}<a href={to.href}>{to.label}</a>{:else}<span
									class={to.private ? 'muted' : 'mono'}>{to.label}</span
								>{/if}
						</td>
						<td class="small faint">{edge.attrs ? JSON.stringify(edge.attrs) : ''}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
	<p class="faint small">
		Reading edges of a {kind === 'cards' ? 'Card' : 'Eval'} at depth 1, both directions.
	</p>
{/if}
