<script lang="ts">
	import { page } from '$app/state';
	import { endpoints } from '$lib/api';
	import { session } from '$lib/auth.svelte';
	import type { Assignment } from '$lib/types';
	import EmptyState from '$lib/components/EmptyState.svelte';

	let courseId = $derived(Number(page.params.id));
	let assignments = $state<Assignment[] | null>(null);
	let failure = $state<string | null>(null);

	$effect(() => {
		if (!Number.isFinite(courseId)) return;
		endpoints.assignments
			.list(courseId)
			.then((list) => (assignments = list), (error) => (failure = error.publicMessage));
	});

	/* Canvas shows the student their own submission (include[]=submission puts it
	 * on the assignment) and the teacher the needs-grading count. Same endpoint,
	 * different column — driven by the role the server reported, never by a claim
	 * parsed out of the token. */
	let studentView: boolean = $derived(!session.isTeacher);

	function due(iso?: string | null): string {
		return iso ? new Date(iso).toLocaleString() : '—';
	}
</script>

<div class="page-header">
	<h1>Assignments</h1>
	{#if session.isTeacher}
		<a class="btn btn-primary" href="/courses/{courseId}/assignments/new">+ Assignment</a>
	{/if}
</div>

{#if failure}
	<div class="ic-flash-error" role="alert">{failure}</div>
{:else if assignments === null}
	<span class="screenreader-only" role="status">Loading assignments</span>
{:else if assignments.length === 0}
	<EmptyState title="No assignments" body="This course has no assignments yet." />
{:else}
	<table class="ic-Table ic-Table--hover-row">
		<thead>
			<tr class="ic-Table--header-row">
				<th>Name</th>
				<th>Due</th>
				<th>Points</th>
				{#if studentView}<th>Submission</th>{/if}
			</tr>
		</thead>
		<tbody>
			{#each assignments as assignment (assignment.id)}
				<tr class:unpublished={assignment.published === false} class:overdue={assignment.overdue}>
					<td>
						<a href={assignment.html_url ?? `/courses/${courseId}/assignments/${assignment.id}`}>
							{assignment.name}
						</a>
					</td>
					<td>{due(assignment.due_at)}</td>
					<td>{assignment.points_possible ?? '—'}</td>
					{#if studentView}
						<td>{assignment.submission?.workflow_state ?? 'not_submitted'}</td>
					{/if}
				</tr>
			{/each}
		</tbody>
	</table>
{/if}
