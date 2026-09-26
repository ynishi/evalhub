<script lang="ts">
	// What a read removed from a record body because the reader may not see
	// it: relation edges to versions they cannot see, and a Card's
	// `run_results[]` entries that judge runs of an Eval they cannot see.
	// Only the counts are shown; the hub itself says no more than that
	// (a withheld relation keeps its commitment, a withheld run result
	// keeps nothing but the fact that there was one). Nothing renders when
	// nothing was withheld.
	import type { Withheld } from '$lib/api/client';

	let { withheld, compact = false }: { withheld: Withheld | null | undefined; compact?: boolean } =
		$props();

	const relations = $derived(withheld?.relations.length ?? 0);
	const runResults = $derived(withheld?.run_results ?? 0);
	const parts = $derived(
		[
			relations > 0 ? `${relations} ${relations === 1 ? 'relation' : 'relations'}` : null,
			runResults > 0 ? `${runResults} ${runResults === 1 ? 'run result' : 'run results'}` : null
		].filter((p): p is string => p !== null)
	);
	const summary = $derived(`withheld: ${parts.join(', ')}`);
</script>

{#if parts.length > 0}
	{#if compact}
		<span
			class="tag"
			title="Removed from the body because they point at records this session may not see."
			>{summary}</span
		>
	{:else}
		<div class="notice" role="note">
			<strong>Part of this record is not shown to you.</strong>
			The hub removed {parts.join(' and ')} that point at records this session may not see, so the body
			below is not the stored one and does not match its content hash.
		</div>
	{/if}
{/if}
