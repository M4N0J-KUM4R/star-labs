<script lang="ts">
	import { page } from '$app/state';
	import { endpoints } from '$lib/api';
	import { session } from '$lib/auth.svelte';
	import type { Quiz } from '$lib/types';
	import EmptyState from '$lib/components/EmptyState.svelte';
	import { loadCanvasStyles } from '$lib/canvasStyles';

	/* Canvas loads exactly these bundles for this page: quizzes, canvas_quizzes.
	 *  See scripts/build-canvas-css.sh for where the names come from. */
	$effect(() => loadCanvasStyles(['quizzes', 'canvas_quizzes']));

	let courseId = $derived(Number(page.params.id));
	let quizzes = $state<Quiz[] | null>(null);
	let failure = $state<string | null>(null);
	$effect(() => {
		if (!Number.isFinite(courseId)) return;
		endpoints.quizzes
			.list(courseId)
			.then((list) => (quizzes = list), (error) => (failure = error.publicMessage));
	});

</script>

<div class="page-header">
	<h1>Quizzes</h1>
	{#if session.isTeacher}
		<a class="btn btn-primary" href="/courses/{courseId}/quizzes/new">+ Quiz</a>
	{/if}
</div>

{#if failure}
	<div class="ic-flash-error" role="alert">{failure}</div>
{:else if quizzes === null}
	<span class="screenreader-only" role="status">Loading quizzes</span>
{:else if quizzes.length === 0}
	<EmptyState title="No quizzes" body="This course has no quizzes yet." />
{:else}
	<table class="ic-Table ic-Table--hover-row">
		<thead>
			<tr class="ic-Table--header-row">
				<th>Title</th>
				<th>Type</th>
				<th>Questions</th>
				<th>Points</th>
				<th>Due</th>
			</tr>
		</thead>
		<tbody>
			{#each quizzes as quiz (quiz.id)}
				<tr class:unpublished={quiz.published === false}>
					<td>
						<!-- Canvas links a quiz to its edit page for a teacher and to
						     the take URL for a student; quiz_url is what the serializer
						     gives us for that. -->
						<a
							href={session.isTeacher
								? `/courses/${courseId}/quizzes/${quiz.id}/edit`
								: (quiz.quiz_url ?? `/courses/${courseId}/quizzes/${quiz.id}`)}
						>{quiz.title}</a>
					</td>
					<td>{quiz.quiz_type ?? 'assignment'}</td>
					<td>{quiz.question_count ?? '—'}</td>
					<td>{quiz.points_possible ?? '—'}</td>
					<td>{quiz.lock_at ? new Date(quiz.lock_at).toLocaleString() : '—'}</td>
				</tr>
			{/each}
		</tbody>
	</table>
{/if}
