/** Same-origin `/api/v1` client.
 *
 *  The bundle is served by axum from the same origin as the API, so no CORS and
 *  no base URL: `fetch('/api/v1/...')` just works. The session rides on the
 *  `star_session` cookie (`credentials: 'same-origin'`), which
 *  `crates/star-ui/src/session_bridge.rs` turns into the `Authorization: Bearer`
 *  header `AuthUser` verifies — so there is no token to store here unless the
 *  deployment hands one to the shell through `window.ENV.api_token`. What an
 *  unsafe method does need is `X-CSRF-Token`, echoing the non-HttpOnly
 *  `star_csrf` cookie, because a cookie is sent cross-site without being asked
 *  and a header is not. `hasApiToken()` lets a page say "log in" instead of
 *  rendering a wall of 401s when there is neither cookie nor ENV token.
 *
 *  Error bodies are plain text, not JSON: `ApiError` renders `(status, to_string())`
 *  with a variant prefix, e.g. `bad request: invalid namespace`. Reading
 *  `.json()` on a 4xx panics the caller, so this client always reads text. */

import { csrfToken, getToken } from '$lib/token';
import type {
	Assignment,
	CalendarEvent,
	Course,
	CourseTab,
	DashboardCardJson,
	DiscussionTopic,
	Enrollment,
	LoginResponse,
	Quiz,
	TodoItem,
	User,
	WikiPage
} from '$lib/types';

export class ApiError extends Error {
	status: number;
	body: string;

	constructor(status: number, body: string) {
		super(body || `request failed (${status})`);
		this.name = 'ApiError';
		this.status = status;
		this.body = body;
	}

	/** Canvas answers 401 with a body we should not show a student. */
	get publicMessage(): string {
		if (this.status === 401) return 'Your session expired. Log in again.';
		if (this.status === 403) return 'You do not have permission to do that.';
		if (this.status === 404) return 'That item does not exist or is not visible to you.';
		if (this.status === 502) return 'The server could not be reached.';
		return this.body || `Request failed (${this.status})`;
	}
}

export type Query = Record<string, string | number | boolean | undefined | (string | number)[]>;

/** Exported for tests: the repeated-`[]` behaviour is Canvas-specific and easy
 *  to break with a `URLSearchParams(record)` shortcut. */
export function buildPath(path: string, query?: Query): string {
	if (!query) return path;
	const usp = new URLSearchParams();
	for (const [key, value] of Object.entries(query)) {
		if (value === undefined) continue;
		if (Array.isArray(value)) {
			// Canvas' API reads repeated `[]` params (`state[]=available`).
			for (const item of value) usp.append(`${key}[]`, String(item));
		} else {
			usp.append(key, String(value));
		}
	}
	const qs = usp.toString();
	return qs ? `${path}?${qs}` : path;
}

/** Methods Rails' `protect_from_forgery` (and `session_bridge::decide`) treat as
 *  safe, i.e. never CSRF-checked. Keep in sync with `star_ui::session_bridge`. */
const SAFE_METHODS = new Set(['GET', 'HEAD', 'OPTIONS']);

/** Same string as `star_ui::session::CSRF_HEADER`. */
const CSRF_HEADER = 'x-csrf-token';

function headers(method: string, extra: Record<string, string> = {}): Record<string, string> {
	const out: Record<string, string> = { accept: 'application/json', ...extra };
	const token = getToken();
	if (token) out.authorization = `Bearer ${token}`;
	if (!SAFE_METHODS.has(method.toUpperCase())) {
		const csrf = csrfToken();
		if (csrf) out[CSRF_HEADER] = csrf;
	}
	return out;
}

export function hasApiToken(): boolean {
	return Boolean(getToken());
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
	const init: RequestInit = { method, headers: headers(method), credentials: 'same-origin' };
	if (body !== undefined) {
		init.headers = headers(method, { 'content-type': 'application/json' });
		init.body = JSON.stringify(body);
	}

	let response: Response;
	try {
		response = await fetch(path, init);
	} catch (error) {
		throw new ApiError(502, error instanceof Error ? error.message : 'network error');
	}

	if (response.status === 204) return undefined as T;

	const text = await response.text();
	if (!response.ok) throw new ApiError(response.status, text);
	if (!text) return undefined as T;
	const type = response.headers.get('content-type') ?? '';
	if (!type.includes('json')) return text as T;
	return JSON.parse(text) as T;
}

export const api = {
	get: <T>(path: string, query?: Query) => request<T>('GET', buildPath(path, query)),
	post: <T>(path: string, body?: unknown) => request<T>('POST', path, body),
	put: <T>(path: string, body?: unknown) => request<T>('PUT', path, body),
	patch: <T>(path: string, body?: unknown) => request<T>('PATCH', path, body),
	del: <T>(path: string, body?: unknown) => request<T>('DELETE', path, body)
};

/** ---- endpoint map ------------------------------------------------------
 *  One function per Canvas API path, named after the controller action it
 *  calls. Paths are written exactly as Canvas' `config/routes.rb` serves them,
 *  because these strings are what the parity scoreboard counts. Add a route
 *  here before adding a page that needs it; do not inline a path in a .svelte
 *  file — that is how invented verbs get shipped. */

