/** Card colour + card-shaped data for the course card component.
 *
 *  Canvas keeps both outside the component: the hero colour comes from a store seeded
 *  with the user's own colour preference (`custom_colors`), and the component itself
 *  takes plain props. See the comments on each function for the file and line. */

import { env } from '$lib/env';
import type { Course, DashboardCardJson } from '$lib/types';

/** Canvas `DashboardCardBackgroundStore.colorForCourse`
 *  (`ui/shared/dashboard-card/react/DashboardCardBackgroundStore.ts:63`), whose state is
 *  `ENV.PREFERENCES.custom_colors` (same file, line 41). The hash is keyed by asset
 *  string (`course_42`) and may hold no entry — Canvas' `DashboardCard.tsx:108` then
 *  paints the hero with its own default slate. */
export function colorForCourse(assetString: string): string {
	const colors = (env().PREFERENCES?.custom_colors ?? {}) as Record<string, string>;
	return colors[assetString] ?? '#334451';
}

/** True when the user asked for no colour wash over a course image — the same
 *  `hide_dashcard_color_overlays` preference Canvas reads for the hero overlay
 *  (`DashboardCardHeaderHero`, `DashboardCard.tsx:43-60`). */
export function hideColorOverlays(): boolean {
	return env().PREFERENCES?.hide_dashcard_color_overlays === true;
}

/** A course from an enrollments listing (`GET /api/v1/users/:id/courses`) shaped like a
 *  dashboard card, so the card component can render it.
 *
 *  Canvas does not do this: its All Courses page
 *  (`app/views/users/all_courses.html.erb`, bundle `all_courses.scss`) and its profile
 *  page (`app/views/profile/show.html.erb`, `shared/_profile_main`) use their own
 *  components, and `grep -rn DashboardCard ui/features/allCourses ui/features/profile`
 *  returns nothing. That port is tracked as its own task; meanwhile these two pages list
 *  courses with the card they already have rather than a second card component, and the
 *  fields below are only ever filled from what the enrollments API actually returns —
 *  `links` is empty because the tray comes from the cards API, not this one. */
export function cardFromCourse(course: Course): DashboardCardJson {
	const code = course.course_code ?? course.name;
	return {
		id: course.id,
		longName: `${course.name} - ${code}`,
		shortName: course.name,
		originalName: course.name,
		courseCode: code,
		assetString: `course_${course.id}`,
		href: `/courses/${course.id}`,
		term: course.term?.name ?? null,
		subtitle: '',
		enrollmentState: course.workflow_state ?? 'active',
		enrollmentType: '',
		isFavorited: false,
		published: course.published ?? course.workflow_state !== 'unpublished',
		color: course.color ?? null,
		image: course.image_url ?? null,
		links: []
	};
}
