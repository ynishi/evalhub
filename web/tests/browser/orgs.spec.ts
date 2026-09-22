/**
 * End-to-end tests for the organisation screens through the real binary.
 *
 * Proves: an admin can create an org, issue a token that names it, sign in with
 * that token, add a member, and remove a member.
 *
 * Skipped when the harness did not provide EVALHUB_E2E_TOKEN (e.g. during
 * `pnpm exec playwright test` without the e2e/smoke.sh wrapper).
 */

import { expect, test } from '@playwright/test';

declare const process: {
	env: Record<string, string | undefined>;
};

const token = process.env.EVALHUB_E2E_TOKEN;
const member = process.env.EVALHUB_E2E_MEMBER ?? 'bob';

test.describe('organisations', () => {
	test.skip(!token, 'EVALHUB_E2E_TOKEN is not set; e2e/smoke.sh provides it');

	test('an admin creates an organisation, issues a token for it, and manages its members', async ({
		page
	}) => {
		// 1. Sign in as alice with the admin token from the harness.
		await page.goto('/login');
		await page.locator('#token').fill(token!);
		await page.getByRole('button', { name: 'Sign in' }).click();
		await expect(
			page.locator('header.masthead').getByRole('link', { name: 'alice' })
		).toBeVisible();

		// 2. Create an organisation from /settings.
		await page.goto('/settings');
		const org = 'acme-' + Date.now().toString(36);
		await page.locator('#org-ns').fill(org);
		await page.getByRole('button', { name: 'Create' }).click();
		await expect(page.getByText(`Organisation ${org} created`)).toBeVisible();
		await expect(page.getByRole('link', { name: `Open ${org}` })).toBeVisible();

		// 3. Issue a token that names alice and the new org.
		await page.locator('#scope').selectOption('admin');
		await page.locator('#namespaces').fill(`alice, ${org}`);
		await page.getByRole('button', { name: 'Issue' }).click();
		await expect(page.getByRole('alert')).toContainText('Copy this token now');
		const secret = (await page.locator('.secret-value').textContent())?.trim() ?? '';
		expect(secret.length).toBeGreaterThan(20);
		await page.getByRole('button', { name: 'I have copied it' }).click();

		// 4. Sign out, then sign back in with the new token that names the org.
		await page.locator('header.masthead').getByRole('button', { name: 'sign out' }).click();
		await expect(page).toHaveURL('/login');
		await page.locator('#token').fill(secret);
		await page.getByRole('button', { name: 'Sign in' }).click();
		await expect(
			page.locator('header.masthead').getByRole('link', { name: 'alice' })
		).toBeVisible();

		// 5. Navigate to the org page and verify admin row.
		await page.goto(`/ns/${org}`);
		await expect(page.getByRole('heading', { level: 1, name: org })).toBeVisible();
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
});
