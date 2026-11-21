//! Canvas account reports REST API — a faithful port of
//! `canvas-src/app/controllers/account_reports_controller.rb`
//! (`AccountReportsController`).
//!
//! Ported routes (Canvas `config/routes.rb`,
//! `scope(controller: :account_reports)`):
//!
//! | Verb   | Path                                               | Ruby action         |
//! |--------|----------------------------------------------------|---------------------|
//! | GET    | `/accounts/{account_id}/reports/{report}`         | `index`             |
//! | GET    | `/accounts/{account_id}/reports`                   | `available_reports` |
//! | GET    | `/accounts/{account_id}/reports/{report}/{id}`     | `show`              |
//! | POST   | `/accounts/{account_id}/reports/{report}`         | `create`            |
//! | DELETE | `/accounts/{account_id}/reports/{report}/{id}`     | `destroy`           |
//! | PUT    | `/accounts/{account_id}/reports/{report}/{id}/abort` | `abort`           |
//!
//! The `/api/v1` prefix is applied by the router that mounts this module
//! (`star-http`), so the paths above are the ones the parity detector sees.
//!
//! ## Faithfulness notes
//!
//! * The `before_action` chain of the Ruby controller is reproduced exactly:
//!   `get_context` (a missing / inactive account is a **404**) runs before
//!   `authorized_action` (a caller without the `read_reports` right is a
//!   **403**), and an anonymous caller is a **401** (the `AuthUser` extractor).
//! * `read_reports` is granted to the `AccountAdmin` role, so a site admin or
//!   an administrator of the account (or of one of its ancestors) may use these
//!   routes.
//! * `available_reports` renders the `description_html` / `parameters_html`
//!   ERB partials. They are reproduced as static HTML here (the partials are
//!   static apart from the enrollment-term selector, which is rendered from
//!   the root account's terms).
//! * `create` enqueues the report generation the way Canvas' delayed job does:
//!   a `jobs` row with the tag `AccountReport#generate`.
//!
//! ## Tables
//!
//! The module uses the tables that already exist in `star-db`: `accounts`,
//! `account_users`, `users`, `user_profiles`, `account_reports`, `attachments`,
//! `enrollment_terms` and `jobs`. No new DDL is required.

use std::collections::HashMap;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use chrono::{Duration, NaiveDateTime, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// Canvas `Api::PER_PAGE`.
const DEFAULT_PER_PAGE: i64 = 10;
/// Canvas `Api::MAX_PER_PAGE`.
const MAX_PER_PAGE: i64 = 100;
/// Canvas `Setting.get("recent_account_report_window_hours", "12")`.
const RECENT_REPORT_WINDOW_HOURS: i64 = 12;

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// The account reports routes.
///
/// Exposes `pub fn router() -> axum::Router<AppState>` like every other domain
/// module of the workspace.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/accounts/{account_id}/reports", get(available_reports))
        .route(
            "/accounts/{account_id}/reports/{report}",
            get(index).post(create),
        )
        .route(
            "/accounts/{account_id}/reports/{report}/{id}",
            get(show).delete(destroy),
        )
        .route(
            "/accounts/{account_id}/reports/{report}/{id}/abort",
            put(abort),
        )
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// The error type of the account reports endpoints.
///
/// Canvas renders a missing record as
/// `{"errors":[{"message":"The specified resource does not exist."}]}` and a
/// rejected request as `{"error":"..."}`. `star_common::ApiError` renders a
/// plain text body, so the JSON shapes are restored here.
#[derive(Debug)]
enum ReportError {
    Api(ApiError),
}

impl From<ApiError> for ReportError {
    fn from(err: ApiError) -> Self {
        ReportError::Api(err)
    }
}

impl From<sqlx::Error> for ReportError {
    fn from(err: sqlx::Error) -> Self {
        ReportError::Api(ApiError::from(err))
    }
}

impl IntoResponse for ReportError {
    fn into_response(self) -> Response {
        match self {
            ReportError::Api(ApiError::NotFound) => (
                StatusCode::NOT_FOUND,
                Json(
                    json!({ "errors": [{ "message": "The specified resource does not exist." }] }),
                ),
            )
                .into_response(),
            ReportError::Api(ApiError::BadRequest(message)) => {
                (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
            }
            ReportError::Api(other) => other.into_response(),
        }
    }
}

// ---------------------------------------------------------------------------
// Report registry
// ---------------------------------------------------------------------------

/// A parameter of a report definition.
struct ReportParameter {
    name: &'static str,
    required: bool,
    description: Option<&'static str>,
}

/// A parameter of the Canvas report registry.
const fn param(name: &'static str, required: bool, description: &'static str) -> ReportParameter {
    ReportParameter {
        name,
        required,
        description: Some(description),
    }
}

/// One entry of the Canvas `AccountReports` registry.
struct ReportDefinition {
    report_type: &'static str,
    title: &'static str,
    parameters: &'static [ReportParameter],
    description_html: &'static str,
    parameters_template: Option<ParametersTemplate>,
}

/// The ERB partial Canvas renders for `parameters_html`.
#[derive(Clone, Copy)]
enum ParametersTemplate {
    Eportfolio,
    GradeExport,
    MgpGradeExport,
    TermSelector,
    OutcomeResults,
    Provisioning,
    SisExport,
    StudentsWithNoSubmissions,
    TermAndDatePicker,
    UserAccessTokens,
    IncludeOnlyDeleted,
}

impl ParametersTemplate {
    /// The static HTML of the partial, with the nested partials left as
    /// `@@PARTIAL:...@@` markers.
    fn template(self) -> &'static str {
        match self {
            ParametersTemplate::Eportfolio => PARAMETERS_EPORTFOLIO_REPORT_CSV,
            ParametersTemplate::GradeExport => PARAMETERS_GRADE_EXPORT_CSV,
            ParametersTemplate::MgpGradeExport => PARAMETERS_MGP_GRADE_EXPORT_CSV,
            ParametersTemplate::TermSelector => PARAMETERS_TERM_SELECTOR,
            ParametersTemplate::OutcomeResults => PARAMETERS_OUTCOME_RESULTS_CSV,
            ParametersTemplate::Provisioning => PARAMETERS_PROVISIONING_CSV,
            ParametersTemplate::SisExport => PARAMETERS_SIS_EXPORT_CSV,
            ParametersTemplate::StudentsWithNoSubmissions => {
                PARAMETERS_STUDENTS_WITH_NO_SUBMISSIONS_CSV
            }
            ParametersTemplate::TermAndDatePicker => PARAMETERS_TERM_AND_DATE_PICKER,
            ParametersTemplate::UserAccessTokens => PARAMETERS_USER_ACCESS_TOKENS_CSV,
            ParametersTemplate::IncludeOnlyDeleted => PARAMETERS_INCLUDE_ONLY_DELETED_PARAMETER,
        }
    }

    /// Canvas renders `accounts/_terms_parameters` with
    /// `all_terms_option: false` for the outcome results report only, which
    /// hides the "All Terms" option of the term selector.
    fn exclude_all_option(self) -> bool {
        matches!(self, ParametersTemplate::OutcomeResults)
    }

    /// The date picker rows of the partial.
    fn date_picker(self) -> &'static str {
        match self {
            ParametersTemplate::StudentsWithNoSubmissions => DATE_PICKER_BOTH,
            _ => DATE_PICKER_START,
        }
    }
}

