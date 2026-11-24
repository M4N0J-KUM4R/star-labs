<script lang="ts">
	import type { TodoItem } from '$lib/types';

	let { items }: { items: TodoItem[] } = $props();

	/**
	 * Canvas' right-hand To Do list. `app/views/courses/_to_do_list.html.erb:38-190`
	 * is the markup this copies element for element: `h2.todo-list-header`, then
	 * `ul.right-side-list.to-do-list` whose `li.todo` rows hold `a.item` with either
	 * a `.todo-badge-wrapper > .todo-badge` (grading) or an `<i>` icon (submitting),
	 * plus `.todo-details > b.todo-details__title`, `p.todo-details__context` and a
	 * bare `p` of "N points • due date".
	 *
	 * Canvas fills this column by xhr from `GET /dashboard-sidebar`, an HTML
	 * fragment. This port mounts the same DOM from the JSON endpoint Canvas also
	 * serves, `GET /api/v1/users/self/todo` (`config/routes.rb:1789`) — the ERB's own
	 * comment says "if you add a new thing here, it probably needs adding to the
	 * /users/self/todo API", so the two are meant to carry the same rows.
	 */

	/* `ToDoListPresenter::VISIBLE_LIMIT = 5`: rows past the fifth in a group stay in
	 * the DOM hidden, and one "N more..." link reveals them. */
	const VISIBLE_LIMIT = 5;

	let revealed = $state(false);

	const grading = $derived(items.filter((item) => item.type === 'grading'));
	const submitting = $derived(items.filter((item) => item.type !== 'grading'));
	const hiddenCount = $derived(
		Math.max(0, grading.length - VISIBLE_LIMIT) +
			Math.max(0, submitting.length - VISIBLE_LIMIT)
	);

	/** `Assignment#submission_action_string` (abstract_assignment.rb:3677): an
	 *  online quiz is "Taken", a graded assignment that expects a file or text is
	 *  "Turned in", anything else is "Completed". */
	function submissionAction(item: TodoItem): string {
		const types = item.assignment.submission_types ?? [];
		const title = item.assignment.name;
		if (types.includes('online_quiz')) return `Take ${title}`;
		const graded = item.assignment.grading_type && item.assignment.grading_type !== 'none';
		if (graded && types.length > 0 && !types.includes('none')) return `Turn in ${title}`;
		return `Complete ${title}`;
	}

	/* `ToDoListPresenter#formatted_due_date` → `ApplicationHelper#due_at`. The
	 * %b %-d, %l:%M %P shape is the one the Askama Today panel was measured
	 * against, so the two renderings cannot drift apart. */
	function dueLabel(iso: string | null | undefined): string {
		if (!iso) return 'No due date';
		const date = new Date(iso);
		const day = date.toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
		const hours = date.getHours() % 12 || 12;
		const minutes = String(date.getMinutes()).padStart(2, '0');
		return `Due ${day}, ${hours}:${minutes}${date.getHours() < 12 ? 'am' : 'pm'}`;
	}

	/** `t({one: '1 point', other: '%{count} points'}, count: round_if_whole(...))`
	 *  followed by `&bullet;` — nothing at all when there is no point value. */
	function pointsLabel(possible: number | null | undefined): string {
		if (possible === null || possible === undefined) return '';
		const rounded = Math.round(possible * 100) / 100;
		return `${rounded} ${rounded === 1 ? 'point' : 'points'} \u2022 `;
	}

	function titleFor(item: TodoItem): string {
		return item.type === 'grading' ? `Grade ${item.assignment.name}` : submissionAction(item);
	}
</script>

{#if items.length > 0}
	<h2 class="todo-list-header" tabindex="-1">To Do</h2>
	<ul class="right-side-list to-do-list">
		{#each grading as item, index (item.assignment.id)}
			<li class="todo" style:display={index < VISIBLE_LIMIT || revealed ? undefined : 'none'}>
				<a class="item" href={item.html_url}>
					<div class="todo-badge-wrapper">
						<div class="todo-badge">
							<span aria-hidden="true">{item.needs_grading_count ?? 0}</span>
							<span class="screenreader-only">
								{item.needs_grading_count ?? 0} submissions need grading
							</span>
						</div>
					</div>
					<div class="todo-details">
						<b class="todo-details__title">{titleFor(item)}</b>
						<p class="todo-details__context">{item.context_short_name}</p>
						<p>
							{pointsLabel(item.assignment.points_possible)}{dueLabel(item.assignment.due_at)}
						</p>
					</div>
				</a>
			</li>
		{/each}
		{#each submitting as item, index (item.assignment.id)}
			<li class="todo" style:display={index < VISIBLE_LIMIT || revealed ? undefined : 'none'}>
				<a class="item" href={item.html_url}>
					<i class="icon-note-editing" aria-hidden="true"></i>
					<div class="todo-details">
						<b class="todo-details__title">{titleFor(item)}</b>
						<p class="todo-details__context">{item.context_short_name}</p>
						<p>
							{pointsLabel(item.assignment.points_possible)}{dueLabel(item.assignment.due_at)}
						</p>
					</div>
				</a>
			</li>
		{/each}
		{#if hiddenCount > 0 && !revealed}
			<li>
				<button type="button" class="more_link" onclick={() => (revealed = true)}>
					{hiddenCount} more...
				</button>
			</li>
		{/if}
	</ul>
{/if}
