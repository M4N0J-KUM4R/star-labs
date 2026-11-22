//! star-conferences — web conferences and recordings (Canvas
//! ConferencesController) over course/group contexts, plus the
//! current-user listing and the recording_ready provider webhook
//! (issue #91, spec `specs/p5-rest-conferences.md`).
//!
//! Endpoints (spec §2, 7 routes):
//! - GET  /courses/{course_id}/conferences   — index (recordings preloaded)
//! - GET  /groups/{group_id}/conferences     — index (group twin)
//! - POST /courses/{course_id}/conferences   — create (invitees + calendar_event)
//! - POST /groups/{group_id}/conferences     — create (group twin)
//! - GET  /conferences                       — for_user (state=live filter)
//! - POST /courses/{course_id}/conferences/{conference_id}/recording_ready
//! - POST /groups/{group_id}/conferences/{conference_id}/recording_ready
//!
//! Behavior contract (spec §6 + canvas-lms ConferencesController):
//! - index: users with granular manage-course-content rights see all
//!   conferences of the context; everyone else sees only the ones they
//!   participate in. 401 anonymous → 404 unknown context → 403 non-member.
//!   Every item carries `recordings` (empty until the webhook fires) and
//!   the model's as_json field set (`users`/`invitees`/`attendees`
//!   arrays, `has_advanced_settings`, `long_running`, `user_settings`).
//! - create: nested `web_conference` params (Canvas
//!   `params.require(:web_conference).permit(:start_at, :end_at,
//!   :sync_attendees, :title, :duration, :description, :conference_type,
//!   user_settings: ..., lti_settings: ...)`); invitees via the `user`
//!   id map / `user[all]` / default all context members;
//!   `observers[remove]` invites context users minus observers; a truthy
//!   `web_conference[calendar_event]` creates a linked calendar_events
//!   row (Course contexts only — the Canvas helper returns nil for
//!   groups; and only with a start_at, CalendarEvent validates its
//!   presence). `conference_type` must be a configured provider type
//!   ("BigBlueButton" or "LtiConference"); unknown → 400 Rails error
//!   body. 200 with the conference JSON (Canvas renders with the
//!   default status).
//! - for_user: the caller's conferences across all Course/Group contexts
//!   where they are a participant, newest first; `state=live` keeps only
//!   conferences started within the last day and not yet ended
//!   (WebConference scope :live).
//! - recording_ready: NO auth user (Canvas skips load_user/require_user).
//!   Body carries `signed_parameters`, a JWT signed with the provider
//!   secret (star-lms: `AppState.jwt_secret`, the platform secret
//!   star-lti also verifies provider JWTs against — verified with the
//!   workspace `jsonwebtoken` crate, same pattern as
//!   star-lti::decode_message_hint). Valid JWT whose `meeting_id` equals
//!   the conference's `conference_key` → recording_ready=true + a
//!   recordings entry → 202 with `[]`; valid JWT, wrong meeting → 422
//!   with Canvas' `signed_id_invalid_json`; invalid JWT → 401 with
//!   Canvas' `invalid_jwt_token_json`.

mod conferences_api;

use axum::Router;
use star_common::AppState;

/// The Canvas-parity conference router (issue #91, spec
/// `specs/p5-rest-conferences.md`).
///
/// The implementation lives in [`conferences_api`]; the crate root only
/// re-exports its router for `star-http`.
pub fn router() -> Router<AppState> {
    conferences_api::router()
}
