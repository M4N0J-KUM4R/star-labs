<script lang="ts">
	import { page } from '$app/state';
	import { endpoints } from '$lib/api';
	import { session } from '$lib/auth.svelte';
	import type { DiscussionTopic } from '$lib/types';
	import EmptyState from '$lib/components/EmptyState.svelte';
	import { loadCanvasStyles } from '$lib/canvasStyles';

	/* Canvas loads exactly these bundles for this page: discussions_index, announcements_index.
	 *  See scripts/build-canvas-css.sh for where the names come from. */
	$effect(() => loadCanvasStyles(['discussions_index', 'announcements_index']));

	let courseId = $derived(Number(page.params.id));
	let topics = $state<DiscussionTopic[] | null>(null);
	let failure = $state<string | null>(null);
	$effect(() => {
		if (!Number.isFinite(courseId)) return;
		endpoints.discussions
			.list(courseId)
			.then((list) => (topics = list), (error) => (failure = error.publicMessage));
	});
	/* Canvas stores an announcement as a DiscussionTopic with `is_announcement`
	 * true (see open task #25 — the Rust port does not emit that field yet, so
	 * `type === 'AnnouncementDiscussionTopic'` is the signal available today).
	 * Splitting the two lists matches the course nav having separate tabs. */
	let discussions: DiscussionTopic[] = $derived(
		(topics ?? []).filter((t) => t.type !== 'AnnouncementDiscussionTopic')
	);
	let announcements: DiscussionTopic[] = $derived(
		(topics ?? []).filter((t) => t.type === 'AnnouncementDiscussionTopic')
	);
	function when(topic: DiscussionTopic): string {
		const iso = topic.posted_at ?? topic.last_reply_at;
		return iso ? new Date(iso).toLocaleDateString() : '—';
	}

</script>

<div class="page-header">
	<h1>Discussions</h1>
	{#if session.isTeacher}
		<a class="btn btn-primary" href="/courses/{courseId}/discussion_topics/new">+ Discussion</a>
	{/if}
</div>

{#if failure}
	<div class="ic-flash-error" role="alert">{failure}</div>
{:else if topics === null}
	<span class="screenreader-only" role="status">Loading discussions</span>
{:else}
	<h2>Announcements</h2>
	{#if announcements.length === 0}
		<EmptyState title="No announcements" />
	{:else}
		<ul>
			{#each announcements as topic (topic.id)}
				<li>
					<a href="/courses/{courseId}/discussion_topics/{topic.id}">{topic.title}</a>
					<span>{when(topic)}</span>
				</li>
			{/each}
		</ul>
	{/if}

	<h2>Discussions</h2>
	{#if discussions.length === 0}
		<EmptyState title="No discussions" />
	{:else}
		<table class="ic-Table ic-Table--hover-row">
			<thead>
				<tr class="ic-Table--header-row">
					<th>Title</th>
					<th>Unread</th>
					<th>Last activity</th>
				</tr>
			</thead>
			<tbody>
				{#each discussions as topic (topic.id)}
					<tr class:unpublished={topic.published === false}>
						<td><a href="/courses/{courseId}/discussion_topics/{topic.id}">{topic.title}</a></td>
						<td>{topic.unread_count ?? 0}</td>
						<td>{when(topic)}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
{/if}
