<script lang="ts">
	import '../app.css';
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { session } from '$lib/auth.svelte';
	import { env } from '$lib/env';
	import { loadCanvasStyles, setCanvasBodyClasses } from '$lib/canvasStyles';
	import AppHeader from '$lib/components/AppHeader.svelte';
	import Flash from '$lib/components/Flash.svelte';

	let { children } = $props();

	/* tokens + fonts + common, i.e. what `include_common_stylesheets` emits on
	 * every Canvas page; each route adds its own bundle on top. */
	$effect(() => loadCanvasStyles());

	/* `body.primary-nav-expanded` is what gives .ic-app-header its 84px rail and
	 * .ic-Layout-wrapper its 84px left margin (common.css); Canvas adds it in
	 * application.html.erb:102 unless @collapse_global_nav. The auth pages are the
	 * exception: app/views/login/canvas/new.html.erb:29 replaces it with
	 * `full-width ic-Login-Body`, which is what paints the #334451 backdrop.
	 * Kept in one place — a page-level call would race this effect for the body. */
	$effect(() => {
		const open = page.url.pathname.startsWith('/login') || page.url.pathname.startsWith('/register');
		setCanvasBodyClasses(open ? ['full-width', 'ic-Login-Body'] : ['primary-nav-expanded']);
	});

	/* Canvas compiles a separate base sheet per accessibility setting
	 * (`variants/new_styles_high_contrast*`, `…_dyslexic*`). The contrast half is
	 * handled by loadCanvasStyles() picking the variant directory; the root
	 * attributes below are what the dyslexic branch keys off, and they mirror
	 * ENV/PREFERENCES so axum can emit or suppress them in the shell later. */
	$effect(() => {
		const e = env();
		document.documentElement.toggleAttribute('data-use-high-contrast', e.use_high_contrast === true);
		document.documentElement.toggleAttribute(
			'data-use-dyslexic-font',
			e.PREFERENCES?.user_system_default_font_family === 'OpenDyslexic'
		);
	});

	$effect(() => {
		void session.bootstrap();
	});

	/* One guard for the whole app, instead of a redirect block in thirteen pages.
	 * `session.ready` gates it so a bootstrap in flight is not read as signed out.
	 * Canvas' own anonymous pages are login/register, so those stay reachable. */
	$effect(() => {
		if (!session.ready) return;
		const open = page.url.pathname.startsWith('/login') || page.url.pathname.startsWith('/register');
		if (!session.signedIn && !open) void goto('/login');
		if (session.signedIn && open) void goto('/dashboard');
	});

</script>

<!-- Nesting is copied from app/views/layouts/application.html.erb:132-283, ids and
     all, because common.css targets them: `#right-side-wrapper`, `.ic-app`,
     `.ic-Layout-wrapper` on #wrapper, `.ic-Layout-contentWrapper` on
     #content-wrapper. Two nodes here have no rule yet and are kept anyway —
     #instructure_ajax_error_box and #right-side — because Canvas' JS and sheets
     address them and their absence would change the flex column widths.
     The former `div.ic-app-container` wrapper is gone: `grep -rl ic-app-container
     canvas-src` returns nothing, so the class existed only in our markup. -->
{#if session.signedIn}
	<Flash />
{/if}
<div id="application" class="ic-app">
	{#if session.signedIn}
		<AppHeader />
	{/if}
	<div id="instructure_ajax_error_box">
		<iframe id="instructure_ajax_error_result" src="about:blank" title="Error" style="border:0"></iframe>
	</div>
	<div id="wrapper" class="ic-Layout-wrapper">
		<div id="not_right_side" class="ic-app-main-content">
			<div id="content-wrapper" class="ic-Layout-contentWrapper">
				<div id="content" class="ic-Layout-contentMain" role="main">
					{@render children()}
				</div>
			</div>
			<div id="right-side-wrapper" class="ic-app-main-content__secondary">
				<aside id="right-side"></aside>
			</div>
		</div>
	</div>
</div>
