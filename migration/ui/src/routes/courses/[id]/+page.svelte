<script lang="ts">
	import { page } from '$app/state';
	import { endpoints } from '$lib/api';
	import type { Assignment, DiscussionTopic } from '$lib/types';
	import EmptyState from '$lib/components/EmptyState.svelte';
	import { loadCanvasStyles } from '$lib/canvasStyles';

	/* Canvas loads exactly these bundles for this page: discussions, wiki_page.
	 *  See scripts/build-canvas-css.sh for where the names come from. */
	$effect(() => loadCanvasStyles(['discussions', 'wiki_page']));

	/* Canvas' course home (app/views/courses/home.html.erb) is a feed of recent
	 * discussions plus upcoming assignments, not a landing card. Keep that shape:
	 * a user who knows Canvas should find the same two blocks here. */
	let courseId = $derived(Number(page.params.id));
	let topics = $state<DiscussionTopic[] | null>(null);
	let assignments = $state<Assignment[] | null>(null);
	$effect(() => {
		if (!Number.isFinite(courseId)) return;
		endpoints.discussions
			.list(courseId)
			.then((list) => (topics = list), () => (topics = []));
		endpoints.assignments
			.list(courseId, { bucket: 'upcoming', order_by: 'position' })
			.then((list) => (assignments = list), () => (assignments = []));
	});
	function due(iso?: string | null): string {
		return iso ? new Date(iso).toLocaleDateString() : '—';
	}

</script>

{#if topics === null || assignments === null}
	<span class="screenreader-only" role="status">Loading course home</span>
{:else}
	<h2>Recent Discussions</h2>
	{#if topics.length === 0}
		<EmptyState title="No discussions" body="Nothing has been posted to this course yet." />
	{:else}
		<table class="ic-Table ic-Table--hover-row">
			<thead>
				<tr class="ic-Table--header-row">
					<th>Title</th>
					<th>Last activity</th>
				</tr>
			</thead>
			<tbody>
				{#each topics.slice(0, 10) as topic (topic.id)}
					<tr>
						<td><a href="/courses/{courseId}/discussion_topics/{topic.id}">{topic.title}</a></td>
						<td>{due(topic.last_reply_at ?? topic.posted_at)}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}

	<h2>Upcoming</h2>
	{#if (assignments ?? []).length === 0}
		<EmptyState title="Nothing due" body="No assignments are upcoming in this course." />
	{:else}
		<table class="ic-Table ic-Table--condensed">
			<tbody>
				{#each (assignments ?? []).slice(0, 15) as assignment (assignment.id)}
					<tr class:unpublished={assignment.published === false}>
						<td><a href={assignment.html_url ?? `/courses/${courseId}/assignments/${assignment.id}`}>{assignment.name}</a></td>
						<td>{due(assignment.due_at)}</td>
						<td>{assignment.points_possible ?? '—'}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
{/if}
