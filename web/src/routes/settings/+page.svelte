<script lang="ts">
	// Tokens, and the two record settings a reader can change from here.
	//
	// A freshly issued secret is shown once and never again — the hub keeps
	// only its hash — so this screen makes that moment loud rather than
	// letting it scroll past as one more row.
	import {
		createOrg,
		createToken,
		listTokens,
		revokeToken,
		setLabel,
		setVisibility,
		type Kind,
		type Scope,
		type Token,
		type Visibility
	} from '$lib/api/client';
	import { page } from '$app/state';
	import Errors from '$lib/components/Errors.svelte';
	import { session } from '$lib/session.svelte';

	let tokens = $state<Token[]>([]);
	let error = $state<unknown>(null);
	let issued = $state<{ secret: string; token_id: string } | null>(null);

	let scope = $state<Scope>('read');
	let namespaces = $state('');

	let recordKind = $state<Kind>('cards');
	let recordNs = $state('');
	let recordName = $state('');
	let visibility = $state<Visibility>('public');
	let labelSeq = $state('');
	let labelName = $state('');
	let notice = $state<string | null>(null);

	let orgNs = $state('');
	let createdOrg = $state<string | null>(null);

	async function reload() {
		error = null;
		try {
			tokens = await listTokens();
		} catch (e) {
			error = e;
		}
	}

	$effect(() => {
		if (session.signedIn) void reload();
	});

	// The form starts with the caller's own login, plus the organisation an
	// organisation's page sent them here for (`?ns=`), so the token that
	// names it takes no typing.
	$effect(() => {
		if (namespaces === '' && session.who.user) {
			const org = page.url.searchParams.get('ns');
			namespaces = [session.who.user, org].filter(Boolean).join(', ');
		}
	});

	async function issue(event: SubmitEvent) {
		event.preventDefault();
		error = null;
		try {
			const list = namespaces
				.split(',')
				.map((n) => n.trim())
				.filter(Boolean);
			const result = await createToken(scope, list);
			issued = { secret: result.secret, token_id: result.token_id };
			await reload();
		} catch (e) {
			error = e;
		}
	}

	async function revoke(token: Token) {
		if (!confirm(`Revoke token ${token.prefix}…? It stops working at once.`)) return;
		try {
			await revokeToken(token.token_id);
			await reload();
		} catch (e) {
			error = e;
		}
	}

	async function applyVisibility(event: SubmitEvent) {
		event.preventDefault();
		error = null;
		notice = null;
		try {
			await setVisibility(recordKind, recordNs.trim(), recordName.trim(), visibility);
			notice = `${recordNs}/${recordName} is now ${visibility}.`;
		} catch (e) {
			error = e;
		}
	}

	async function applyLabel(event: SubmitEvent) {
		event.preventDefault();
		error = null;
		notice = null;
		try {
			await setLabel(
				recordKind,
				recordNs.trim(),
				`${recordName.trim()}@${labelSeq.trim()}`,
				labelName.trim()
			);
			notice = `Label “${labelName}” now points at version ${labelSeq}.`;
		} catch (e) {
			error = e;
		}
	}

	async function createOrganisation(event: SubmitEvent) {
		event.preventDefault();
		error = null;
		notice = null;
		try {
			const result = await createOrg(orgNs.trim());
			// The token in use predates the organisation and cannot see it;
			// the next step is a token that names it, so fill that form in.
			notice =
				`Organisation ${result.ns} created; you are its admin. Your current token does not ` +
				`name it: issue one that does (the form above is filled in) and sign in with it ` +
				`to see its members. Managing them takes an admin token.`;
			namespaces = [session.who.user, result.ns].filter(Boolean).join(', ');
			createdOrg = result.ns;
			orgNs = '';
		} catch (e) {
			error = e;
		}
	}
</script>

<svelte:head><title>Settings · evalhub</title></svelte:head>

<h1>Settings</h1>