/// The Canvas `AccountReports` registry, ported from
/// `gems/plugins/account_reports/lib/account_reports/engine.rb`.
const REPORT_CATALOG: &[ReportDefinition] = &[
    ReportDefinition {
        report_type: "eportfolio_report_csv",
        title: "Eportfolio Report",
        parameters: &[
            param(
                "no_enrollments",
                false,
                "Only include ePortfolios from users with no enrollments",
            ),
            param("include_deleted", false, "Include deleted objects"),
        ],
        description_html: DESCRIPTION_EPORTFOLIO_REPORT_CSV,
        parameters_template: Some(ParametersTemplate::Eportfolio),
    },
    ReportDefinition {
        report_type: "grade_export_csv",
        title: "Grade Export",
        parameters: &[
            param(
                "enrollment_term_id",
                false,
                "The canvas id of the term to get grades from",
            ),
            param("include_deleted", false, "Include deleted objects"),
        ],
        description_html: DESCRIPTION_GRADE_EXPORT_CSV,
        parameters_template: Some(ParametersTemplate::GradeExport),
    },
    ReportDefinition {
        report_type: "mgp_grade_export_csv",
        title: "Multiple Grading Periods Grade Export",
        parameters: &[param(
            "enrollment_term_id",
            true,
            "The canvas id of the term to get grades from",
        )],
        description_html: DESCRIPTION_MGP_GRADE_EXPORT_CSV,
        parameters_template: Some(ParametersTemplate::MgpGradeExport),
    },
    ReportDefinition {
        report_type: "last_user_access_csv",
        title: "Last User Access",
        parameters: &[
            param(
                "enrollment_term_id",
                false,
                "The canvas id of the term to get grades from",
            ),
            param("course_id", false, "The course to report on"),
            param("include_deleted", false, "Include deleted objects"),
        ],
        description_html: DESCRIPTION_LAST_USER_ACCESS_CSV,
        parameters_template: Some(ParametersTemplate::GradeExport),
    },
    ReportDefinition {
        report_type: "last_enrollment_activity_csv",
        title: "Last Enrollment Activity",
        parameters: &[
            param(
                "enrollment_term_id",
                false,
                "The canvas id of the term's courses to get activity from",
            ),
            param("course_id", false, "The course's enrollments to report on"),
        ],
        description_html: DESCRIPTION_LAST_ENROLLMENT_ACTIVITY_CSV,
        parameters_template: Some(ParametersTemplate::TermSelector),
    },
    ReportDefinition {
        report_type: "outcome_export_csv",
        title: "Outcome Export",
        parameters: &[],
        description_html: DESCRIPTION_OUTCOME_EXPORT_CSV,
        parameters_template: None,
    },
    ReportDefinition {
        report_type: "outcome_results_csv",
        title: "Outcome Results",
        parameters: &[
            param(
                "enrollment_term_id",
                false,
                "The canvas id of the term of courses to report on",
            ),
            param(
                "order",
                false,
                "The sort order for the csv, Options: 'users', 'courses', 'outcomes'",
            ),
            param("include_deleted", false, "Include deleted objects"),
        ],
        description_html: DESCRIPTION_OUTCOME_RESULTS_CSV,
        parameters_template: Some(ParametersTemplate::OutcomeResults),
    },
    ReportDefinition {
        report_type: "provisioning_csv",
        title: "Provisioning",
        parameters: &[
            param(
                "enrollment_term_id",
                false,
                "The canvas id of the term of courses to report on",
            ),
            param("users", false, "Get the Provisioning file for users"),
            param("accounts", false, "Get the Provisioning file for accounts"),
            param("terms", false, "Get the Provisioning file for terms"),
            param("courses", false, "Get the Provisioning file for courses"),
            param("sections", false, "Get the Provisioning file for sections"),
            param(
                "enrollments",
                false,
                "Get the Provisioning file for enrollments",
            ),
            param("groups", false, "Get the Provisioning file for groups"),
            param(
                "group_categories",
                false,
                "Get the Provisioning file for group_categories",
            ),
            param(
                "group_membership",
                false,
                "Get the Provisioning file for group_membership",
            ),
            param(
                "xlist",
                false,
                "Get the Provisioning file for cross listed courses",
            ),
            param(
                "user_observers",
                false,
                "Get the Provisioning file for user_observers",
            ),
            param("admins", false, "Get the Provisioning file for admins"),
            param(
                "institutional_tag_categories",
                false,
                "Get the Provisioning file for institutional tag categories",
            ),
            param(
                "institutional_tags",
                false,
                "Get the Provisioning file for institutional tags",
            ),
            param(
                "institutional_tag_associations",
                false,
                "Get the Provisioning file for institutional tag associations",
            ),
            param(
                "created_by_sis",
                false,
                "Only include objects that were created by sis",
            ),
            param("include_deleted", false, "Include deleted objects"),
            param(
                "enrollment_filter",
                false,
                "Include objects from the specified role types. Accepted values are \
                 TeacherEnrollment, TaEnrollment, DesignerEnrollment, StudentEnrollment, or \
                 ObserverEnrollment",
            ),
            param(
                "enrollment_states",
                false,
                "Include objects from the specified states.",
            ),
        ],
        description_html: DESCRIPTION_PROVISIONING_CSV,
        parameters_template: Some(ParametersTemplate::Provisioning),
    },
    ReportDefinition {
        report_type: "recently_deleted_courses_csv",
        title: "Recently Deleted Courses",
        parameters: &[param(
            "enrollment_term_id",
            false,
            "The canvas id of the term to get grades from",
        )],
        description_html: DESCRIPTION_RECENTLY_DELETED_COURSES_CSV,
        parameters_template: Some(ParametersTemplate::TermSelector),
    },
    ReportDefinition {
        report_type: "sis_export_csv",
        title: "SIS Export",
        parameters: &[
            param(
                "enrollment_term_id",
                false,
                "The canvas id of the term of courses to report on",
            ),
            param("users", false, "Get the SIS file for users"),
            param("accounts", false, "Get the SIS file for accounts"),
            param("terms", false, "Get the SIS file for terms"),
            param("courses", false, "Get the SIS file for courses"),
            param("sections", false, "Get the SIS file for sections"),
            param("enrollments", false, "Get the SIS file for enrollments"),
            param("groups", false, "Get the SIS file for groups"),
            param(
                "group_categories",
                false,
                "Get the SIS file for group_categories",
            ),
            param(
                "group_membership",
                false,
                "Get the SIS file for group_membership",
            ),
            param("xlist", false, "Get the SIS file for cross listed courses"),
            param(
                "user_observers",
                false,
                "Get the SIS file for user_observers",
            ),
            param("admins", false, "Get the SIS file for admins"),
            param(
                "institutional_tag_categories",
                false,
                "Get the SIS file for institutional tag categories",
            ),
            param(
                "institutional_tags",
                false,
                "Get the SIS file for institutional tags",
            ),
            param(
                "institutional_tag_associations",
                false,
                "Get the SIS file for institutional tag associations",
            ),
            param(
                "created_by_sis",
                false,
                "Only include objects that were created by sis",
            ),
            param("include_deleted", false, "Include deleted objects"),
        ],
        description_html: DESCRIPTION_SIS_EXPORT_CSV,
        parameters_template: Some(ParametersTemplate::SisExport),
    },
    ReportDefinition {
        report_type: "student_assignment_outcome_map_csv",
        title: "Student Competency",
        parameters: &[
            param(
                "enrollment_term_id",
                false,
                "The canvas id of the term of courses to report on",
            ),
            param("include_deleted", false, "Include deleted objects"),
        ],
        description_html: DESCRIPTION_STUDENT_ASSIGNMENT_OUTCOME_MAP_CSV,
        parameters_template: Some(ParametersTemplate::GradeExport),
    },
    ReportDefinition {
        report_type: "students_with_no_submissions_csv",
        title: "Students with no submissions",
        parameters: &[
            param("enrollment_term_id", false, "The term to report on"),
            param("course_id", false, "The course to report on"),
            param(
                "start_at",
                true,
                "The beginning date for submissions. Max time range is 2 weeks.",
            ),
            param(
                "end_at",
                true,
                "The end date for submissions. Max time range is 2 weeks.",
            ),
            param(
                "include_enrollment_state",
                false,
                "Include enrollment state.",
            ),
            param(
                "enrollment_state",
                false,
                "Enrollment states to include, defaults to 'all', Options \
                 'active'|'invited'|'creation_pending'|'deleted'|'rejected'|'completed'|'inactive'",
            ),
        ],
        description_html: DESCRIPTION_STUDENTS_WITH_NO_SUBMISSIONS_CSV,
        parameters_template: Some(ParametersTemplate::StudentsWithNoSubmissions),
    },
    ReportDefinition {
        report_type: "unpublished_courses_csv",
        title: "Unpublished Courses",
        parameters: &[param(
            "enrollment_term_id",
            false,
            "The canvas id of the term to get unpublished courses from",
        )],
        description_html: DESCRIPTION_UNPUBLISHED_COURSES_CSV,
        parameters_template: Some(ParametersTemplate::TermSelector),
    },
    ReportDefinition {
        report_type: "public_courses_csv",
        title: "Public Courses",
        parameters: &[param(
            "enrollment_term_id",
            false,
            "The canvas id of the term to get public courses from",
        )],
        description_html: DESCRIPTION_PUBLIC_COURSES_CSV,
        parameters_template: Some(ParametersTemplate::TermSelector),
    },
    ReportDefinition {
        report_type: "course_storage_csv",
        title: "Course Storage",
        parameters: &[param(
            "enrollment_term_id",
            false,
            "The canvas id of the term to get courses from for storage report",
        )],
        description_html: DESCRIPTION_COURSE_STORAGE_CSV,
        parameters_template: Some(ParametersTemplate::TermSelector),
    },
    ReportDefinition {
        report_type: "unused_courses_csv",
        title: "Unused Courses",
        parameters: &[param(
            "enrollment_term_id",
            false,
            "The canvas id of the term to get courses from",
        )],
        description_html: DESCRIPTION_UNUSED_COURSES_CSV,
        parameters_template: Some(ParametersTemplate::TermSelector),
    },
    ReportDefinition {
        report_type: "zero_activity_csv",
        title: "Zero Activity",
        parameters: &[
            param(
                "enrollment_term_id",
                false,
                "The canvas id of the term to get grades from",
            ),
            param(
                "start_at",
                false,
                "The first date in the date range, the second date is the time the report is run.",
            ),
            param("course_id", false, "The course to report on"),
        ],
        description_html: DESCRIPTION_ZERO_ACTIVITY_CSV,
        parameters_template: Some(ParametersTemplate::TermAndDatePicker),
    },
    ReportDefinition {
        report_type: "user_access_tokens_csv",
        title: "User Access Tokens",
        parameters: &[
            param(
                "exclude_deleted_and_expired",
                false,
                "Exclude deleted and expired access tokens",
            ),
            param(
                "include_deleted",
                false,
                "Include access tokens from deleted users",
            ),
        ],
        description_html: DESCRIPTION_USER_ACCESS_TOKENS_CSV,
        parameters_template: Some(ParametersTemplate::UserAccessTokens),
    },
    ReportDefinition {
        report_type: "lti_report_csv",
        title: "LTI Report",
        parameters: &[param("include_deleted", false, "Include deleted objects")],
        description_html: DESCRIPTION_LTI_REPORT_CSV,
        parameters_template: Some(ParametersTemplate::IncludeOnlyDeleted),
    },
    ReportDefinition {
        report_type: "developer_key_report_csv",
        title: "Developer Keys Report",
        parameters: &[],
        description_html: DESCRIPTION_DEVELOPER_KEY_REPORT_CSV,
        parameters_template: None,
    },
];

/// The report types of the registry, used by the `last_reports` lookup.
fn catalog_report_types() -> Vec<String> {
    REPORT_CATALOG
        .iter()
        .map(|definition| definition.report_type.to_string())
        .collect()
}

/// The registry entry of a report type, if the type is known.
fn catalog_entry(report_type: &str) -> Option<&'static ReportDefinition> {
    REPORT_CATALOG
        .iter()
        .find(|definition| definition.report_type == report_type)
}

// ---------------------------------------------------------------------------
// Database rows
// ---------------------------------------------------------------------------

/// One `account_reports` row.
#[derive(Debug, sqlx::FromRow)]
struct ReportRow {
    id: i64,
    user_id: Option<i64>,
    message: Option<String>,
    account_id: i64,
    attachment_id: Option<i64>,
    workflow_state: String,
    report_type: Option<String>,
    progress: Option<i64>,
    current_line: Option<i64>,
    parameters: Option<String>,
    start_at: Option<NaiveDateTime>,
    end_at: Option<NaiveDateTime>,
    created_at: Option<NaiveDateTime>,
}

/// The columns of an `account_reports` row, in the order the queries use.
const REPORT_COLUMNS: &str = "id, user_id, message, account_id, attachment_id, workflow_state, \
                              report_type, progress, current_line, parameters, start_at, end_at, \
                              created_at";

/// One `attachments` row.
#[derive(Debug, sqlx::FromRow)]
struct AttachmentRow {
    id: i64,
    folder_id: Option<i64>,
    display_name: Option<String>,
    filename: Option<String>,
    uuid: Option<String>,
    content_type: Option<String>,
    size: Option<i64>,
    created_at: Option<NaiveDateTime>,
    updated_at: Option<NaiveDateTime>,
    unlock_at: Option<NaiveDateTime>,
    locked: Option<bool>,
    lock_at: Option<NaiveDateTime>,
    modified_at: Option<NaiveDateTime>,
    media_entry_id: Option<String>,
    category: Option<String>,
    visibility_level: Option<String>,
}

/// One `enrollment_terms` row.
#[derive(Debug, sqlx::FromRow)]
struct EnrollmentTermRow {
    id: i64,
    name: Option<String>,
    start_at: Option<NaiveDateTime>,
    end_at: Option<NaiveDateTime>,
}

// ---------------------------------------------------------------------------
// Context and permissions
// ---------------------------------------------------------------------------

/// The account a request targets plus the root account that owns the
/// enrollment terms.
struct ReportContext {
    account_id: i64,
    root_account_id: i64,
}

/// Canvas `get_context`: `api_find(Account.active, params[:account_id])`.
///
/// A missing account — or one that is not `active` — is a 404, and it is
/// checked *before* the permission of the caller.
async fn load_context(pool: &PgPool, raw_account_id: &str) -> Result<ReportContext, ReportError> {
    let account_id: i64 = raw_account_id.parse().map_err(|_| ApiError::NotFound)?;
    let row: Option<(i64, Option<i64>)> = sqlx::query_as(
        "SELECT id, parent_account_id FROM accounts WHERE id = $1 AND workflow_state = 'active'",
    )
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    let (id, parent) = row.ok_or(ApiError::NotFound)?;
    let root_account_id = root_account_id(pool, id, parent).await?;
    Ok(ReportContext {
        account_id: id,
        root_account_id,
    })
}

/// Walks the account hierarchy up to the root account.
async fn root_account_id(
    pool: &PgPool,
    account_id: i64,
    parent_account_id: Option<i64>,
) -> Result<i64, ReportError> {
    let mut current_id = account_id;
    let mut parent = parent_account_id;
    while let Some(parent_id) = parent {
        let row: Option<(i64, Option<i64>)> =
            sqlx::query_as("SELECT id, parent_account_id FROM accounts WHERE id = $1")
                .bind(parent_id)
                .fetch_optional(pool)
                .await?;
        let (id, next) = row.ok_or(ApiError::NotFound)?;
        current_id = id;
        parent = next;
    }
    Ok(current_id)
}

/// Canvas `authorized_action(@account, @current_user, :read_reports)`.
///
/// `read_reports` is granted to the `AccountAdmin` role (see
/// `config/initializers/permissions_registry.rb`), so a site admin or an
/// administrator of the account — or of one of its ancestors — may use these
/// routes. Everybody else gets a 403.
async fn authorize(pool: &PgPool, auth: &AuthUser, account_id: i64) -> Result<(), ReportError> {
    if auth.role == "admin" {
        return Ok(());
    }
    let mut current = Some(account_id);
    while let Some(id) = current {
        let granted: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM account_users
                 WHERE account_id = $1 AND user_id = $2 AND role = 'admin'
                   AND workflow_state <> 'deleted'
             )",
        )
        .bind(id)
        .bind(auth.id)
        .fetch_one(pool)
        .await?;
        if granted {
            return Ok(());
        }
        let parent: Option<i64> =
            sqlx::query_scalar("SELECT parent_account_id FROM accounts WHERE id = $1")
                .bind(id)
                .fetch_one(pool)
                .await?;
        current = parent;
    }
    Err(ApiError::Forbidden.into())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Canvas `AccountReportsController#available_reports`.
async fn available_reports(
    State(state): State<AppState>,
    Path(account_id): Path<String>,
    auth: AuthUser,
    Query(query): Query<Vec<(String, String)>>,
) -> Result<Response, ReportError> {
    let context = load_context(&state.db, &account_id).await?;
    authorize(&state.db, &auth, context.account_id).await?;

    let includes: Vec<&str> = query
        .iter()
        .filter(|(key, _)| key == "include" || key == "include[]")
        .map(|(_, value)| value.as_str())
        .collect();
    let wants_description = includes.iter().any(|value| *value == "description_html");
    let wants_parameters = includes.iter().any(|value| *value == "parameters_html");

    let last_runs = last_reports(&state.db, context.account_id).await?;

    let mut results: Vec<Value> = Vec::with_capacity(REPORT_CATALOG.len());
    for definition in REPORT_CATALOG {
        let mut entry = Map::new();
        entry.insert("title".into(), Value::String(definition.title.to_string()));
        entry.insert("parameters".into(), parameters_json(definition));
        entry.insert(
            "report".into(),
            Value::String(definition.report_type.to_string()),
        );
        let last_run = match last_runs.get(definition.report_type) {
            Some(report) => report_json(&state.db, report).await?,
            None => Value::Null,
        };
        entry.insert("last_run".into(), last_run);

        if wants_description {
            entry.insert(
                "description_html".into(),
                Value::String(definition.description_html.to_string()),
            );
        }
        if wants_parameters {
            if let Some(template) = definition.parameters_template {
                let body = parameters_html(&state.db, context.root_account_id, template).await?;
                let action = format!(
                    "/api/v1/accounts/{}/reports/{}",
                    context.account_id, definition.report_type
                );
                entry.insert(
                    "parameters_html".into(),
                    Value::String(format!(
                        "<form id=\"{}_form\" class=\"run_report_form\" action=\"{}\">\n  {}\n</form>\n",
                        definition.report_type, action, body
                    )),
                );
            }
        }

        results.push(Value::Object(entry));
    }
    results.sort_by(|left, right| {
        left.get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                right
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    });

    Ok(Json(Value::Array(results)).into_response())
}

