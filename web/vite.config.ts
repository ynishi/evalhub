import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [sveltekit()],
	server: {
		// `pnpm dev` serves the UI on 5173 and forwards the API to a hub
		// running the usual `evalhub serve --bind 127.0.0.1:8080`, so the
		// session cookie stays same-origin in development too.
		proxy: {
			'/api': 'http://127.0.0.1:8080',
			'/schemas': 'http://127.0.0.1:8080',
			'/openapi.json': 'http://127.0.0.1:8080'
		}
	}
});
