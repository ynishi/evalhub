/**
 * The Eval page's Runs table through the real binary: the rows come from
 * `GET …/runs`, not from the record body (an `evalhub.eval/2.0` header has
 * no `runs`), and choosing a Card joins its judgements.
 *
 * The data is seeded through the API with the admin token the harness
 * hands over, then made public so the page is read without signing in.
 * Skipped when the harness did not provide EVALHUB_E2E_TOKEN (e.g. during
 * `pnpm exec playwright test` without the e2e/smoke.sh wrapper).
 */

import { expect, test, type APIRequestContext } from '@playwright/test';

declare const process: {
	env: Record<string, string | undefined>;
};

const token = process.env.EVALHUB_E2E_TOKEN;
const ns = 'alice';

/** A fresh name per run of the suite: the database outlives one test. */
function unique(prefix: string): string {
	return prefix + '-' + Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
}

async function call(
	request: APIRequestContext,
	method: 'POST' | 'PUT' | 'PATCH',
	path: string,
	data: unknown
) {
	const response = await request.fetch(`/api/v1${path}`, {
		method,
		headers: { authorization: `Bearer ${token}` },
		data
	});
	expect(response.status(), `${method} ${path}: ${await response.text()}`).toBeLessThan(300);
	return response;
}

const facets = {
	model: { id: 'qwen3.6-32b' },
	task: { id: 'single2', version: '3', split: 'test', n: 33 }
};

test.describe('runs', () => {
	test.skip(!token, 'EVALHUB_E2E_TOKEN is not set; e2e/smoke.sh provides it');

	test('the Eval page paints the runs GET …/runs returns, and a Card’s judgements', async ({
		page,
		request
	}) => {
		const evalName = unique('runs-e2e');
		const cardName = unique('runs-e2e-card');

		// 1. A 2.0 header, then its runs, one PUT each.
		await call(request, 'POST', `/evals/${ns}/${evalName}`, {
			schema: 'evalhub.eval/2.0',
			title: 'runs e2e',
			producer: { name: 'e2e', version: '0' },
			eval_kind: 'run_set',
			origin: 'live',
			harness: { name: 'e2e', version: '0' },
			...facets,
			ext: {}
		});
		await call(request, 'PUT', `/evals/${ns}/${evalName}/runs/r1`, {
			run_id: 'r1',
			status: 'ok',
			started_at: '2026-09-20T10:00:00Z',
			ended_at: '2026-09-20T10:02:11Z',
			metrics: { 'core/tokens_out': 1834.0 }
		});
		await call(request, 'PUT', `/evals/${ns}/${evalName}/runs/r2`, {
			run_id: 'r2',
			status: 'error',
			error: { kind: 'timeout', message: 'no answer in 600 s' },
			started_at: '2026-09-20T10:03:00Z',
			ended_at: '2026-09-20T10:13:00Z'
		});

		// 2. A Card that used both runs and judged them.
		await call(request, 'POST', `/cards/${ns}/${cardName}`, {
			schema: 'evalhub.card/1.1',
			title: 'runs e2e card',
			producer: { name: 'e2e', version: '0' },
			...facets,
			grading: { graders: [{ name: 'exact_match', kind: 'deterministic' }] },
			results: [{ metric: 'core/pass_rate', value: 0.5, n: 2, aggregation: 'mean' }],
			counts: { attempted: 2, completed: 1, failed: 1, skipped: 0, errored: 0 },
			run_results: [
				{ eval: `${ns}/${evalName}`, run_id: 'r1', metric: 'core/pass', value: 1.0, label: 'pass' },
				{ eval: `${ns}/${evalName}`, run_id: 'r2', metric: 'core/pass', label: 'fail' }
			],
			relations: [
				{ type: 'core/uses_eval', to: `${ns}/${evalName}@1`, attrs: { runs: ['r1', 'r2'] } }
			],
			ext: {}
		});
		await call(request, 'PATCH', `/evals/${ns}/${evalName}/settings`, { visibility: 'public' });
		await call(request, 'PATCH', `/cards/${ns}/${cardName}/settings`, { visibility: 'public' });

		// 3. The page: the header counts the runs from the envelope, the
		// table has one row per run from `/runs`.
		const fetched = page.waitForResponse(
			(r) => r.url().includes(`/api/v1/evals/${ns}/${evalName}/runs`) && r.status() === 200
		);
		await page.goto(`/evals/${ns}/${evalName}`);
		await fetched;
		await expect(page.getByRole('heading', { level: 1, name: 'runs e2e' })).toBeVisible();
		await expect(page.getByText('2 runs: 1 ok, 1 error, 0 skipped')).toBeVisible();

		const table = page.getByRole('table').filter({ has: page.getByRole('cell', { name: 'r1' }) });
		await expect(table.getByRole('columnheader', { name: 'core/tokens_out' })).toBeVisible();
		const r1 = table.getByRole('row').filter({ has: page.getByRole('cell', { name: 'r1' }) });
		await expect(r1.getByRole('cell', { name: 'ok', exact: true })).toBeVisible();
		await expect(r1.getByRole('cell', { name: '1834' })).toBeVisible();
		const r2 = table.getByRole('row').filter({ has: page.getByRole('cell', { name: 'r2' }) });
		await expect(r2.getByRole('cell', { name: 'error', exact: true })).toBeVisible();
		await expect(r2.getByRole('cell', { name: 'timeout' })).toBeVisible();

		// 4. Choosing the Card joins its judgements: one column per metric it
		// judged, filled from `GET …/runs?cards=`.
		const joined = page.waitForResponse(
			(r) => r.url().includes('cards=') && r.url().includes(`/runs`) && r.status() === 200
		);
		await page.locator('#judge-card').selectOption(`${ns}/${cardName}`);
		await joined;
		await expect(table.getByRole('columnheader', { name: /core\/pass/ })).toBeVisible();
		await expect(r1.getByRole('cell', { name: '1 pass' })).toBeVisible();
		await expect(r2.getByRole('cell', { name: 'fail' })).toBeVisible();
		await expect(page.getByRole('alert')).toHaveCount(0);
	});
});