{#if !session.signedIn}
	<p class="empty">
		<a href="/login">Sign in</a> to manage tokens and record settings.
	</p>
{:else}
	<Errors {error} />
	{#if notice}<p class="notice">{notice}</p>{/if}

	{#if issued}
		<div class="notice secret" role="alert">
			<strong>Copy this token now. It is not shown again.</strong>
			<p class="mono secret-value">{issued.secret}</p>
			<p class="small muted">
				The hub keeps only its sha256, so nobody — including the hub — can show it to you a second
				time. Lose it and issue another.
			</p>
			<button onclick={() => (issued = null)}>I have copied it</button>
		</div>
	{/if}

	<h2>Issue a token</h2>
	<form class="controls" onsubmit={issue}>
		<div>
			<label for="scope">Scope</label>
			<select id="scope" bind:value={scope}>
				<option value="read">read</option>
				<option value="write">write</option>
				<option value="admin">admin</option>
			</select>
		</div>
		<div class="grow">
			<label for="namespaces">Namespaces (comma separated)</label>
			<input
				id="namespaces"
				type="text"
				bind:value={namespaces}
				placeholder={session.who.user ?? ''}
			/>
		</div>
		<button class="primary" type="submit">Issue</button>
	</form>
	<p class="faint small">
		A new token may name your own login and organisations where you are an admin, and its scope
		cannot exceed the one you are using now.
	</p>

	<h2>Your tokens</h2>
	{#if tokens.length === 0}
		<p class="empty">No tokens.</p>
	{:else}
		<div class="scroll-x">
			<table>
				<thead>
					<tr>
						<th scope="col">Prefix</th>
						<th scope="col">Scope</th>
						<th scope="col">Namespaces</th>
						<th scope="col">Issued</th>
						<th scope="col">State</th>
						<th scope="col"></th>
					</tr>
				</thead>
				<tbody>
					{#each tokens as token (token.token_id)}
						<tr>
							<td class="mono">{token.prefix}…</td>
							<td><span class="tag">{token.scope}</span></td>
							<td class="small">{token.namespaces.join(', ')}</td>
							<td class="faint small">{token.created_at.slice(0, 10)}</td>
							<td class="small">
								{#if token.revoked_at}
									<span class="muted">revoked {token.revoked_at.slice(0, 10)}</span>
								{:else}
									active
								{/if}
							</td>
							<td>
								{#if !token.revoked_at}
									<button class="link danger" onclick={() => revoke(token)}>revoke</button>
								{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}

	<h2>Organisations</h2>
	<form class="controls" onsubmit={createOrganisation}>
		<div class="grow">
			<label for="org-ns">Namespace</label>
			<input id="org-ns" type="text" bind:value={orgNs} placeholder="acme" />
		</div>
		<button class="primary" type="submit" disabled={!orgNs.trim()}>Create</button>
	</form>
	{#if createdOrg}
		<p class="small"><a href="/ns/{createdOrg}">Open {createdOrg}</a></p>
	{/if}
	<p class="faint small">
		An organisation is a namespace with members; the creator is its first admin. A token must name
		the organisation in its namespaces to act in it.
	</p>

	<h2>Record settings</h2>
	<p class="muted small">
		Both need <code>write</code> on the namespace. Creating records is the API's job, not this UI's.
	</p>

	<div class="controls">
		<div>
			<label for="kind">Kind</label>
			<select id="kind" bind:value={recordKind}>
				<option value="cards">cards</option>
				<option value="evals">evals</option>
			</select>
		</div>
		<div>
			<label for="rec-ns">Namespace</label>
			<input id="rec-ns" type="text" bind:value={recordNs} />
		</div>
		<div class="grow">
			<label for="rec-name">Name</label>
			<input id="rec-name" type="text" bind:value={recordName} />
		</div>
	</div>

	<form class="controls" onsubmit={applyVisibility}>
		<div>
			<label for="visibility">Visibility</label>
			<select id="visibility" bind:value={visibility}>
				<option value="public">public</option>
				<option value="private">private</option>
			</select>
		</div>
		<button type="submit" disabled={!recordNs || !recordName}>Apply visibility</button>
	</form>

	<form class="controls" onsubmit={applyLabel}>
		<div>
			<label for="label-seq">Version</label>
			<input id="label-seq" type="text" bind:value={labelSeq} placeholder="seq" />
		</div>
		<div>
			<label for="label-name">Label</label>
			<input id="label-name" type="text" bind:value={labelName} placeholder="baseline" />
		</div>
		<button type="submit" disabled={!recordNs || !recordName || !labelSeq || !labelName}>
			Move label
		</button>
	</form>
	<p class="faint small">A label is unique within the name and is never purely numeric.</p>
{/if}

<style>
	.secret-value {
		font-size: 1rem;
		word-break: break-all;
		background: var(--bg);
		border: 1px solid var(--line);
		border-radius: var(--radius);
		padding: 0.5rem;
		margin: 0.5rem 0;
		user-select: all;
	}
</style>
