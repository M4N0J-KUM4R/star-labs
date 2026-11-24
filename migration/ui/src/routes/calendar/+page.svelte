<script lang="ts">
	import { endpoints } from '$lib/api';
	import type { CalendarEvent } from '$lib/types';
	import EmptyState from '$lib/components/EmptyState.svelte';

	/* GET /api/v1/calendar_events with type=all is Canvas' own event feed. Note
	 * the open question in task #22: Canvas scopes the feed with
	 * `include_contexts[]`, while our Rust route currently reads a different
	 * param name — so no context filter is sent here until that is settled, and
	 * the page shows everything the API is willing to return. */
	let events = $state<CalendarEvent[] | null>(null);
	let failure = $state<string | null>(null);

	$effect(() => {
		endpoints.calendar
			.events()
			.then((list) => (events = list), (error) => (failure = error.publicMessage));
	});

	function when(event: CalendarEvent): string {
		if (!event.start_at) return '—';
		const date = new Date(event.start_at);
		return event.all_day ? date.toLocaleDateString() : date.toLocaleString();
	}
</script>

<div class="page-header">
	<h1>Calendar</h1>
</div>

{#if failure}
	<div class="ic-flash-error" role="alert">{failure}</div>
{:else if events === null}
	<span class="screenreader-only" role="status">Loading events</span>
{:else if events.length === 0}
	<EmptyState title="No events" body="Nothing on the calendar for this range." />
{:else}
	<table class="ic-Table ic-Table--hover-row">
		<thead>
			<tr class="ic-Table--header-row">
				<th>Title</th>
				<th>When</th>
				<th>Type</th>
			</tr>
		</thead>
		<tbody>
			{#each events as event (event.id)}
				<tr>
					<td>
						{#if event.html_url}
							<a href={event.html_url}>{event.title}</a>
						{:else}
							{event.title}
						{/if}
					</td>
					<td>{when(event)}</td>
					<td>{event.type ?? 'event'}</td>
				</tr>
			{/each}
		</tbody>
	</table>
{/if}
