<script lang="ts">
	// Badges are facts the hub checked, not a verdict on the numbers. The
	// tooltip says which fact, because a bare word like `env_pinned` tells
	// a first-time reader nothing.
	let { badges }: { badges: string[] } = $props();

	const MEANING: Record<string, string> = {
		refs_resolved: 'Every relation this version cites resolved to a version on this hub.',
		harness_registered: 'The harness it names has a registry entry.',
		metric_registered: 'Every metric it reports has a registry entry.',
		env_pinned: 'It records a clean git commit and a sandbox image digest.',
		redacted: 'The producer states that something was removed before publishing.'
	};
</script>

{#if badges.length > 0}
	<span class="row">
		{#each badges as badge (badge)}
			<span class="badge" title={MEANING[badge] ?? 'A fact the hub checked at ingest.'}>
				{badge}
			</span>
		{/each}
	</span>
{:else}
	<span class="faint small">none</span>
{/if}
