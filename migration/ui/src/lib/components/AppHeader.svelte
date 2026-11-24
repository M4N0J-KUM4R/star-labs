<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { session } from '$lib/auth.svelte';
	import { env } from '$lib/env';

	/* Mirrors Canvas' global nav markup (app/stylesheets/base/_ic_app_header.scss:
	 * .ic-app-header > .ic-app-header__logomark-container + .ic-app-header__menu-list
	 * > .ic-app-header__menu-list-link > .menu-item-icon-container + .menu-item__text),
	 * so the vendored header sheet styles it without us writing a rule. */
	let items: { label: string; href: string }[] = $derived(
		session.signedIn
			? [
					{ label: 'Dashboard', href: '/dashboard' },
					{ label: 'Courses', href: '/courses' },
					{ label: 'Calendar', href: '/calendar' }
				]
			: []
	);

	async function signOut() {
		await session.signOut();
		await goto('/login');
	}
</script>

<div id="header" class="ic-app-header">
	<div class="ic-app-header__logomark-container">
		<a class="ic-app-header__logomark" href="/dashboard">
			<span class="screenreader-only">{env().help_link_name ?? 'Canvas'}</span>
		</a>
	</div>
	<nav aria-label="Main Navigation">
		<ul class="ic-app-header__menu-list">
			{#each items as item (item.href)}
				<li class="ic-app-header__menu-list-item">
					<a
						class="ic-app-header__menu-list-link"
						href={item.href}
						aria-current={page.url.pathname === item.href ? 'page' : undefined}
					>
						<span class="menu-item__text">{item.label}</span>
					</a>
				</li>
			{/each}
		</ul>
	</nav>
	<nav class="ic-app-header__secondary-navigation" aria-label="User">
		<ul class="ic-app-header__menu-list">
			{#if session.signedIn}
				<li class="ic-app-header__menu-list-item">
					<a class="ic-app-header__menu-list-link" href="/profile">
						<span class="menu-item-icon-container">
							<span class="ic-avatar" aria-hidden="true">
								{(session.user?.name ?? '?').slice(0, 1)}
							</span>
						</span>
						<span class="menu-item__text">{session.user?.name ?? 'Profile'}</span>
					</a>
				</li>
				<li class="ic-app-header__menu-list-item">
					<button class="ic-app-header__menu-list-link" type="button" onclick={signOut}>
						<span class="menu-item__text">Log Out</span>
					</button>
				</li>
			{:else}
				<li class="ic-app-header__menu-list-item">
					<a class="ic-app-header__menu-list-link" href="/login">
						<span class="menu-item__text">Log In</span>
					</a>
				</li>
			{/if}
		</ul>
	</nav>
</div>
