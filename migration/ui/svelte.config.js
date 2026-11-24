import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
	preprocess: vitePreprocess(),
	kit: {
		// adapter-static: `npm run build` emits plain files under `build/` that
		// axum serves from `crates/star-ui/static/`. There is deliberately no
		// adapter-node here — a Node SSR process would be a second runtime per
		// deployment for an app whose pages are all post-login and uncachable,
		// so SSR would buy nothing and cost a process.
		adapter: adapter({
			pages: 'build',
			assets: 'build',
			// Every unmatched path returns this shell; the shell is the same
			// document the Rust layout renders (with window.ENV injected), so
			// client-side routing takes over from there.
			fallback: 'index.html',
			// No prerendering: pages need the per-user ENV bootstrap that only
			// the Rust request cycle has.
			precompress: false,
			strict: true
		}),
		paths: { base: '' },
	}
};

export default config;
