import { expect, test } from '@playwright/test';

/**
 * What the curl checks in `e2e/smoke.sh` cannot see: that the bundle the
 * hub serves actually boots in a browser, routes on the client, and talks
 * to `/api/v1` from the same origin. Every page here is public; nothing
 * signs in.
 */

test('the home page is the SPA, and the masthead has resolved the session', async ({ page }) => {
	await page.goto('/');
	await expect(page.getByRole('heading', { level: 1, name: 'evalhub' })).toBeVisible();
	await expect(page.getByRole('navigation', { name: 'Main' })).toBeVisible();
	// `whoami` answered: the masthead settles on "sign in" instead of the
	// loading mark, which is the first API call the UI makes.
	await expect(page.getByRole('link', { name: 'sign in' })).toBeVisible();
});

test('a deep link is served as the page it names, not a 404', async ({ page }) => {
	await page.goto('/cards');
	await expect(page.getByRole('heading', { level: 1, name: 'Cards' })).toBeVisible();
	await expect(page.getByText('Loading…')).toHaveCount(0);
	await expect(page.getByRole('alert')).toHaveCount(0);
});

test('client-side navigation reaches a list the API filled', async ({ page }) => {
	await page.goto('/');
	await page
		.getByRole('navigation', { name: 'Main' })
		.getByRole('link', { name: 'Registry' })
		.click();
	await expect(page).toHaveURL(/\/registry$/);
	await expect(page.getByRole('heading', { level: 1, name: 'Registry' })).toBeVisible();
	// `core/` entries ship with the hub, so this table is never empty on a
	// fresh database: rows here mean `GET /api/v1/registry/*` answered.
	await expect(page.getByRole('table')).toBeVisible();
	await expect(page.getByRole('alert')).toHaveCount(0);
});
