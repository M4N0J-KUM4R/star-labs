<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { session } from '$lib/auth.svelte';
	import { env } from '$lib/env';
	import coursesIcon from '$lib/icons/_svg_icon_courses_new_styles.svg?raw';
	import calendarIcon from '$lib/icons/_svg_icon_calendar_new_styles.svg?raw';
	import dashboardIcon from '$lib/icons/_svg_icon_dashboard.svg?raw';
	import loginIcon from '$lib/icons/_svg_login_new_styles.svg?raw';
	import navToggleIcon from '$lib/icons/_svg_icon_navtoggle.svg?raw';

	/* Structure, ids and classes are copied from
	 * app/views/shared/_new_nav_header.html.erb:106-279 — `header#header.ic-app-header.no-print`,
	 * `a#skip_navigation_link`, `div[role=region].ic-app-header__main-navigation`,
	 * `ul#menu`, `li.menu-item.ic-app-header__menu-list-item[…--active]`,
	 * `a.ic-app-header__menu-list-link` > `.menu-item-icon-container` +
	 * `.menu-item__text`, then `div.ic-app-header__secondary-navigation`.
	 * base/_ic_app_header.scss keys off that nesting, so a flattened version loses
	 * the 84px rail, the hover tray and the active-item marker at once.
	 *
	 * Icons are Canvas' own inline SVGs, vendored under src/lib/icons/. They are
	 * not decorative: `.ic-icon-svg` fills from
	 * `var(--ic-brand-global-nav-ic-icon-svg-fill--active)`, which is the only
	 * thing that makes the active item brand blue.
	 *
	 * Not ported: the k5 branch of `_new_nav_header.html.erb:29-38`, which swaps
	 * the dashboard icon for svg_icon_home and labels Courses "Subjects"; and the
	 * Groups / Conversations / Files / Help items, since this app has no route for
	 * them yet. Those are `<li>` additions to this list, not a change to its structure. */
	let active = (prefix: string): string =>
		page.url.pathname === prefix || page.url.pathname.startsWith(`${prefix}/`)
			? ' ic-app-header__menu-list-item--active'
			: '';

	async function signOut() {
		await session.signOut();
		await goto('/login');
	}

	/* #primaryNavToggle is what Canvas binds to flip body.primary-nav-expanded,
	 * which sizes the rail and shifts .ic-Layout-wrapper by 84px. */
	function toggleNav(): void {
		document.body.classList.toggle('primary-nav-expanded');
	}
</script>

<header id="header" class="ic-app-header no-print" aria-label="Global Header">
	<a href="#content" id="skip_navigation_link">Skip To Content</a>
	<div role="region" class="ic-app-header__main-navigation" aria-label="Global Navigation">
		<!-- erb:110 hides the logomark in high contrast, because the sheet there
		     has no brand image and the mark would sit on the wrong background. -->
		{#if !env().use_high_contrast}
			<div class="ic-app-header__logomark-container">
				<a href="/dashboard" class="ic-app-header__logomark">
					<span class="screenreader-only">Dashboard</span>
				</a>
			</div>
		{/if}
		<ul id="menu" class="ic-app-header__menu-list">
			{#if session.signedIn}
				<li class="menu-item ic-app-header__menu-list-item{active('/profile')}">
					<a id="global_nav_profile_link" role="button" href="/profile" class="ic-app-header__menu-list-link">
						<div class="menu-item-icon-container">
							<!-- Canvas puts an <img> here from avatar_image_attrs
							     (erb:122-123). star-auth's UserResponse has no avatar field
							     yet — grep "avatar" crates/star-auth/src/lib.rs is empty — so
							     the styled initial is what we can render honestly today. -->
							<div aria-hidden="true" class="fs-exclude ic-avatar">
								{(session.user?.name ?? '?').slice(0, 1)}
							</div>
							<span class="menu-item__badge"></span>
						</div>
						<div class="menu-item__text">Account</div>
					</a>
				</li>
			{:else}
				<li class="menu-item ic-app-header__menu-list-item">
					<a id="global_nav_login_link" href="/login" class="ic-app-header__menu-list-link">
						<div class="menu-item-icon-container" aria-hidden="true">{@html loginIcon}</div>
						<div class="menu-item__text">Log In</div>
					</a>
				</li>
			{/if}
			<li class="ic-app-header__menu-list-item{active('/dashboard')}">
				<a id="global_nav_dashboard_link" href="/dashboard" class="ic-app-header__menu-list-link">
					<div class="menu-item-icon-container" aria-hidden="true">{@html dashboardIcon}</div>
					<div class="menu-item__text">Dashboard</div>
				</a>
			</li>
			{#if session.signedIn}
				<li class="menu-item ic-app-header__menu-list-item{active('/courses')}">
					<a id="global_nav_courses_link" role="button" href="/courses" class="ic-app-header__menu-list-link">
						<div class="menu-item-icon-container" aria-hidden="true">{@html coursesIcon}</div>
						<div class="menu-item__text">Courses</div>
					</a>
				</li>
			{/if}
			<li class="menu-item ic-app-header__menu-list-item{active('/calendar')}">
				<a id="global_nav_calendar_link" href="/calendar" class="ic-app-header__menu-list-link">
					<div class="menu-item-icon-container" aria-hidden="true">{@html calendarIcon}</div>
					<div class="menu-item__text">Calendar</div>
				</a>
			</li>
		</ul>
	</div>
	<div class="ic-app-header__secondary-navigation">
		<ul class="ic-app-header__menu-list">
			<li class="menu-item ic-app-header__menu-list-item">
				<!-- svelte-ignore a11y_invalid_attribute --><!-- Canvas' own markup is href="#" here (erb:265); the --nav-toggle class styles the <a>, an <button> would need a rule of ours -->
				<a
					id="primaryNavToggle"
					role="button"
					href="#"
					class="ic-app-header__menu-list-link ic-app-header__menu-list-link--nav-toggle"
					aria-label="Minimize global navigation"
					title="Minimize global navigation"
					onclick={(e) => {
						e.preventDefault();
						toggleNav();
					}}
				>
					<div class="menu-item-icon-container" aria-hidden="true">{@html navToggleIcon}</div>
				</a>
			</li>
			{#if session.signedIn}
				<li class="menu-item ic-app-header__menu-list-item">
					<button class="ic-app-header__menu-list-link" type="button" onclick={signOut}>
						<div class="menu-item__text">Log Out</div>
					</button>
				</li>
			{/if}
		</ul>
	</div>
	<div id="global_nav_tray_container"></div>
	<div id="global_nav_tour"></div>
</header>
