<script lang="ts">
	import type { Course } from '$lib/types';

	let { course }: { course: Course } = $props();

	/* Sub-class names are taken from app/stylesheets/bundles/dashboard_card.scss,
	 * which defines .ic-DashboardCard, __header, __header_hero, __header_content,
	 * __header_button(-bg), __header-title, __header-subtitle, __header-term and
	 * __box. Note `__header_content` with an underscore — that is Canvas' spelling,
	 * and "fixing" it would silently unstyle the card. */
	let actions: { label: string; href: string }[] = $derived(
		[
			{ label: 'Assignments', href: `/courses/${course.id}/assignments` },
			{ label: 'Quizzes', href: `/courses/${course.id}/quizzes` },
			{ label: 'Discussions', href: `/courses/${course.id}/discussions` },
			{ label: 'Grades', href: `/courses/${course.id}/grades` }
		]
	);
</script>

<div class="ic-DashboardCard">
	<div class="ic-DashboardCard__header">
		<div
			class="ic-DashboardCard__header_hero"
			style="background-color: {course.color ?? 'var(--ic-brand-primary)'};"
		></div>
		<div class="ic-DashboardCard__header_content">
			<div class="ic-DashboardCard__header-button">
				<div
					class="ic-DashboardCard__header-button-bg"
					style="background-color: {course.color ?? 'var(--ic-brand-primary)'};"
				></div>
				<h3 class="ic-DashboardCard__header-title">
					<a href="/courses/{course.id}">{course.name}</a>
				</h3>
				{#if course.course_code}
					<p class="ic-DashboardCard__header-subtitle">{course.course_code}</p>
				{/if}
				{#if course.term?.name}
					<p class="ic-DashboardCard__header-term">{course.term.name}</p>
				{/if}
			</div>
		</div>
	</div>
	<!-- __box is Canvas' container for the action links inside one card. -->
	<div class="ic-DashboardCard__box">
		<ul class="ic-DashboardCard__action-container">
			{#each actions as action (action.href)}
				<li class="ic-DashboardCard__action-layout">
					<a class="ic-DashboardCard__action" href={action.href}>{action.label}</a>
				</li>
			{/each}
		</ul>
	</div>
</div>
