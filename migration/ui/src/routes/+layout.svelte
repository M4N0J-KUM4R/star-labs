<script lang="ts">
	import '../app.css';
import { page } from '$app/state';
import { goto } from '$app/navigation';
import { session } from '$lib/auth.svelte';
import { env } from '$lib/env';
import AppHeader from '$lib/components/AppHeader.svelte';
import Flash from '$lib/components/Flash.svelte';

let { children } = $props();

/* Canvas' base sheet switches variants with `$use_high_contrast` at compile
 * time; we drive the same token overrides from a root attribute so one CSS file
 * serves both. The value comes from the ENV key the Rust shell already emits
 * (star-ui/src/env.rs ENV_KEYS: "use_high_contrast"). */
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

<!-- The ids below are Canvas' own (#header, #content) because its sheets and
     its JS both target them; renaming them would break the vendored CSS. -->
<div class="ic-app-container">
	{#if session.signedIn}
		<AppHeader />
	{/if}
	<div id="not_right_side" class="ic-app-main-content">
		<div class="ic-Layout-wrapper">
			<div class="ic-Layout-columns">
				<div id="content" class="ic-Layout-contentMain" role="main">
					<Flash />
					{@render children()}
				</div>
			</div>
		</div>
	</div>
</div>
