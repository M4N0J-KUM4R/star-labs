/** The credential the SPA sends, which is normally none.
 *
 *  Canvas' browser code holds no API token. Its API controllers authenticate the web
 *  session — `class AnnouncementsApiController < ApplicationController`
 *  (app/controllers/announcements_api_controller.rb:26) — and `window.ENV`
 *  (app/controllers/application_controller.rb:264-309) has no access-token key. This
 *  port reaches the same shape from the other direction: instead of teaching
 *  star-auth a second transport, `crates/star-ui/src/session_bridge.rs` rewrites the
 *  HttpOnly `star_session` cookie into the `Authorization: Bearer` header that
 *  `AuthUser` already verifies. So a page served by axum needs no client-side
 *  credential at all, and the cookie stays unreadable to script.
 *
 *  `getToken()` therefore returns a token only when a deployment hands one to the
 *  shell through `window.ENV.api_token`. `setToken()` keeps one in memory for the
 *  `npm run dev` flow, where Vite serves the bundle without the Rust session.
 *
 *  Not `localStorage`, and not sessionStorage either: a credential in web storage
 *  outlives the logout that should have killed it, and `users.last_logged_out` only
 *  bounds the cookie path. */

import { env } from '$lib/env';

let memory: string | null = null;

/** Hold a token for the lifetime of this document only. */
export function setToken(token: string): void {
	memory = token;
}

export function getToken(): string | null {
	return env().api_token ?? memory;
}

export function clearToken(): void {
	memory = null;
}

/** The `star_csrf` cookie value, for `X-CSRF-Token`.
 *
 *  That cookie is deliberately not HttpOnly — `session::ensure_csrf_cookie` says so in
 *  its own comment, "so a future JS layer can echo it in headers (Canvas parity)", and
 *  `packages/canvas-rce/src/rce/plugins/shared/do-fetch-api-effect/defaultFetchOptions.ts:35`
 *  is Canvas' own JS doing exactly that. It is a CSRF token, not a session credential:
 *  reading it grants nothing on its own, which is why exposing it to script is safe
 *  while exposing `star_session` would not be. */
export function csrfToken(): string | null {
	if (typeof document === 'undefined') return null;
	for (const pair of document.cookie.split(';')) {
		const [name, value] = pair.split('=');
		if (name?.trim() === 'star_csrf' && value) return decodeURIComponent(value.trim());
	}
	return null;
}
