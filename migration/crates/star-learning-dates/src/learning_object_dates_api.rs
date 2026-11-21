//! Canvas `LearningObjectDatesController` — the seven `learning_object_dates`
//! routes that the crate's first pass did not register.
//!
//! Canvas routes (`config/routes.rb`, the
//! `scope(controller: :learning_object_dates)` block of the API v1 course
//! scope):
//!
//! | Method | Path | Canvas action |
//! |--------|------|---------------|
//! | GET | `/courses/{courseId}/modules/{contextModuleId}/date_details` | `show` |
//! | GET | `/courses/{courseId}/files/{attachmentId}/date_details` | `show` |
//! | PUT | `/courses/{courseId}/files/{attachmentId}/date_details` | `update` |
//! | PUT | `/courses/{courseId}/assignments/{assignmentId}/date_details/convert_tag_overrides` | `convert_tag_overrides_to_adhoc_overrides` |
//! | PUT | `/courses/{courseId}/quizzes/{quizId}/date_details/convert_tag_overrides` | `convert_tag_overrides_to_adhoc_overrides` |
//! | PUT | `/courses/{courseId}/discussion_topics/{discussionTopicId}/date_details/convert_tag_overrides` | `convert_tag_overrides_to_adhoc_overrides` |
//! | PUT | `/courses/{courseId}/pages/{urlOrId}/date_details/convert_tag_overrides` | `convert_tag_overrides_to_adhoc_overrides` |
//!
//! Controller: `canvas-src/app/controllers/learning_object_dates_controller.rb`.
//! JSON: `lib/api/v1/learning_object_dates.rb` (`learning_object_dates_json`)
//! and `lib/api/v1/assignment_override.rb` (`assignment_override_json`).
//! Conversion: `app/services/differentiation_tag/override_converter_service.rb`
//! and `app/services/differentiation_tag/converters/*`.
//!
//! ## The `before_action` chain (the order matters)
//!
//! 1. `require_context` — the course must exist (404).
//! 2. `check_authorized_action`:
//!    a. the asset is looked up first, so a missing asset is a **404**;
//!    b. for an `Attachment` the site-admin `differentiated_files` feature
//!       must be on, otherwise **400** `{"error": "This API does not
//!       support files."}`;
//!    c. `asset.grants_right?(@current_user, :manage_assign_to)` — otherwise
//!       **403**.
//!
//! An anonymous caller is rejected with **401** by the `AuthUser` extractor
//! before any of that runs.
//!
//! ## Permissions
//!
//! `manage_assign_to` is a teacher-level course permission in every case
//! (`manage_assignments_edit` for assignments and quizzes, `moderate_forum`
//! for discussions, `manage_wiki_update` for pages, `manage_course_content_edit`
//! for modules, `manage_files_edit` for files). The workspace has no
//! role-override table, so — like `star-modules/src/course_pacing.rs` — the
//! granular set collapses into one gate: a teacher/ta/designer enrollment in
//! the course, global admins pass everywhere.
//!
//! ## Deviations from Canvas (all forced by the port schema)
//!
//! * The port keeps the learning-object dates in `learning_object_dates` and
//!   the overrides in `learning_object_date_overrides` (the crate's own
//!   model). A context module and a file are stored there with the
//!   `learning_object_type` values `ContextModule` and `Attachment`; the
//!   migration comment of `0090_learning_object_dates.sql` only lists the
//!   four original values.
//! * Canvas stores a module's unlock date in `context_modules.unlock_at` and
//!   a file's dates in `attachments.unlock_at` / `attachments.lock_at`; the
//!   port keeps them there too, so the two `show` routes read the dates from
//!   the asset itself and the overrides from `learning_object_date_overrides`.
//! * `groups.non_collaborative` does not exist in the port. Canvas keeps the
//!   column and mirrors it from the group category
//!   (`Group#non_collaborative = group_category.non_collaborative`), so a
//!   differentiation tag is a group whose `group_categories.non_collaborative`
//!   is true — that is the test used here.
//! * `discussion_topics` has no `assignment_id` in the port, so a graded
//!   discussion cannot be told apart from an ungraded one: the discussion is
//!   always its own overridable (`DiscussionTopic`).
//! * Canvas has no sub-assignments in the port, so the checkpointed-discussion
//!   converter (`CheckpointedDiscussionOverrideConverter`) never applies and
//!   the general converter is always used.
//! * The port has no `all_day` / `all_day_date` columns, so those two keys of
//!   an assignment override are not rendered.
//!
//! ## Tables used
//!
//! No new table and no new column is needed: `context_modules`,
//! `attachments`, `learning_object_dates`, `learning_object_date_overrides`,
//! `groups`, `group_categories`, `group_memberships`, `course_sections`,
//! `feature_flags` and `accounts` all exist in `star-db`.
//!
//! The only schema note for the coordinator: the
//! `learning_object_dates.learning_object_type` discriminator now also takes
//! the values `ContextModule` and `Attachment`. The column is a plain `TEXT`
//! with `UNIQUE (learning_object_type, learning_object_id)` and no `CHECK`
//! constraint, so no migration is required — only the comment of
//! `0090_learning_object_dates.sql` is out of date.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use chrono::{DateTime, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};
use star_auth::AuthUser;
use star_common::{ApiError, AppState};
use std::collections::HashSet;

