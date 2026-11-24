<script lang="ts">
	import { page } from '$app/state';
	import type { CourseTab } from '$lib/types';
	import { env } from '$lib/env';

	let { courseId, tabs = [] }: { courseId: number; tabs?: CourseTab[] } = $props();

	/* Canvas marks the active tab from `ENV.active_context_tab` (a key the Rust
	 * shell already emits — star-ui/src/env.rs ENV_KEYS) rather than from the URL,
	 * because two tabs can share a prefix. We use the same signal, falling back to
	 * the pathname so the SPA works before that key is set on a page. */
	let active: string = $derived.by(() => {
		const fromEnv = env().active_context_tab;
		if (fromEnv) return fromEnv;
		const rest = page.url.pathname.replace(`/courses/${courseId}/`, '');
		return rest.split('/')[0] || 'home';
	});

	/* A tab's `id` is Canvas' own underscored name ("assignments", "quizzes");
	 * its url is the web path, which is what the Rust shell serves. */
	function href(tab: CourseTab): string {
		if (tab.html_url) return tab.html_url;
		return `/courses/${courseId}/${tab.id}`;
	}
</script>

{#if tabs.length}
	<div class="ui-tabs">
		<ul class="ui-tabs-nav ui-helper-reset ui-helper-clearfix ui-widget-header ui-corner-all">
			{#each tabs.filter((t) => t.visible !== false) as tab (tab.id)}
				<li
					class:ui-state-active={active === tab.id}
					class:ui-tabs-active={active === tab.id}
					class:ui-state-default={active !== tab.id}
				>
					<a class="ui-tabs-anchor" href={href(tab)}>
						{tab.label}
						{#if tab.unread_count}
							<span class="menu-item__badge">{tab.unread_count}</span>
						{/if}
					</a>
				</li>
			{/each}
		</ul>
	</div>
{/if}
