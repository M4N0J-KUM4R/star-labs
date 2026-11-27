//! The Canvas enrollment payload.
//!
//! Canvas renders one enrollment through `Api::V1::User#enrollment_json`
//! (lib/api/v1/user.rb:292); `EnrollmentsApiController` calls it from its
//! #index and #show actions, so that payload is the contract every
//! enrollment route answers with.
//!
//! The row fields come from the `enrollments` table (see migration 0262 for
//! the Canvas-shaped table). `enrollment_state`, `grades`, `user` and
//! `html_url` are projections: Canvas computes them per request, and so does
//! this module.
//!
//! Which of the optional parts a caller sees follows Canvas:
//!
//! - `user` is rendered by the index actions (`includes = [:user] +
//!   Array(params[:include])`), not by the show action.
//! - `last_activity_at`, `last_attended_at` and `total_activity_time` are
//!   rendered for the enrolled user and for callers who may read course
//!   reports.
//! - the `sis_*` ids are rendered for callers who may read SIS data.
//! - `grades` is rendered for student enrollments only, and its contents
//!   depend on the caller's grade rights.

use chrono::{DateTime, SubsecRound, Utc};
use serde_json::{json, Map, Value};

/// The grade rights a caller has on the enrollment's course.
///
/// Canvas splits the grades hash in three: a caller without grade rights
/// sees only `html_url`, a caller who may read grades sees the effective
/// scores, and a caller who may manage grades also sees the unposted ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GradeRights {
    /// No grade rights on the course.
    None,
    /// Grade rights without grade management.
    Read,
    /// Grade management.
    Manage,
}

/// Which parts of the payload the caller may see.
#[derive(Debug, Clone, Copy)]
pub struct EnrollmentJsonOptions {
    /// Canvas renders the nested `user` object on the index actions.
    pub include_user: bool,
    /// Canvas renders the activity fields for the enrolled user and for
    /// callers who may read course reports.
    pub include_activity: bool,
    /// Canvas renders the SIS ids for callers who may read SIS data.
    pub include_sis: bool,
    /// The caller's grade rights on the enrollment's course.
    pub grade_rights: GradeRights,
}

/// One enrollment row, with the columns the Canvas payload is built from.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EnrollmentApiRow {
    pub id: i64,
    pub root_account_id: Option<i64>,
    pub user_id: i64,
    pub course_id: i64,
    pub course_section_id: Option<i64>,
    pub associated_user_id: Option<i64>,
    pub limit_privileges_to_course_section: bool,
    pub updated_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub start_at: Option<DateTime<Utc>>,
    pub end_at: Option<DateTime<Utc>>,
    pub enrollment_state: String,
    /// The raw state of the row; Canvas renames it to `enrollment_state`.
    pub workflow_state: String,
    #[sqlx(rename = "type")]
    pub enrollment_type: Option<String>,
    pub role: String,
    pub role_id: Option<i64>,
    pub last_activity_at: Option<DateTime<Utc>>,
    pub last_attended_at: Option<DateTime<Utc>>,
    pub total_activity_time: i32,
    pub sis_account_id: Option<String>,
    pub sis_course_id: Option<String>,
    pub course_integration_id: Option<String>,
    pub sis_section_id: Option<String>,
    pub section_integration_id: Option<String>,
    pub sis_user_id: Option<String>,
    pub html_url: String,
    /// `users.id` of the joined user; None when the user row is gone.
    pub user_row_id: Option<i64>,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
    pub user_created_at: Option<DateTime<Utc>>,
    pub user_sortable_name: Option<String>,
    pub current_score: Option<f64>,
    pub final_score: Option<f64>,
    pub unposted_current_score: Option<f64>,
    pub unposted_final_score: Option<f64>,
}

/// The enrollment type Canvas stores for a role.
///
/// Canvas keeps the enrollment type on the row (`StudentViewEnrollment` for a
/// student, `TeacherEnrollment` for a teacher, ...); the port derives it from
/// the role when the row does not carry one.
fn canvas_type(role: &str) -> &'static str {
    match role {
        "student" => "StudentViewEnrollment",
        "teacher" => "TeacherEnrollment",
        "ta" => "TaEnrollment",
        "observer" => "ObserverEnrollment",
        "designer" => "DesignerEnrollment",
        _ => "StudentViewEnrollment",
    }
}

/// The columns the enrollment payload is built from.
///
/// The enrollment type falls back to the role and the role id falls back to
/// the built-in role rows, so the rows other crates insert without those
/// columns still render a Canvas payload.
const TYPE_EXPR: &str = "COALESCE(e.type, CASE e.role \
    WHEN 'student' THEN 'StudentViewEnrollment' \
    WHEN 'teacher' THEN 'TeacherEnrollment' \
    WHEN 'ta' THEN 'TaEnrollment' \
    WHEN 'observer' THEN 'ObserverEnrollment' \
    WHEN 'designer' THEN 'DesignerEnrollment' \
    END)";

