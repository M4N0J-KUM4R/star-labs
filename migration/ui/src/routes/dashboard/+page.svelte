<script lang="ts">
	import { endpoints } from '$lib/api';
	import { session } from '$lib/auth.svelte';
	import type { Course } from '$lib/types';
	import DashboardCard from '$lib/components/DashboardCard.svelte';
	import EmptyState from '$lib/components/EmptyState.svelte';

	let courses = $state<Course[] | null>(null);
	let failure = $state<string | null>(null);

	/* Two sources for the same list because there are two sessions: the Bearer
	 * token lists the signed-in user's courses, while a cookie-only shell has no
	 * token yet and falls back to the courses the Rust page handler embedded. */
	$effect(() => {
		if (!session.ready) return;
		const id = session.user?.id;
		if (!id) return;
		endpoints.users
			.courses(id)
			.then((list) => (courses = list), (error) => (failure = error.publicMessage));
	});
</script>

<div class="ic-Dashboard-header">
	<div class="ic-Dashboard-header__title">
		<h1>Dashboard</h1>
	</div>
</div>

{#if failure}
	<div class="ic-flash-error" role="alert">{failure}</div>
{:else if courses === null}
	<span class="screenreader-only" role="status">Loading courses</span>
{:else if courses.length === 0}
	<EmptyState title="No courses yet" body="Enrollments you are added to will show up here." />
{:else}
	<!-- Nesting copied from app/views/shared/_dashboard_card.html.erb:50-53:
	     #DashboardCard_Container > .ic-DashboardCard__box > .ic-DashboardCard. -->
	<div id="DashboardCard_Container">
		<div class="ic-DashboardCard__box">
			{#each courses as course (course.id)}
				<DashboardCard {course} />
			{/each}
		</div>
	</div>
{/if}
