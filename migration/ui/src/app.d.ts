import '@sveltejs/kit';

declare global {
	namespace App {
		interface Error {
			message: string;
			status?: number;
		}
		// interface PageData {}
		// interface Platform {}
	}
}

export {};

// Canvas' nav icons ship as the .svg partials it renders inline
// (app/views/shared/svg/_svg_icon_*.svg), imported as text so the markup that
// reaches the DOM is Canvas' own and `.ic-icon-svg` in common.css styles it.
// vite/client already types this; the declaration is repeated so `svelte-check`
// resolves it without depending on that include.
declare module '*.svg?raw' {
	const content: string;
	export default content;
}
