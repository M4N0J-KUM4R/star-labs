<script lang="ts">
	import type { DashboardCardJson } from '$lib/types';
	import { colorForCourse, hideColorOverlays } from '$lib/dashboardCards';

	let { card }: { card: DashboardCardJson } = $props();

	/* Canvas' container hands the component the colour, not the payload:
	 * DashboardCardBox.tsx:189 `backgroundColor={this.colorForCard(card.assetString)}`. */
	const backgroundColor = $derived(colorForCourse(card.assetString));

	/* The tray behind the kebab (DashboardCard.tsx:264-372, called at :369) and, for an
	 * unpublished course a manager can publish, the PublishButton at
	 * DashboardCard.tsx:353-368 both need writes this port has not made yet —
	 * `PUT /users/:id/colors`, `PUT /users/:id/course_nickname`, `PUT /users/self/favorites`
	 * and the course publish PUT. So they are absent rather than inert.
	 * See MIGRATION.md "Known gap". */
</script>

<!-- Nesting copied from ui/shared/dashboard-card/react/DashboardCard.tsx:310-377:
     .ic-DashboardCard > .ic-DashboardCard__header > (sr span, hero,
     a.ic-DashboardCard__link > __header_content) then a SIBLING
     nav.ic-DashboardCard__action-container. The actions are direct children of
     the nav — DashboardCardAction.jsx:70 renders no <li>, so wrapping them in a
     <ul> would fight `.ic-DashboardCard__action { flex: 0 0 25% }`. -->
<div class="ic-DashboardCard" aria-label={card.originalName}>
	<div class="ic-DashboardCard__header">
		<span class="screenreader-only">
			{card.image
				? `Course image for ${card.shortName}`
				: `Course card color region for ${card.shortName}`}
		</span>
		{#if card.image}
			<!-- DashboardCardHeaderHero, DashboardCard.tsx:49-61: the image is the
			     wrapper's background and the hero sits over it at 0.6 opacity (0 when
			     the user hides colour overlays). -->
			<div class="ic-DashboardCard__header_image" style:background-image={`url(${card.image})`}>
				<div
					class="ic-DashboardCard__header_hero"
					style:background-color={backgroundColor}
					style:opacity={hideColorOverlays() ? 0 : 0.6}
					aria-hidden="true"
				></div>
			</div>
		{:else}
			<div
				class="ic-DashboardCard__header_hero"
				style:background-color={backgroundColor}
				aria-hidden="true"
			></div>
		{/if}
		<!-- data-sveltekit-reload: Canvas' card links to a document, and so must ours.
		     The destination is `GET /courses/:id`, served by axum as server-rendered
		     HTML (crates/star-ui/src/lib.rs:119), which the bundle has no route for that
		     answers it — leaving the interceptor on would render our /courses/[id] page
		     instead and `#section-tabs` (browser-smoke/tests/dashboard-course-home.spec.ts)
		     would never appear. -->
		<a href={card.href} data-sveltekit-reload class="ic-DashboardCard__link">
			<div class="ic-DashboardCard__header_content">
				<h3 class="ic-DashboardCard__header-title ellipsis" title={card.originalName}>
					<span style:color={backgroundColor}>{card.shortName}</span>
				</h3>
				<div class="ic-DashboardCard__header-subtitle ellipsis" title={card.courseCode}>
					{card.courseCode}
				</div>
				<!-- DashboardCard.tsx:342-344 emits the term row unconditionally and puts
				     `{term || null}` inside it, so an unterm'd course still reserves the
				     element. Skipping it here would change the card's height. -->
				<div class="ic-DashboardCard__header-term ellipsis" title={card.term ?? undefined}>
					{card.term ?? ''}
				</div>
				{#if card.enrollmentType === 'ObserverEnrollment' && card.observee}
					<div class="ic-DashboardCard__header-term ellipsis" title={card.observee}>
						Observing: {card.observee}
					</div>
				{/if}
			</div>
		</a>
	</div>
	<nav
		class="ic-DashboardCard__action-container"
		aria-label={`Actions for ${card.shortName}`}
	>
		{#each card.links.filter((link) => !link.hidden) as link (link.path)}
			<a
				class={`ic-DashboardCard__action ${link.css_class}`}
				href={link.path}
				data-sveltekit-reload
				title={`${link.label} - ${card.shortName}`}
			>
				<span class="screenreader-only">{link.label} - {card.shortName}</span>
				<div class="ic-DashboardCard__action-layout">
					<i class={link.icon ?? 'icon-user'} aria-hidden="true"></i>
				</div>
			</a>
		{/each}
	</nav>
</div>