/// Canvas `AccountReportsController#index`.
async fn index(
    State(state): State<AppState>,
    Path((account_id, report_type)): Path<(String, String)>,
    auth: AuthUser,
    Query(query): Query<Vec<(String, String)>>,
) -> Result<Response, ReportError> {
    let context = load_context(&state.db, &account_id).await?;
    authorize(&state.db, &auth, context.account_id).await?;

    let per_page = requested_per_page(&query);
    let page = requested_page(&query);
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_reports
          WHERE account_id = $1 AND report_type = $2 AND workflow_state <> 'deleted'",
    )
    .bind(context.account_id)
    .bind(&report_type)
    .fetch_one(&state.db)
    .await?;

    let rows: Vec<ReportRow> = sqlx::query_as(&format!(
        "SELECT {REPORT_COLUMNS} FROM account_reports
          WHERE account_id = $1 AND report_type = $2 AND workflow_state <> 'deleted'
          ORDER BY created_at DESC
          LIMIT $3 OFFSET $4"
    ))
    .bind(context.account_id)
    .bind(&report_type)
    .bind(per_page)
    .bind((page - 1) * per_page)
    .fetch_all(&state.db)
    .await?;

    let mut reports = Vec::with_capacity(rows.len());
    for row in &rows {
        reports.push(report_json(&state.db, row).await?);
    }

    let link = pagination_link(
        &format!(
            "/api/v1/accounts/{}/reports/{}",
            context.account_id, report_type
        ),
        &query,
        page,
        per_page,
        total,
    );
    Ok(json_response(Value::Array(reports), link))
}

/// Canvas `AccountReportsController#show`.
async fn show(
    State(state): State<AppState>,
    Path((account_id, report_type, report_id)): Path<(String, String, String)>,
    auth: AuthUser,
) -> Result<Response, ReportError> {
    let context = load_context(&state.db, &account_id).await?;
    authorize(&state.db, &auth, context.account_id).await?;

    let report = find_report(&state.db, context.account_id, &report_type, &report_id).await?;
    Ok(Json(report_json(&state.db, &report).await?).into_response())
}

/// Canvas `AccountReportsController#create`.
async fn create(
    State(state): State<AppState>,
    Path((account_id, report_type)): Path<(String, String)>,
    auth: AuthUser,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ReportError> {
    let context = load_context(&state.db, &account_id).await?;
    authorize(&state.db, &auth, context.account_id).await?;

    // `raise ActiveRecord::RecordNotFound unless available_reports.include? report_type`
    if catalog_entry(&report_type).is_none() {
        return Err(ApiError::NotFound.into());
    }

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let parameters = parse_parameters(content_type, &body);

    if let Some(enrollment_term_id) = enrollment_term_parameter(parameters.as_ref()) {
        if !enrollment_term_id.trim().is_empty()
            && !valid_enrollment_term_id(&state.db, context.root_account_id, &enrollment_term_id)
                .await?
        {
            return Err(ApiError::BadRequest(format!(
                "invalid enrollment_term_id '{enrollment_term_id}'"
            ))
            .into());
        }
    }

    // A recent report of the same type and parameters is reported as a
    // conflict; the `Location` header points at the running report.
    if let Some(existing) = recent_report(
        &state.db,
        context.account_id,
        &report_type,
        parameters.as_ref(),
    )
    .await?
    {
        let mut response = (
            StatusCode::CONFLICT,
            Json(report_json(&state.db, &existing).await?),
        )
            .into_response();
        if let Ok(location) = HeaderValue::from_str(&format!(
            "/api/v1/accounts/{}/reports/{}/{}",
            context.account_id, report_type, existing.id
        )) {
            response.headers_mut().insert(header::LOCATION, location);
        }
        return Ok(response);
    }

    let now = Utc::now().naive_utc();
    let parameters_text = parameters.as_ref().map(|value| value.to_string());
    let report: ReportRow = sqlx::query_as(&format!(
        "INSERT INTO account_reports
             (user_id, message, account_id, attachment_id, workflow_state, report_type,
              progress, created_at, updated_at, parameters)
         VALUES ($1, NULL, $2, NULL, 'created', $3, 0, $4, $4, $5)
         RETURNING {REPORT_COLUMNS}"
    ))
    .bind(auth.id)
    .bind(context.account_id)
    .bind(&report_type)
    .bind(now)
    .bind(parameters_text.as_deref())
    .fetch_one(&state.db)
    .await?;

    // Canvas runs the report through a delayed job; star-lms queues it in the
    // `jobs` table instead.
    sqlx::query(
        "INSERT INTO jobs (tag, workflow_state, progress, user_id, context_type, context_id)
         VALUES ('AccountReport#generate', 'queued', 0, $1, 'AccountReport', $2)",
    )
    .bind(auth.id)
    .bind(report.id)
    .execute(&state.db)
    .await?;

    Ok(Json(report_json(&state.db, &report).await?).into_response())
}

/// Canvas `AccountReportsController#destroy`.
async fn destroy(
    State(state): State<AppState>,
    Path((account_id, report_type, report_id)): Path<(String, String, String)>,
    auth: AuthUser,
) -> Result<Response, ReportError> {
    let context = load_context(&state.db, &account_id).await?;
    authorize(&state.db, &auth, context.account_id).await?;

    let mut report = find_report(&state.db, context.account_id, &report_type, &report_id).await?;
    sqlx::query(
        "UPDATE account_reports SET workflow_state = 'deleted', updated_at = $2 WHERE id = $1",
    )
    .bind(report.id)
    .bind(Utc::now().naive_utc())
    .execute(&state.db)
    .await?;
    report.workflow_state = "deleted".to_string();

    Ok(Json(report_json(&state.db, &report).await?).into_response())
}

/// Canvas `AccountReportsController#abort`.
async fn abort(
    State(state): State<AppState>,
    Path((account_id, report_type, report_id)): Path<(String, String, String)>,
    auth: AuthUser,
) -> Result<Response, ReportError> {
    let context = load_context(&state.db, &account_id).await?;
    authorize(&state.db, &auth, context.account_id).await?;

    let mut report =
        find_created_or_running_report(&state.db, context.account_id, &report_type, &report_id)
            .await?;
    sqlx::query(
        "UPDATE account_reports SET workflow_state = 'aborted', updated_at = $2 WHERE id = $1",
    )
    .bind(report.id)
    .bind(Utc::now().naive_utc())
    .execute(&state.db)
    .await?;
    report.workflow_state = "aborted".to_string();

    Ok(Json(report_json(&state.db, &report).await?).into_response())
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// Canvas `type_scope.active.find(params[:id])`.
async fn find_report(
    pool: &PgPool,
    account_id: i64,
    report_type: &str,
    raw_id: &str,
) -> Result<ReportRow, ReportError> {
    let report_id: i64 = raw_id.parse().map_err(|_| ApiError::NotFound)?;
    let row: Option<ReportRow> = sqlx::query_as(&format!(
        "SELECT {REPORT_COLUMNS} FROM account_reports
          WHERE id = $1 AND account_id = $2 AND report_type = $3
            AND workflow_state <> 'deleted'"
    ))
    .bind(report_id)
    .bind(account_id)
    .bind(report_type)
    .fetch_optional(pool)
    .await?;
    row.ok_or_else(|| ApiError::NotFound.into())
}

/// Canvas `type_scope.created_or_running.find(params[:id])`.
async fn find_created_or_running_report(
    pool: &PgPool,
    account_id: i64,
    report_type: &str,
    raw_id: &str,
) -> Result<ReportRow, ReportError> {
    let report_id: i64 = raw_id.parse().map_err(|_| ApiError::NotFound)?;
    let row: Option<ReportRow> = sqlx::query_as(&format!(
        "SELECT {REPORT_COLUMNS} FROM account_reports
          WHERE id = $1 AND account_id = $2 AND report_type = $3
            AND workflow_state IN ('created', 'running', 'compiling')"
    ))
    .bind(report_id)
    .bind(account_id)
    .bind(report_type)
    .fetch_optional(pool)
    .await?;
    row.ok_or_else(|| ApiError::NotFound.into())
}

/// Canvas `AccountReport.last_reports(account:)`: the most recent active
/// report of every known report type.
async fn last_reports(
    pool: &PgPool,
    account_id: i64,
) -> Result<HashMap<String, ReportRow>, ReportError> {
    let report_types = catalog_report_types();
    let rows: Vec<ReportRow> = sqlx::query_as(&format!(
        "SELECT {REPORT_COLUMNS} FROM account_reports
          WHERE account_id = $1 AND workflow_state <> 'deleted'
            AND report_type = ANY($2)
          ORDER BY created_at DESC"
    ))
    .bind(account_id)
    .bind(&report_types)
    .fetch_all(pool)
    .await?;

    let mut reports: HashMap<String, ReportRow> = HashMap::new();
    for row in rows {
        let Some(report_type) = row.report_type.clone() else {
            continue;
        };
        reports.entry(report_type).or_insert(row);
    }
    Ok(reports)
}

/// Canvas `AccountReport.recent_for(account:, report_type:, parameters:)`.
async fn recent_report(
    pool: &PgPool,
    account_id: i64,
    report_type: &str,
    parameters: Option<&Value>,
) -> Result<Option<ReportRow>, ReportError> {
    let window_start = Utc::now().naive_utc() - Duration::hours(RECENT_REPORT_WINDOW_HOURS);
    let rows: Vec<ReportRow> = sqlx::query_as(&format!(
        "SELECT {REPORT_COLUMNS} FROM account_reports
          WHERE account_id = $1 AND report_type = $2
            AND workflow_state IN ('created', 'running', 'compiling')
            AND created_at >= $3
          ORDER BY created_at DESC"
    ))
    .bind(account_id)
    .bind(report_type)
    .bind(window_start)
    .fetch_all(pool)
    .await?;

    for row in rows {
        let matches = match parameters {
            // Canvas only filters on the parameters when the request carries
            // some.
            None => true,
            Some(requested) => {
                let stored = parse_parameters_column(row.parameters.as_deref());
                if stored.is_null() {
                    requested.as_object().is_some_and(|map| map.is_empty())
                } else {
                    stored == *requested
                }
            }
        };
        if matches {
            return Ok(Some(row));
        }
    }
    Ok(None)
}

/// Canvas `valid_enrollment_term_id?`.
async fn valid_enrollment_term_id(
    pool: &PgPool,
    root_account_id: i64,
    enrollment_term_id: &str,
) -> Result<bool, ReportError> {
    if enrollment_term_id == "active_terms" {
        return Ok(true);
    }
    for candidate in enrollment_term_id.split(',') {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            continue;
        }
        if enrollment_term_exists(pool, root_account_id, candidate).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Canvas `api_find_all(@account.root_account.enrollment_terms, ids).exists?`.
async fn enrollment_term_exists(
    pool: &PgPool,
    root_account_id: i64,
    candidate: &str,
) -> Result<bool, ReportError> {
    if let Some(sis_source_id) = candidate.strip_prefix("sis_term_id:") {
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM enrollment_terms
                 WHERE root_account_id = $1 AND sis_source_id = $2
             )",
        )
        .bind(root_account_id)
        .bind(sis_source_id)
        .fetch_one(pool)
        .await?;
        return Ok(found);
    }
    if let Some(integration_id) = candidate.strip_prefix("sis_integration_id:") {
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM enrollment_terms WHERE integration_id = $1)",
        )
        .bind(integration_id)
        .fetch_one(pool)
        .await?;
        return Ok(found);
    }
    if candidate == "default" {
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM enrollment_terms
                 WHERE root_account_id = $1 AND name = 'Default Term'
             )",
        )
        .bind(root_account_id)
        .fetch_one(pool)
        .await?;
        return Ok(found);
    }
    if candidate == "current" {
        let now = Utc::now().naive_utc();
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM enrollment_terms
                 WHERE root_account_id = $1 AND workflow_state <> 'deleted'
                   AND (start_at IS NULL OR start_at <= $2)
                   AND (end_at IS NULL OR end_at >= $2)
                   AND NOT (start_at IS NULL AND end_at IS NULL)
             )",
        )
        .bind(root_account_id)
        .bind(now)
        .fetch_one(pool)
        .await?;
        return Ok(found);
    }
    let Ok(term_id) = candidate.parse::<i64>() else {
        return Ok(false);
    };
    let found: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM enrollment_terms WHERE id = $1)")
            .bind(term_id)
            .fetch_one(pool)
            .await?;
    Ok(found)
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

/// The `parameters` of a report definition, or `null` when the report takes
/// no parameters (Canvas keeps `parameters` nil for an empty definition).
fn parameters_json(definition: &ReportDefinition) -> Value {
    if definition.parameters.is_empty() {
        return Value::Null;
    }
    let mut map = Map::new();
    for parameter in definition.parameters {
        map.insert(
            parameter.name.to_string(),
            json!({
                "required": parameter.required,
                "description": parameter.description,
            }),
        );
    }
    Value::Object(map)
}