const ROLE_ID_EXPR: &str = "COALESCE(e.role_id, ( \
    SELECT r.id FROM roles r \
    WHERE r.name = CASE e.role \
        WHEN 'student' THEN 'StudentEnrollment' \
        WHEN 'teacher' THEN 'TeacherEnrollment' \
        WHEN 'ta' THEN 'TaEnrollment' \
        WHEN 'observer' THEN 'ObserverEnrollment' \
        WHEN 'designer' THEN 'DesignerEnrollment' \
    END \
    ORDER BY r.id LIMIT 1 \
))";

/// The select list of an enrollment row.
pub const ENROLLMENT_COLUMNS: &str = "\
    e.id, \
    COALESCE(e.root_account_id, c.account_id) AS root_account_id, \
    e.user_id, \
    e.course_id, \
    e.course_section_id, \
    e.associated_user_id, \
    e.limit_privileges_to_course_section, \
    e.updated_at, \
    e.created_at, \
    e.start_at, \
    e.end_at, \
    COALESCE( \
        CASE \
            WHEN c.workflow_state = 'deleted' THEN 'deleted' \
            WHEN cs.workflow_state = 'deleted' THEN 'deleted' \
        END, \
        e.enrollment_state \
    ) AS enrollment_state, \
    e.workflow_state, \
    {type_expr} AS type, \
    e.role, \
    {role_id_expr} AS role_id, \
    e.last_activity_at, \
    e.last_attended_at, \
    e.total_activity_time, \
    e.sis_account_id, \
    e.sis_course_id, \
    e.course_integration_id, \
    e.sis_section_id, \
    e.section_integration_id, \
    e.sis_user_id, \
    e.html_url, \
    u.id AS user_row_id, \
    u.name AS user_name, \
    u.email AS user_email, \
    u.created_at AS user_created_at, \
    COALESCE(u.sortable_name, u.name) AS user_sortable_name, \
    sc.current_score, \
    sc.final_score, \
    sc.unposted_current_score, \
    sc.unposted_final_score";

/// The joins of an enrollment row: the course and the section decide the
/// enrollment state, the user is the nested `user` payload and the score
/// rollup row carries the grades.
pub const ENROLLMENT_JOINS: &str = "\
FROM enrollments e \
LEFT JOIN courses c ON c.id = e.course_id \
LEFT JOIN course_sections cs ON cs.id = e.course_section_id \
LEFT JOIN users u ON u.id = e.user_id \
LEFT JOIN LATERAL ( \
    SELECT s.current_score, s.final_score, \
           s.unposted_current_score, s.unposted_final_score \
    FROM scores s \
    WHERE s.enrollment_id = e.id \
    ORDER BY s.id DESC \
    LIMIT 1 \
) sc ON TRUE";

/// The select list of the enrollment rows, with the type and the role id
/// expressions filled in.
pub fn enrollment_columns() -> String {
    ENROLLMENT_COLUMNS
        .replace("{type_expr}", TYPE_EXPR)
        .replace("{role_id_expr}", ROLE_ID_EXPR)
}

/// Canvas orders the enrollments of a course by type, then by the sortable
/// name of the user, then by id.
pub fn enrollment_order() -> String {
    format!(
        "ORDER BY {}, COALESCE(u.sortable_name, u.name), e.id",
        TYPE_EXPR
    )
}

impl EnrollmentApiRow {
    /// The Canvas enrollment payload.
    pub fn to_json(&self, options: &EnrollmentJsonOptions) -> Value {
        let mut map = Map::new();

        map.insert("id".into(), json!(self.id));
        map.insert("root_account_id".into(), json!(self.root_account_id));
        map.insert("user_id".into(), json!(self.user_id));
        map.insert("course_id".into(), json!(self.course_id));
        map.insert("course_section_id".into(), json!(self.course_section_id));
        map.insert("associated_user_id".into(), json!(self.associated_user_id));
        map.insert(
            "limit_privileges_to_course_section".into(),
            json!(self.limit_privileges_to_course_section),
        );
        map.insert("updated_at".into(), timestamp(self.updated_at));
        map.insert("created_at".into(), timestamp(self.created_at));
        map.insert("start_at".into(), timestamp_opt(self.start_at));
        map.insert("end_at".into(), timestamp_opt(self.end_at));
        map.insert(
            "type".into(),
            json!(self
                .enrollment_type
                .clone()
                .unwrap_or_else(|| canvas_type(&self.role).to_string())),
        );
        map.insert("enrollment_state".into(), json!(self.enrollment_state));
        map.insert("role".into(), json!(self.role));
        map.insert("role_id".into(), json!(self.role_id));

        if options.include_activity {
            map.insert(
                "last_activity_at".into(),
                timestamp_opt(self.last_activity_at),
            );
            map.insert(
                "last_attended_at".into(),
                timestamp_opt(self.last_attended_at),
            );
            map.insert(
                "total_activity_time".into(),
                json!(self.total_activity_time),
            );
        }

        // Canvas renders the grades of a student enrollment only.
        if self.role == "student" {
            map.insert("grades".into(), self.grades_json(options.grade_rights));
        }

        if options.include_sis {
            map.insert("sis_account_id".into(), json!(self.sis_account_id));
            map.insert("sis_course_id".into(), json!(self.sis_course_id));
            map.insert(
                "course_integration_id".into(),
                json!(self.course_integration_id),
            );
            map.insert("sis_section_id".into(), json!(self.sis_section_id));
            map.insert(
                "section_integration_id".into(),
                json!(self.section_integration_id),
            );
            map.insert("sis_user_id".into(), json!(self.sis_user_id));
        }

        map.insert("html_url".into(), json!(self.html_url));

        if options.include_user {
            if let Some(user) = self.user_json() {
                map.insert("user".into(), user);
            }
        }

        Value::Object(map)
    }

