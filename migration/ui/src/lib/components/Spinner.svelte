<script lang="ts">
	/* Every page renders through this so a slow API call never paints an empty
	 * shell. `role=status` + aria-live keeps it announced, which matters because
	 * with ssr=false a screen reader otherwise hears nothing until data lands.
	 * No class here: `ic-Ellipsis` is not in this Canvas checkout, and inventing a
	 * Canvas-prefixed name is exactly the drift src/styles/README.md forbids. */
	let { label = 'Loading' }: { label?: string } = $props();
</script>

<span class="screenreader-only" role="status" aria-live="polite">{label}</span>
<span aria-hidden="true">{label}...</span>
