<script lang="ts">
	import { endpoints } from '$lib/api';
	import { session } from '$lib/auth.svelte';
	import type { Course } from '$lib/types';
	import DashboardCard from '$lib/components/DashboardCard.svelte';
	import { cardFromCourse } from '$lib/dashboardCards';
	import EmptyState from '$lib/components/EmptyState.svelte';
	import { loadCanvasStyles } from '$lib/canvasStyles';

	/* Canvas loads exactly these bundles for this page: all_courses, course_list.
	 *  See scripts/build-canvas-css.sh for where the names come from. */
	$effect(() => loadCanvasStyles(['all_courses', 'course_list']));

	/* Canvas' All Courses page (app/views/users/all_courses.html.erb, bundle
	 * all_courses.scss) is the same card grid as the dashboard, but grouped by
	 * enrollment term and including past/unpublished enrollments. */
	let courses = $state<Course[] | null>(null);
	let failure = $state<string | null>(null);
	let filter = $state('');
	let visible: Course[] = $derived(
		(courses ?? []).filter(
			(c) =>
				!filter ||
				c.name.toLowerCase().includes(filter.toLowerCase()) ||
				(c.course_code ?? '').toLowerCase().includes(filter.toLowerCase())
		)
	);
	$effect(() => {
		if (!session.ready) return;
		endpoints.courses
			.list({ include: ['term'], state: ['available', 'completed'] })
			.then((list) => (courses = list), (error) => (failure = error.publicMessage));
	});

</script>

<div class="page-header">
	<h1>All Courses</h1>
</div>

{#if failure}
	<div class="ic-flash-error" role="alert">{failure}</div>
{:else if courses === null}
	<span class="screenreader-only" role="status">Loading courses</span>
{:else}
	<p>
		<label class="screenreader-only" for="course_filter">Filter courses</label>
		<input id="course_filter" placeholder="Filter by course name or code" bind:value={filter} />
	</p>

	{#if visible.length === 0}
		<EmptyState title="No matching courses" body="Nothing in your enrollments matches that." />
	{:else}
	<div class="ic-DashboardCard__box">
		{#each visible as course (course.id)}
			<!-- Canvas' All Courses page renders its own component, not the dashboard
			     card (grep -rn DashboardCard ui/features/allCourses returns nothing);
			     this grid is our approximation and reuses the card through
			     cardFromCourse. Porting Canvas' real All Courses view is its own task. -->
			<DashboardCard card={cardFromCourse(course)} />
		{/each}
	</div>
	{/if}
{/if}
