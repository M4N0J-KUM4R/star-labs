<script lang="ts">
	import { mount, unmount } from 'svelte';
	import { endpoints } from '$lib/api';
	import { session } from '$lib/auth.svelte';
	import type { DashboardCardJson, TodoItem } from '$lib/types';
	import DashboardCard from '$lib/components/DashboardCard.svelte';
	import DashboardTodoPanel from '$lib/components/DashboardTodoPanel.svelte';
	import EmptyState from '$lib/components/EmptyState.svelte';
	import { loadCanvasStyles } from '$lib/canvasStyles';
	import { env } from '$lib/env';

	/* Canvas loads exactly these bundles for this page: dashboard, dashboard_card.
	 *  See scripts/build-canvas-css.sh for where the names come from. On a first
	 *  paint the shell already emitted both (`embed/shell-map.txt`), so this only
	 *  matters when the page is reached by client-side navigation. */
	$effect(() => loadCanvasStyles(['dashboard', 'dashboard_card']));

	/* `_dashboard_card.html.erb:22` → `User#dashboard_view` (app/models/user.rb:2032):
	 * `preferences[:dashboard_view] || account.default_dashboard_view || "cards"`. The
	 * shell already puts that preference in ENV, so the page reads it rather than
	 * asking for it again. */
	const dashboardView = $derived(String(env().PREFERENCES?.dashboard_view ?? 'cards'));

	let cards = $state<DashboardCardJson[] | null>(null);
	let todos = $state<TodoItem[] | null>(null);
	let failure = $state<string | null>(null);

	/* Two Canvas prefetches, copied from the ERB that still does them:
	 * `shared/_dashboard_card.html.erb:36` fires `/api/v1/dashboard/dashboard_cards`
	 * and `users#dashboard_sidebar` renders the To Do rows. The card list is NOT
	 * `/users/:id/courses` — Canvas' cards carry `assetString`, `enrollmentType` and
	 * the tab links, which the course list does not. */
	$effect(() => {
		if (!session.ready || !session.user) return;
		endpoints.dashboard
			.cards()
			.then((list) => (cards = list))
			.catch((error) => {
				failure = error.publicMessage;
				cards = [];
			});
		endpoints.dashboard
			.todo()
			.then((list) => (todos = list))
			.catch(() => {
				todos = [];
			});
	});

	/* Canvas puts the right column in `<aside id="right-side">` (the layout renders it
	 * at `application.html.erb:273-274`) and fills it by xhr. `+layout.svelte` owns the
	 * empty aside, so this mounts into it and unmounts on teardown — the one part of
	 * this page that is not inside `#content`. */
	$effect(() => {
		if (!todos) return;
		const target = document.getElementById('right-side');
		if (!target) return;
		const app = mount(DashboardTodoPanel, { target, props: { items: todos } });
		return () => unmount(app);
	});
</script>

<!-- user_dashboard.html.erb:71 — #dashboard.ic-dashboard-app wraps everything,
     and the visible title is a screenreader-only h1 (the page heading lives in
     the React header container below). -->
<div id="dashboard" class="ic-dashboard-app">
	<h1 class="screenreader-only">Dashboard</h1>

	{#if failure}
		<div class="ic-flash-error" role="alert">{failure}</div>
	{/if}

	<!-- `dashboard_header_container` is Canvas' header (title, course search, the
	     Cards/Stream switch, Start a New Course). Only the title is ported; see
	     MIGRATION.md "Known gap". -->
	<div id="dashboard_header_container" class="ic-Dashboard-header">
		<div class="ic-Dashboard-header__layout">
			<div class="ic-Dashboard-header__title">
				<h2>Dashboard</h2>
			</div>
		</div>
	</div>

	{#if cards === null}
		<span class="screenreader-only" role="status">Loading courses</span>
	{:else if cards.length === 0}
		<EmptyState title="No courses yet" body="Enrollments you are added to will show up here." />
	{:else}
		<!-- _dashboard_card.html.erb:34,51: `render_on_pageload = user_dashboard_view ==
		     'cards'` and the container carries that as an inline display, so the stream
		     view can hide the grid without a class. `user_dashboard_view` is
		     `@user_preferences[:dashboard_layout] || :cards` (application_helper), which is
		     the same value the shell puts in ENV. -->
		<div id="DashboardCard_Container" style:display={dashboardView === 'cards' ? 'block' : 'none'}>
			<div class="ic-DashboardCard__box">
				{#each cards as card (card.id)}
					<DashboardCard {card} />
				{/each}
			</div>
		</div>
	{/if}
</div>
