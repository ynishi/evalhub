<script lang="ts">
	// The hub has no passwords: a token is the only credential it can
	// check. Handing one over here exchanges it for a session cookie the
	// script cannot read, so the token never sits in browser storage.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { login } from '$lib/api/client';
	import Errors from '$lib/components/Errors.svelte';
	import { session } from '$lib/session.svelte';

	let token = $state('');
	let busy = $state(false);
	let error = $state<unknown>(null);

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = null;
		try {
			await login(token.trim());
			token = '';
			await session.refresh();
			await goto(page.url.searchParams.get('next') ?? '/cards');
		} catch (e) {
			error = e;
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head><title>Sign in · evalhub</title></svelte:head>

<h1>Sign in</h1>
<p class="muted">
	Paste a token. The hub exchanges it for a session cookie; the token itself is not kept in the
	browser. A first token comes from <code>evalhub user create &lt;login&gt;</code> on the server, and
	later ones from Settings.
</p>

<Errors {error} />

<form onsubmit={submit} class="form">
	<label for="token">Token</label>
	<input
		id="token"
		type="password"
		bind:value={token}
		autocomplete="off"
		spellcheck="false"
		placeholder="43 characters, base64url"
		required
	/>
	<button class="primary" type="submit" disabled={busy || token.trim() === ''}>
		{busy ? 'Signing in…' : 'Sign in'}
	</button>
</form>

<p class="faint small">
	Reading public records needs no token. Writing, and reading a private record, needs one that
	covers the namespace.
</p>

<style>
	.form {
		max-width: 26rem;
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
		align-items: flex-start;
		margin: 1rem 0;
	}
	.form input {
		font-family: var(--mono);
	}
</style>