use crate::UpdateOverride;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// The learning object a `date_details` route addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LearningObject {
    Assignment,
    Quiz,
    DiscussionTopic,
    WikiPage,
}

impl LearningObject {
    /// The `learning_object_dates.learning_object_type` discriminator.
    fn as_str(self) -> &'static str {
        match self {
            LearningObject::Assignment => "Assignment",
            LearningObject::Quiz => "Quiz",
            LearningObject::DiscussionTopic => "DiscussionTopic",
            LearningObject::WikiPage => "WikiPage",
        }
    }

    /// The table the learning object lives in.
    fn table(self) -> &'static str {
        match self {
            LearningObject::Assignment => "assignments",
            LearningObject::Quiz => "quizzes",
            LearningObject::DiscussionTopic => "discussion_topics",
            LearningObject::WikiPage => "pages",
        }
    }

    /// Canvas `GeneralAssignmentOverrideConverter#get_sort_key`: assignments
    /// and quizzes sort their tag overrides by `due_at`, everything else by
    /// `lock_at`.
    fn sort_column(self) -> &'static str {
        match self {
            LearningObject::Assignment | LearningObject::Quiz => "o.due_at",
            LearningObject::DiscussionTopic | LearningObject::WikiPage => "o.lock_at",
        }
    }
}

/// The learning object that owns a set of override rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverrideOwner {
    ContextModule,
    Attachment,
}

/// Canvas `LearningObjectDates` for a context module
/// (`Api::V1::LearningObjectDates#learning_object_dates_json`).
///
/// A module answers `unlock_at`, `only_visible_to_overrides` and
/// `visible_to_everyone`; it has no due date, no lock date and no
/// `group_category_id`.
#[derive(Debug, Serialize)]
struct ModuleDateDetails {
    id: i64,
    unlock_at: Option<DateTime<Utc>>,
    only_visible_to_overrides: bool,
    visible_to_everyone: bool,
    graded: bool,
    overrides: Vec<OverrideJson>,
}

/// Canvas `LearningObjectDates` for a file (an `Attachment`).
#[derive(Debug, Serialize)]
struct FileDateDetails {
    id: i64,
    unlock_at: Option<DateTime<Utc>>,
    lock_at: Option<DateTime<Utc>>,
    only_visible_to_overrides: bool,
    graded: bool,
    visible_to_everyone: bool,
    overrides: Vec<OverrideJson>,
}

/// Canvas `AssignmentOverride` JSON
/// (`Api::V1::AssignmentOverride#assignment_override_json`).
#[derive(Debug, Serialize)]
struct OverrideJson {
    id: i64,
    title: Option<String>,
    unassign_item: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_module_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attachment_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_module_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    course_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    course_section_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    group_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    group_category_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    non_collaborative: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    noop_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    student_ids: Option<Vec<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    due_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unlock_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lock_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sub_assignment_due_dates: Option<Vec<Value>>,
}

