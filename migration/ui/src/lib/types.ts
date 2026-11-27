/** Shapes returned by the Rust API. Every field name is the key Canvas' own
 *  serializer emits (`canvas-src/app/serializers/*`), because the Rust port is
 *  graded against those names — a rename here is a parity break, not a styling
 *  choice. Optional members are the ones Canvas emits only for an `include[]`
 *  the caller asked for. */

export interface User {
	id: number;
	name: string;
	sortable_name?: string | null;
	short_name?: string | null;
	email: string;
	role: string;
	created_at?: string;
	avatar_url?: string | null;
	time_zone?: string | null;
	locale?: string | null;
}

export interface Grades {
	current_grade?: string | null;
	current_score?: number | null;
	final_grade?: string | null;
	final_score?: number | null;
	unscored_grade?: string | null;
	grade_matches_current_selection?: boolean;
}

export interface Enrollment {
	id?: number;
	type?: string;
	role?: string;
	state?: string;
	workflow_state?: string;
	created_at?: string;
	total_active_time?: number | null;
	user?: User;
	grades?: Grades;
}

export interface Course {
	id: number;
	name: string;
	course_code?: string;
	uuid?: string;
	sis_course_id?: string | null;
	integration_id?: string | null;
	account_id?: number;
	enrollment_term_id?: number;
	term?: { id: number; name: string; start_at?: string | null; end_at?: string | null };
	start_at?: string | null;
	end_at?: string | null;
	created_at?: string;
	updated_at?: string;
	workflow_state?: string;
	public_syllabus?: boolean;
	access_restrictions?: Record<string, unknown>;
	published?: boolean;
	unpublishable?: boolean;
	grading_standard_id?: number | null;
	blueprint?: boolean;
	enrollments?: Enrollment[];
	calendar?: { icl_event_url?: string; course_id?: number };
	image_url?: string | null;
	muted_colors?: boolean;
	color?: string | null;
	needs_grading_count?: number;
	total_scores?: {
		current_grade?: string | null;
		current_score?: number | null;
		final_grade?: string | null;
		final_score?: number | null;
	};
}

export interface CourseTab {
	id: string;
	label: string;
	html_url?: string;
	full_url?: string;
	position?: number;
	visible?: boolean;
	unread_count?: number | null;
	css_class?: string;
}

export interface Submission {
	id?: number | null;
	body?: string | null;
	url?: string | null;
	grade?: string | null;
	score?: number | null;
	submitted_at?: string | null;
	workflow_state?: string;
	graded_anonymously?: boolean;
	needs_grading?: boolean;
}

export interface Assignment {
	id: number;
	name: string;
	description?: string | null;
	due_at?: string | null;
	updated_at?: string;
	points_possible?: number | null;
	assignment_group_id?: number;
	grading_type?: string;
	position?: number | null;
	workflow_state?: string;
	published?: boolean;
	unlock_at?: string | null;
	lock_at?: string | null;
	needs_grading_count?: number;
	html_url?: string;
	submission_types?: string[];
	peer_reviews?: boolean;
	auto_submit?: boolean;
	muted?: boolean;
	submission?: Submission | null;
	overdue?: boolean;
	grade_group_students_individually?: boolean;
	allowed_extensions?: string[];
}

export interface Quiz {
	id: number;
	title: string;
	html_url?: string;
	description?: string | null;
	quiz_url?: string | null;
	code?: string | null;
	quiz_type?: string;
	time_limit?: number | null;
	comments_enabled?: boolean;
	assignment_group_id?: number;
	shuffle_answers?: boolean;
	require_ssl?: boolean;
	hide_results?: string | null;
	show_correct_answers?: boolean;
	published?: boolean;
	unpublished_last?: boolean;
	locked_for_user?: boolean;
	lock_at?: string | null;
	unlock_at?: string | null;
	preview_url?: string | null;
	secure_preview_url?: string | null;
	question_count?: number;
	points_possible?: number | null;
	assignment_id?: number | null;
	one_time_per_student?: boolean;
	description_plain?: string;
	permissions?: Record<string, boolean>;
}

export interface DiscussionTopic {
	id: number;
	title: string;
	last_reply_at?: string;
	posted_at?: string;
	type?: string | null;
	message?: string;
	subscribed?: boolean;
	topic_children?: unknown[];
	discussion_type?: string | null;
	deleted?: boolean;
	is_announcement?: boolean;
	require_initial_post?: boolean | null;
	user?: { id: number; name?: string; display_name?: string; avatar_image_url?: string };
	read_state?: string;
	unread_count?: number;
	html_url?: string;
	published?: boolean;
	locked_for_user?: boolean;
}

export interface WikiPage {
	page_id?: number;
	url: string;
	title: string;
	slug?: string;
	body?: string;
	published?: boolean;
	editing_date?: string;
	updated_at?: string;
	wiki_page_id?: number;
	hide_from_final_grade_review?: boolean;
}

export interface CalendarEvent {
	id: number;
	title: string;
	description?: string | null;
	start_at?: string;
	end_at?: string;
	all_day?: boolean;
	type?: string;
	context_type?: string;
	context_code?: string;
	html_url?: string;
	location_name?: string;
}

/** POST /api/v1/auth/login — `{ token, user }`. */
export interface LoginResponse {
	token: string;
	user: User;
}

/**
 * One link inside a dashboard card — Canvas `CourseForMenuPresenter#to_h`
 * (`app/presenters/course_for_menu_presenter.rb`), rendered by
 * `ui/shared/dashboard-card/react/DashboardCardAction.jsx`. The keys are
 * snake_case because that is what Canvas serialises; `css_class` is the class
 * the action anchor gets, and `icon` is an `icon-*` sprite name.
 */
export interface DashboardCardLink {
	css_class: string;
	icon: string | null;
	hidden: boolean;
	path: string;
	label: string;
}

/**
 * `GET /api/v1/dashboard/dashboard_cards` (`config/routes.rb:1839`) — one
 * course card. camelCase because Canvas' presenter emits camelCase here
 * (`shortName`, `originalName`, `assetString`, `isFavorited`).
 */
export interface DashboardCardJson {
	id: number;
	longName: string;
	shortName: string;
	originalName: string;
	courseCode: string;
	assetString: string;
	href: string;
	term?: string | null;
	subtitle: string;
	enrollmentState: string;
	enrollmentType: string;
	observee?: string | null;
	isFavorited: boolean;
	isK5Subject?: boolean;
	isHomeroom?: boolean;
	published: boolean;
	color?: string | null;
	image?: string | null;
	position?: number | null;
	canManage?: boolean;
	canReadAnnouncements?: boolean;
	canChangeCoursePublishState?: boolean;
	defaultView?: string;
	pagesUrl?: string;
	frontPageTitle?: string | null;
	links: DashboardCardLink[];
}

/**
 * `GET /api/v1/users/self/todo` (`config/routes.rb:1789`) — one row of the
 * right-hand To Do list, as `app/views/courses/_to_do_list.html.erb` renders
 * it. `type` is `grading` or `submitting`; `assignment` carries the due date
 * and the URL the row links to.
 */
export interface TodoItem {
	context_type: string;
	course_id: number;
	context_name: string;
	context_short_name: string;
	type: string;
	ignore: string;
	ignore_permanently: string;
	html_url: string;
	assignment: Assignment;
	needs_grading_count?: number;
}
