/**
 * End-to-end tests for the organisation screens through the real binary.
 *
 * Proves: an admin can create an org, is told on its page that the token in use
 * does not name it, finds the token form filled in for it, signs in with the
 * token that names it, adds a member and removes one; and that a `write` token
 * naming the org shows the roster without the controls the hub would refuse.
 *
 * Skipped when the harness did not provide EVALHUB_E2E_TOKEN (e.g. during
 * `pnpm exec playwright test` without the e2e/smoke.sh wrapper).
 */

import { expect, test, type Page } from '@playwright/test';

declare const process: {
	env: Record<string, string | undefined>;
};

const token = process.env.EVALHUB_E2E_TOKEN;
const member = process.env.EVALHUB_E2E_MEMBER ?? 'bob';

async function signIn(page: Page, secret: string) {
	await page.goto('/login');
	await page.locator('#token').fill(secret);
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page.locator('header.masthead').getByRole('link', { name: 'alice' })).toBeVisible();
}

async function signOut(page: Page) {
	await page.locator('header.masthead').getByRole('button', { name: 'sign out' }).click();
	await expect(page).toHaveURL('/login');
}

/** Create an organisation from /settings as the signed-in user; returns its slug. */
async function createOrg(page: Page): Promise<string> {
	await page.goto('/settings');
	const org = 'acme-' + Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
	await page.locator('#org-ns').fill(org);
	await page.getByRole('button', { name: 'Create' }).click();
	await expect(page.getByText(`Organisation ${org} created`)).toBeVisible();
	return org;
}

/** The token form names the caller and the organisation without typing. */
async function expectPrefilled(page: Page, org: string) {
	await expect(page.locator('#namespaces')).toHaveValue(`alice, ${org}`);
}

/** Issue a token with the namespaces the form already holds; returns the secret. */
async function issue(page: Page, scope: 'read' | 'write' | 'admin'): Promise<string> {
	await page.locator('#scope').selectOption(scope);
	await page.getByRole('button', { name: 'Issue' }).click();
	await expect(page.getByRole('alert')).toContainText('Copy this token now');
	const secret = (await page.locator('.secret-value').textContent())?.trim() ?? '';
	expect(secret.length).toBeGreaterThan(20);
	await page.getByRole('button', { name: 'I have copied it' }).click();
	return secret;
}

test.describe('organisations', () => {
	test.skip(!token, 'EVALHUB_E2E_TOKEN is not set; e2e/smoke.sh provides it');

	test('an admin creates an organisation, issues a token for it, and manages its members', async ({
		page
	}) => {
		// 1. Sign in as alice with the admin token from the harness.
		await signIn(page, token!);

		// 2. Create an organisation from /settings.
		const org = await createOrg(page);
		await expect(page.getByRole('link', { name: `Open ${org}` })).toBeVisible();

		// 3. Its page, with the token that created it: no roster, and a line
		// saying why instead of silence.
		await page.getByRole('link', { name: `Open ${org}` }).click();
		await expect(page.getByRole('heading', { level: 1, name: org })).toBeVisible();
		await expect(page.getByText(`Your token does not name ${org}`)).toBeVisible();
		await expect(page.getByRole('heading', { name: 'Members' })).toHaveCount(0);

		// 4. The line links back to /settings with the token form already
		// naming alice and the org.
		await page.getByRole('link', { name: 'issue one in Settings' }).click();
		await expectPrefilled(page, org);
		const secret = await issue(page, 'admin');

		// Sign out, then sign back in with the new token that names the org.
		await signOut(page);
		await signIn(page, secret);

		// 5. Navigate to the org page and verify admin row.
		await page.goto(`/ns/${org}`);
		await expect(page.getByRole('heading', { level: 1, name: org })).toBeVisible();
		await expect(page.getByText(`Your token does not name ${org}`)).toHaveCount(0);
		await expect(page.getByRole('heading', { name: 'Members' })).toBeVisible();
		const aliceRow = page.getByRole('row', { name: /alice/ });
		await expect(aliceRow).toContainText('admin');
		await expect(aliceRow.getByRole('button', { name: 'remove' })).toHaveCount(0);

		// 6. Add member.
		await page.locator('#member-user').fill(member);
		await page.locator('#member-role').selectOption('write');
		await page.getByRole('button', { name: 'Add or update' }).click();
		await expect(page.getByText(`${member} is now write in ${org}.`)).toBeVisible();
		const memberRow = page.getByRole('row', {
			name: new RegExp(member)
		});
		await expect(memberRow).toContainText('write');

		// 7. Remove member.
		page.once('dialog', (d) => d.accept());
		await memberRow.getByRole('button', { name: 'remove' }).click();
		await expect(page.getByText(`${member} removed from ${org}.`)).toBeVisible();
		await expect(page.getByRole('row', { name: new RegExp(member) })).toHaveCount(0);

		// 8. No error notices.
		await expect(page.getByRole('alert')).toHaveCount(0);
	});

	test('a write token that names the organisation shows its roster without member controls', async ({
		page
	}) => {
		await signIn(page, token!);
		const org = await createOrg(page);
		// Straight after creating it, the same screen's form names it.
		await expectPrefilled(page, org);
		const secret = await issue(page, 'write');
		await signOut(page);
		await signIn(page, secret);

		await page.goto(`/ns/${org}`);
		await expect(page.getByRole('heading', { name: 'Members' })).toBeVisible();
		await expect(page.getByRole('row', { name: /alice/ })).toContainText('admin');
		await expect(page.locator('#member-user')).toHaveCount(0);
		await expect(page.getByRole('button', { name: 'remove' })).toHaveCount(0);
		await expect(page.getByText('Adding and removing members takes an admin token')).toBeVisible();
	});
});
