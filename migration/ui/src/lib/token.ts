/** Where the Bearer token comes from, in priority order.
 *
 *  `/api/v1` accepts `Authorization: Bearer <jwt>` only (star-auth's `AuthUser`
 *  extractor has no cookie fallback), while the Rust web shell authenticates with
 *  the `star_session` cookie. So the SPA has two bootstrap paths and this module
 *  is the seam between them:
 *
 *  1. `window.ENV.api_token` — set by the Rust shell once step 1 of
 *     MIGRATION.md lands. Preferred: no client-side storage of a credential.
 *  2. `sessionStorage` — written by the /login page from the
 *     `POST /api/v1/auth/login` response. Survives SPA navigation but not a tab
 *     close, which matches how long a UI session should live.
 *
 *  Never `localStorage`: a token there outlives the logout that should have
 *  killed it, and `users.last_logged_out` only bounds the cookie path. */

import { env } from '$lib/env';

const STORAGE_KEY = 'star.api_token';

let memory: string | null = null;

export function setToken(token: string): void {
	memory = token;
	if (typeof sessionStorage !== 'undefined') {
		try {
			sessionStorage.setItem(STORAGE_KEY, token);
		} catch {
			// Private-mode browsers throw on write; the in-memory copy still works
			// for this document, which is all the SPA needs until it reloads.
		}
	}
}

export function getToken(): string | null {
	const fromEnv = env().api_token;
	if (fromEnv) return fromEnv;
	if (memory) return memory;
	if (typeof sessionStorage === 'undefined') return null;
	try {
		return sessionStorage.getItem(STORAGE_KEY);
	} catch {
		return null;
	}
}

export function clearToken(): void {
	memory = null;
	if (typeof sessionStorage === 'undefined') return;
	try {
		sessionStorage.removeItem(STORAGE_KEY);
	} catch {
		// Nothing else to do — the in-memory copy is already gone.
	}
}
