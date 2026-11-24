<script lang="ts">
	import { page } from '$app/state';
	import { endpoints } from '$lib/api';
	import type { Course, CourseTab } from '$lib/types';
	import CourseTabs from '$lib/components/CourseTabs.svelte';

	let { children } = $props();

	/* The tabs come from GET /api/v1/courses/:course_id/tabs, not a constant:
	 * Canvas hides and renames them per course (navigation defaults, workflow
	 * state, feature flags), and a hardcoded list is how a port ends up linking to
	 * pages the course does not have. */
	let course = $state<Course | null>(null);
	let tabs = $state<CourseTab[]>([]);
	let failure = $state<string | null>(null);

	$effect(() => {
		const id = Number(page.params.id);
		if (!Number.isFinite(id)) return;
		course = null;
		failure = null;
		endpoints.courses.show(id).then(
			(loaded) => {
				course = loaded;
				// A tab fetch that fails should not blank the page; the shell still
				// renders with no nav, which is what Canvas does for a hidden nav.
				endpoints.courses.tabs(id).then(
					(list) => (tabs = list),
					() => (tabs = [])
				);
			},
			(error) => (failure = error.publicMessage)
		);
	});

</script>

{#if failure}
	<div class="ic-flash-error" role="alert">{failure}</div>
{:else if course}
	<div class="ic-Layout-wrapper">
		<div class="ic-Layout-contentWrapper">
			<div class="page-header">
				<h1>{course.name}</h1>
			</div>
			<CourseTabs courseId={course.id} {tabs} />
			{@render children()}
		</div>
	</div>
{:else}
	<span class="screenreader-only" role="status">Loading course</span>
{/if}
