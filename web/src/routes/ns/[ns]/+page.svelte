<script lang="ts">
	// A namespace: what it holds and, for an organisation, who is in it.
	// The counts are the hub's, and they count only what this caller may
	// see — the same number looks different to a member and to a stranger,
	// which is the visibility rule working rather than a bug.
	import { page } from '$app/state';
	import {
		getNamespace,
		listMembers,
		listRecords,
		addMember,
		removeMember,
		type ListItem,
		type Member,
		type Namespace,
		type Scope
	} from '$lib/api/client';
	import Errors from '$lib/components/Errors.svelte';
	import { session } from '$lib/session.svelte';

	const ns = $derived(page.params.ns ?? '');

	let info = $state<Namespace | null>(null);
	let cards = $state<ListItem[]>([]);
	let evals = $state<ListItem[]>([]);
	let members = $state<Member[] | null>(null);
	let error = $state<unknown>(null);

	let newUser = $state('');
	let newRole = $state<Scope>('read');
	let notice = $state<string | null>(null);

	async function reloadMembers() {
		members = await listMembers(ns).catch(() => null);
	}

	$effect(() => {
		const current = ns;
		error = null;
		info = null;
		members = null;
		(async () => {
			try {
				info = await getNamespace(current);
				cards = (await listRecords('cards', { ns: current, limit: 25 })).items;
				evals = (await listRecords('evals', { ns: current, limit: 25 })).items;
				if (info.kind === 'org') {
					// Only a member may read the roster; a 403 here is normal.
					members = await listMembers(current).catch(() => null);
				}
			} catch (e) {
				error = e;
			}
		})();
	});

	async function add(event: SubmitEvent) {
		event.preventDefault();
		error = null;
		notice = null;
		try {
			const user = newUser.trim();
			const role = newRole;
			await addMember(ns, user, role);
			notice = `${user} is now ${role} in ${ns}.`;
			newUser = '';
			await reloadMembers();
		} catch (e) {
			error = e;
		}
	}

	async function remove(member: Member) {
		if (!confirm(`Remove ${member.user} from ${ns}?`)) return;
		try {
			await removeMember(ns, member.user);
			notice = `${member.user} removed from ${ns}.`;
			await reloadMembers();
		} catch (e) {
			error = e;
		}
	}
</script>

<svelte:head><title>{ns} · evalhub</title></svelte:head>

<h1>{ns}</h1>
{#if info}
	<p class="muted small">
		<span class="tag">{info.kind}</span>
		created {info.created_at.slice(0, 10)} · {info.cards} Cards · {info.evals} Evals visible to you
	</p>
{/if}

<Errors {error} />

{#if members}
	<h2>Members</h2>
	<div class="scroll-x">
		<table>
			<thead>
				<tr>
					<th scope="col">User</th>
					<th scope="col">Role</th>
				</tr>
			</thead>
			<tbody>
				{#each members as member (member.user)}
					<tr>
						<td><a href="/ns/{member.user}">{member.user}</a></td>
						<td><span class="tag">{member.role}</span></td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
	<p class="faint small">
		A member's token acts with the lesser of its own scope and their role here.
	</p>
{/if}

{#snippet recordTable(kind: 'cards' | 'evals', items: ListItem[])}
	{#if items.length === 0}
		<p class="empty">None visible.</p>
	{:else}
		<div class="scroll-x">
			<table>
				<thead>
					<tr>
						<th scope="col">Name</th>
						<th scope="col">Title</th>
						<th scope="col">Version</th>
						<th scope="col">Visibility</th>
					</tr>
				</thead>
				<tbody>
					{#each items as item (item.name)}
						<tr>
							<td><a href="/{kind}/{item.ns}/{item.name}">{item.name}</a></td>
							<td>{item.title ?? ''}</td>
							<td class="num">{item.latest.seq}</td>
							<td class="small">{item.visibility}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
{/snippet}

<h2>Cards</h2>
{@render recordTable('cards', cards)}

<h2>Evals</h2>
{@render recordTable('evals', evals)}
