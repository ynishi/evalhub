<script lang="ts">
	// Whatever the hub refused, shown as the hub said it: one line per
	// violation, with the JSON pointer that located it. A 422 from this API
	// is a list, not a sentence, and flattening it loses the part a
	// producer needs.
	import { ApiError } from '$lib/api/client';

	let { error }: { error: unknown } = $props();

	const api = $derived(error instanceof ApiError ? error : null);
	const message = $derived(error instanceof Error ? error.message : error ? String(error) : null);
</script>

{#if error}
	<div class="notice error" role="alert">
		<strong>{message}</strong>
		{#if api && api.entries.length > 0}
			<ul>
				{#each api.entries as entry (entry.path + entry.code)}
					<li>
						<code>{entry.path || '(root)'}</code>
						<span class="tag">{entry.code}</span>
						{#if entry.hint}<span class="muted">{entry.hint}</span>{/if}
					</li>
				{/each}
			</ul>
		{/if}
	</div>
{/if}