/// A `context_modules` row.
///
/// `context_modules.unlock_at` is a `timestamp` (no time zone) in the port, so
/// it is read as a naive UTC value.
#[derive(Debug, sqlx::FromRow)]
struct ContextModuleRow {
    id: i64,
    unlock_at: Option<NaiveDateTime>,
}

/// An `attachments` row.
///
/// `attachments.unlock_at` / `attachments.lock_at` are `timestamp` (no time
/// zone) in the port, so they are read as naive UTC values.
#[derive(Debug, sqlx::FromRow)]
struct AttachmentRow {
    id: i64,
    unlock_at: Option<NaiveDateTime>,
    lock_at: Option<NaiveDateTime>,
    only_visible_to_overrides: bool,
}

/// A `learning_object_date_overrides` row with its set resolved.
#[derive(Debug, sqlx::FromRow)]
struct OverrideRow {
    id: i64,
    title: Option<String>,
    set_type: Option<String>,
    set_id: Option<i64>,
    student_ids: Vec<i64>,
    due_at: Option<DateTime<Utc>>,
    unlock_at: Option<DateTime<Utc>>,
    lock_at: Option<DateTime<Utc>>,
    group_name: Option<String>,
    group_category_id: Option<i64>,
    group_non_collaborative: Option<bool>,
    section_name: Option<String>,
    module_name: Option<String>,
}

/// A differentiation tag override (a group override of a non-collaborative
/// group).
#[derive(Debug, sqlx::FromRow)]
struct TagOverrideRow {
    id: i64,
    set_id: i64,
    due_at: Option<DateTime<Utc>>,
    unlock_at: Option<DateTime<Utc>>,
    lock_at: Option<DateTime<Utc>>,
}

/// The body of `PUT .../files/{attachmentId}/date_details`.
///
/// Canvas permits `unlock_at`, `lock_at`, `only_visible_to_overrides` and
/// `assignment_overrides` for an attachment; `due_at` is not permitted
/// (`allow_due_at?` is false for a file) and is dropped.
#[derive(Debug, Deserialize)]
struct FileUpdate {
    #[serde(default)]
    unlock_at: Option<Option<DateTime<Utc>>>,
    #[serde(default)]
    lock_at: Option<Option<DateTime<Utc>>>,
    #[serde(default)]
    only_visible_to_overrides: Option<bool>,
    #[serde(default)]
    assignment_overrides: Option<Vec<UpdateOverride>>,
}

// ---------------------------------------------------------------------------
// Permissions and feature flags
// ---------------------------------------------------------------------------