    /// The grades hash of a student enrollment.
    ///
    /// Canvas builds it from the enrollment's score rollups: the score fields
    /// come from the rollup row, the letter grades from the grading scheme
    /// of the course. The port has no grading scheme computation here, so the
    /// letter grades stay null.
    fn grades_json(&self, rights: GradeRights) -> Value {
        let mut grades = Map::new();

        grades.insert(
            "html_url".into(),
            json!(format!(
                "/courses/{}/grades/{}",
                self.course_id, self.user_id
            )),
        );

        match rights {
            GradeRights::None => {}
            GradeRights::Read => {
                grades.insert("current_grade".into(), Value::Null);
                grades.insert("current_score".into(), json!(self.current_score));
                grades.insert("final_grade".into(), Value::Null);
                grades.insert("final_score".into(), json!(self.final_score));
            }
            GradeRights::Manage => {
                grades.insert("current_grade".into(), Value::Null);
                grades.insert("current_score".into(), json!(self.current_score));
                grades.insert("final_grade".into(), Value::Null);
                grades.insert("final_score".into(), json!(self.final_score));
                grades.insert(
                    "unposted_current_score".into(),
                    json!(self.unposted_current_score),
                );
                grades.insert("unposted_current_grade".into(), Value::Null);
                grades.insert(
                    "unposted_final_score".into(),
                    json!(self.unposted_final_score),
                );
                grades.insert("unposted_final_grade".into(), Value::Null);
            }
        }

        Value::Object(grades)
    }

    /// The nested user payload of an enrollment.
    ///
    /// Canvas renders `id`, `name` and `created_at` plus the `sortable_name`
    /// and `short_name` methods, and adds the SIS fields of the user's
    /// pseudonym when the caller may read them. The port has no pseudonym
    /// rows yet, so the SIS fields stay null.
    fn user_json(&self) -> Option<Value> {
        let id = self.user_row_id?;
        let created_at = self.user_created_at?;

        Some(json!({
            "id": id,
            "name": self.user_name,
            "created_at": timestamp(created_at),
            "sortable_name": self.user_sortable_name,
            // Canvas falls back to the name when the user has no short name.
            "short_name": self.user_name,
            "sis_user_id": Value::Null,
            "integration_id": Value::Null,
            "login_id": Value::Null,
        }))
    }
}

/// The enrollment payload of the port's own routes.
///
/// It is the Canvas payload plus the three keys the port's routes have always
/// carried: the raw `workflow_state` (Canvas renames it to
/// `enrollment_state`), and the name and email of the user. The crate's tests
/// pin them, so they stay until the port's own routes are renamed after the
/// Canvas ones.
pub fn port_enrollment_json(row: &EnrollmentApiRow, options: &EnrollmentJsonOptions) -> Value {
    let mut value = row.to_json(options);

    if let Some(map) = value.as_object_mut() {
        map.insert("workflow_state".into(), json!(row.workflow_state));
        map.insert("name".into(), json!(row.user_name));
        map.insert("email".into(), json!(row.user_email));
    }

    value
}

/// A timestamp in the Canvas format.
///
/// Canvas renders timestamps with second precision
/// (`ActiveSupport::JSON::Encoding.time_precision = 0`,
/// config/initializers/json.rb), so the sub-second part is dropped.
fn timestamp(value: DateTime<Utc>) -> Value {
    json!(value.trunc_subsecs(0))
}

/// A nullable timestamp in the Canvas format.
fn timestamp_opt(value: Option<DateTime<Utc>>) -> Value {
    match value {
        Some(value) => timestamp(value),
        None => Value::Null,
    }
}
