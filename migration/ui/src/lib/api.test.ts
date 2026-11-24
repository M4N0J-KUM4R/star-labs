import { describe, expect, it } from 'vitest';
import { ApiError, buildPath } from '$lib/api';

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
