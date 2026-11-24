import { afterEach, describe, expect, it } from 'vitest';
import { cardFromCourse, colorForCourse } from '$lib/dashboardCards';

/** `env.ts` reads `window.ENV`, and with adapter-static there is no server render, so a
 *  Node run has to stand in for the document the shell just wrote. */
function withEnv(ENV: Record<string, unknown>): void {
	(globalThis as { window?: unknown }).window = { ENV };
}

afterEach(() => {
	delete (globalThis as { window?: unknown }).window;
});

describe('colorForCourse', () => {
	it("uses the colour from the user's own preference, keyed by asset string", () => {
		// DashboardCardBackgroundStore.ts:41,63 — ENV.PREFERENCES.custom_colors[assetString].
		withEnv({ PREFERENCES: { custom_colors: { course_42: '#0076B8' } } });
		expect(colorForCourse('course_42')).toBe('#0076B8');
	});

	it('falls back to the slate Canvas defaults to', () => {
		// DashboardCard.tsx:108 `backgroundColor = '#334451'`.
		withEnv({ PREFERENCES: { custom_colors: {} } });
		expect(colorForCourse('course_7')).toBe('#334451');
		withEnv({});
		expect(colorForCourse('course_7')).toBe('#334451');
	});
});

describe('cardFromCourse', () => {
	it('fills the fields the card reads from what the enrollments API returns', () => {
		const card = cardFromCourse({
			id: 42,
			name: 'Smoke Course Alpha',
			course_code: 'SMOKE-42',
			published: true,
			term: { id: 3, name: 'Fall 2026' }
		});
		expect(card.assetString).toBe('course_42');
		expect(card.href).toBe('/courses/42');
		expect(card.originalName).toBe('Smoke Course Alpha');
		expect(card.courseCode).toBe('SMOKE-42');
		expect(card.term).toBe('Fall 2026');
		// The action tray comes from the cards API, so a derived card has none rather
		// than links invented here.
		expect(card.links).toEqual([]);
	});

	it('derives published from workflow_state when the API sends no flag', () => {
		expect(cardFromCourse({ id: 1, name: 'Draft', workflow_state: 'unpublished' }).published).toBe(
			false
		);
		expect(cardFromCourse({ id: 1, name: 'Live', workflow_state: 'available' }).published).toBe(
			true
		);
	});
});
