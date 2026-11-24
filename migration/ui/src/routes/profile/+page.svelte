<script lang="ts">
	import { endpoints } from '$lib/api';
	import { session } from '$lib/auth.svelte';
	import type { Course } from '$lib/types';
	import DashboardCard from '$lib/components/DashboardCard.svelte';
	import { loadCanvasStyles } from '$lib/canvasStyles';

	/* Canvas loads exactly these bundles for this page: profile_show.
	 *  See scripts/build-canvas-css.sh for where the names come from. */
	$effect(() => loadCanvasStyles(['profile_show']));

	/* Canvas' /profile is the logged-in user's own page; the enrollments API
	 * (GET /api/v1/users/:id/courses) is how it lists their courses. Time zone and
	 * locale are editable there — omitted here because the Rust profile update
	 * route has not been ported, and a form that 404s on submit is worse than no
	 * form. */
	let courses = $state<Course[]>([]);
	$effect(() => {
		const id = session.user?.id;
		if (!id) return;
		endpoints.users.courses(id).then(
			(list) => (courses = list),
			() => (courses = [])
		);
	});

</script>

<div class="page-header">
	<h1>Profile</h1>
</div>

<h2>{session.user?.name ?? 'Signed in'}</h2>
<p>{session.user?.email}</p>
<p class="ic-DashboardCard__header-term">Role: {session.user?.role ?? '—'}</p>

{#if courses.length === 0}
	<span class="screenreader-only">No course enrollments</span>
{:else}
	<h2>Enrollments</h2>
	<div id="DashboardCard_Container">
		<div class="ic-DashboardCard__box">
			{#each courses as course (course.id)}
				<DashboardCard {course} />
			{/each}
		</div>
	</div>
{/if}