/// Canvas `account_report_json`.
async fn report_json(pool: &PgPool, report: &ReportRow) -> Result<Value, ReportError> {
    let mut map = Map::new();
    map.insert("id".into(), Value::from(report.id));
    map.insert(
        "progress".into(),
        report
            .progress
            .map_or(Value::Null, |value| Value::from(value)),
    );
    map.insert(
        "parameters".into(),
        parse_parameters_column(report.parameters.as_deref()),
    );
    map.insert(
        "current_line".into(),
        report
            .current_line
            .map_or(Value::Null, |value| Value::from(value)),
    );
    map.insert(
        "status".into(),
        Value::String(report.workflow_state.clone()),
    );
    map.insert(
        "report".into(),
        report
            .report_type
            .clone()
            .map_or(Value::Null, Value::String),
    );
    map.insert("created_at".into(), iso8601(report.created_at));
    map.insert("started_at".into(), iso8601(report.start_at));
    map.insert("ended_at".into(), iso8601(report.end_at));
    map.insert("run_time".into(), Value::from(run_time(report)));
    map.insert("file_url".into(), file_url(report));
    map.insert(
        "message".into(),
        report.message.clone().map_or(Value::Null, Value::String),
    );
    map.insert(
        "user".into(),
        user_display_json(pool, report.user_id).await?,
    );
    if let Some(attachment_id) = report.attachment_id {
        if let Some(attachment) = attachment_row(pool, attachment_id).await? {
            map.insert(
                "attachment".into(),
                attachment_json(&attachment, report.account_id),
            );
        }
    }
    Ok(Value::Object(map))
}

/// Canvas `user_display_json`.
async fn user_display_json(pool: &PgPool, user_id: Option<i64>) -> Result<Value, ReportError> {
    let Some(user_id) = user_id else {
        return Ok(Value::Null);
    };
    let row: Option<(i64, String)> = sqlx::query_as("SELECT id, name FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    let Some((id, name)) = row else {
        return Ok(Value::Null);
    };
    let avatar: Option<String> =
        sqlx::query_scalar("SELECT avatar_url FROM user_profiles WHERE user_id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .flatten();
    Ok(json!({
        "id": id,
        "anonymous_id": to_base36(id),
        "display_name": name,
        "avatar_image_url": avatar,
        "html_url": format!("/users/{id}"),
        "pronouns": Value::Null,
    }))
}

/// Canvas `attachment_json`.
fn attachment_json(attachment: &AttachmentRow, account_id: i64) -> Value {
    json!({
        "id": attachment.id,
        "folder_id": attachment.folder_id,
        "display_name": attachment.display_name,
        "filename": attachment.filename,
        "uuid": attachment.uuid,
        "upload_status": "success",
        "content-type": attachment.content_type,
        "url": format!("/accounts/{account_id}/files/{}/download", attachment.id),
        "size": attachment.size,
        "created_at": iso8601(attachment.created_at),
        "updated_at": iso8601(attachment.updated_at),
        "unlock_at": iso8601(attachment.unlock_at),
        "locked": attachment.locked.unwrap_or(false),
        "hidden": false,
        "lock_at": iso8601(attachment.lock_at),
        "hidden_for_user": false,
        "thumbnail_url": "",
        "modified_at": iso8601(attachment.modified_at.or(attachment.updated_at)),
        "mime_class": mime_class(attachment.content_type.as_deref()),
        "media_entry_id": attachment.media_entry_id,
        "category": attachment.category,
        "locked_for_user": false,
        "visibility_level": attachment.visibility_level,
    })
}

/// Loads one `attachments` row.
async fn attachment_row(
    pool: &PgPool,
    attachment_id: i64,
) -> Result<Option<AttachmentRow>, ReportError> {
    let row: Option<AttachmentRow> = sqlx::query_as(
        "SELECT id, folder_id, display_name, filename, uuid, content_type, size, created_at,
                updated_at, unlock_at, locked, lock_at, modified_at, media_entry_id, category,
                visibility_level
           FROM attachments WHERE id = $1",
    )
    .bind(attachment_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Canvas `Attachment.mime_class`.
fn mime_class(content_type: Option<&str>) -> &'static str {
    let base_type = content_type
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim());
    match base_type {
        Some("text/html") => "html",
        Some("text/x-csharp") | Some("text/xml") | Some("text/css") | Some("application/xml") => {
            "code"
        }
        Some("text") | Some("text/plain") => "text",
        Some("application/rtf") | Some("text/rtf") => "doc",
        Some("application/vnd.oasis.opendocument.text") => "doc",
        Some("application/pdf") => "pdf",
        Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document") => "doc",
        Some("application/vnd.apple.pages") => "doc",
        Some("application/x-docx") => "doc",
        Some("application/msword") => "doc",
        Some("application/vnd.ms-powerpoint") => "ppt",
        Some("application/vnd.openxmlformats-officedocument.presentationml.presentation") => "ppt",
        Some("applicatoin/vnd.apple.key") => "ppt",
        Some("application/vnd.ms-excel") => "xls",
        Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet") => "xls",
        Some("application/vnd.apple.numbers") => "xls",
        Some("application/vnd.oasis.opendocument.spreadsheet") => "xls",
        Some("image/jpeg")
        | Some("image/pjpeg")
        | Some("image/png")
        | Some("image/gif")
        | Some("image/bmp")
        | Some("image/svg+xml")
        | Some("image/webp")
        | Some("image/vnd.microsoft.icon") => "image",
        Some("application/x-rar")
        | Some("application/x-rar-compressed")
        | Some("application/x-zip")
        | Some("application/x-zip-compressed")
        | Some("application/zip") => "zip",
        Some("audio/mpeg")
        | Some("audio/mp3")
        | Some("audio/basic")
        | Some("audio/mid")
        | Some("audio/3gpp")
        | Some("audio/x-aiff")
        | Some("audio/x-m4a")
        | Some("audio/x-mpegurl")
        | Some("audio/x-pn-realaudio")
        | Some("audio/x-wav")
        | Some("audio/mp4")
        | Some("audio/wav")
        | Some("audio/webm")
        | Some("audio/avi")
        | Some("audio/*")
        | Some("audio") => "audio",
        Some("video/mpeg")
        | Some("video/quicktime")
        | Some("video/x-la-asf")
        | Some("video/x-ms-asf")
        | Some("video/x-ms-wma")
        | Some("video/x-ms-wmv")
        | Some("video/x-msvideo")
        | Some("video/x-sgi-movie")
        | Some("video/3gpp")
        | Some("video/mp4")
        | Some("video/webm")
        | Some("video/avi")
        | Some("video/*")
        | Some("video") => "video",
        Some("application/x-shockwave-flash") => "flash",
        _ => "file",
    }
}

/// Canvas `account_file_download_url(report.account_id, report.attachment_id)`.
fn file_url(report: &ReportRow) -> Value {
    match report.attachment_id {
        Some(attachment_id) => Value::String(format!(
            "/accounts/{}/files/{attachment_id}/download",
            report.account_id
        )),
        None => Value::Null,
    }
}

/// Canvas `[(report.end_at || Time.now.utc) - (report.start_at || report.created_at ||
/// Time.now.utc), 0.0].max`.
fn run_time(report: &ReportRow) -> f64 {
    let now = Utc::now().naive_utc();
    let end = report.end_at.unwrap_or(now);
    let start = report.start_at.or(report.created_at).unwrap_or(now);
    let seconds = (end - start).num_milliseconds() as f64 / 1000.0;
    seconds.max(0.0)
}

/// Ruby `Time#iso8601` for the UTC timestamps Canvas stores.
fn iso8601(value: Option<NaiveDateTime>) -> Value {
    match value {
        Some(timestamp) => Value::String(timestamp.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
        None => Value::Null,
    }
}

/// Ruby `id.to_s(36)`.
fn to_base36(value: i64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }
    let negative = value < 0;
    let mut rest = value.unsigned_abs();
    let mut digits: Vec<u8> = Vec::new();
    while rest > 0 {
        digits.push(DIGITS[(rest % 36) as usize]);
        rest /= 36;
    }
    if negative {
        digits.push(b'-');
    }
    digits.reverse();
    String::from_utf8_lossy(&digits).into_owned()
}

/// The `parameters` column of a report, stored as JSON.
fn parse_parameters_column(raw: Option<&str>) -> Value {
    match raw {
        Some(text) => serde_json::from_str::<Value>(text).unwrap_or(Value::Null),
        None => Value::Null,
    }
}

// ---------------------------------------------------------------------------
// Request parameters
// ---------------------------------------------------------------------------

/// Canvas `Api.per_page_requested`: `per_page.to_i.clamp(1, MAX_PER_PAGE)`.
fn requested_per_page(query: &[(String, String)]) -> i64 {
    let requested = query
        .iter()
        .find(|(key, _)| key == "per_page")
        .map(|(_, value)| value.parse::<i64>().unwrap_or(0))
        .unwrap_or(DEFAULT_PER_PAGE);
    requested.clamp(1, MAX_PER_PAGE)
}

/// The requested page, one based.
fn requested_page(query: &[(String, String)]) -> i64 {
    query
        .iter()
        .find(|(key, _)| key == "page")
        .map(|(_, value)| value.parse::<i64>().unwrap_or(1))
        .unwrap_or(1)
        .max(1)
}

/// Canvas `params[:parameters]&.to_unsafe_h`.
///
/// Canvas documents the endpoint with a `multipart/form-data` body, so the
/// parameters are read from the form fields as well as from a JSON body.
fn parse_parameters(content_type: &str, body: &[u8]) -> Option<Value> {
    let lower = content_type.to_ascii_lowercase();
    if lower.contains("application/json") {
        return parameters_from_json(body);
    }
    if lower.contains("multipart/form-data") {
        return parameters_from_fields(&parse_multipart_fields(content_type, body));
    }
    if lower.contains("application/x-www-form-urlencoded") {
        return parameters_from_fields(&parse_urlencoded_fields(body));
    }
    parameters_from_json(body).or_else(|| parameters_from_fields(&parse_urlencoded_fields(body)))
}

/// The `parameters` object of a JSON body.
fn parameters_from_json(body: &[u8]) -> Option<Value> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let parameters = value.get("parameters")?;
    match parameters {
        Value::Object(map) => Some(Value::Object(map.clone())),
        _ => None,
    }
}

/// The `parameters[...]` fields of a form body.
fn parameters_from_fields(fields: &[(String, String)]) -> Option<Value> {
    let mut map = Map::new();
    for (key, value) in fields {
        let Some(name) = key.strip_prefix("parameters[") else {
            continue;
        };
        let Some(name) = name.strip_suffix(']') else {
            continue;
        };
        map.insert(name.to_string(), Value::String(value.clone()));
    }
    if map.is_empty() {
        None
    } else {
        Some(Value::Object(map))
    }
}

/// The fields of an `application/x-www-form-urlencoded` body.
fn parse_urlencoded_fields(body: &[u8]) -> Vec<(String, String)> {
    let raw = String::from_utf8_lossy(body);
    raw.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) => (url_decode(key), url_decode(value)),
            None => (url_decode(pair), String::new()),
        })
        .collect()
}

/// The text fields of a `multipart/form-data` body.
fn parse_multipart_fields(content_type: &str, body: &[u8]) -> Vec<(String, String)> {
    let Some(boundary) = content_type.split(';').find_map(|part| {
        part.trim()
            .strip_prefix("boundary=")
            .map(|value| value.trim_matches('"').to_string())
    }) else {
        return Vec::new();
    };
    let delimiter = format!("--{boundary}");
    let mut fields = Vec::new();
    for chunk in body.split(delimiter.as_bytes()).skip(1) {
        let chunk = chunk.strip_prefix(&b"\r\n"[..]).unwrap_or(chunk);
        if chunk.starts_with(b"--") {
            break;
        }
        let Some(split) = find_subsequence(chunk, b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&chunk[..split]);
        let mut value = &chunk[split + 4..];
        while value.ends_with(b"\r\n") {
            value = &value[..value.len() - 2];
        }
        if let Some(name) = content_disposition_name(&headers) {
            fields.push((name, String::from_utf8_lossy(value).into_owned()));
        }
    }
    fields
}

/// The `name` of the `Content-Disposition` header of a multipart part.
fn content_disposition_name(headers: &str) -> Option<String> {
    for line in headers.lines() {
        let trimmed = line.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        if !lower.starts_with("content-disposition:") {
            continue;
        }
        let disposition = &trimmed["content-disposition:".len()..];
        for part in disposition.split(';') {
            let part = part.trim();
            if !part.to_ascii_lowercase().starts_with("name=") {
                continue;
            }
            return Some(part["name=".len()..].trim_matches('"').to_string());
        }
    }
    None
}

