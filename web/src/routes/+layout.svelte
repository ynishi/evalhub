<script lang="ts">
	import '../app.css';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { logout, setUnauthorizedHandler } from '$lib/api/client';
	import { session } from '$lib/session.svelte';

	let { children } = $props();

	// A 401 from anywhere means the session is gone: forget it and ask for
	// a token again, remembering where the reader was.
	setUnauthorizedHandler(() => {
		session.clear();
		const here = page.url.pathname + page.url.search;
		if (!here.startsWith('/login')) {
			void goto(`/login?next=${encodeURIComponent(here)}`, { replaceState: true });
		}
	});

	$effect(() => {
		void session.refresh();
	});

	async function signOut() {
		await logout();
		session.clear();
		await goto('/login');
	}
</script>

<header class="masthead">
	<div class="masthead-inner">
		<a class="brand" href="/">evalhub</a>
		<nav aria-label="Main">
			<a href="/cards" aria-current={page.url.pathname.startsWith('/cards') ? 'page' : undefined}>
				Cards
			</a>
			<a href="/evals" aria-current={page.url.pathname.startsWith('/evals') ? 'page' : undefined}>
				Evals
			</a>
			<a
				href="/registry"
				aria-current={page.url.pathname.startsWith('/registry') ? 'page' : undefined}
			>
				Registry
			</a>
			{#if session.signedIn}
				<a
					href="/settings"
					aria-current={page.url.pathname.startsWith('/settings') ? 'page' : undefined}
				>
					Settings
				</a>
			{/if}
		</nav>
		<div class="whoami">
			{#if session.loading}
				<span class="faint">…</span>
			{:else if session.signedIn}
				<a href="/ns/{session.who.user}">{session.who.user}</a>
				<span class="tag">{session.who.scope}</span>
				<button class="link" onclick={signOut}>sign out</button>
			{:else}
				<a href="/login">sign in</a>
			{/if}
		</div>
	</div>
</header>

<main class="page">
	{@render children()}
</main>