export const endpoints = {
	session: {
		/** POST /api/v1/auth/login — `{ token, user }` (star-auth/src/lib.rs:302). */
		login: (email: string, password: string) =>
			request<LoginResponse>('POST', '/api/v1/auth/login', { email, password }),
		/** POST /api/v1/auth/register — Canvas' self-service signup. */
		register: (body: { name: string; email: string; password: string }) =>
			request<LoginResponse>('POST', '/api/v1/auth/register', body),
		/** GET /api/v1/auth/me — `{ id, name, sortable_name, email, role, created_at }`. */
		me: () => api.get<User>('/api/v1/auth/me'),
		/** POST /api/v1/auth/logout → 204. The JWT is stateless, so this only
		 *  bounds the cookie session; the SPA drops its own token in `signOut()`. */
		logout: () => api.post<void>('/api/v1/auth/logout')
	},

	courses: {
		// index (courses_controller#index, account-scoped list for the dashboard)
		list: (query?: Query) =>
			api.get<Course[]>(
				'/api/v1/courses',
				{ state: ['available'], include: ['term', 'total_scores'], per_page: 50, ...query }
			),
		show: (courseId: number) => api.get<Course>(`/api/v1/courses/${courseId}`),
		/** GET /api/v1/courses/:course_id/tabs (routes.rb:1363). */
		tabs: (courseId: number) => api.get<CourseTab[]>(`/api/v1/courses/${courseId}/tabs`),
		update: (courseId: number, body: Record<string, unknown>) =>
			api.put<Course>(`/api/v1/courses/${courseId}`, { course: body }),
		/** GET /api/v1/courses/:course_id/students (routes.rb:204). Each enrollment
		 *  carries `grades`, which is how Canvas' gradebook gets its totals — there is
		 *  no per-student grades endpoint on a course (routes.rb has `grades/:id` only
		 *  as the HTML grade_summary page, :264), so do not invent one. */
		students: (courseId: number, query?: Query) =>
			api.get<Enrollment[]>(`/api/v1/courses/${courseId}/students`, { per_page: 50, ...query })
	},

	assignments: {
		list: (courseId: number, query?: Query) =>
			api.get<Assignment[]>(`/api/v1/courses/${courseId}/assignments`, {
				include: ['submission'],
				per_page: 100,
				...query
			}),
		show: (courseId: number, assignmentId: number) =>
			api.get<Assignment>(`/api/v1/courses/${courseId}/assignments/${assignmentId}`),
		create: (courseId: number, body: Record<string, unknown>) =>
			api.post<Assignment>(`/api/v1/courses/${courseId}/assignments`, { assignment: body }),
		update: (courseId: number, assignmentId: number, body: Record<string, unknown>) =>
			api.put<Assignment>(`/api/v1/courses/${courseId}/assignments/${assignmentId}`, {
				assignment: body
			}),
		submissions: (courseId: number, assignmentId: number) =>
			api.get<import('$lib/types').Submission[]>(
				`/api/v1/courses/${courseId}/assignments/${assignmentId}/submissions`
			)
	},

	quizzes: {
		list: (courseId: number) => api.get<Quiz[]>(`/api/v1/courses/${courseId}/quizzes`),
		show: (courseId: number, quizId: number) =>
			api.get<Quiz>(`/api/v1/courses/${courseId}/quizzes/${quizId}`),
		update: (courseId: number, quizId: number, body: Record<string, unknown>) =>
			api.put<Quiz>(`/api/v1/courses/${courseId}/quizzes/${quizId}`, { quiz: body })
	},

	discussions: {
		list: (courseId: number) =>
			api.get<DiscussionTopic[]>(`/api/v1/courses/${courseId}/discussion_topics`, {
				per_page: 50
			}),
		show: (courseId: number, topicId: number) =>
			api.get<DiscussionTopic>(`/api/v1/courses/${courseId}/discussion_topics/${topicId}`)
	},

	pages: {
		list: (courseId: number) => api.get<WikiPage[]>(`/api/v1/courses/${courseId}/pages`),
		show: (courseId: number, url: string) =>
			api.get<WikiPage>(`/api/v1/courses/${courseId}/pages/${encodeURIComponent(url)}`)
	},

	calendar: {
		/** GET /api/v1/calendar_events — Canvas' own event feed. */
		events: (query?: Query) =>
			api.get<CalendarEvent[]>('/api/v1/calendar_events', {
				type: 'all',
				per_page: 50,
				...query
			})
	},

	dashboard: {
		/** GET /api/v1/dashboard/dashboard_cards (routes.rb:1839,
		 *  `users#dashboard_cards`). Canvas prefetches this exact path from
		 *  `app/views/shared/_dashboard_card.html.erb:36`, so the dashboard's
		 *  cards come from here and not from `/users/:id/courses`. */
		cards: () => api.get<DashboardCardJson[]>('/api/v1/dashboard/dashboard_cards'),
		/** GET /api/v1/users/self/todo (routes.rb:1789). The rows of the
		 *  right-hand To Do list Canvas renders in
		 *  `app/views/courses/_to_do_list.html.erb`. */
		todo: () => api.get<TodoItem[]>('/api/v1/users/self/todo')
	},

	users: {
		show: (userId: number) => api.get<User>(`/api/v1/users/${userId}`),
		courses: (userId: number) =>
			api.get<Course[]>(`/api/v1/users/${userId}/courses`, { per_page: 50 })
	}
};
