<script lang="ts">
	// The front page says what the hub is and what it refuses to do, then
	// points at the two things it holds. Anything more would be a
	// dashboard, and the hub has nothing to summarise: the numbers are
	// producers' claims, not the hub's findings.
</script>

<svelte:head><title>evalhub</title></svelte:head>

<h1>evalhub</h1>
<p class="lede">
	A hub for LLM evaluation results. A <strong>Card</strong> says what was measured, how, and what
	the score was. An <strong>Eval</strong> is the material it was measured from: runs, prompts, tasks
	or traces. Both are published under a name, <code>{'{ns}/{name}'}</code>, as an immutable sequence
	of versions.
</p>
<p class="muted">
	The hub validates, indexes and lets you line records up. It does not run evaluations, does not
	rewrite what it receives, and never calls anything verified.
</p>

<div class="cards">
	<a class="card" href="/cards">
		<h2>Cards</h2>
		<p class="muted small">Search by model, task, harness, metric value or fingerprint.</p>
	</a>
	<a class="card" href="/evals">
		<h2>Evals</h2>
		<p class="muted small">The material, and the Cards measured on it side by side.</p>
	</a>
	<a class="card" href="/registry">
		<h2>Registry</h2>
		<p class="muted small">Metrics, harnesses, relation types and extension schemas.</p>
	</a>
	<a class="card" href="/openapi.json">
		<h2>The contract</h2>
		<p class="muted small">OpenAPI 3.1. This UI is one client of it, with no private routes.</p>
	</a>
</div>

<style>
	.lede {
		max-width: 46rem;
		font-size: 1.05rem;
	}
	.cards {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(15rem, 1fr));
		gap: 0.75rem;
		margin-top: 1.5rem;
	}
	.card {
		border: 1px solid var(--line);
		border-radius: var(--radius);
		padding: 0.75rem 1rem 1rem;
		color: inherit;
	}
	.card:hover {
		border-color: var(--accent);
		text-decoration: none;
	}
	.card h2 {
		margin: 0 0 0.25rem;
		border: none;
		padding: 0;
		color: var(--accent);
	}
	.card p {
		margin: 0;
	}
</style>
