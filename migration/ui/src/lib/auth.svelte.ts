/** The one place that knows who is signed in.
 *
 *  Svelte 5 module state (`$state` in a `.svelte.ts` file) rather than a
 *  `writable` store: components read `session.user` reactively without a
 *  `$`-prefix subscription, and the state is typed without a store generic.
 *
 *  Identity has two sources, and both are the server's. When axum renders the shell it
 *  puts `current_user_id`/`current_user_roles` in `window.ENV` — Canvas' own mechanism
 *  (app/controllers/application_controller.rb:264-309) — and that is enough to paint the
 *  page without a request. Then `GET /api/v1/auth/me` runs anyway, over the
 *  `star_session` cookie that `crates/star-ui/src/session_bridge.rs` turns into the
 *  header `AuthUser` verifies: the same handler the server uses per request, so the UI
 *  cannot show a button the API will reject, and it returns the name and email ENV omits.
 *  In `npm run dev` there is no cookie, so the token `/login` returned stands in. */

import { endpoints } from '$lib/api';
import { currentUserFromEnv, env } from '$lib/env';
import type { User } from '$lib/types';
import { clearToken, csrfToken, getToken, setToken } from '$lib/token';

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

		// Either credential is enough to ask: a token in `npm run dev`, a cookie
		// under axum. `getToken()` cannot see the cookie — it is HttpOnly, which is
		// the point — so the shell-hosted case is detected from the ENV the shell
		// only writes when it has a session.
		if (getToken() || fromEnv) {
			try {
				this.user = await endpoints.session.me();
			} catch {
				// A stale token must not lock the shell: keep the ENV identity if
				// there was one, otherwise the visitor is anonymous. A failed /me
				// over a session the shell just validated is a transport blip, and
				// logging the user out of the UI for that would be worse than a
				// header without a display name.
				if (!fromEnv) this.user = null;
				clearToken();
			}
		}
		this.ready = true;
	}

	/** Sign in.
	 *
	 *  Canvas' own login is a form POST to `/login` (config/routes.rb `post "login"`
	 *  → SessionsController#create) that sets the session cookie and redirects; the
	 *  API token this returns is the OAuth-shaped credential the mobile/LTI clients
	 *  use. Under the shell, the form endpoint is the one that establishes the
	 *  `star_session` cookie every later request needs — `POST /api/v1/auth/login`
	 *  does not set a cookie (star-auth never names `star_session`), so the token
	 *  kept here only lives as long as this document. `migration/` tracks moving
	 *  `/login` itself onto the shell (and therefore onto the form POST) as the next
	 *  step in MIGRATION.md rather than inventing a cookie-granting API route, which
	 *  is the one thing parity cannot accept: Canvas has no such route. */
	async signIn(email: string, password: string): Promise<User> {
		const result = await endpoints.session.login(email, password);
		setToken(result.token);
		this.user = result.user;
		return result.user;
	}

	/** Sign out of the session the server holds.
	 *
	 *  Under the shell the credential is the `star_session` cookie, and a cookie is not
	 *  something this document can delete — only `/logout` can, because that handler
	 *  both expires the cookie (`clear_session_cookie`) and stamps
	 *  `users.last_logged_out`, which is what invalidates the JWT for `AuthUser` too.
	 *  Canvas' verb there is `delete "logout" => "login#destroy"` (config/routes.rb:930),
	 *  and Rails reaches it with `form_tag logout_path, method: :delete`
	 *  (app/views/login/logout_confirm.html.erb:39) — a POST carrying a hidden
	 *  `_method`, which the shell's `method_override` rewrites before routing. So the
	 *  fields below are exactly what Canvas' own form emits, not an invention here.
	 *  Calling the API's logout instead would leave a visitor who presses Back, or who
	 *  reloads, still authenticated. So the shell-hosted path submits Rails' own form and
	 *  never resolves: the browser is navigating away, and a caller that resumed to
	 *  `goto('/login')` would race — and cancel — that POST. */
	async signOut(): Promise<void> {
		clearToken();
		this.user = null;

		if (shellHosted()) {
			submitShellForm('/logout', { _method: 'delete', _csrf: csrfToken() ?? '' });
			return new Promise<void>(() => {});
		}

		try {
			await endpoints.session.logout();
		} finally {
			// Dev-only: no cookie to clear, and the API's logout is a 204 no-op.
			clearToken();
		}
	}
}

/** True when axum rendered this document, which is the only way `window.ENV` exists —
 *  `shell.rs` writes it inline and Vite's `app.html` has no such tag. */
function shellHosted(): boolean {
	return typeof window !== 'undefined' && window.ENV !== undefined;
}

/** POST a form the way a browser would, for a shell endpoint that answers with a
 *  redirect and `Set-Cookie`. `application/x-www-form-urlencoded` is what
 *  `axum_extra::extract::Form` decodes, and the `_csrf` field is `session::CSRF_FIELD`. */
function submitShellForm(action: string, fields: Record<string, string>): void {
	const form = document.createElement('form');
	form.method = 'post';
	form.action = action;
	form.acceptCharset = 'UTF-8';
	for (const [name, value] of Object.entries(fields)) {
		const input = document.createElement('input');
		input.type = 'hidden';
		input.name = name;
		input.value = value;
		form.appendChild(input);
	}
	document.body.appendChild(form);
	form.submit();
}

export const session = new Session();
