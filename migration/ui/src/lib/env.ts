/** The `window.ENV` reader.
 *
 *  This is the contract Canvas' own front end runs on: `layouts/_head.html.erb`
 *  injects `window.ENV`, and 124 js_env files / 490 call sites read it. The Rust
 *  port already models it as a typed, governance-tested struct
 *  (`crates/star-ui/src/env.rs`, `ENV_KEYS`) — so the Svelte app reads the same
 *  keys rather than inventing a bootstrap endpoint. Every key here must exist in
 *  that list; a new key is a deliberate change to both files. */

import type { User } from '$lib/types';

export interface JsEnv {
	// --- phase 1: shell + journey data (env.rs ENV_KEYS, first block) ---
	current_user_id?: number;
	current_user_roles?: string[];
	current_user_is_student?: boolean;
	DOMAIN_ROOT_ACCOUNT_ID?: number;
	RAILS_ENVIRONMENT?: string;
	ASSET_HOST?: string;
	DEEP_LINKING_POST_MESSAGE_ORIGIN?: string;
	active_context_tab?: string;
	context_asset_string?: string;
	PREFERENCES?: Record<string, unknown>;
	SETTINGS?: Record<string, unknown>;
	use_high_contrast?: boolean;
	help_link_name?: string;
	k12?: boolean;
	files_domain?: string;
	notices?: string[];
	// --- phase 2: frozen shape ---
	current_user_global_id?: string;
	current_user_uuid?: string;
	current_user_types?: string[];
	FEATURES?: Record<string, boolean>;
	/** NOT YET EMITTED. `JsEnv` has no token field today; until step 1 of
	 *  MIGRATION.md lands it, `api.ts` falls back to the cookie session. */
	api_token?: string;
}

declare global {
	interface Window {
		ENV?: JsEnv;
	}
}

/** SSR-safe: with adapter-static there is no server render, but a test run
 *  importing this module in Node must not touch `window`. */
export function env(): JsEnv {
	if (typeof window === 'undefined') return {};
	return window.ENV ?? {};
}

export function isSignedIn(): boolean {
	return typeof env().current_user_id === 'number';
}

export function currentUserId(): number | null {
	return env().current_user_id ?? null;
}

/** True only when the session's role list actually carries the role — mirrors
 *  how the handlers gate on `ctx.is_teacher` rather than trusting a token. */
export function hasRole(role: string): boolean {
	return (env().current_user_roles ?? []).includes(role);
}

export function isTeacher(): boolean {
	return hasRole('teacher') || hasRole('designer') || hasRole('admin');
}

/** The course the current page is scoped to, from `context_asset_string`
 *  (`"Course_42"` in Canvas). */
export function contextCourseId(): number | null {
	const match = /^Course_(\d+)$/.exec(env().context_asset_string ?? '');
	return match ? Number(match[1]) : null;
}

export function notices(): string[] {
	return env().notices ?? [];
}

export function feature(name: string): boolean {
	return env().FEATURES?.[name] === true;
}

/** A minimal user view for the header, built from ENV only — the app never
 *  needs `/auth/me` to paint the shell. */
export function currentUserFromEnv(): Pick<User, 'id' | 'role'> | null {
	const id = env().current_user_id;
	if (typeof id !== 'number') return null;
	return { id, role: (env().current_user_roles ?? [])[0] ?? 'student' };
}
