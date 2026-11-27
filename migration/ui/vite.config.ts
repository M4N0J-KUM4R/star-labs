import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// No Tailwind plugin here, deliberately. The stylesheet is Canvas' own compiled
// SCSS (see src/styles/README.md), and Tailwind's preflight resets box-sizing,
// margins, borders and font smoothing — which would move every Canvas measurement
// off the reference render. One styling system, and it is Canvas'.
export default defineConfig({
	plugins: [sveltekit()],
	// No `css.preprocessorOptions.scss.loadPaths` here: Vite's sass types expose
	// `importers`/`additionalData`, not a load path. Canvas bundles are compiled
	// by scripts/build-canvas-css.sh, which passes --load-path to sass directly,
	// so the SPA only ever imports finished CSS.
	server: {
		port: 5173
	},
	test: {
		include: ['src/**/*.{test,spec}.{js,ts}']
	}
});