/// Canvas `asset.grants_right?(@current_user, :manage_assign_to)`.
async fn require_manage_assign_to(
    db: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<(), ApiError> {
    if auth.role == "admin" {
        return Ok(());
    }

    let row: Option<(String,)> = sqlx::query_as(
        "SELECT role FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND workflow_state = 'active' \
           AND role IN ('teacher', 'ta', 'designer') \
         LIMIT 1",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;

    match row {
        Some(_) => Ok(()),
        None => Err(ApiError::Forbidden),
    }
}

/// Canvas `Account.site_admin.feature_enabled?(feature)`.
async fn site_admin_feature_enabled(db: &PgPool, feature: &str) -> Result<bool, ApiError> {
    let root: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL AND workflow_state <> 'deleted' \
         ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(db)
    .await?;
    let Some((root_id,)) = root else {
        return Ok(false);
    };

    let state: Option<(String,)> = sqlx::query_as(
        "SELECT state FROM feature_flags \
         WHERE context_type = 'Account' AND context_id = $1 AND feature = $2 LIMIT 1",
    )
    .bind(root_id)
    .bind(feature)
    .fetch_optional(db)
    .await?;

    Ok(match state {
        Some((state,)) => matches!(state.as_str(), "on" | "allowed_on" | "enabled"),
        None => false,
    })
}

/// Canvas `Course#discussion_checkpoints_enabled?` (an account feature flag)
/// — it decides whether the overrides carry `sub_assignment_due_dates`.
async fn include_child_override_due_dates(db: &PgPool) -> Result<bool, ApiError> {
    site_admin_feature_enabled(db, "discussion_checkpoints").await
}

// ---------------------------------------------------------------------------
// Asset lookup
// ---------------------------------------------------------------------------

/// Canvas `@context.context_modules.not_deleted.find(id)`.
async fn load_context_module(
    db: &PgPool,
    course_id: i64,
    module_id: i64,
) -> Result<ContextModuleRow, ApiError> {
    let row: Option<ContextModuleRow> = sqlx::query_as(
        "SELECT id, unlock_at FROM context_modules \
         WHERE id = $1 AND context_id = $2 AND context_type = 'Course' \
           AND workflow_state <> 'deleted'",
    )
    .bind(module_id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;

    row.ok_or(ApiError::NotFound)
}

/// Canvas `@context.attachments.not_deleted.find(id)`.
async fn load_attachment(
    db: &PgPool,
    course_id: i64,
    attachment_id: i64,
) -> Result<AttachmentRow, ApiError> {
    let row: Option<AttachmentRow> = sqlx::query_as(
        "SELECT id, unlock_at, lock_at, only_visible_to_overrides FROM attachments \
         WHERE id = $1 AND context_id = $2 AND context_type = 'Course' \
           AND file_state IS DISTINCT FROM 'deleted'",
    )
    .bind(attachment_id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;

    row.ok_or(ApiError::NotFound)
}

/// Canvas `asset` for the `convert_tag_overrides` routes: the assignment, the
/// quiz, the discussion topic or the wiki page (by url or by id).
async fn load_learning_object(
    db: &PgPool,
    course_id: i64,
    kind: LearningObject,
    key: &str,
) -> Result<i64, ApiError> {
    let sql = if kind == LearningObject::WikiPage {
        format!(
            "SELECT id FROM {} WHERE course_id = $1 AND (url = $2 OR id::text = $2) LIMIT 1",
            kind.table()
        )
    } else {
        format!(
            "SELECT id FROM {} WHERE course_id = $1 AND id::text = $2 LIMIT 1",
            kind.table()
        )
    };

    let row: Option<(i64,)> = sqlx::query_as(&sql)
        .bind(course_id)
        .bind(key)
        .fetch_optional(db)
        .await?;

    row.map(|(id,)| id).ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// Overrides
// ---------------------------------------------------------------------------

/// The overrides of one learning object, in Canvas' `AssignmentOverride`
/// JSON shape.
async fn load_overrides(
    db: &PgPool,
    kind: &str,
    object_id: i64,
    owner: OverrideOwner,
    owner_id: i64,
    with_sub_assignment_due_dates: bool,
) -> Result<Vec<OverrideJson>, ApiError> {
    let rows: Vec<OverrideRow> = sqlx::query_as(
        "SELECT o.id, o.title, o.set_type, o.set_id, o.student_ids, \
                o.due_at, o.unlock_at, o.lock_at, \
                g.name AS group_name, g.group_category_id AS group_category_id, \
                gc.non_collaborative AS group_non_collaborative, \
                s.name AS section_name, m.name AS module_name \
         FROM learning_object_date_overrides o \
         JOIN learning_object_dates d ON d.id = o.learning_object_date_id \
         LEFT JOIN groups g ON o.set_type = 'Group' AND g.id = o.set_id \
         LEFT JOIN group_categories gc ON gc.id = g.group_category_id \
         LEFT JOIN course_sections s ON o.set_type = 'CourseSection' AND s.id = o.set_id \
         LEFT JOIN context_modules m ON m.id = $3 \
         WHERE d.learning_object_type = $1 AND d.learning_object_id = $2 \
         ORDER BY o.id",
    )
    .bind(kind)
    .bind(object_id)
    .bind(owner_id)
    .fetch_all(db)
    .await?;

    Ok(rows
        .iter()
        .map(|row| override_json(row, owner, owner_id, with_sub_assignment_due_dates))
        .collect())
}

/// Canvas `Api::V1::AssignmentOverride#assignment_override_json`.
fn override_json(
    row: &OverrideRow,
    owner: OverrideOwner,
    owner_id: i64,
    with_sub_assignment_due_dates: bool,
) -> OverrideJson {
    let mut json = OverrideJson {
        id: row.id,
        title: row.title.clone(),
        unassign_item: false,
        context_module_id: None,
        attachment_id: None,
        context_module_name: None,
        course_id: None,
        course_section_id: None,
        group_id: None,
        group_category_id: None,
        non_collaborative: None,
        noop_id: None,
        student_ids: None,
        due_at: row.due_at,
        unlock_at: row.unlock_at,
        lock_at: row.lock_at,
        sub_assignment_due_dates: None,
    };

    match owner {
        OverrideOwner::ContextModule => {
            json.context_module_id = Some(owner_id);
            json.context_module_name = row.module_name.clone();
        }
        OverrideOwner::Attachment => json.attachment_id = Some(owner_id),
    }

    match row.set_type.as_deref() {
        Some("ADHOC") => json.student_ids = Some(row.student_ids.clone()),
        Some("Group") => {
            json.group_id = row.set_id;
            json.group_category_id = row.group_category_id;
            json.non_collaborative = row.group_non_collaborative;
            // Canvas takes the title from the set when the set still exists.
            if let Some(name) = &row.group_name {
                json.title = Some(name.clone());
            }
        }
        Some("CourseSection") => {
            json.course_section_id = row.set_id;
            if let Some(name) = &row.section_name {
                json.title = Some(name.clone());
            }
        }
        Some("Course") => json.course_id = row.set_id,
        Some("Noop") => json.noop_id = row.set_id,
        _ => {}
    }

    if with_sub_assignment_due_dates {
        json.sub_assignment_due_dates = Some(Vec::new());
    }

    json
}

/// The `set_type` / `set_id` of an override of a PUT body.
fn override_target(item: &UpdateOverride) -> (Option<String>, Option<i64>) {
    if let Some(set_type) = item.set_type.clone() {
        return (Some(set_type), item.set_id);
    }
    if let Some(group_id) = item.group_id {
        return (Some("Group".to_string()), Some(group_id));
    }
    if let Some(section_id) = item.course_section_id {
        return (Some("CourseSection".to_string()), Some(section_id));
    }
    if let Some(noop_id) = item.noop_id {
        return (Some("Noop".to_string()), Some(noop_id));
    }
    if !item.student_ids.is_empty() {
        return (Some("ADHOC".to_string()), None);
    }
    (None, None)
}

// ---------------------------------------------------------------------------
// Handlers — date details
// ---------------------------------------------------------------------------

/// `GET /courses/{courseId}/modules/{contextModuleId}/date_details`.
async fn get_module_date_details(
    State(state): State<AppState>,
    Path((course_id, context_module_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<ModuleDateDetails>, ApiError> {
    let module = load_context_module(&state.db, course_id, context_module_id).await?;
    require_manage_assign_to(&state.db, &auth, course_id).await?;

    let with_sub_dates = include_child_override_due_dates(&state.db).await?;
    let overrides = load_overrides(
        &state.db,
        "ContextModule",
        module.id,
        OverrideOwner::ContextModule,
        module.id,
        with_sub_dates,
    )
    .await?;

    Ok(Json(ModuleDateDetails {
        id: module.id,
        unlock_at: module.unlock_at.map(|value| value.and_utc()),
        only_visible_to_overrides: !overrides.is_empty(),
        visible_to_everyone: overrides.is_empty(),
        graded: false,
        overrides,
    }))
}

/// `GET /courses/{courseId}/files/{attachmentId}/date_details`.
async fn get_file_date_details(
    State(state): State<AppState>,
    Path((course_id, attachment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let attachment = load_attachment(&state.db, course_id, attachment_id).await?;

    if !site_admin_feature_enabled(&state.db, "differentiated_files").await? {
        return Ok(files_not_supported());
    }

    require_manage_assign_to(&state.db, &auth, course_id).await?;

    let with_sub_dates = include_child_override_due_dates(&state.db).await?;
    let overrides = load_overrides(
        &state.db,
        "Attachment",
        attachment.id,
        OverrideOwner::Attachment,
        attachment.id,
        with_sub_dates,
    )
    .await?;

    let course_override = overrides.iter().any(|item| item.course_id.is_some());
    let details = FileDateDetails {
        id: attachment.id,
        unlock_at: attachment.unlock_at.map(|value| value.and_utc()),
        lock_at: attachment.lock_at.map(|value| value.and_utc()),
        only_visible_to_overrides: attachment.only_visible_to_overrides,
        graded: false,
        visible_to_everyone: course_override || !attachment.only_visible_to_overrides,
        overrides,
    };

    Ok(Json(details).into_response())
}

/// Canvas `check_authorized_action`: the files API is behind the
/// `differentiated_files` feature flag.
fn files_not_supported() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": "This API does not support files." })),
    )
        .into_response()
}

/// `PUT /courses/{courseId}/files/{attachmentId}/date_details`.
async fn put_file_date_details(
    State(state): State<AppState>,
    Path((course_id, attachment_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<FileUpdate>,
) -> Result<Response, ApiError> {
    let attachment = load_attachment(&state.db, course_id, attachment_id).await?;

    if !site_admin_feature_enabled(&state.db, "differentiated_files").await? {
        return Ok(files_not_supported());
    }

    require_manage_assign_to(&state.db, &auth, course_id).await?;

    let (set_unlock, unlock_at): (bool, Option<NaiveDateTime>) = match payload.unlock_at {
        Some(value) => (true, value.map(|date| date.naive_utc())),
        None => (false, None),
    };
    let (set_lock, lock_at): (bool, Option<NaiveDateTime>) = match payload.lock_at {
        Some(value) => (true, value.map(|date| date.naive_utc())),
        None => (false, None),
    };

    // Canvas `update_ungraded_object` → `attachment.update!(params)`.
    sqlx::query(
        "UPDATE attachments SET \
             unlock_at = CASE WHEN $2 THEN $3 ELSE unlock_at END, \
             lock_at = CASE WHEN $4 THEN $5 ELSE lock_at END, \
             only_visible_to_overrides = COALESCE($6, only_visible_to_overrides), \
             updated_at = now() \
         WHERE id = $1",
    )
    .bind(attachment.id)
    .bind(set_unlock)
    .bind(unlock_at)
    .bind(set_lock)
    .bind(lock_at)
    .bind(payload.only_visible_to_overrides)
    .execute(&state.db)
    .await?;

    if let Some(overrides) = payload.assignment_overrides {
        let date_id = learning_object_date_id(
            &state.db,
            course_id,
            "Attachment",
            attachment.id,
            payload.only_visible_to_overrides,
        )
        .await?;

        sqlx::query(
            "DELETE FROM learning_object_date_overrides WHERE learning_object_date_id = $1",
        )
        .bind(date_id)
        .execute(&state.db)
        .await?;

        for item in overrides {
            let (set_type, set_id) = override_target(&item);
            sqlx::query(
                "INSERT INTO learning_object_date_overrides \
                     (learning_object_date_id, title, set_type, set_id, student_ids, \
                      due_at, unlock_at, lock_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            )
            .bind(date_id)
            .bind(item.title)
            .bind(set_type)
            .bind(set_id)
            .bind(item.student_ids)
            .bind(item.due_at)
            .bind(item.unlock_at)
            .bind(item.lock_at)
            .execute(&state.db)
            .await?;
        }
    }

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// The `learning_object_dates` row of a learning object, created on demand.
async fn learning_object_date_id(
    db: &PgPool,
    course_id: i64,
    kind: &str,
    object_id: i64,
    only_visible_to_overrides: Option<bool>,
) -> Result<i64, ApiError> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO learning_object_dates \
             (course_id, learning_object_type, learning_object_id, only_visible_to_overrides) \
         VALUES ($1, $2, $3, COALESCE($4, false)) \
         ON CONFLICT (learning_object_type, learning_object_id) \
         DO UPDATE SET updated_at = now(), \
             only_visible_to_overrides = \
                 COALESCE($4, learning_object_dates.only_visible_to_overrides) \
         RETURNING id",
    )
    .bind(course_id)
    .bind(kind)
    .bind(object_id)
    .bind(only_visible_to_overrides)
    .fetch_one(db)
    .await?;

    Ok(id)
}

// ---------------------------------------------------------------------------
// Handlers — convert tag overrides
// ---------------------------------------------------------------------------

/// `PUT /courses/{courseId}/assignments/{assignmentId}/date_details/convert_tag_overrides`.
async fn convert_assignment_tag_overrides(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    convert_tag_overrides(
        &state.db,
        course_id,
        LearningObject::Assignment,
        &assignment_id.to_string(),
        &auth,
    )
    .await
}

/// `PUT /courses/{courseId}/quizzes/{quizId}/date_details/convert_tag_overrides`.
async fn convert_quiz_tag_overrides(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    convert_tag_overrides(
        &state.db,
        course_id,
        LearningObject::Quiz,
        &quiz_id.to_string(),
        &auth,
    )
    .await
}

/// `PUT /courses/{courseId}/discussion_topics/{discussionTopicId}/date_details/convert_tag_overrides`.
async fn convert_discussion_tag_overrides(
    State(state): State<AppState>,
    Path((course_id, discussion_topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    convert_tag_overrides(
        &state.db,
        course_id,
        LearningObject::DiscussionTopic,
        &discussion_topic_id.to_string(),
        &auth,
    )
    .await
}

/// `PUT /courses/{courseId}/pages/{urlOrId}/date_details/convert_tag_overrides`.
async fn convert_page_tag_overrides(
    State(state): State<AppState>,
    Path((course_id, url_or_id)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    convert_tag_overrides(
        &state.db,
        course_id,
        LearningObject::WikiPage,
        &url_or_id,
        &auth,
    )
    .await
}

/// Canvas `LearningObjectDatesController#convert_tag_overrides_to_adhoc_overrides`
/// → `OverrideConverterService.convert_tags_to_adhoc_overrides_for`.
///
/// The service replaces every differentiation tag override (an override that
/// targets a non-collaborative group) of the learning object with an ad-hoc
/// override that carries the students of the tag, then destroys the tag
/// overrides. A learning object without tag overrides is a no-op.
async fn convert_tag_overrides(
    db: &PgPool,
    course_id: i64,
    kind: LearningObject,
    key: &str,
    auth: &AuthUser,
) -> Result<Response, ApiError> {
    let object_id = load_learning_object(db, course_id, kind, key).await?;
    require_manage_assign_to(db, auth, course_id).await?;

    let date_id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM learning_object_dates \
         WHERE learning_object_type = $1 AND learning_object_id = $2 LIMIT 1",
    )
    .bind(kind.as_str())
    .bind(object_id)
    .fetch_optional(db)
    .await?;

    // No dates row, no overrides: nothing to convert.
    let Some(date_id) = date_id else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };

    let mut tx: Transaction<'_, Postgres> = db.begin().await?;

    let tag_overrides = load_tag_overrides(&mut tx, kind, object_id).await?;
    if tag_overrides.is_empty() {
        tx.rollback().await?;
        return Ok(StatusCode::NO_CONTENT.into_response());
    }

    let mut already_overridden = students_already_overridden(&mut tx, kind, object_id).await?;

    for tag in &tag_overrides {
        let mut students = students_in_tag(&mut tx, tag.set_id).await?;
        students.retain(|student| !already_overridden.contains(student));
        if students.is_empty() {
            continue;
        }

        let title = adhoc_title(students.len());
        let _: (i64,) = sqlx::query_as(
            "INSERT INTO learning_object_date_overrides \
                 (learning_object_date_id, title, set_type, student_ids, \
                  due_at, unlock_at, lock_at) \
             VALUES ($1, $2, 'ADHOC', $3, $4, $5, $6) RETURNING id",
        )
        .bind(date_id)
        .bind(title)
        .bind(students.as_slice())
        .bind(tag.due_at)
        .bind(tag.unlock_at)
        .bind(tag.lock_at)
        .fetch_one(&mut *tx)
        .await?;

        for student in students {
            already_overridden.insert(student);
        }
    }

    // Canvas destroys the tag overrides once the ad-hoc ones exist.
    let tag_ids: Vec<i64> = tag_overrides.iter().map(|tag| tag.id).collect();
    sqlx::query("DELETE FROM learning_object_date_overrides WHERE id = ANY($1)")
        .bind(tag_ids.as_slice())
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Canvas `TagOverrideConverter#differentiation_tag_overrides_for`: the
/// active overrides that target a non-collaborative group, latest date first.
async fn load_tag_overrides(
    tx: &mut Transaction<'_, Postgres>,
    kind: LearningObject,
    object_id: i64,
) -> Result<Vec<TagOverrideRow>, ApiError> {
    let sql = format!(
        "SELECT o.id, o.set_id, o.due_at, o.unlock_at, o.lock_at \
         FROM learning_object_date_overrides o \
         JOIN learning_object_dates d ON d.id = o.learning_object_date_id \
         JOIN groups g ON g.id = o.set_id \
         JOIN group_categories gc ON gc.id = g.group_category_id \
         WHERE d.learning_object_type = $1 AND d.learning_object_id = $2 \
           AND o.set_type = 'Group' AND gc.non_collaborative = true \
         ORDER BY {} DESC NULLS LAST, o.id",
        kind.sort_column()
    );

    let rows: Vec<TagOverrideRow> = sqlx::query_as(&sql)
        .bind(kind.as_str())
        .bind(object_id)
        .fetch_all(&mut **tx)
        .await?;

    Ok(rows)
}

/// Canvas `TagOverrideConverter#students_already_overridden`: the students an
/// ad-hoc override already covers.
async fn students_already_overridden(
    tx: &mut Transaction<'_, Postgres>,
    kind: LearningObject,
    object_id: i64,
) -> Result<HashSet<i64>, ApiError> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT DISTINCT student_id \
         FROM learning_object_date_overrides o \
         JOIN learning_object_dates d ON d.id = o.learning_object_date_id, \
              unnest(o.student_ids) AS student_id \
         WHERE d.learning_object_type = $1 AND d.learning_object_id = $2 \
           AND o.set_type = 'ADHOC'",
    )
    .bind(kind.as_str())
    .bind(object_id)
    .fetch_all(&mut **tx)
    .await?;

    Ok(rows.into_iter().map(|(student_id,)| student_id).collect())
}

/// Canvas `TagOverrideConverter#find_students_in_tags`: the members of the
/// group of a tag override.
async fn students_in_tag(
    tx: &mut Transaction<'_, Postgres>,
    group_id: i64,
) -> Result<Vec<i64>, ApiError> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT user_id FROM group_memberships \
         WHERE group_id = $1 AND workflow_state = 'accepted' \
         ORDER BY user_id",
    )
    .bind(group_id)
    .fetch_all(&mut **tx)
    .await?;

    Ok(rows.into_iter().map(|(user_id,)| user_id).collect())
}

/// Canvas `AssignmentOverride#title_from_students`.
fn adhoc_title(student_count: usize) -> String {
    if student_count == 1 {
        "1 student".to_string()
    } else {
        format!("{student_count} students")
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// The learning-object-date routes of `LearningObjectDatesController`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{courseId}/modules/{contextModuleId}/date_details",
            get(get_module_date_details),
        )
        .route(
            "/courses/{courseId}/files/{attachmentId}/date_details",
            get(get_file_date_details),
        )
        .route(
            "/courses/{courseId}/files/{attachmentId}/date_details",
            put(put_file_date_details),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/date_details/convert_tag_overrides",
            put(convert_assignment_tag_overrides),
        )
        .route(
            "/courses/{courseId}/quizzes/{quizId}/date_details/convert_tag_overrides",
            put(convert_quiz_tag_overrides),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{discussionTopicId}/date_details/convert_tag_overrides",
            put(convert_discussion_tag_overrides),
        )
        .route(
            "/courses/{courseId}/pages/{urlOrId}/date_details/convert_tag_overrides",
            put(convert_page_tag_overrides),
        )
}
