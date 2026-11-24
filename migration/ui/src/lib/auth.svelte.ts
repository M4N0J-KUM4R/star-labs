/** The one place that knows who is signed in.
 *
 *  Svelte 5 module state (`$state` in a `.svelte.ts` file) rather than a
 *  `writable` store: components read `session.user` reactively without a
 *  `$`-prefix subscription, and the state is typed without a store generic.
 *
 *  Identity comes from `GET /api/v1/auth/me` when there is a token — the same
 *  request the server makes per-request, so the UI cannot show a button the API
 *  will reject — and falls back to `window.ENV` for the cookie-rendered shell. */

import { endpoints } from '$lib/api';
import { currentUserFromEnv, env } from '$lib/env';
import type { User } from '$lib/types';
import { clearToken, getToken, setToken } from '$lib/token';

class Session {
	user: User | null = $state(null);
	/** True once bootstrap() has resolved, so pages don't flash a login
	 *  redirect at a session that is about to be found. */
	ready: boolean = $state(false);

	signedIn: boolean = $derived(this.user !== null);

	/** Roles as the server sees them. `current_user_roles` is the ENV mirror of
	 *  the same list the handlers gate on, so it is correct for the shell even
	 *  before /auth/me returns. */
	roles: string[] = $derived(this.user?.role ? [this.user.role] : (env().current_user_roles ?? []));

	is(...roles: string[]): boolean {
		if (!this.user) return (env().current_user_roles ?? []).some((r) => roles.includes(r));
		return roles.includes(this.user.role);
	}

	get isTeacher(): boolean {
		return this.is('admin', 'teacher', 'designer');
	}

	get isStudent(): boolean {
		return env().current_user_is_student ?? this.is('student', 'learner');
	}

	/** Adopt whatever the server-rendered shell already knows, then verify it. */
	async bootstrap(): Promise<void> {
		if (this.ready) return;
		const fromEnv = currentUserFromEnv();
		if (fromEnv) this.user = { ...fromEnv, name: '', email: '' };

		if (getToken()) {
			try {
				this.user = await endpoints.session.me();
			} catch {
				// A stale token must not lock the shell: keep the ENV identity if
				// there was one, otherwise the visitor is anonymous.
				if (!fromEnv) this.user = null;
				clearToken();
			}
		}
		this.ready = true;
	}

	async signIn(email: string, password: string): Promise<User> {
		const result = await endpoints.session.login(email, password);
		setToken(result.token);
		this.user = result.user;
		return result.user;
	}

	/** Canvas' logout clears the cookie server-side; the SPA additionally drops
	 *  its copy of the token so a back-button cannot act as a session. */
	async signOut(): Promise<void> {
		try {
			if (getToken()) await endpoints.session.logout();
		} finally {
			clearToken();
			this.user = null;
		}
	}
}

export const session = new Session();
