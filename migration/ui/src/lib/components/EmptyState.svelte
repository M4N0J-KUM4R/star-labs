<script lang="ts">
	import type { Snippet } from 'svelte';

	/* .ic-EmptyState is Canvas' own class (ui/features/context_modules/react/
	 * ModulesHomePage.tsx) — it centres and spaces a nothing-here panel, so the
	 * empty states across pages stay visually identical without shared CSS here. */
	let {
		title,
		body = '',
		children = undefined
	}: { title: string; body?: string; children?: Snippet } = $props();
</script>

<div class="ic-EmptyState">
	<h3 class="ic-EmptyState__title">{title}</h3>
	{#if body}
		<p class="ic-EmptyState__body">{body}</p>
	{/if}
	{#if children}
		<div class="ic-EmptyState__actions">
			{@render children()}
		</div>
	{/if}
</div>
