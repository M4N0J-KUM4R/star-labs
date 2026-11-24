<script lang="ts">
	import { page } from '$app/state';
	import { endpoints } from '$lib/api';
	import { session } from '$lib/auth.svelte';
	import type { Assignment, Enrollment } from '$lib/types';
	import EmptyState from '$lib/components/EmptyState.svelte';

	/* Canvas' /courses/:id/grades is gradebooks#grade_summary (routes.rb:262). It
	 * renders one of two shapes depending on who asks: a teacher gets the whole
	 * roster from the enrollments API, a student gets only their own scores. The
	 * roster comes from GET /courses/:id/students (routes.rb:204) whose enrollments
	 * each carry `grades`; the student view comes from the assignment list with
	 * include[]=submission. Both are routes Canvas actually serves. */
	let courseId = $derived(Number(page.params.id));
	let roster = $state<Enrollment[] | null>(null);
	let mine = $state<Assignment[] | null>(null);
	let failure = $state<string | null>(null);

	$effect(() => {
		if (!Number.isFinite(courseId) || !session.ready) return;
		if (session.isTeacher) {
			endpoints.courses
				.students(courseId)
				.then((list) => (roster = list), (error) => (failure = error.publicMessage));
		} else {
			endpoints.assignments
				.list(courseId)
				.then((list) => (mine = list), (error) => (failure = error.publicMessage));
		}
	});
</script>

<div class="page-header">
	<h1>Grades</h1>
</div>

{#if failure}
	<div class="ic-flash-error" role="alert">{failure}</div>
{:else if session.isTeacher}
	{#if roster === null}
		<span class="screenreader-only" role="status">Loading grades</span>
	{:else if roster.length === 0}
		<EmptyState title="No students" body="Nobody is enrolled in this course." />
	{:else}
		<table class="ic-Table ic-Table--hover-row">
			<thead>
				<tr class="ic-Table--header-row">
					<th>Student</th>
					<th>Current</th>
					<th>Final</th>
				</tr>
			</thead>
			<tbody>
				{#each roster as enrollment (enrollment.id ?? enrollment.user?.id)}
					<tr>
						<td>
							<a href="/courses/{courseId}/grades/{enrollment.user?.id}">
								{enrollment.user?.name ?? 'Unknown'}
							</a>
						</td>
						<td>{enrollment.grades?.current_grade ?? '—'}</td>
						<td>{enrollment.grades?.final_grade ?? '—'}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
{:else}
	{#if mine === null}
		<span class="screenreader-only" role="status">Loading grades</span>
	{:else if mine.length === 0}
		<EmptyState title="Nothing graded yet" />
	{:else}
		<table class="ic-Table ic-Table--condensed">
			<thead>
				<tr class="ic-Table--header-row">
					<th>Assignment</th>
					<th>Score</th>
					<th>Grade</th>
				</tr>
			</thead>
			<tbody>
				{#each mine as assignment (assignment.id)}
					<tr>
						<td>{assignment.name}</td>
						<td>
							{assignment.submission?.score ?? '—'} / {assignment.points_possible ?? '—'}
						</td>
						<td>{assignment.submission?.grade ?? '—'}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
{/if}
