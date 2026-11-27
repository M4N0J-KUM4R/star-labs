<script lang="ts">
	import { env } from '$lib/env';

	/* Canvas renders an empty `#flash_message_holder` and fills it from JS
	 * (app/views/shared/_flash_notices.html.erb:19). The Rust shell puts the
	 * messages straight into ENV.notices, so we fill that same container from the
	 * same source and keep Canvas' id — its sheet positions the holder, and a
	 * renamed container loses that. */
	let messages: string[] = $derived(env().notices ?? []);

	/* .ic-flash-success/_error/_warning/_info are the real selectors
	 * (app/stylesheets/components/_alerts.scss:82-155). Nothing else in the tree
	 * is unlabelled: "notices" carries no severity, so it reads as info. */
	function kind(message: string): string {
		if (/must|can't|cannot|invalid|failed|error/i.test(message)) return 'ic-flash-error';
		if (/saved|created|updated|deleted|sent/i.test(message)) return 'ic-flash-success';
		return 'ic-flash-info';
	}
</script>

{#if messages.length}
	<div id="flash_message_holder">
		{#each messages as message (message)}
			<div class="alert {kind(message)}" role="status">
				<h4 class="ic-flash__headline">{message}</h4>
			</div>
		{/each}
	</div>
{/if}