/// The `enrollment_term_id` (or `enrollment_term`) of a report request.
fn enrollment_term_parameter(parameters: Option<&Value>) -> Option<String> {
    let map = parameters?.as_object()?;
    let value = map
        .get("enrollment_term_id")
        .or_else(|| map.get("enrollment_term"))?;
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

/// A JSON response with an optional `Link` header.
fn json_response(body: Value, link: Option<String>) -> Response {
    let mut response = (StatusCode::OK, Json(body)).into_response();
    if let Some(link) = link {
        if let Ok(value) = HeaderValue::from_str(&link) {
            response.headers_mut().insert(header::LINK, value);
        }
    }
    response
}

/// The `Link` header Canvas builds in `Api.build_links_from_hash`.
fn pagination_link(
    base_url: &str,
    query: &[(String, String)],
    page: i64,
    per_page: i64,
    total: i64,
) -> Option<String> {
    let last_page = if per_page > 0 {
        ((total + per_page - 1) / per_page).max(1)
    } else {
        1
    };
    let mut base = format!("{base_url}?");
    let mut kept: Vec<&(String, String)> = query
        .iter()
        .filter(|(key, _)| {
            // Canvas `Api::EXCLUDE_IN_PAGINATION_LINKS`.
            key != "page" && key != "per_page" && key != "access_token" && key != "api_key"
        })
        .collect();
    kept.sort_by(|left, right| left.0.cmp(&right.0));
    for (key, value) in kept {
        base.push_str(&url_encode(key));
        base.push('=');
        base.push_str(&url_encode(value));
        base.push('&');
    }
    let link = |page: i64| -> String { format!("{base}page={page}&per_page={per_page}") };

    let mut links: Vec<String> = vec![format!("<{}>; rel=\"current\"", link(page))];
    if page < last_page {
        links.push(format!("<{}>; rel=\"next\"", link(page + 1)));
    }
    if page > 1 {
        links.push(format!("<{}>; rel=\"prev\"", link(page - 1)));
    }
    links.push(format!("<{}>; rel=\"first\"", link(1)));
    links.push(format!("<{}>; rel=\"last\"", link(last_page)));
    Some(links.join(","))
}

/// Percent encodes a query string component.
fn url_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// Decodes a query string component.
fn url_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut decoded: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 3 <= bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        decoded.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        decoded.push(b'%');
                        index += 1;
                    }
                }
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

// ---------------------------------------------------------------------------
// HTML
// ---------------------------------------------------------------------------

/// Canvas `accounts/_date_picker_parameter` with `start_at_option: true` and
/// `end_at_option: false`.
const DATE_PICKER_START: &str = r##"<tr>
    <td>Start at:
      <input type="text" name="parameters[start_at]" class="datetime_field" value="" />
    </td>
  </tr>"##;

/// Canvas `accounts/_date_picker_parameter` with both options enabled.
const DATE_PICKER_BOTH: &str = r##"<tr>
    <td>Start at:
      <input type="text" name="parameters[start_at]" class="datetime_field" value="" />
    </td>
  </tr>
  <tr>
    <td>
      End at:
      <input type="text" name="parameters[end_at]" class="datetime_field" value="" />
    </td>
  </tr>"##;

/// Canvas `accounts/_terms_parameters` with the enrollment term selector.
async fn terms_parameters_html(
    pool: &PgPool,
    root_account_id: i64,
    exclude_all_option: bool,
) -> Result<String, ReportError> {
    let terms: Vec<EnrollmentTermRow> = sqlx::query_as(
        "SELECT id, name, start_at, end_at FROM enrollment_terms
          WHERE root_account_id = $1 AND workflow_state <> 'deleted'
          ORDER BY name",
    )
    .bind(root_account_id)
    .fetch_all(pool)
    .await?;

    let now = Utc::now().naive_utc();
    let default_term = terms
        .iter()
        .find(|term| term.name.as_deref() == Some("Default Term"));

    let mut select = String::from(
        "<select id=\"enrollment_term_id\" name=\"parameters[enrollment_term_id]\">\n",
    );
    if !exclude_all_option {
        select.push_str("  <option value=\"\" selected>All Terms</option>\n");
    }
    if let Some(term) = default_term {
        select.push_str(&format!(
            "  <option value=\"{}\">{}</option>\n",
            term.id,
            html_escape(term.name.as_deref().unwrap_or_default())
        ));
    }
    select.push_str(&term_options(&terms, now, TermGroup::Active));
    select.push_str(&term_options(&terms, now, TermGroup::Future));
    select.push_str(&term_options(&terms, now, TermGroup::Past));
    select.push_str("</select>\n");

    Ok(format!(
        "<tr>\n  <!--Select the term to run the report on-->\n  <td>\n    <label>\n      \
         Term:&nbsp;\n      {select}\n    </label>\n  </td>\n</tr>"
    ))
}

/// The groups the enrollment term selector renders.
#[derive(Clone, Copy)]
enum TermGroup {
    Active,
    Future,
    Past,
}

/// One `optgroup` of the enrollment term selector.
fn term_options(terms: &[EnrollmentTermRow], now: NaiveDateTime, group: TermGroup) -> String {
    let mut options = String::new();
    for term in terms {
        if term.name.as_deref() == Some("Default Term") {
            continue;
        }
        let started = term.start_at.is_none_or(|start_at| start_at < now);
        let ended = term.end_at.is_some_and(|end_at| end_at < now);
        let not_ended = term.end_at.is_none_or(|end_at| end_at >= now);
        let belongs = match group {
            TermGroup::Active => started && not_ended,
            TermGroup::Future => term.start_at.is_some_and(|start_at| start_at > now),
            TermGroup::Past => ended,
        };
        if !belongs {
            continue;
        }
        options.push_str(&format!(
            "    <option value=\"{}\">{}</option>\n",
            term.id,
            html_escape(term.name.as_deref().unwrap_or_default())
        ));
    }
    if options.is_empty() {
        return String::new();
    }
    let label = match group {
        TermGroup::Active => "Active Terms",
        TermGroup::Future => "Future Terms",
        TermGroup::Past => "Past Terms",
    };
    format!("  <optgroup label=\"{label}\">\n{options}  </optgroup>\n")
}

/// The parameters HTML of a report.
async fn parameters_html(
    pool: &PgPool,
    root_account_id: i64,
    template: ParametersTemplate,
) -> Result<String, ReportError> {
    let terms = terms_parameters_html(pool, root_account_id, template.exclude_all_option()).await?;
    Ok(template
        .template()
        .replace("@@PARTIAL:terms_parameters@@", &terms)
        .replace(
            "@@PARTIAL:include_deleted_parameter@@",
            PARAMETERS_INCLUDE_DELETED_PARAMETER,
        )
        .replace("@@PARTIAL:date_picker_parameter@@", template.date_picker()))
}

