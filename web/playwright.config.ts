import { defineConfig, devices } from '@playwright/test';

/**
 * Browser tests against a running hub.
 *
 * There is deliberately no `webServer` here: the server under test is the
 * release `evalhub` binary with the UI embedded, and `e2e/smoke.sh` (via
 * `just e2e`) is what builds, migrates and starts it. A `vite preview`
 * server would prove nothing about the binary. The harness hands the URL
 * over in `EVALHUB_E2E_BASE_URL`; the default only serves a developer
 * pointing the suite at a hub they started by hand.
 *
 * One worker: the tests share one Postgres, and a hub with a handful of
 * pages does not need more.
 */
export default defineConfig({
	testDir: './tests/browser',
	fullyParallel: false,
	workers: 1,
	retries: 0,
	forbidOnly: !!process.env.CI,
	reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'list',
	use: {
		baseURL: process.env.EVALHUB_E2E_BASE_URL ?? 'http://127.0.0.1:8080',
		trace: 'retain-on-failure'
	},
	projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }]
});
