/** Loads Canvas' vendored stylesheet bundles at run time.
 *
 *  Why links and not a bundled `import` of the sheets: the sheet a user gets depends on
 *  `ENV.use_high_contrast`, which is known per request, not per build. Anything
 *  Vite bundles is frozen at build time, so the variant switch would need two
 *  builds. Canvas solves this server-side (`include_common_stylesheets` emits the
 *  `<link>`s from `layouts/_head.html.erb`), and MIGRATION.md step 2 moves these
 *  same tags into axum's `shell.html`. Until then the app appends exactly the
 *  `<link>` elements Canvas would have emitted, from the same paths, in the same
 *  order: tokens, then fonts, then common, then the page bundle.
 *
 *  One difference from Canvas, stated plainly: a full-page navigation in Canvas
 *  unloads the previous page's bundle, while an SPA keeps everything it has ever
 *  loaded. Canvas' rules are class-scoped (`.ic-DashboardCard`, `.ic-Login`) so
 *  extra sheets cost bytes on the first SPA visit but cannot restyle a page.
 *  Nothing here removes a link, because removing one a sibling route still needs
 *  would flicker.
 */

import { env } from '$lib/env';

/** Mirrors `layout_body_classes`/`include_common_stylesheets`: the two sheets
 *  every Canvas page gets, before any page-specific bundle. */
const ALWAYS = ['tokens', 'fonts', 'common'];

let currentVariant: string | null = null;

export function canvasVariant(): 'normal_contrast' | 'high_contrast' {
	return env().use_high_contrast ? 'high_contrast' : 'normal_contrast';
}

function hrefFor(variant: string, name: string): string {
	return `/canvas/${variant}/${name}.css`;
}

function ensureLink(variant: string, name: string): void {
	if (typeof document === 'undefined') return;
	const href = hrefFor(variant, name);
	if (document.head.querySelector(`link[data-canvas-bundle="${name}"]`)) return;
	const link = document.createElement('link');
	link.rel = 'stylesheet';
	link.href = href;
	link.dataset.canvasBundle = name;
	document.head.appendChild(link);
}

/** Re-stack the sheets so their document order is always tokens → fonts → common
 *  → page bundles.
 *
 *  Cascade order is the whole game for a stylesheet port: `.ic-Login` and
 *  `.ic-Login__content` set overlapping backgrounds, and which one wins depends on
 *  file order, not specificity. Svelte gives no guarantee that the layout's
 *  `$effect` runs before a page's, so relying on insertion order would make the
 *  winner a property of effect scheduling. Re-appending in rank order makes it a
 *  property of this file instead — `appendChild` moves an existing node rather
 *  than refetching it. */
function restack(): void {
	const links = [...document.head.querySelectorAll<HTMLLinkElement>('link[data-canvas-bundle]')];
	const rank = (name: string): number => {
		const i = ORDER.indexOf(name);
		return i === -1 ? ORDER.length : i;
	};
	links.sort((a, b) => rank(a.dataset.canvasBundle ?? '') - rank(b.dataset.canvasBundle ?? ''));
	for (const link of links) document.head.appendChild(link);
}

/** Canvas' own emission order, from `_head.html.erb` and `include_common_stylesheets`. */
const ORDER = ['tokens', 'fonts', 'common'];

/** Append the bundles `bundles` needs on top of the ones already loaded.
 *  Called from a component's `$effect`, so it runs after `window.ENV` exists. */
export function loadCanvasStyles(bundles: string[] = []): void {
	const variant = canvasVariant();
	// A variant change means a different directory, not a different file, so the
	// links from the old variant must go rather than stack — stacking would show
	// normal-contrast colours under high contrast.
	if (currentVariant !== null && currentVariant !== variant) {
		for (const link of document.head.querySelectorAll('link[data-canvas-bundle]')) link.remove();
	}
	currentVariant = variant;
	for (const name of [...ALWAYS, ...bundles]) ensureLink(variant, name);
	restack();
}


/** The classes `app/views/layouts/application.html.erb:83-110` would have put on
 *  `<body>` for this page.
 *
 *  They are load-bearing, not decoration: `.ic-Layout-wrapper` only gets its
 *  1366px max-width from `body:not(.full-width):not(.outcomes)…`, the 84px side
 *  nav from `body.primary-nav-expanded .ic-app-header`, and `ic-Login-Body` is
 *  what paints the dark #334451 backdrop behind the login card. Canvas computes
 *  them in the controller; a static shell has no such hook, so each route states
 *  its own and the previous route's are removed on the next call.
 *
 *  `full-width` and `ic-Login-Body` are the only two a page needs today; the
 *  rest of Canvas' list (`with-right-side`, `padless-content`,
 *  `is-masquerading-or-student-view`, `context-<asset>`) describe panes this port
 *  does not render yet. */
let appliedBodyClasses: string[] = [];

export function setCanvasBodyClasses(classes: string[]): void {
	if (typeof document === 'undefined') return;
	for (const stale of appliedBodyClasses) if (!classes.includes(stale)) document.body.classList.remove(stale);
	for (const cls of classes) document.body.classList.add(cls);
	appliedBodyClasses = classes;
}