/// Escapes the characters Rails escapes in the report partials.
fn html_escape(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for character in input.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

/// Finds a byte sequence in a byte slice.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

// ---------------------------------------------------------------------------
// Static HTML of the report partials
// ---------------------------------------------------------------------------
//
// The ERB partials of `gems/plugins/account_reports/app/views/accounts` are
// rendered by Canvas as `description_html` / `parameters_html`. They are static
// apart from the enrollment term selector, which is rendered from the root
// account's terms.

const DESCRIPTION_COURSE_STORAGE_CSV: &str = r##"<p>This report shows all the courses for a given term. The resulting csv
  file will have one row per course and will show the course id, course sis id,
  course short name, course name, account id, account sis id, account name,
  storage used in MB, and sum of all files in MB. The storage used reflects all
  files that go against the courses quota. The sum of all files in the course is
  the sum of all files that are in the course regardless of impact to the
  courses quota.</p>

<h3>Example</h3>

<table class="report_example">
  <thead>
  <tr>
    <th>id</th>
    <th>sis id</th>
    <th>short name</th>
    <th>name</th>
    <th>account id</th>
    <th>account sis id</th>
    <th>account name</th>
    <th>storage used in MB</th>
    <th>sum of all files in MB</th>
  </tr>
  </thead>
  <tbody>
  <tr>
    <td>345</td>
    <td>c342</td>
    <td>MATH101</td>
    <td>Mathematics 101</td>
    <td>14</td>
    <td>MATH</td>
    <td>Math Department</td>
    <td>18.82</td>
    <td>53.15</td>
  </tr>
  </tbody>
</table>"##;

const DESCRIPTION_DEVELOPER_KEY_REPORT_CSV: &str = r##"<p>This report provides a list of all Developer Keys available to the institution.
These keys can either be installed directly in the root account the report is run from
or can be inherited from parent root accounts. Developer Keys can either be API or LTI
keys.

</p>

<h3>Example</h3>

<table class="report_example">
  <thead>
  <tr>
    <th>Global ID</th>
    <th>Key Name</th>
    <th>Inherited from Parent Account</th>
    <th>Contact Info</th>
    <th>Key Type</th>
    <th>Permitted API Endpoints</th>
  </tr>
  </thead>
  <tbody>
  <tr>
    <td>10000000000004</td>
    <td>A Sample API Key</td>
    <td>false</td>
    <td>test_key@example.com</td>
    <td>API Key</td>
    <td>"[""url:POST|/api/v1/courses/:course_id/quizzes/:quiz_id/reports"", ""url:DELETE|/api/v1/courses/:course_id/quizzes/:quiz_id/reports/:id"", ""url:GET|/api/v1/courses/:course_id/quizzes/:quiz_id/reports"", ""url:GET|/api/v1/courses/:course_id/quizzes/:quiz_id/reports/:id"", ""url:GET|/api/v1/courses/:course_id/quizzes/:quiz_id/statistics"", ""url:POST|/api/v1/accounts/:account_id/admins"", ""url:DELETE|/api/v1/accounts/:account_id/admins/:user_id"", ""url:GET|/api/v1/accounts/:account_id/admins""]"</td>
  </tr>
  <tr>
    <td>10000000000014</td>
    <td>A Sample LTI Key</td>
    <td>true</td>
    <td>lti_key@example.com</td>
    <td>LTI Key</td>
    <td>"[""https://purl.imsglobal.org/spec/lti-ags/scope/lineitem"", ""https://purl.imsglobal.org/spec/lti-ags/scope/result.readonly"", ""https://purl.imsglobal.org/spec/lti-ags/scope/score"", ""https://canvas.instructure.com/lti/feature_flags/scope/show"", ""https://canvas.instructure.com/lti-ags/progress/scope/show"", ""https://purl.imsglobal.org/spec/lti-nrps/scope/contextmembership.readonly"", ""https://canvas.instructure.com/lti/public_jwk/scope/update"", ""https://canvas.instructure.com/lti/data_services/scope/create"", ""https://canvas.instructure.com/lti/data_services/scope/update"", ""https://canvas.instructure.com/lti/data_services/scope/list"", ""https://canvas.instructure.com/lti/data_services/scope/destroy"", ""https://canvas.instructure.com/lti/data_services/scope/show"", ""https://canvas.instructure.com/lti/data_services/scope/list_event_types""]"</td>
  </tr>
  </tbody>
</table>"##;

const DESCRIPTION_EPORTFOLIO_REPORT_CSV: &str = r##"<p>This report provides all active ePortfolios for the current institution.
Optionally select 'Only include ePortfolios from users with no enrollments' to
reduce the scope of the ePortfolio results to just those users with no enrollments.
You can also run the report with 'Include Deleted Objects' to only include those
ePortfolios that have been removed.</p>

<h3>Example</h3>

<table class="report_example">
  <thead>
  <tr>
    <th>eportfolio_name</th>
    <th>eportfolio_id</th>
    <th>author_name</th>
    <th>author_id</th>
    <th>author_sis_id</th>
    <th>author_login_id</th>
    <th>created_at</th>
    <th>updated_at</th>
    <th>is_public</th>
    <th>workflow_state</th>
  </tr>
  </thead>
  <tbody>
  <tr>
    <td>My Public Portfolio</td>
    <td>3</td>
    <td>Student</td>
    <td>1</td>
    <td>sis1</td>
    <td>login1@example.com</td>
    <td>2021-02-23 15:35:00 UTC</td>
    <td>2021-02-23 15:33:56 UTC</td>
    <td>true</td>
    <td>active</td>
  </tr>
  <tr>
    <td>My Private Portfolio</td>
    <td>4</td>
    <td>Student</td>
    <td>2</td>
    <td>sis2</td>
    <td>login2@example.com</td>
    <td>2021-02-23 15:35:13 UTC</td>
    <td>2021-02-23 15:35:13 UTC</td>
    <td>false</td>
    <td>deleted</td>
  </tr>
  </tbody>
</table>"##;

const DESCRIPTION_GRADE_EXPORT_CSV: &str = r##"<p>This report shows the
  student final grade results for all students in a given term. The resulting
  csv file will have one row per enrollment, and will show the current and final
  grade that is displayed in the course gradebook.</p>

<h3>Example</h3>

<table class="report_example">
  <thead>
  <tr>
    <th>student name</th>
    <th>student id</th>
    <th>student sis</th>
    <th>course</th>
    <th>course id</th>
    <th>course sis</th>
    <th>section</th>
    <th>section id</th>
    <th>section sis</th>
    <th>term</th>
    <th>term id</th>
    <th>term sis</th>
    <th>current score</th>
    <th>final score</th>
    <th>enrollment state</th>
  </tr>
  </thead>
  <tbody>
  <tr>
    <td>smith, heather</td>
    <td>423945</td>
    <td>u96872</td>
    <td>Programming 330</td>
    <td>3217</td>
    <td>c5976</td>
    <td>Section A</td>
    <td>4985</td>
    <td>s7854</td>
    <td>Fall 2012</td>
    <td>4561</td>
    <td>t1649</td>
    <td>96.3</td>
    <td>79.8</td>
    <td>active</td>
  </tr>
  <tr>
    <td>smith, kacie</td>
    <td>104837</td>
    <td>u48956</td>
    <td>English 101</td>
    <td>4564</td>
    <td>c7681</td>
    <td>Section A</td>
    <td>4563</td>
    <td>s9456</td>
    <td>Fall 2012</td>
    <td>4561</td>
    <td>t1649</td>
    <td>91.1</td>
    <td>70.9</td>
    <td>concluded</td>
  </tr>
  </tbody>
</table>"##;

const DESCRIPTION_LAST_ENROLLMENT_ACTIVITY_CSV: &str = r##"<p>This report shows the last activity on a user's enrollments.</p>

<h3>Example</h3>

<table class="report_example">
  <thead>
  <tr>
    <th>user id</th>
    <th>user name</th>
    <th>last activity at</th>
  </tr>
  </thead>
  <tbody>
  <tr>
    <td>Danton, Georges</td>
    <td>423945</td>
    <td>2014-12-21T11:54:34-07:00</td>
  </tr>
  <tr>
    <td>Sieyes, Emmanuel</td>
    <td>104837</td>
    <td>2015-12-21T11:54:34-03:00</td>
    <td></td>
    <td></td>
  </tr>
  </tbody>
</table>"##;

const DESCRIPTION_LAST_USER_ACCESS_CSV: &str = r##"<p>This report shows
  the last login for active users. Please note, Last Access At is updated
  once every 10 minutes. That is our current threshold to determine
  activity for any given user session.</p>

<h3>Example</h3>

<table class="report_example">
  <thead>
  <tr>
    <th>user id</th>
    <th>user sis id</th>
    <th>user name</th>
    <th>last access at</th>
    <th>last ip</th>
  </tr>
  </thead>
  <tbody>
  <tr>
    <td>smith, heather</td>
    <td>423945</td>
    <td>u96872</td>
    <td>2012-12-21T11:54:34-07:00</td>
    <td>10.10.0.55</td>
  </tr>
  <tr>
    <td>smith, kacie</td>
    <td>104837</td>
    <td>u48956</td>
    <td></td>
    <td></td>
  </tr>
  </tbody>
</table>"##;

const DESCRIPTION_LTI_REPORT_CSV: &str = r##"<p>This report provides an export of data that shows LTI (Learning Tool Interoperability) usage information.</p>

<h3>Example</h3>

<table class="report_example">
  <thead>
  <tr>
    <th>Configuration Location</th>
    <th>Associated Account/Course ID</th>
    <th>Associated Account Name</th>
    <th>Associated Course Name</th>
    <th>Tool ID</th>
    <th>Tool Name</th>
    <th></th>
    <th>Tool Created At</th>
    <th>Tool Privacy Level</th>
    <th>Launch URL</th>
    <th>Custom Fields</th>
  </tr>
  </thead>
  <tbody>
  <tr>
    <td>Account</td>
    <td>1</td>
    <td>Test Account</td>
    <td></td>
    <td>4</td>
    <td>Example Tool Name</td>
    <td>Example Tool</td>
    <td>2014-10-24 20:34:53 UTC</td>
    <td>anonymous</td>
    <td>https://www.edu-apps.org/lti_public_resources/?tool_id=vimeo</td>
    <td>{"canvas_account_id"=>"$Canvas.account.id"}</td>
  </tr>
  <tr>
    <td>Course</td>
    <td>2</td>
    <td></td>
    <td>Test Course</td>
    <td>5</td>
    <td>Example Tool Name</td>
    <td>Example Tool</td>
    <td>2015-12-29 20:53:00 UTC</td>
    <td>public</td>
    <td>https://www.edu-apps.org/lti_public_resources/?tool_id=youtube</td>
    <td>{"canvas_course_id"=>"$Canvas.course.id"}</td>
  </tr>
  </tbody>
</table>"##;

const DESCRIPTION_MGP_GRADE_EXPORT_CSV: &str = r##"<p>This report shows grading period grades (and overall course grades)
for each student enrolled in a given term. The resulting csv file will have one
row per enrollment.</p>

<h3>Example</h3>

<table class="report_example">
  <thead>
  <tr>
    <th>student name</th>
    <th>student id</th>
    <th>student sis</th>
    <th>course</th>
    <th>course id</th>
    <th>course sis</th>
    <th>section</th>
    <th>section id</th>
    <th>section sis</th>
    <th>term</th>
    <th>term id</th>
    <th>term sis</th>
    <th>grading period set
    <th>grading period set id
    <th>Spring grading period id
    <th>Spring grading period current score
    <th>Spring grading period final score
    <th>Fall grading period id
    <th>Fall grading period current score
    <th>Fall grading period final score
    <th>current score</th>
    <th>final score</th>
    <th>enrollment state</th>
  </tr>
  </thead>
  <tbody>
  <tr>
    <td>smith, heather</td>
    <td>423945</td>
    <td>u96872</td>
    <td>Programming 330</td>
    <td>3217</td>
    <td>c5976</td>
    <td>Section A</td>
    <td>4985</td>
    <td>s7854</td>
    <td>Fall 2012</td>
    <td>4561</td>
    <td>t1649</td>
    <td>1</td>
    <td>70.5</td>
    <td>68.5</td>
    <td>2</td>
    <td></td>
    <td>0</td>
    <td>96.3</td>
    <td>79.8</td>
    <td>active</td>
  </tr>
  <tr>
    <td>smith, kacie</td>
    <td>104837</td>
    <td>u48956</td>
    <td>English 101</td>
    <td>4564</td>
    <td>c7681</td>
    <td>Section A</td>
    <td>4563</td>
    <td>s9456</td>
    <td>Fall 2012</td>
    <td>4561</td>
    <td>t1649</td>
    <td>1</td>
    <td>75.0</td>
    <td>73.2</td>
    <td>2</td>
    <td></td>
    <td>0</td>
    <td>91.1</td>
    <td>70.9</td>
    <td>concluded</td>
  </tr>
  </tbody>
</table>"##;

const DESCRIPTION_OUTCOME_EXPORT_CSV: &str = r##"<p>This report shows all learning outcomes that exist within this account.
The resulting csv file will have one row per outcome, and will show the details of
all associated attributes with each outcome.</p>

<h3>Example</h3>

<table class="report_example">
  <thead>
  <tr>
    <th>vendor_guid</th>
    <th>object_type</th>
    <th>title</th>
    <th>description</th>
    <th>friendly_description</th>
    <th>display_name</th>
    <th>calculation_method</th>
    <th>calculation_int</th>
    <th>parent_guids</th>
    <th>workflow_state</th>
    <th>mastery_points</th>
    <th>ratings</th>
    <th></th>
    <th></th>
    <th></th>
    <th></th>
    <th></th>
  </tr>
  </thead>
  <tbody>
  <tr>
    <td>a00201</td>
    <td>outcome_group</td>
    <td>Mathematics</td>
    <td>Standards related to mathematics</td>
    <td></td>
    <td>MATH</td>
    <td></td>
    <td></td>
    <td></td>
    <td>active</td>
    <td></td>
    <td></td>
    <td></td>
    <td></td>
    <td></td>
    <td></td>
    <td></td>
  </tr>
  <tr>
    <td>a00202</td>
    <td>outcome_group</td>
    <td>Mathematics Grade 2</td>
    <td>Standards related to mathematics, grade 2</td>
    <td></td>
    <td>MATH-2</td>
    <td></td>
    <td></td>
    <td>a00201</td>
    <td>active</td>
    <td></td>
    <td></td>
    <td></td>
    <td></td>
    <td></td>
    <td></td>
    <td></td>
  </tr>
  <tr>
    <td>canvas_outcome:81</td>
    <td>outcome</td>
    <td>Order of Operations, +/-</td>
    <td>Students will be able add and subtract using parentheses to define order of operations</td>
    <td>I can add and subtract using parentheses to define order of operations</td>
    <td>MATH-2-OO</td>
    <td>decaying_average</td>
    <td>40</td>
    <td>a00201 a00202</td>
    <td>deleted</td>
    <td>3.0</td>
    <td>0.80</td>
    <td>Excellent</td>
    <td>0.40</td>
    <td>Fair</td>
    <td>0.0</td>
    <td>Needs Improvement</td>
  </tr>
  <tr>
    <td>g0335</td>
    <td>outcome</td>
    <td>Order of Operations, ×/÷</td>
    <td>Students will be able multiply and divide using parentheses to define order of operations</td>
    <td>I can multiply and divide using parentheses to define order of operations</td>
    <td>MATH-2-O1</td>
    <td>n_mastery</td>
    <td>4</td>
    <td>a00201</td>
    <td>active</td>
    <td>5.0</td>
    <td>0.60</td>
    <td>Pass</td>
    <td>0.0</td>
    <td>Fail</td>
    <td></td>
    <td></td>
  </tr>
  </tbody>
</table>

<p>If mastery scales are enabled, the report will be truncated to not include calculation method,
calculation int, mastery points, or ratings </p>

<h3>Example</h3>

<table class="report_example">
  <thead>
  <tr>
    <th>vendor_guid</th>
    <th>object_type</th>
    <th>title</th>
    <th>description</th>
    <th>friendly_description</th>
    <th>display_name</th>
    <th>parent_guids</th>
    <th>workflow_state</th>
  </tr>
  </thead>
  <tbody>
  <tr>
    <td>a00201</td>
    <td>outcome_group</td>
    <td>Mathematics</td>
    <td>Standards related to mathematics</td>
    <td></td>
    <td>MATH</td>
    <td></td>
    <td>active</td>
  </tr>
  <tr>
    <td>a00202</td>
    <td>outcome_group</td>
    <td>Mathematics Grade 2</td>
    <td>Standards related to mathematics, grade 2</td>
    <td></td>
    <td>MATH-2</td>
    <td>a00201</td>
    <td>active</td>
  </tr>
  <tr>
    <td>canvas_outcome:81</td>
    <td>outcome</td>
    <td>Order of Operations, +/-</td>
    <td>Students will be able add and subtract using parentheses to define order of operations</td>
    <td>I can add and subtract using parentheses to define order of operations</td>
    <td>MATH-2-OO</td>
    <td>a00201 a00202</td>
    <td>deleted</td>
  </tr>
  <tr>
    <td>g0335</td>
    <td>outcome</td>
    <td>Order of Operations, ×/÷</td>
    <td>Students will be able multiply and divide using parentheses to define order of operations</td>
    <td>I can multiply and divide using parentheses to define order of operations</td>
    <td>MATH-2-O1</td>
    <td>a00201</td>
    <td>active</td>
  </tr>
  </tbody>
</table>"##;

const DESCRIPTION_OUTCOME_RESULTS_CSV: &str = r##"<p>This report shows the
 learning outcome results for all students. The resulting csv file will have
 one row per user-outcome-result pair, and will show the details of the result
 including the associated assignment.</p>

<h3>Example</h3>

<table class="report_example">
  <thead>
  <tr>
    <th>student name</th>
    <th>student id</th>
    <th>student sis id</th>
    <th>assessment title</th>
    <th>assessment id</th>
    <th>assessment type</th>
    <th>submission date</th>
    <th>submission score</th>
    <th>learning outcome name</th>
    <th>learning outcome id</th>
    <th>attempt</th>
    <th>outcome score</th>
    <th>assessment question</th>
    <th>assessment question id</th>
    <th>course name</th>
    <th>course id</th>
    <th>course sis id</th>
    <th>section name</th>
    <th>section id</th>
    <th>section sis id</th>
    <th>assignment url</th>
    <th>learning outcome friendly name</th>
    <th>learning outcome points possible</th>
    <th>learning outcome mastery score</th>
    <th>learning outcome mastered</th>
    <th>learning outcome rating</th>
    <th>learning outcome rating points</th>
    <th>account id</th>
    <th>account name</th>
    <th>enrollment state</th>
  </tr>
  </thead>
  <tbody>
  <tr>
    <td>smith, heather</td>
    <td>423945</td>
    <td>u16450</td>
    <td>Assignment 1</td>
    <td>3217</td>
    <td>assignment</td>
    <td>01 Apr 2011 18:16:12 UTC</td>
    <td>85</td>
    <td>Can write basic Ruby</td>
    <td>1</td>
    <td>1</td>
    <td>4</td>
    <td></td>
    <td></td>
    <td>computer science 101</td>
    <td>46489</td>
    <td>c500190</td>
    <td>Section 1</td>
    <td>332992</td>
    <td>sec001</td>
    <td>https://example.com/courses/46489/assignments/3217</td>
    <td>Ruby 1A</td>
    <td>5</td>
    <td>3</td>
    <td>1</td>
    <td>Needs Improvement</td>
    <td>1.0</td>
    <td>992</td>
    <td>Example Account</td>
    <td>active</td>
  </tr>
  <tr>
    <td>smith, kacie</td>
    <td>104837</td>
    <td>u50214</td>
    <td>Quiz 1</td>
    <td>4565</td>
    <td>quiz</td>
    <td>02 Apr 2011 17:24:30 UTC</td>
    <td>93</td>
    <td>Can understand basic Ruby</td>
    <td>1</td>
    <td>1</td>
    <td>4</td>
    <td>Question 1</td>
    <td>4558</td>
    <td>computer science 101</td>
    <td>46489</td>
    <td>c500190</td>
    <td>Section 2</td>
    <td>332994</td>
    <td>sec003</td>
    <td>https://example.com/courses/46489/assignments/3217</td>
    <td>Ruby 1B</td>
    <td>5</td>
    <td>3</td>
    <td>0</td>
    <td>Progressing</td>
    <td>2.0</td>
    <td>994</td>
    <td>Example Subaccount</td>
    <td>active</td>
  </tr>
  </tbody>
</table>"##;

const DESCRIPTION_PROVISIONING_CSV: &str = r##"<p>This report will export all the needed information that
relates to provisioning Canvas in its respective categories.</p>"##;

const DESCRIPTION_PUBLIC_COURSES_CSV: &str = r##"<p>This report shows all the courses that are not deleted for a given
          term that are public. The resulting csv file will have one row per
          course and will show the course id, course sis id, course short name,
          course name, course start and end dates.</p>

<h3>Example</h3>

<table class="report_example">
  <thead>
  <tr>
    <th>id</th>
    <th>sis id</th>
    <th>short name</th>
    <th>name</th>
    <th>start date</th>
    <th>end date</th>
  </tr>
  </thead>
  <tbody>
  <tr>
    <td>345</td>
    <td>c342</td>
    <td>MATH101</td>
    <td>Mathematics 101</td>
    <td>2012-09-26T00:00:00-05:00</td>
    <td>2012-12-21T11:00:00-05:00</td>
  </tr>
  </tbody>
</table>"##;

const DESCRIPTION_RECENTLY_DELETED_COURSES_CSV: &str = r##"<p>This report shows all the courses for a given term that have been
  deleted in the last 30 days. The resulting csv file will have one row per
  course and will show the course id, course sis id, course short name, course
  name, course start and end dates.</p>

<h3>Example</h3>

<table class="report_example">
  <thead>
  <tr><th>id</th><th>sis id</th><th>short name</th><th>name</th>
    <th>start date</th><th>end date</th></tr>
  </thead>
  <tbody>
  <tr><td>345</td><td>c342</td><td>MATH101</td><td>Mathematics 101</td>
    <td>2012-09-26T00:00:00-05:00</td><td>2012-12-21T11:00:00-05:00</td>
  </tr>
  </tbody>
</table>"##;

const DESCRIPTION_SIS_EXPORT_CSV: &str = r##"<p> SIS Export report: This report will export all the needed
files for Users, Accounts, Terms, Courses, Sections, Enrollments, Groups, Group Memberships, and Cross Listings for an
SIS import in their respective categories. This will only return items created or modified through SIS. </p>"##;

const DESCRIPTION_STUDENT_ASSIGNMENT_OUTCOME_MAP_CSV: &str = r##"<p>This
report shows the learning outcome results for all students. The
resulting csv file will have one row per user-outcome-result pair, and will
show the details of the result including the associated assignment.
This report only shows outcome results for assignments and does not include
outcomes tied to question banks.</p>

<h3>Example</h3>


<table class="report_example">
  <thead>
  <tr>
    <th>student name</th>
    <th>student id</th>
    <th>student sis id</th>
    <th>assignment title</th>
    <th>assignment id</th>
    <th>submission date</th>
    <th>submission score</th>
    <th>learning outcome name</th>
    <th>learning outcome id</th>
    <th>attempt</th>
    <th>outcome score</th>
    <th>course name</th>
    <th>course id</th>
    <th>course sis id</th>
    <th>section name</th>
    <th>section id</th>
    <th>section sis id</th>
    <th>assignment url</th>
    <th>learning outcome friendly name</th>
    <th>learning outcome points possible</th>
    <th>learning outcome mastery score</th>
    <th>learning outcome mastered</th>
    <th>learning outcome rating</th>
    <th>learning outcome rating points</th>
    <th>account id</th>
    <th>account name</th>
    <th>enrollment state</th>
  </tr>
  </thead>
  <tbody>
  <tr>
    <td>smith, heather</td>
    <td>423945</td>
    <td>u16450</td>
    <td>Assignment 1</td>
    <td>3217</td>
    <td>01 Apr 2011 18:16:12 UTC</td>
    <td>85</td>
    <td>Can write basic Ruby</td>
    <td>1</td>
    <td>1</td>
    <td>4</td>
    <td>computer science 101</td>
    <td>46489</td>
    <td>c500190</td>
    <td>Section1</td>
    <td>678435</td>
    <td>s5001901</td>
    <td>https://example.com/courses/46489/assignments/3217</td>
    <td>Ruby A1</td>
    <td>5</td>
    <td>3</td>
    <td>1</td>
    <td>Needs Improvement</td>
    <td>1.0</td>
    <td>399</td>
    <td>Example Account</td>
    <td>active</td>
  </tr>
  <tr>
    <td>smith, kacie</td>
    <td>104837</td>
    <td>u50214</td>
    <td>Assignment 1</td>
    <td>3217</td>
    <td>02 Apr 2011 17:24:30 UTC</td>
    <td>93</td>
    <td>Can write basic Ruby</td>
    <td>1</td>
    <td>1</td>
    <td>4</td>
    <td>computer science 101</td>
    <td>46489</td>
    <td>c500190</td>
    <td>Section1</td>
    <td>678435</td>
    <td>s5001901</td>
    <td>https://example.com/courses/46489/assignments/3217</td>
    <td>Ruby A1</td>
    <td>5</td>
    <td>3</td>
    <td>1</td>
    <td>Progressing</td>
    <td>2.0</td>
    <td>403</td>
    <td>Example Subaccount</td>
    <td>active</td>
  </tr>
  </tbody>
</table>"##;

const DESCRIPTION_STUDENTS_WITH_NO_SUBMISSIONS_CSV: &str = r##"<p>This is a report of all students enrolled in a course in a given term
           that have not submitted an assignment between the given date range in
           a course. This report contains one row per student
           enrollment that meet the criteria. The date range can be no longer
           than two weeks.</p>

<h3>Example</h3>

<table class="report_example">
  <thead>
  <tr>
    <th>user id</th>
    <th>user sis id</th>
    <th>user name</th>
    <th>section id</th>
    <th>section sis id</th>
    <th>section name</th>
    <th>course id</th>
    <th>course sis id</th>
    <th>course name</th>
  </tr>
  </thead>
  <tbody>
  <tr>
    <td>423945</td>
    <td>u96872</td>
    <td>smith, heather</td>
    <td>53245</td>
    <td>s19931a</td>
    <td>History01</td>
    <td>44559</td>
    <td>c19931</td>
    <td>History</td>
  </tr>
  <tr>
    <td>423945</td>
    <td>u96872</td>
    <td>smith, heather</td>
    <td>53980</td>
    <td>c20389</td>
    <td>Math</td>
    <td>76797</td>
    <td>s20389a</td>
    <td>Math01</td>
  </tr>
  </tbody>
</table>"##;

const DESCRIPTION_UNPUBLISHED_COURSES_CSV: &str = r##"<p>This report shows all the courses for a given term that are not
  published. The resulting csv file will have one row per course and will
  show the course id, course sis id, course short name, course name, course
  start and end dates.</p>

<h3>Example</h3>

<table class="report_example">
  <thead>
  <tr>
    <th>id</th>
    <th>sis id</th>
    <th>short name</th>
    <th>name</th>
    <th>start date</th>
    <th>end date</th>
  </tr>
  </thead>
  <tbody>
  <tr>
    <td>345</td>
    <td>c342</td>
    <td>MATH101</td>
    <td>Mathematics 101</td>
    <td>2012-09-26T00:00:00-05:00</td>
    <td>2012-12-21T11:00:00-05:00</td>
  </tr>
  </tbody>
</table>"##;

const DESCRIPTION_UNUSED_COURSES_CSV: &str = r##"<p>This is a report of all courses that have none of the following
          items: assignments, announcements, discussions, files, modules,
          pages, or quizzes.</p>

<h3>Example</h3>

<table class="report_example">
  <thead>
  <tr>
    <th>course id</th>
    <th>course sis id</th>
    <th>short name</th>
    <th>long name</th>
    <th>status</th>
    <th>created at</th>
  </tr>
  </thead>
  <tbody>
  <tr>
    <td>423945</td>
    <td>c96872</td>
    <td>MAT101</td>
    <td>Math 101</td>
    <td>unpublished</td>
    <td>2013-02-26T11:50:36-07:00</td>
  </tr>
  <tr>
    <td>423948</td>
    <td>c94378</td>
    <td>SCI101</td>
    <td>Science 101</td>
    <td>unpublished</td>
    <td>2013-02-24T08:43:38-07:00</td>
  </tr>
  </tbody>
</table>"##;

const DESCRIPTION_USER_ACCESS_TOKENS_CSV: &str = r##"<p>This report shows users with access tokens.</p>

<h3>Example</h3>

<table class="report_example">
  <thead>
  <tr>
    <th>user id</th>
    <th>user name</th>
    <th>id</th>
    <th>purpose</th>
    <th>token hint</th>
    <th>visible token</th>
    <th>creation</th>
    <th>expiration</th>
    <th>last used</th>
    <th>status</th>
    <th>dev key id</th>
    <th>dev key name</th>
  </tr>
  </thead>
  <tbody>
  <tr>
    <td>u96872</td>
    <td>smith, heather</td>
    <td>112</td>
    <td>testing</td>
    <td>yXj1V</td>
    <td>496~yXj1V...</td>
    <td>2024-07-16T14:24:02-06:00</td>
    <td>2026-02-18T11:20:43-07:00</td>
    <td>2024-07-17T16:08:40-06:00</td>
    <td>active</td>
    <td>10000000000002</td>
    <td>User Generated</td>
  </tr>
  <tr>
    <td>u48956</td>
    <td>Montgomery, Montgomery</td>
    <td>455</td>
    <td>August's Tool of Unfortunate Events</td>
    <td>FeFQi</td>
    <td>496~FeFQi...</td>
    <td>2024-08-16T15:23:52-06:00</td>
    <td>2025-10-08T00:00:00-06:00</td>
    <td>never</td>
    <td>deleted</td>
    <td>123</td>
    <td>August's Tool of Unfortunate Events</td>
  </tr>
  </tbody>
</table>"##;

const DESCRIPTION_ZERO_ACTIVITY_CSV: &str = r##"<p>This report shows all the students enrolled in any
           courses in a given term. Resulting csv file will have
           one row per user enrollment that have not visited the course
           since the date.</p>

<h3>Example</h3>

<table class="report_example">
  <thead>
  <tr>
    <th>user id</th>
    <th>user sis id</th>
    <th>name</th>
    <th>section id</th>
    <th>section sis id</th>
    <th>section name</th>
    <th>course id</th>
    <th>course sis id</th>
    <th>course name</th>
  </tr>
  </thead>
  <tbody>
  <tr>
    <td>234523</td>
    <td>u8989</td>
    <td>Rolls, Jeff</td>
    <td>23458</td>
    <td>s54672</td>
    <td>math01</td>
    <td>57833</td>
    <td>c4567</td>
    <td>Math 101</td>
  </tr>
  <tr>
    <td>324522</td>
    <td>u4563</td>
    <td>Thomas, Bob</td>
    <td>23458</td>
    <td>s54672</td>
    <td>math01</td>
    <td>57833</td>
    <td>c4567</td>
    <td>Math 101</td>
  </tr>
  </tbody>
</table>"##;

const PARAMETERS_EPORTFOLIO_REPORT_CSV: &str = r##"<table>
  <!--Select to run with ePortfolios from users with no enrollments-->
  <tr>
    <td>
      <label class="checkbox">
        <input type="checkbox" name="parameters[no_enrollments]" id="parameters_no_enrollments" value="1" />
        Only include ePortfolios from users with no enrollments
      </label>
    </td>
  </tr>
  <!--Select to run with deleted users, courses, enrollments-->
  <tr>
    <td>
      <label class="checkbox">
        <input type="checkbox" name="parameters[include_deleted]" id="parameters_include_deleted" value="1" />
        Only include ePortfolios that have been removed
      </label>
    </td>
  </tr>
</table>"##;

const PARAMETERS_GRADE_EXPORT_CSV: &str = r##"<table>
  @@PARTIAL:terms_parameters@@
  @@PARTIAL:include_deleted_parameter@@
</table>"##;

const PARAMETERS_MGP_GRADE_EXPORT_CSV: &str = r##"<table>
  @@PARTIAL:terms_parameters@@
</table>"##;

const PARAMETERS_OUTCOME_RESULTS_CSV: &str = r##"<table>
  @@PARTIAL:terms_parameters@@
  @@PARTIAL:include_deleted_parameter@@
  <tr><td><h4>Order</h4></td></tr>
  <tr>
    <td>
      <label class="radio">
        <input type="radio" name="parameters[order]" id="parameters_order_users" value="Users" checked="checked" />
        Users
      </label>
    </td>
  </tr>
  <tr>
    <td>
      <label class="radio">
        <input type="radio" name="parameters[order]" id="parameters_order_courses" value="Courses" />
        Courses
      </label>
    </td>
    </td>
  </tr>
  <tr>
    <td>
      <label class="radio">
        <input type="radio" name="parameters[order]" id="parameters_order_outcomes" value="Outcomes" />
        Outcomes
      </label>
    </td>
    </td>
  </tr>
</table>"##;

const PARAMETERS_PROVISIONING_CSV: &str = r##"<table>
    @@PARTIAL:terms_parameters@@
    <tr>
        <td>
            <label class="checkbox">
                <input type="checkbox" name="parameters[users]" id="parameters_users" value="1" />
                Users CSV
            </label>
        </td>
    </tr>
    <tr>
        <td>
            <label class="checkbox">
                <input type="checkbox" name="parameters[accounts]" id="parameters_accounts" value="1" />
                Accounts CSV
            </label>
        </td>
    </tr>
    <tr>
        <td>
            <label class="checkbox">
                <input type="checkbox" name="parameters[terms]" id="parameters_terms" value="1" />
                Terms CSV
            </label>
        </td>
    </tr>
    <tr>
        <td>
            <label class="checkbox">
                <input type="checkbox" name="parameters[courses]" id="parameters_courses" value="1" />
                Courses CSV
            </label>
        </td>
    </tr>
    <tr>
        <td>
            <label class="checkbox">
                <input type="checkbox" name="parameters[sections]" id="parameters_sections" value="1" />
                Sections CSV
            </label>
        </td>
    </tr>
    <tr>
        <td>
            <label class="checkbox">
                <input type="checkbox" name="parameters[enrollments]" id="parameters_enrollments" value="1" />
                Enrollments CSV
            </label>
        </td>
    </tr>
    <tr>
        <td>
            <label class="checkbox">
                <input type="checkbox" name="parameters[groups]" id="parameters_groups" value="1" />
                Groups CSV
            </label>
        </td>
    </tr>
    <tr>
        <td>
            <label class="checkbox">
                <input type="checkbox" name="parameters[group_membership]" id="parameters_group_membership" value="1" />
                Group Membership CSV
            </label>
        </td>
    </tr>
    <tr>
        <td>
            <label class="checkbox">
                <input type="checkbox" name="parameters[group_categories]" id="parameters_group_categories" value="1" />
                Group Categories CSV
            </label>
        </td>
    </tr>
    <tr>
        <td>
            <label class="checkbox">
                <input type="checkbox" name="parameters[xlist]" id="parameters_xlist" value="1" />
                X list CSV
            </label>
        </td>
    </tr>
  <tr>
    <td>
      <label class="checkbox">
        <input type="checkbox" name="parameters[user_observers]" id="parameters_user_observers" value="1" />
        User Observer CSV
      </label>
    </td>
  </tr>
  <tr>
    <td>
      <label class="checkbox">
        <input type="checkbox" name="parameters[admins]" id="parameters_admins" value="1" />
        Admin CSV
      </label>
    </td>
  </tr>
  
    <tr>
      <td>
        <label class="checkbox">
          <input type="checkbox" name="parameters[institutional_tag_categories]" id="parameters_institutional_tag_categories" value="1" />
          Institutional Tag Categories CSV
        </label>
      </td>
    </tr>
    <tr>
      <td>
        <label class="checkbox">
          <input type="checkbox" name="parameters[institutional_tags]" id="parameters_institutional_tags" value="1" />
          Institutional Tags CSV
        </label>
      </td>
    </tr>
    <tr>
      <td>
        <label class="checkbox">
          <input type="checkbox" name="parameters[institutional_tag_associations]" id="parameters_institutional_tag_associations" value="1" />
          Institutional Tag Associations CSV
        </label>
      </td>
    </tr>
  
  <tr>
    <tr>
        <td>
            <label class="checkbox">
                <input type="checkbox" name="parameters[created_by_sis]" id="parameters_created_by_sis" value="1" disabled="disabled" />
                Created by SIS
            </label>
        </td>
    </tr>
    <tr>
        <td>
            <label>
                *Term settings only impact Courses, Sections, Enrollments, and X list CSV files.
            </label>
        </td>
    </tr>
    <tr>
      <td>
        <label class="checkbox">
          <input type="checkbox" name="parameters[include_deleted]" id="parameters_include_deleted" value="1" disabled="disabled" />
          Include Deleted/Concluded Objects
        </label>
      </td>
    </tr>
</table>
<script>
  (() => {
    const modalBody = document.getElementById('configure_modal_body')
    container = modalBody?.querySelector('form#provisioning_csv_form')
    if (container) {
      const checkboxes = container.querySelectorAll(
        'input[type="checkbox"]:not(#parameters_created_by_sis):not(#parameters_include_deleted)',
      )
      container.onclick = () => {
        const reportIsChecked = checkboxes.values().some(cb => cb.checked)
        const createdBySis = container.querySelector('#parameters_created_by_sis')
        const includeDeleted = container.querySelector('#parameters_include_deleted')

        createdBySis.disabled = !reportIsChecked
        includeDeleted.disabled = !reportIsChecked

        if (!reportIsChecked) {
          createdBySis.checked = false
          includeDeleted.checked = false
        }
      }
    }
  })()
</script>"##;

const PARAMETERS_SIS_EXPORT_CSV: &str = r##"<table>
  @@PARTIAL:terms_parameters@@
  <tr>
    <td>
      <label class="checkbox">
        <input type="checkbox" name="parameters[users]" id="parameters_users" value="1" />
        Users CSV
      </label>
    </td>
  </tr>
  <tr>
    <td>
      <label class="checkbox">
        <input type="checkbox" name="parameters[accounts]" id="parameters_accounts" value="1" />
        Accounts CSV
      </label>
    </td>
  </tr>
  <tr>
    <td>
      <label class="checkbox">
        <input type="checkbox" name="parameters[terms]" id="parameters_terms" value="1" />
        Terms CSV
      </label>
    </td>
  </tr>
  <tr>
    <td>
      <label class="checkbox">
        <input type="checkbox" name="parameters[courses]" id="parameters_courses" value="1" />
        Courses CSV
      </label>
    </td>
  </tr>
  <tr>
    <td>
      <label class="checkbox">
        <input type="checkbox" name="parameters[sections]" id="parameters_sections" value="1" />
        Sections CSV
      </label>
    </td>
  </tr>
  <tr>
    <td>
      <label class="checkbox">
        <input type="checkbox" name="parameters[enrollments]" id="parameters_enrollments" value="1" />
        Enrollments CSV
      </label>
    </td>
  </tr>
  <tr>
    <td>
      <label class="checkbox">
        <input type="checkbox" name="parameters[group_categories]" id="parameters_group_categories" value="1" />
        Group Categories CSV
      </label>
    </td>
  </tr>
  <tr>
  <tr>
    <td>
      <label class="checkbox">
        <input type="checkbox" name="parameters[groups]" id="parameters_groups" value="1" />
        Groups CSV
      </label>
    </td>
  </tr>
  <tr>
    <td>
      <label class="checkbox">
        <input type="checkbox" name="parameters[group_membership]" id="parameters_group_membership" value="1" />
        Group Membership CSV
      </label>
    </td>
  </tr>
  <tr>
    <td>
      <label class="checkbox">
        <input type="checkbox" name="parameters[xlist]" id="parameters_xlist" value="1" />
        X list CSV
      </label>
    </td>
  </tr>
  <tr>
    <td>
      <label class="checkbox">
        <input type="checkbox" name="parameters[user_observers]" id="parameters_user_observers" value="1" />
        User Observer CSV
      </label>
    </td>
  </tr>
  <tr>
    <td>
      <label class="checkbox">
        <input type="checkbox" name="parameters[admins]" id="parameters_admins" value="1" />
        Admin CSV
      </label>
    </td>
  </tr>
  
    <tr>
      <td>
        <label class="checkbox">
          <input type="checkbox" name="parameters[institutional_tag_categories]" id="parameters_institutional_tag_categories" value="1" />
          Institutional Tag Categories CSV
        </label>
      </td>
    </tr>
    <tr>
      <td>
        <label class="checkbox">
          <input type="checkbox" name="parameters[institutional_tags]" id="parameters_institutional_tags" value="1" />
          Institutional Tags CSV
        </label>
      </td>
    </tr>
    <tr>
      <td>
        <label class="checkbox">
          <input type="checkbox" name="parameters[institutional_tag_associations]" id="parameters_institutional_tag_associations" value="1" />
          Institutional Tag Associations CSV
        </label>
      </td>
    </tr>
  
  <tr>
    <td>
      <label class="checkbox">
        <input type="checkbox" name="parameters[created_by_sis]" id="parameters_created_by_sis" value="1" disabled="disabled" />
        Created by SIS
      </label>
    </td>
  </tr>
  <tr>
    <td>
      <label>
        *Term settings only impact Courses, Sections, Enrollments, and X list CSV files.
      </label>
    </td>
  </tr>
    <tr>
      <td>
        <label class="checkbox">
          <input type="checkbox" name="parameters[include_deleted]" id="parameters_include_deleted" value="1" disabled="disabled" />
          Include Deleted/Concluded Objects
        </label>
      </td>
    </tr>
</table>
<script>
  (() => {
    const modalBody = document.getElementById('configure_modal_body')
    container = modalBody?.querySelector('form#sis_export_csv_form')
    if (container) {
      const checkboxes = container.querySelectorAll(
        'input[type="checkbox"]:not(#parameters_created_by_sis):not(#parameters_include_deleted)',
      )
      container.onclick = () => {
        const reportIsChecked = checkboxes.values().some(cb => cb.checked)
        const createdBySis = container.querySelector('#parameters_created_by_sis')
        const includeDeleted = container.querySelector('#parameters_include_deleted')

        createdBySis.disabled = !reportIsChecked
        includeDeleted.disabled = !reportIsChecked

        if (!reportIsChecked) {
          createdBySis.checked = false
          includeDeleted.checked = false
        }
      }
    }
  })()
</script>"##;

const PARAMETERS_STUDENTS_WITH_NO_SUBMISSIONS_CSV: &str = r##"<table>
  @@PARTIAL:terms_parameters@@
  @@PARTIAL:date_picker_parameter@@
  <tr>
    <td>
      <label class="checkbox">
        <input type="checkbox" name="parameters[include_enrollment_state]" id="parameters_include_enrollment_state" value="1" />
        Include Enrollment State
      </label>
    </td>
  </tr>
  <tr>
    <td>Enrollment State
      <select name="parameters[enrollment_state][]">
        <option value="all" selected>All Enrollments</option>
        <option value="active">Active Enrollments</option>
      </select>
    </td>
  </tr>

</table>"##;

const PARAMETERS_TERM_AND_DATE_PICKER: &str = r##"<table>
  @@PARTIAL:terms_parameters@@
  @@PARTIAL:date_picker_parameter@@
</table>"##;

const PARAMETERS_TERM_SELECTOR: &str = r##"<table>
  @@PARTIAL:terms_parameters@@
</table>"##;

const PARAMETERS_USER_ACCESS_TOKENS_CSV: &str = r##"<table>
  <!--Select to exclude deleted and expired access tokens-->
  <tr>
    <td>
      <label class="checkbox">
        <input type="checkbox" name="parameters[exclude_deleted_and_expired]" id="parameters_exclude_deleted_and_expired" value="1" />
        Exclude deleted and expired access tokens
      </label>
    </td>
  </tr>
  <!--Select to run with deleted users i.e. users with no active pseudonyms-->
  <tr>
    <td>
      <label class="checkbox">
        <input type="checkbox" name="parameters[include_deleted]" id="parameters_include_deleted" value="1" />
        Include access tokens from deleted users
      </label>
    </td>
  </tr>
</table>"##;

const PARAMETERS_INCLUDE_ONLY_DELETED_PARAMETER: &str = r##"<table>
  @@PARTIAL:include_deleted_parameter@@
</table>"##;

const PARAMETERS_INCLUDE_DELETED_PARAMETER: &str = r##"<!--Select to run with deleted users, courses, enrollments-->
<tr>
  <td>
    <label class="checkbox">
      <input type="checkbox" name="parameters[include_deleted]" id="parameters_include_deleted" value="1" />
      Include Deleted/Concluded Objects
    </label>
  </td>
</tr>"##;
