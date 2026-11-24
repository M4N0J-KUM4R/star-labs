import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError, api, buildPath } from '$lib/api';

describe('buildPath', () => {
	it('leaves the path untouched without a query', () => {
		expect(buildPath('/api/v1/courses/42')).toBe('/api/v1/courses/42');
	});

	it('drops undefined rather than sending "undefined"', () => {
		expect(buildPath('/p', { a: 1, b: undefined })).toBe('/p?a=1');
	});

	it('emits repeated bracketed params, which is how Canvas reads arrays', () => {
		// routes.rb / the API controllers read `state[]=available&state[]=completed`,
		// not `state=available,completed`. A comma here silently filters nothing.
		expect(buildPath('/p', { state: ['available', 'completed'] })).toBe(
			'/p?state%5B%5D=available&state%5B%5D=completed'
		);
	});

	it('keeps booleans and numbers as their literal strings', () => {
		expect(buildPath('/p', { published: true, per_page: 50 })).toBe(
			'/p?published=true&per_page=50'
		);
	});
});

describe('ApiError', () => {
	it('wraps a 401 so a raw body never reaches a student', () => {
		const error = new ApiError(401, 'unauthorized');
		expect(error.publicMessage).toBe('Your session expired. Log in again.');
	});

	it('keeps the plain-text body for a 4xx the user caused', () => {
		// ApiError renders `(status, self.to_string())`, so bodies arrive as
		// `bad request: invalid namespace` — text, never JSON.
		const error = new ApiError(400, 'bad request: title is required');
		expect(error.publicMessage).toBe('bad request: title is required');
	});

	it('falls back to the status when the body is empty', () => {
		expect(new ApiError(500, '').publicMessage).toBe('Request failed (500)');
	});
});

type Sent = { path: string; init: RequestInit };

/** Record what `api` actually put on the wire, and answer with a JSON 200. */
function stubFetch(status = 200, body = '{}'): Sent[] {
	const sent: Sent[] = [];
	vi.stubGlobal(
		'fetch',
		vi.fn(async (path: string, init: RequestInit) => {
			sent.push({ path, init });
			return {
				status,
				ok: status < 400,
				headers: new Headers({ 'content-type': 'application/json' }),
				text: async () => body
			};
		})
	);
	return sent;
}

function sentHeaders(sent: Sent): Record<string, string> {
	return (sent.init.headers ?? {}) as Record<string, string>;
}

describe('credentials on the wire', () => {
	afterEach(() => vi.unstubAllGlobals());

	/* The session rides on the HttpOnly `star_session` cookie, which
	 * crates/star-ui/src/session_bridge.rs turns into the bearer header AuthUser
	 * verifies. So the client sends credentials, not a token — and `credentials`
	 * is what makes the cookie arrive at all. */
	it('relies on the cookie session rather than an Authorization header', async () => {
		const sent = stubFetch();
		vi.stubGlobal('document', { cookie: '' });
		await api.get('/api/v1/courses');

		expect(sent[0].init.credentials).toBe('same-origin');
		expect(sentHeaders(sent[0]).authorization).toBeUndefined();
	});

	/* Rails' `protect_from_forgery` — and session_bridge::decide — only demand
	 * the double-submit pair on unsafe methods. A GET that carries a CSRF header
	 * would imply the safe ones are exempt from the check the server makes. */
	it('does not ask a safe method for a CSRF token', async () => {
		const sent = stubFetch();
		vi.stubGlobal('document', { cookie: 'star_csrf=deadbeef' });
		await api.get('/api/v1/courses/42');

		expect(sentHeaders(sent[0])['x-csrf-token']).toBeUndefined();
	});

	/* Without this header every write from the bundle answers 403, because a
	 * cookie is replayed cross-site without the page being asked. Header name and
	 * value must match star_ui::session::CSRF_HEADER / the `star_csrf` cookie. */
	it.each(['post', 'put', 'patch', 'del'] as const)(
		'echoes the star_csrf cookie in X-CSRF-Token on %s',
		async (verb) => {
			const sent = stubFetch();
			vi.stubGlobal('document', { cookie: 'other=1; star_csrf=deadbeef' });
			await api[verb]('/api/v1/courses/42/announcements', { title: 'hi' });

			expect(sentHeaders(sent[0])['x-csrf-token']).toBe('deadbeef');
		}
	);

	it('keeps the JSON content type next to the CSRF header', async () => {
		const sent = stubFetch();
		vi.stubGlobal('document', { cookie: 'star_csrf=deadbeef' });
		await api.post('/api/v1/courses/42/announcements', { title: 'hi' });

		const headers = sentHeaders(sent[0]);
		expect(headers['content-type']).toBe('application/json');
		expect(headers['x-csrf-token']).toBe('deadbeef');
		expect(sent[0].init.body).toBe(JSON.stringify({ title: 'hi' }));
	});

	/* A visitor whose shell never issued `star_csrf` gets no header, so the
	 * server's 403 stays the honest answer instead of a blank-token bypass. */
	it('sends no CSRF header when the cookie is absent', async () => {
		const sent = stubFetch();
		vi.stubGlobal('document', { cookie: 'guest=1' });
		await api.del('/api/v1/courses/42/announcements/7');

		expect(sentHeaders(sent[0])['x-csrf-token']).toBeUndefined();
	});
});
