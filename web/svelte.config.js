import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/**
 * Single-page application: one `index.html` that the Rust binary embeds
 * from `web/build` and serves for every browser path, with routing done in
 * the client. `fallback` is what makes that work; `pages`/`assets` name the
 * directory `rust-embed` reads.
 *
 * @type {import('@sveltejs/kit').Config}
 */
export default {
	preprocess: vitePreprocess(),
	kit: {
		adapter: adapter({
			pages: 'build',
			assets: 'build',
			fallback: 'index.html',
			precompress: false,
			strict: false
		}),
		// The hub serves the UI at the origin root, same origin as the API,
		// so there is no base path and no CORS.
		paths: { base: '' }
	}
};
