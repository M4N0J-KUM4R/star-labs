//! conditional_release — Canvas `ConditionalRelease::RulesController` parity.
//!
//! Routes (the crate router mounts this module under `/api/v1`):
//! - `GET    /courses/{courseId}/mastery_paths/rules`
//! - `GET    /courses/{courseId}/mastery_paths/rules/{id}`
//! - `POST   /courses/{courseId}/mastery_paths/rules`
//! - `PUT    /courses/{courseId}/mastery_paths/rules/{id}`
//! - `DELETE /courses/{courseId}/mastery_paths/rules/{id}`
//!
//! Canvas route (config/routes.rb, the `ApiRouteSet.draw` course block):
//!   scope(controller: "conditional_release/rules") do
//!     get "courses/:course_id/mastery_paths/rules", action: "index"
//!     get "courses/:course_id/mastery_paths/rules/:id", action: "show"
//!     post "courses/:course_id/mastery_paths/rules", action: "create"
//!     put "courses/:course_id/mastery_paths/rules/:id", action: "update"
//!     delete "courses/:course_id/mastery_paths/rules/:id", action: "destroy"
//!   end
//!
//! Canvas controller (app/controllers/conditional_release/rules_controller.rb):
//! the `before_action` chain is `get_context` (the course, 404 when it does
//! not exist), `require_user` (401 for an anonymous caller), the assignment
//! management right of the action (`manage_assignments_edit` for update and
//! destroy, `manage_assignments_add` or `manage_assignments_edit` for
//! create) and finally the `read` right of the course. The rule itself is
//! loaded inside the action, so a missing rule is a 404 *after* the
//! permission checks, exactly like Canvas.
//!
//! JSON shapes are the plain ActiveRecord `as_json` of the four models,
//! without `root_account_id` and `deleted_at`:
//! - a rule: id, course_id, trigger_assignment_id, created_at, updated_at
//! - a scoring range: id, rule_id, lower_bound, upper_bound, position, ...
//! - an assignment set: id, scoring_range_id, position, ...
//! - an assignment set association: id, assignment_set_id, assignment_id, ...
//!
//! The nested collections are rendered when the request asks for them:
//! `include=all` on the reads (`Rule.includes_for_json`), always on the
//! writes (`Rule.includes_for_json`).
//!
//! Faithfulness notes:
//! - The bounds are `numeric` columns; Canvas renders them as JSON numbers
//!   (`encode_big_decimal_as_string = false`), so the port casts them to
//!   `float8`.
//! - The timestamps are rendered like Canvas does with
//!   `ActiveSupport::JSON::Encoding.time_precision = 0`:
//!   `2024-05-06T07:08:09Z`.
//! - `ConditionalRelease::Deletion` is a soft delete: `destroy` only stamps
//!   `deleted_at`, it never cascades to the scoring ranges, the assignment
//!   sets or the associations.
//! - `acts_as_list` gives a new scoring range / assignment set / association
//!   the position `max(position) + 1` of its siblings when the request does
//!   not carry one; on an update the positions are renumbered `1..n` in the
//!   order of the request (Canvas `add_ordering_to`).
//! - `ScoringRange#assignment_sets` builds an empty assignment set when the
//!   range has none, so the JSON of such a range carries an empty set.
//! - A failed validation answers `400` with the plain
//!   `rule.errors.as_json` hash (attribute => [messages]); the nested
//!   attributes are prefixed the way Rails composes them
//!   (`scoring_ranges.assignment_sets.assignment_set_associations.<attr>`).
//! - The request body is the flat parameter hash of the Canvas controller
//!   (`trigger_assignment_id`, `scoring_ranges`, ...). The query string wins
//!   over the body, like Rails' `request_parameters.merge(query_parameters)`.
//! - An explicit `null` in the request clears the attribute it belongs to
//!   (`trigger_assignment_id`, `lower_bound`, `upper_bound`,
//!   `assignment_id`), exactly like the Rails attribute assignment; a key
//!   the request does not carry leaves the attribute alone.
//! - Canvas permits a `position` on the rule itself, but the rule has no
//!   such column; the port ignores it instead of raising.

use std::collections::HashMap;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::NaiveDateTime;
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// The `now()` of the database, in UTC. The conditional release tables store
/// plain `timestamp` columns, so the value is written as UTC.
const NOW: &str = "now() AT TIME ZONE 'UTC'";

const RULE_COLUMNS: &str = "id, course_id, trigger_assignment_id, created_at, updated_at";
const RANGE_COLUMNS: &str = "id, rule_id, lower_bound::float8 AS lower_bound, \
     upper_bound::float8 AS upper_bound, position, created_at, updated_at";
const SET_COLUMNS: &str = "id, scoring_range_id, position, created_at, updated_at";
const ASSOCIATION_COLUMNS: &str =
    "id, assignment_set_id, assignment_id, position, created_at, updated_at";

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// One `conditional_release_rules` row.
#[derive(Debug, Clone, sqlx::FromRow)]
struct RuleRow {
    id: i64,
    course_id: i64,
    trigger_assignment_id: i64,
    created_at: NaiveDateTime,
    updated_at: NaiveDateTime,
}

/// One `conditional_release_scoring_ranges` row.
#[derive(Debug, Clone, sqlx::FromRow)]
struct RangeRow {
    id: i64,
    rule_id: i64,
    lower_bound: Option<f64>,
    upper_bound: Option<f64>,
    position: Option<i32>,
    created_at: NaiveDateTime,
    updated_at: NaiveDateTime,
}

/// One `conditional_release_assignment_sets` row.
#[derive(Debug, Clone, sqlx::FromRow)]
struct SetRow {
    id: i64,
    scoring_range_id: i64,
    position: Option<i32>,
    created_at: NaiveDateTime,
    updated_at: NaiveDateTime,
}

/// One `conditional_release_assignment_set_associations` row.
#[derive(Debug, Clone, sqlx::FromRow)]
struct AssociationRow {
    id: i64,
    assignment_set_id: i64,
    assignment_id: i64,
    position: Option<i32>,
    created_at: NaiveDateTime,
    updated_at: NaiveDateTime,
}

/// A scoring range with its assignment sets.
#[derive(Debug, Clone)]
struct RangeTree {
    row: RangeRow,
    sets: Vec<SetTree>,
}

/// An assignment set with its associations.
#[derive(Debug, Clone)]
struct SetTree {
    row: SetRow,
    associations: Vec<AssociationRow>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// The Canvas `rule.errors` hash: attribute => [messages], in the order the
/// validations ran.
#[derive(Debug, Default)]
struct ErrorBag {
    entries: Vec<(String, Vec<String>)>,
}

impl ErrorBag {
    /// `errors.add(attribute, message)`.
    fn add(&mut self, attribute: &str, message: &str) {
        if let Some((_, messages)) = self.entries.iter_mut().find(|(key, _)| key == attribute) {
            messages.push(message.to_string());
            return;
        }
        self.entries
            .push((attribute.to_string(), vec![message.to_string()]));
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `rule.errors.as_json`.
    fn to_json(&self) -> Value {
        let mut map = Map::new();
        for (attribute, messages) in &self.entries {
            map.insert(attribute.clone(), json!(messages));
        }
        Value::Object(map)
    }
}

/// The Canvas answer of a failed validation: `render json: rule.errors,
/// status: :bad_request`.
fn bad_request(errors: &ErrorBag) -> Response {
    (StatusCode::BAD_REQUEST, Json(errors.to_json())).into_response()
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/// The permitted rule parameters.
#[derive(Debug, Default, Clone)]
struct RuleParams {
    /// The raw `trigger_assignment_id`; `None` when the request did not
    /// carry the key at all, `Some(Value::Null)` when the request carried an
    /// explicit `null` (Rails assigns that `nil` to the attribute).
    trigger_assignment_id: Option<Value>,
    /// True when the request carried a `scoring_ranges` array.
    scoring_ranges_given: bool,
    scoring_ranges: Vec<RangeParams>,
}

/// The permitted scoring range parameters.
#[derive(Debug, Default, Clone)]
struct RangeParams {
    id: Option<i64>,
    lower_bound: Option<f64>,
    upper_bound: Option<f64>,
    /// True when the request carried the `lower_bound` key, even when the
    /// value was an explicit `null` (Rails clears the attribute then).
    lower_bound_given: bool,
    /// True when the request carried the `upper_bound` key, even when the
    /// value was an explicit `null`.
    upper_bound_given: bool,
    position: Option<i32>,
    /// True when the request carried an `assignment_sets` array.
    assignment_sets_given: bool,
    assignment_sets: Vec<SetParams>,
}

/// The permitted assignment set parameters.
#[derive(Debug, Default, Clone)]
struct SetParams {
    id: Option<i64>,
    position: Option<i32>,
    /// True when the request carried an `assignment_set_associations` array.
    associations_given: bool,
    associations: Vec<AssociationParams>,
}

/// The permitted assignment set association parameters.
#[derive(Debug, Default, Clone)]
struct AssociationParams {
    id: Option<i64>,
    assignment_id: Option<i64>,
    /// True when the request carried the `assignment_id` key, even when the
    /// value was an explicit `null`.
    assignment_id_given: bool,
    position: Option<i32>,
}

/// Rails casts an integer column from the request: a value that is not a
/// number becomes 0 (`"doh".to_i`).
fn cast_integer(value: &Value) -> i64 {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|value| value.trunc() as i64))
            .unwrap_or(0),
        Value::String(text) => leading_integer(text),
        Value::Bool(flag) => i64::from(*flag),
        _ => 0,
    }
}

/// `String#to_i`: the leading integer of the string, 0 when there is none.
fn leading_integer(text: &str) -> i64 {
    let trimmed = text.trim();
    let mut digits = String::new();
    for ch in trimmed.chars() {
        if ch.is_ascii_digit() || (digits.is_empty() && (ch == '-' || ch == '+')) {
            digits.push(ch);
        } else {
            break;
        }
    }
    digits.parse::<i64>().unwrap_or(0)
}

/// Rails casts a decimal column from the request: a value that is not a
/// number becomes nil.
fn cast_decimal(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        Value::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// The integer of a request parameter, when the key is present.
fn json_integer(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Null) | None => None,
        Some(value) => Some(cast_integer(value)),
    }
}

/// The decimal of a request parameter, when the key is present.
fn json_decimal(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Null) | None => None,
        Some(value) => cast_decimal(value),
    }
}

/// The rule parameters of the request.
fn parse_rule_params(body: &Value) -> RuleParams {
    let mut params = RuleParams::default();
    if let Some(value) = body.get("trigger_assignment_id") {
        params.trigger_assignment_id = Some(value.clone());
    }
    if let Some(ranges) = body.get("scoring_ranges").and_then(Value::as_array) {
        params.scoring_ranges_given = true;
        for range in ranges.iter().filter_map(Value::as_object) {
            params.scoring_ranges.push(parse_range_params(range));
        }
    }
    params
}

/// The scoring range parameters of the request.
fn parse_range_params(range: &Map<String, Value>) -> RangeParams {
    let mut params = RangeParams {
        id: json_integer(range.get("id")),
        lower_bound: json_decimal(range.get("lower_bound")),
        upper_bound: json_decimal(range.get("upper_bound")),
        lower_bound_given: range.contains_key("lower_bound"),
        upper_bound_given: range.contains_key("upper_bound"),
        position: json_integer(range.get("position")).map(|value| value as i32),
        ..RangeParams::default()
    };
    if let Some(sets) = range.get("assignment_sets").and_then(Value::as_array) {
        params.assignment_sets_given = true;
        for set in sets.iter().filter_map(Value::as_object) {
            params.assignment_sets.push(parse_set_params(set));
        }
    }
    params
}

/// The assignment set parameters of the request.
fn parse_set_params(set: &Map<String, Value>) -> SetParams {
    let mut params = SetParams {
        id: json_integer(set.get("id")),
        position: json_integer(set.get("position")).map(|value| value as i32),
        ..SetParams::default()
    };
    if let Some(associations) = set
        .get("assignment_set_associations")
        .and_then(Value::as_array)
    {
        params.associations_given = true;
        for association in associations.iter().filter_map(Value::as_object) {
            params
                .associations
                .push(parse_association_params(association));
        }
    }
    params
}

/// The assignment set association parameters of the request.
fn parse_association_params(association: &Map<String, Value>) -> AssociationParams {
    AssociationParams {
        id: json_integer(association.get("id")),
        assignment_id: json_integer(association.get("assignment_id")),
        assignment_id_given: association.contains_key("assignment_id"),
        position: json_integer(association.get("position")).map(|value| value as i32),
    }
}

/// The effective parameters of the request.
///
/// Rails merges the request body and the query string and lets the query
/// string win (`request_parameters.merge(query_parameters)`). Only the
/// scalar keys of the controller are read from the query string, because the
/// nested collections cannot be expressed there.
fn effective_params(body: &Value, query: &HashMap<String, String>) -> Value {
    let mut merged = match body {
        Value::Object(map) => map.clone(),
        _ => Map::new(),
    };
    if let Some(value) = query.get("trigger_assignment_id") {
        merged.insert(
            "trigger_assignment_id".to_string(),
            Value::String(value.clone()),
        );
    }
    Value::Object(merged)
}

/// The parsed request body. A body that is not a JSON object is an empty
/// parameter hash, like a request without a body.
fn parse_body(body: &Bytes) -> Value {
    serde_json::from_slice::<Value>(body).unwrap_or(Value::Object(Map::new()))
}

// ---------------------------------------------------------------------------
// Context, permissions
// ---------------------------------------------------------------------------

/// A path id. Canvas casts the path segment to an integer, so a value that
/// is not a number is a missing record (404).
fn parse_id(raw: &str) -> Result<i64, ApiError> {
    let id = leading_integer(raw);
    if id == 0 {
        Err(ApiError::NotFound)
    } else {
        Ok(id)
    }
}

/// Canvas `get_context`: the course must exist (404).
async fn load_course(db: &PgPool, course_id: i64) -> Result<(), ApiError> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM courses WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(course_id)
            .fetch_optional(db)
            .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Canvas `authorized_action(@context, @current_user, :read)`.
///
/// The caller has to be enrolled in the course or administrate the account
/// the course belongs to.
async fn require_course_read(db: &PgPool, auth: &AuthUser, course_id: i64) -> Result<(), ApiError> {
    if auth.role == "admin" {
        return Ok(());
    }
    let enrolled: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments \
         WHERE course_id = $1 AND user_id = $2 AND workflow_state = 'active' LIMIT 1",
    )
    .bind(course_id)
    .bind(auth.id)
    .fetch_optional(db)
    .await?;
    if enrolled.is_some() {
        return Ok(());
    }

    let account: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT account_id FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(db)
            .await?;
    if let Some((Some(account_id),)) = account {
        let admin: Option<(i32,)> = sqlx::query_as(
            "SELECT 1 FROM account_users WHERE account_id = $1 AND user_id = $2 AND role = 'admin' \
             LIMIT 1",
        )
        .bind(account_id)
        .bind(auth.id)
        .fetch_optional(db)
        .await?;
        if admin.is_some() {
            return Ok(());
        }
    }
    Err(ApiError::Forbidden)
}

/// Canvas `authorized_action(@context, @current_user, :manage_assignments_edit)`
/// and `:manage_assignments_add`.
///
/// Canvas grants `manage_assignments` to the teacher, the TA and the
/// designer of a course, so both rights collapse into the same set.
async fn require_course_manage_assignments(
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
           AND role IN ('teacher', 'ta', 'designer') LIMIT 1",
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

/// Canvas `course.root_account_id`: the account at the top of the account
/// chain of the course.
async fn course_root_account_id(db: &PgPool, course_id: i64) -> Result<i64, ApiError> {
    let mut account_id: Option<i64> =
        sqlx::query_scalar("SELECT account_id FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(db)
            .await?
            .flatten();
    for _ in 0..16 {
        let Some(id) = account_id else { break };
        let parent: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
                .bind(id)
                .fetch_optional(db)
                .await?;
        match parent {
            Some((None,)) => return Ok(id),
            Some((Some(parent_id),)) => account_id = Some(parent_id),
            None => return Ok(id),
        }
    }
    // Canvas always has a root account; the port seeds it with the id 1.
    Ok(account_id.unwrap_or(1))
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Canvas `get_rule`: the active rule of the course (404).
async fn load_rule(db: &PgPool, course_id: i64, rule_id: i64) -> Result<RuleRow, ApiError> {
    let sql = format!(
        "SELECT {RULE_COLUMNS} FROM conditional_release_rules \
         WHERE id = $1 AND course_id = $2 AND deleted_at IS NULL"
    );
    let rule: Option<RuleRow> = sqlx::query_as(&sql)
        .bind(rule_id)
        .bind(course_id)
        .fetch_optional(db)
        .await?;
    rule.ok_or(ApiError::NotFound)
}

/// The active scoring ranges of a rule, ordered by position.
async fn load_ranges(db: &PgPool, rule_id: i64) -> Result<Vec<RangeRow>, ApiError> {
    let sql = format!(
        "SELECT {RANGE_COLUMNS} FROM conditional_release_scoring_ranges \
         WHERE rule_id = $1 AND deleted_at IS NULL ORDER BY position ASC, id ASC"
    );
    Ok(sqlx::query_as(&sql).bind(rule_id).fetch_all(db).await?)
}

/// The active assignment sets of a scoring range, ordered by position.
async fn load_sets(db: &PgPool, range_id: i64) -> Result<Vec<SetRow>, ApiError> {
    let sql = format!(
        "SELECT {SET_COLUMNS} FROM conditional_release_assignment_sets \
         WHERE scoring_range_id = $1 AND deleted_at IS NULL ORDER BY position ASC, id ASC"
    );
    Ok(sqlx::query_as(&sql).bind(range_id).fetch_all(db).await?)
}

/// The active associations of an assignment set, ordered by position.
async fn load_associations(db: &PgPool, set_id: i64) -> Result<Vec<AssociationRow>, ApiError> {
    let sql = format!(
        "SELECT {ASSOCIATION_COLUMNS} FROM conditional_release_assignment_set_associations \
         WHERE assignment_set_id = $1 AND deleted_at IS NULL ORDER BY position ASC, id ASC"
    );
    Ok(sqlx::query_as(&sql).bind(set_id).fetch_all(db).await?)
}

/// The scoring ranges of a rule with their assignment sets and associations.
async fn load_range_tree(db: &PgPool, rule_id: i64) -> Result<Vec<RangeTree>, ApiError> {
    let mut trees = Vec::new();
    for row in load_ranges(db, rule_id).await? {
        let mut sets = Vec::new();
        for set in load_sets(db, row.id).await? {
            let associations = load_associations(db, set.id).await?;
            sets.push(SetTree {
                row: set,
                associations,
            });
        }
        trees.push(RangeTree { row, sets });
    }
    Ok(trees)
}

/// The course of an assignment (`assignment.context_id`).
async fn assignment_course(db: &PgPool, assignment_id: i64) -> Result<Option<i64>, ApiError> {
    Ok(
        sqlx::query_scalar("SELECT course_id FROM assignments WHERE id = $1")
            .bind(assignment_id)
            .fetch_optional(db)
            .await?,
    )
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// The effective trigger assignment of the request.
///
/// A create has no trigger until the request carries one; an update keeps
/// the trigger of the rule when the request does not name one. An explicit
/// `null` clears the trigger, exactly like the Rails attribute assignment.
fn effective_trigger(params: &RuleParams, current: Option<i64>) -> Option<i64> {
    match &params.trigger_assignment_id {
        Some(Value::Null) => None,
        Some(value) => Some(cast_integer(value)),
        None => current,
    }
}

/// The validations of the rule itself.
///
/// Canvas `ConditionalRelease::Rule`:
/// - `validates :course_id, presence: true`
/// - `validates :trigger_assignment_id, presence: true`
/// - `validate :trigger_assignment_in_same_course`
/// - `belongs_to :trigger_assignment` (the assignment must exist)
///
/// The order is the order of the validations in the model: the explicit
/// validations run first, the `belongs_to` validation last.
async fn validate_rule(
    db: &PgPool,
    course_id: i64,
    trigger: Option<i64>,
    trigger_changed: bool,
) -> Result<ErrorBag, ApiError> {
    let mut errors = ErrorBag::default();
    let Some(trigger_id) = trigger else {
        errors.add("trigger_assignment_id", "can't be blank");
        errors.add("trigger_assignment", "must exist");
        return Ok(errors);
    };

    let trigger_course = assignment_course(db, trigger_id).await?;
    // `trigger_assignment&.context_id != course_id`
    if trigger_changed && trigger_course != Some(course_id) {
        errors.add("trigger_assignment_id", "invalid trigger assignment");
    }
    // `belongs_to :trigger_assignment`
    if trigger_course.is_none() {
        errors.add("trigger_assignment", "must exist");
    }
    Ok(errors)
}

/// The validations of a scoring range.
///
/// Canvas `ConditionalRelease::BoundsValidations`:
/// - `lower_bound` and `upper_bound` are `>= 0`
/// - `lower_bound <= upper_bound`
/// - at least one of the two bounds exists
fn validate_range(range: &RangePlan, errors: &mut ErrorBag) {
    if range.lower_bound.is_some_and(|value| value < 0.0) {
        errors.add(
            "scoring_ranges.lower_bound",
            "must be greater than or equal to 0",
        );
    }
    if range.upper_bound.is_some_and(|value| value < 0.0) {
        errors.add(
            "scoring_ranges.upper_bound",
            "must be greater than or equal to 0",
        );
    }
    if let (Some(lower), Some(upper)) = (range.lower_bound, range.upper_bound) {
        if lower > upper {
            errors.add(
                "scoring_ranges.base",
                "lower bound must be less than upper bound",
            );
        }
    }
    if range.lower_bound.is_none() && range.upper_bound.is_none() {
        errors.add("scoring_ranges.base", "one bound must exist");
    }
}

/// The validations of an assignment set association.
///
/// Canvas `ConditionalRelease::AssignmentSetAssociation`:
/// - `validates :assignment_id, presence: true`
/// - `validates :assignment_id, uniqueness: { scope: :assignment_set_id }`
/// - `validate :not_trigger`
/// - `validate :assignment_in_same_course`
/// - `belongs_to :assignment` (the assignment must exist)
///
/// The order is the order of the validations in the model.
async fn validate_association(
    db: &PgPool,
    association: &AssociationPlan,
    course_id: i64,
    trigger_id: Option<i64>,
    set_id: Option<i64>,
) -> Result<ErrorBag, ApiError> {
    let mut errors = ErrorBag::default();
    let prefix = "scoring_ranges.assignment_sets.assignment_set_associations";
    let Some(assignment_id) = association.assignment_id else {
        // `validates :assignment_id, presence: true`
        errors.add(&format!("{prefix}.assignment_id"), "can't be blank");
        // `validate :not_trigger`
        if trigger_id.is_none() {
            errors.add(
                &format!("{prefix}.assignment_id"),
                "can't match rule trigger_assignment_id",
            );
        }
        // `validate :assignment_in_same_course`
        if association.assignment_changed {
            errors.add(&format!("{prefix}.assignment_id"), "invalid assignment");
        }
        // `belongs_to :assignment`
        errors.add(&format!("{prefix}.assignment"), "must exist");
        return Ok(errors);
    };

    // `validates :assignment_id, uniqueness: { scope: :assignment_set_id }`
    if let Some(set_id) = set_id {
        let taken: Option<(i32,)> = sqlx::query_as(
            "SELECT 1 FROM conditional_release_assignment_set_associations \
             WHERE assignment_set_id = $1 AND assignment_id = $2 AND deleted_at IS NULL \
               AND ($3::bigint IS NULL OR id <> $3) LIMIT 1",
        )
        .bind(set_id)
        .bind(assignment_id)
        .bind(association.id)
        .fetch_optional(db)
        .await?;
        if taken.is_some() {
            errors.add(&format!("{prefix}.assignment_id"), "has already been taken");
        }
    }
    // `validate :not_trigger`
    if Some(assignment_id) == trigger_id {
        errors.add(
            &format!("{prefix}.assignment_id"),
            "can't match rule trigger_assignment_id",
        );
    }
    let assignment_course = assignment_course(db, assignment_id).await?;
    // `assignment_id_changed? && assignment&.context_id != course_id`
    if association.assignment_changed && assignment_course != Some(course_id) {
        errors.add(&format!("{prefix}.assignment_id"), "invalid assignment");
    }
    // `belongs_to :assignment`
    if assignment_course.is_none() {
        errors.add(&format!("{prefix}.assignment"), "must exist");
    }
    Ok(errors)
}

// ---------------------------------------------------------------------------
// Plans
// ---------------------------------------------------------------------------

/// A scoring range the request asks for.
#[derive(Debug, Clone)]
struct RangePlan {
    /// The id of the existing row, `None` for a new range.
    id: Option<i64>,
    lower_bound: Option<f64>,
    upper_bound: Option<f64>,
    position: i32,
    /// True when the request changes the range.
    changed: bool,
    /// The assignment sets of the range, when the request carried them.
    sets: Option<Vec<SetPlan>>,
    /// The ids of the active sets of the existing range.
    existing_set_ids: Vec<i64>,
}

/// An assignment set the request asks for.
#[derive(Debug, Clone)]
struct SetPlan {
    /// The id of the existing row, `None` for a new set.
    id: Option<i64>,
    position: i32,
    /// True when the request changes the set.
    changed: bool,
    /// The associations of the set, when the request carried them.
    associations: Option<Vec<AssociationPlan>>,
    /// The ids of the active associations of the existing set.
    existing_association_ids: Vec<i64>,
}

/// An assignment set association the request asks for.
#[derive(Debug, Clone)]
struct AssociationPlan {
    /// The id of the existing row, `None` for a new association.
    id: Option<i64>,
    assignment_id: Option<i64>,
    /// True when the request changes the assignment of the association.
    assignment_changed: bool,
    position: i32,
    /// True when the request changes the association.
    changed: bool,
}

/// The plan of the scoring ranges of a request.
struct RulePlan {
    ranges: Option<Vec<RangePlan>>,
    /// The ids of the active ranges of the rule.
    existing_range_ids: Vec<i64>,
}

/// Build the plan of an update.
///
/// The positions of the request win: Canvas renumbers the ranges, the sets
/// and the associations `1..n` in the order of the request
/// (`add_ordering_to`). A range, set or association the request does not
/// mention is destroyed.
fn plan_update(params: &RuleParams, existing: &[RangeTree]) -> Result<RulePlan, ApiError> {
    let existing_range_ids = existing.iter().map(|tree| tree.row.id).collect();
    if !params.scoring_ranges_given {
        return Ok(RulePlan {
            ranges: None,
            existing_range_ids,
        });
    }

    let mut ranges = Vec::with_capacity(params.scoring_ranges.len());
    for (index, param) in params.scoring_ranges.iter().enumerate() {
        let position = index as i32 + 1;
        let existing_range = match param.id {
            Some(id) => Some(
                existing
                    .iter()
                    .find(|tree| tree.row.id == id)
                    .ok_or(ApiError::NotFound)?,
            ),
            None => None,
        };

        let lower_bound = if param.lower_bound_given {
            param.lower_bound
        } else {
            existing_range.and_then(|tree| tree.row.lower_bound)
        };
        let upper_bound = if param.upper_bound_given {
            param.upper_bound
        } else {
            existing_range.and_then(|tree| tree.row.upper_bound)
        };
        let changed = match existing_range {
            Some(tree) => {
                tree.row.lower_bound != lower_bound
                    || tree.row.upper_bound != upper_bound
                    || tree.row.position != Some(position)
            }
            None => true,
        };

        let mut existing_set_ids = Vec::new();
        let mut sets = None;
        if param.assignment_sets_given {
            let existing_sets = existing_range.map(|tree| &tree.sets);
            let mut planned = Vec::with_capacity(param.assignment_sets.len());
            for (set_index, set_param) in param.assignment_sets.iter().enumerate() {
                let set_position = set_index as i32 + 1;
                let existing_set = match set_param.id {
                    Some(id) => Some(
                        existing_sets
                            .and_then(|sets| sets.iter().find(|set| set.row.id == id))
                            .ok_or(ApiError::NotFound)?,
                    ),
                    None => None,
                };
                let set_changed = match existing_set {
                    Some(set) => set.row.position != Some(set_position),
                    None => true,
                };

                let mut existing_association_ids = Vec::new();
                let mut associations = None;
                if set_param.associations_given {
                    let existing_associations: &[AssociationRow] = match existing_set {
                        Some(set) => &set.associations,
                        None => &[],
                    };
                    let mut planned_associations = Vec::with_capacity(set_param.associations.len());
                    for (assoc_index, assoc_param) in set_param.associations.iter().enumerate() {
                        let assoc_position = assoc_index as i32 + 1;
                        let existing_association = match assoc_param.id {
                            Some(id) => Some(
                                existing_associations
                                    .iter()
                                    .find(|assoc| assoc.id == id)
                                    .ok_or(ApiError::NotFound)?,
                            ),
                            None => None,
                        };
                        // The request only carries the assignment when it
                        // changes it, an existing association keeps its own.
                        // An explicit `null` clears the assignment, like the
                        // Rails attribute assignment.
                        let assignment_id = if assoc_param.assignment_id_given {
                            assoc_param.assignment_id
                        } else {
                            existing_association.map(|assoc| assoc.assignment_id)
                        };
                        let assignment_changed = match (
                            assignment_id,
                            existing_association.map(|assoc| assoc.assignment_id),
                        ) {
                            (Some(new_id), Some(old_id)) => new_id != old_id,
                            (Some(_), None) => true,
                            (None, Some(_)) => true,
                            (None, None) => false,
                        };
                        let assoc_changed = match existing_association {
                            Some(assoc) => {
                                assignment_id != Some(assoc.assignment_id)
                                    || assoc.position != Some(assoc_position)
                            }
                            None => true,
                        };
                        planned_associations.push(AssociationPlan {
                            id: assoc_param.id,
                            assignment_id,
                            assignment_changed,
                            position: assoc_position,
                            changed: assoc_changed,
                        });
                    }
                    existing_association_ids =
                        existing_associations.iter().map(|assoc| assoc.id).collect();
                    associations = Some(planned_associations);
                } else if let Some(set) = existing_set {
                    existing_association_ids =
                        set.associations.iter().map(|assoc| assoc.id).collect();
                }
                planned.push(SetPlan {
                    id: set_param.id,
                    position: set_position,
                    changed: set_changed,
                    associations,
                    existing_association_ids,
                });
            }
            if let Some(sets) = existing_sets {
                existing_set_ids = sets.iter().map(|set| set.row.id).collect();
            }
            sets = Some(planned);
        } else if let Some(range) = existing_range {
            existing_set_ids = range.sets.iter().map(|set| set.row.id).collect();
        }

        ranges.push(RangePlan {
            id: param.id,
            lower_bound,
            upper_bound,
            position,
            changed,
            sets,
            existing_set_ids,
        });
    }

    Ok(RulePlan {
        ranges: Some(ranges),
        existing_range_ids,
    })
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Canvas renders a timestamp as RFC3339 with seconds and a `Z` suffix
/// (`ActiveSupport::JSON::Encoding.time_precision = 0`).
fn ts(value: &NaiveDateTime) -> String {
    value.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// The `as_json` of a rule.
fn rule_json(rule: &RuleRow) -> Value {
    json!({
        "id": rule.id,
        "course_id": rule.course_id,
        "trigger_assignment_id": rule.trigger_assignment_id,
        "created_at": ts(&rule.created_at),
        "updated_at": ts(&rule.updated_at),
    })
}

/// The `as_json` of a scoring range.
fn range_json(range: &RangeRow) -> Value {
    json!({
        "id": range.id,
        "rule_id": range.rule_id,
        "lower_bound": range.lower_bound,
        "upper_bound": range.upper_bound,
        "position": range.position,
        "created_at": ts(&range.created_at),
        "updated_at": ts(&range.updated_at),
    })
}

/// The `as_json` of an assignment set.
fn set_json(set: &SetRow) -> Value {
    json!({
        "id": set.id,
        "scoring_range_id": set.scoring_range_id,
        "position": set.position,
        "created_at": ts(&set.created_at),
        "updated_at": ts(&set.updated_at),
    })
}

/// The `as_json` of an assignment set association.
fn association_json(association: &AssociationRow) -> Value {
    json!({
        "id": association.id,
        "assignment_set_id": association.assignment_set_id,
        "assignment_id": association.assignment_id,
        "position": association.position,
        "created_at": ts(&association.created_at),
        "updated_at": ts(&association.updated_at),
    })
}

/// The `as_json` of the assignment set `ScoringRange#assignment_sets` builds
/// when the range has none: an unsaved set of the range.
fn empty_set_json(range: &RangeRow) -> Value {
    json!({
        "id": Value::Null,
        "scoring_range_id": range.id,
        "position": Value::Null,
        "created_at": Value::Null,
        "updated_at": Value::Null,
        "assignment_set_associations": [],
    })
}

/// The `as_json` of a rule with its nested collections
/// (`Rule.includes_for_json`).
async fn rule_with_children(db: &PgPool, rule: &RuleRow) -> Result<Value, ApiError> {
    let mut rendered = rule_json(rule);
    let mut ranges = Vec::new();
    for tree in load_range_tree(db, rule.id).await? {
        let mut range = range_json(&tree.row);
        let mut sets = Vec::new();
        for set in &tree.sets {
            let mut set_value = set_json(&set.row);
            let associations: Vec<Value> = set.associations.iter().map(association_json).collect();
            if let Value::Object(map) = &mut set_value {
                map.insert(
                    "assignment_set_associations".to_string(),
                    Value::Array(associations),
                );
            }
            sets.push(set_value);
        }
        if sets.is_empty() {
            sets.push(empty_set_json(&tree.row));
        }
        if let Value::Object(map) = &mut range {
            map.insert("assignment_sets".to_string(), Value::Array(sets));
        }
        ranges.push(range);
    }
    if let Value::Object(map) = &mut rendered {
        map.insert("scoring_ranges".to_string(), Value::Array(ranges));
    }
    Ok(rendered)
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// The Canvas conditional release rule routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{courseId}/mastery_paths/rules",
            get(index).post(create),
        )
        .route(
            "/courses/{courseId}/mastery_paths/rules/{id}",
            get(show).put(update).delete(destroy),
        )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Canvas `ConditionalRelease::RulesController#index`.
async fn index(
    State(state): State<AppState>,
    Path(course_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let course_id = parse_id(&course_id)?;
    // `get_context`
    load_course(&state.db, course_id).await?;
    // `require_course_view_permissions`
    require_course_read(&state.db, &auth, course_id).await?;

    let trigger_assignment_id = query
        .get("trigger_assignment_id")
        .filter(|value| !value.trim().is_empty())
        .map(|value| leading_integer(value));
    let with_assignments = value_to_boolean(query.get("active"));
    let with_children = include_all(&query);

    let sql = format!(
        "SELECT {RULE_COLUMNS} FROM conditional_release_rules \
         WHERE course_id = $1 AND deleted_at IS NULL \
           AND ($2::bigint IS NULL OR trigger_assignment_id = $2) \
           AND ($3 = false OR EXISTS ( \
                 SELECT 1 FROM conditional_release_scoring_ranges sr \
                 JOIN conditional_release_assignment_sets s \
                   ON s.scoring_range_id = sr.id AND s.deleted_at IS NULL \
                 JOIN conditional_release_assignment_set_associations a \
                   ON a.assignment_set_id = s.id AND a.deleted_at IS NULL \
                 WHERE sr.rule_id = conditional_release_rules.id AND sr.deleted_at IS NULL)) \
         ORDER BY id ASC"
    );
    let rules: Vec<RuleRow> = sqlx::query_as(&sql)
        .bind(course_id)
        .bind(trigger_assignment_id)
        .bind(with_assignments)
        .fetch_all(&state.db)
        .await?;

    let mut rendered = Vec::with_capacity(rules.len());
    for rule in &rules {
        if with_children {
            rendered.push(rule_with_children(&state.db, rule).await?);
        } else {
            rendered.push(rule_json(rule));
        }
    }
    Ok(Json(rendered).into_response())
}

/// Canvas `ConditionalRelease::RulesController#show`.
async fn show(
    State(state): State<AppState>,
    Path((course_id, rule_id)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let course_id = parse_id(&course_id)?;
    let rule_id = parse_id(&rule_id)?;
    // `get_context`
    load_course(&state.db, course_id).await?;
    // `require_course_view_permissions`
    require_course_read(&state.db, &auth, course_id).await?;

    let rule = load_rule(&state.db, course_id, rule_id).await?;
    let rendered = if include_all(&query) {
        rule_with_children(&state.db, &rule).await?
    } else {
        rule_json(&rule)
    };
    Ok(Json(rendered).into_response())
}

/// Canvas `ConditionalRelease::RulesController#create`.
async fn create(
    State(state): State<AppState>,
    Path(course_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Response, ApiError> {
    let course_id = parse_id(&course_id)?;
    // `get_context`
    load_course(&state.db, course_id).await?;
    // `require_course_assignment_add_or_edit_permissions`
    require_course_manage_assignments(&state.db, &auth, course_id).await?;
    // `require_course_view_permissions`
    require_course_read(&state.db, &auth, course_id).await?;

    let params = parse_rule_params(&effective_params(&parse_body(&body), &query));
    let trigger = effective_trigger(&params, None);
    // A new record only counts the trigger as changed when it carries a
    // value; assigning `nil` to a new record is not a change.
    let trigger_changed = matches!(&params.trigger_assignment_id, Some(value) if !value.is_null());

    let mut errors = validate_rule(&state.db, course_id, trigger, trigger_changed).await?;
    let plan = RulePlan {
        ranges: Some(create_plans(&params)),
        existing_range_ids: Vec::new(),
    };
    validate_plan(&state.db, course_id, trigger, &plan, &mut errors).await?;
    if !errors.is_empty() {
        return Ok(bad_request(&errors));
    }
    // The rule is valid, so the trigger assignment exists.
    let trigger_assignment_id = trigger.unwrap_or_default();

    let root_account_id = course_root_account_id(&state.db, course_id).await?;
    let mut tx = state.db.begin().await?;

    let rule_id: i64 = sqlx::query_scalar(&format!(
        "INSERT INTO conditional_release_rules \
           (course_id, trigger_assignment_id, root_account_id, created_at, updated_at) \
         VALUES ($1, $2, $3, {NOW}, {NOW}) RETURNING id"
    ))
    .bind(course_id)
    .bind(trigger_assignment_id)
    .bind(root_account_id)
    .fetch_one(&mut *tx)
    .await?;

    insert_ranges(&mut tx, rule_id, root_account_id, &plan).await?;

    tx.commit().await?;

    let rule = load_rule(&state.db, course_id, rule_id).await?;
    Ok(Json(rule_with_children(&state.db, &rule).await?).into_response())
}

/// Canvas `ConditionalRelease::RulesController#update`.
async fn update(
    State(state): State<AppState>,
    Path((course_id, rule_id)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Response, ApiError> {
    let course_id = parse_id(&course_id)?;
    let rule_id = parse_id(&rule_id)?;
    // `get_context`
    load_course(&state.db, course_id).await?;
    // `require_course_assignment_edit_permissions`
    require_course_manage_assignments(&state.db, &auth, course_id).await?;
    // `require_course_view_permissions`
    require_course_read(&state.db, &auth, course_id).await?;

    // `get_rule` — the rule is loaded in the action, after the permissions.
    let rule = load_rule(&state.db, course_id, rule_id).await?;

    let params = parse_rule_params(&effective_params(&parse_body(&body), &query));
    let trigger = effective_trigger(&params, Some(rule.trigger_assignment_id));
    let trigger_changed = match &params.trigger_assignment_id {
        // The request does not mention the trigger, it stays as it is.
        None => false,
        // An explicit `null` clears the trigger.
        Some(Value::Null) => true,
        Some(value) => cast_integer(value) != rule.trigger_assignment_id,
    };

    let mut errors = validate_rule(&state.db, course_id, trigger, trigger_changed).await?;
    let existing = load_range_tree(&state.db, rule.id).await?;
    let plan = plan_update(&params, &existing)?;
    validate_plan(&state.db, course_id, trigger, &plan, &mut errors).await?;
    if !errors.is_empty() {
        return Ok(bad_request(&errors));
    }
    let trigger_assignment_id = trigger.unwrap_or_default();

    let root_account_id = course_root_account_id(&state.db, course_id).await?;
    let mut tx = state.db.begin().await?;

    if trigger_changed {
        sqlx::query(&format!(
            "UPDATE conditional_release_rules SET trigger_assignment_id = $1, updated_at = {NOW} \
             WHERE id = $2"
        ))
        .bind(trigger_assignment_id)
        .bind(rule.id)
        .execute(&mut *tx)
        .await?;
    }

    apply_plan(&mut tx, rule.id, root_account_id, &plan).await?;
    tx.commit().await?;

    let rule = load_rule(&state.db, course_id, rule_id).await?;
    Ok(Json(rule_with_children(&state.db, &rule).await?).into_response())
}

/// Canvas `ConditionalRelease::RulesController#destroy`.
async fn destroy(
    State(state): State<AppState>,
    Path((course_id, rule_id)): Path<(String, String)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let course_id = parse_id(&course_id)?;
    let rule_id = parse_id(&rule_id)?;
    // `get_context`
    load_course(&state.db, course_id).await?;
    // `require_course_assignment_edit_permissions`
    require_course_manage_assignments(&state.db, &auth, course_id).await?;
    // `require_course_view_permissions`
    require_course_read(&state.db, &auth, course_id).await?;

    // `get_rule`
    let rule = load_rule(&state.db, course_id, rule_id).await?;

    // `ConditionalRelease::Deletion#destroy` is a soft delete.
    sqlx::query(&format!(
        "UPDATE conditional_release_rules SET deleted_at = {NOW}, updated_at = {NOW} WHERE id = $1"
    ))
    .bind(rule.id)
    .execute(&state.db)
    .await?;

    Ok(Json(json!({ "success": true })).into_response())
}

// ---------------------------------------------------------------------------
// Query parameters
// ---------------------------------------------------------------------------

/// Canvas `Canvas::Plugin.value_to_boolean`: the controllers delegate the
/// helper to the plugin.
///
/// Everything that is not a recognised word is false, an absent parameter
/// included (`nil.to_i != 0` is false).
fn value_to_boolean(value: Option<&String>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let lowered = value.trim().to_ascii_lowercase();
    if matches!(lowered.as_str(), "yes" | "y" | "true" | "t" | "on" | "1") {
        return true;
    }
    if matches!(lowered.as_str(), "no" | "n" | "false" | "f" | "off" | "0") {
        return false;
    }
    leading_integer(value) != 0
}

/// Canvas `include_param`: the nested collections are rendered when the
/// request asks for "all".
fn include_all(query: &HashMap<String, String>) -> bool {
    query
        .get("include")
        .map(|value| value == "all")
        .unwrap_or(false)
        || query
            .get("include[]")
            .map(|value| value == "all")
            .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// The plan of a create: every scoring range, assignment set and association
/// of the request is a new record.
///
/// The positions follow `acts_as_list`: the request wins when it carries
/// one, otherwise the record gets the next free position of its siblings.
fn create_plans(params: &RuleParams) -> Vec<RangePlan> {
    let mut ranges = Vec::with_capacity(params.scoring_ranges.len());
    let mut max_range_position = 0i32;
    for param in params.scoring_ranges.iter() {
        let position = param.position.unwrap_or(max_range_position + 1);
        max_range_position = max_range_position.max(position);

        let mut sets = Vec::with_capacity(param.assignment_sets.len());
        let mut max_set_position = 0i32;
        for set_param in param.assignment_sets.iter() {
            let set_position = set_param.position.unwrap_or(max_set_position + 1);
            max_set_position = max_set_position.max(set_position);

            let mut associations = Vec::with_capacity(set_param.associations.len());
            let mut max_association_position = 0i32;
            for assoc_param in set_param.associations.iter() {
                let assoc_position = assoc_param.position.unwrap_or(max_association_position + 1);
                max_association_position = max_association_position.max(assoc_position);
                associations.push(AssociationPlan {
                    id: None,
                    assignment_id: assoc_param.assignment_id,
                    assignment_changed: assoc_param.assignment_id.is_some(),
                    position: assoc_position,
                    changed: true,
                });
            }
            sets.push(SetPlan {
                id: None,
                position: set_position,
                changed: true,
                associations: if set_param.associations_given {
                    Some(associations)
                } else {
                    None
                },
                existing_association_ids: Vec::new(),
            });
        }
        ranges.push(RangePlan {
            id: None,
            lower_bound: param.lower_bound,
            upper_bound: param.upper_bound,
            position,
            changed: true,
            sets: if param.assignment_sets_given {
                Some(sets)
            } else {
                None
            },
            existing_set_ids: Vec::new(),
        });
    }
    ranges
}

/// Validate the plan of a request.
async fn validate_plan(
    db: &PgPool,
    course_id: i64,
    trigger: Option<i64>,
    plan: &RulePlan,
    errors: &mut ErrorBag,
) -> Result<(), ApiError> {
    let Some(ranges) = &plan.ranges else {
        return Ok(());
    };
    for range in ranges {
        validate_range(range, errors);
        let Some(sets) = &range.sets else { continue };
        for set in sets {
            let Some(associations) = &set.associations else {
                continue;
            };
            for association in associations {
                let association_errors =
                    validate_association(db, association, course_id, trigger, set.id).await?;
                for (attribute, messages) in association_errors.entries {
                    for message in messages {
                        errors.add(&attribute, &message);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Insert the scoring ranges of a create.
async fn insert_ranges(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule_id: i64,
    root_account_id: i64,
    plan: &RulePlan,
) -> Result<(), ApiError> {
    let Some(ranges) = &plan.ranges else {
        return Ok(());
    };
    for range in ranges {
        let range_id: i64 = sqlx::query_scalar(&format!(
            "INSERT INTO conditional_release_scoring_ranges \
               (rule_id, lower_bound, upper_bound, position, root_account_id, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, {NOW}, {NOW}) RETURNING id"
        ))
        .bind(rule_id)
        .bind(range.lower_bound)
        .bind(range.upper_bound)
        .bind(range.position)
        .bind(root_account_id)
        .fetch_one(&mut **tx)
        .await?;

        let Some(sets) = &range.sets else { continue };
        for set in sets {
            let set_id: i64 = sqlx::query_scalar(&format!(
                "INSERT INTO conditional_release_assignment_sets \
                   (scoring_range_id, position, root_account_id, created_at, updated_at) \
                 VALUES ($1, $2, $3, {NOW}, {NOW}) RETURNING id"
            ))
            .bind(range_id)
            .bind(set.position)
            .bind(root_account_id)
            .fetch_one(&mut **tx)
            .await?;

            let Some(associations) = &set.associations else {
                continue;
            };
            for association in associations {
                let Some(assignment_id) = association.assignment_id else {
                    continue;
                };
                sqlx::query(&format!(
                    "INSERT INTO conditional_release_assignment_set_associations \
                       (assignment_set_id, assignment_id, position, root_account_id, created_at, updated_at) \
                     VALUES ($1, $2, $3, $4, {NOW}, {NOW})"
                ))
                .bind(set_id)
                .bind(assignment_id)
                .bind(association.position)
                .bind(root_account_id)
                .execute(&mut **tx)
                .await?;
            }
        }
    }
    Ok(())
}

/// Apply the plan of an update.
///
/// The rows the request does not mention are destroyed (a soft delete, like
/// `ConditionalRelease::Deletion`), the rows it mentions are written.
async fn apply_plan(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule_id: i64,
    root_account_id: i64,
    plan: &RulePlan,
) -> Result<(), ApiError> {
    let Some(ranges) = &plan.ranges else {
        return Ok(());
    };

    // The ranges the request does not mention are destroyed first, exactly
    // like the Rails autosave callbacks do.
    let kept_range_ids: Vec<i64> = ranges.iter().filter_map(|range| range.id).collect();
    for id in &plan.existing_range_ids {
        if !kept_range_ids.contains(id) {
            soft_delete(tx, "conditional_release_scoring_ranges", *id).await?;
        }
    }

    for range in ranges {
        let range_id = match range.id {
            Some(id) => {
                if range.changed {
                    sqlx::query(&format!(
                        "UPDATE conditional_release_scoring_ranges \
                         SET lower_bound = $1, upper_bound = $2, position = $3, updated_at = {NOW} \
                         WHERE id = $4"
                    ))
                    .bind(range.lower_bound)
                    .bind(range.upper_bound)
                    .bind(range.position)
                    .bind(id)
                    .execute(&mut **tx)
                    .await?;
                }
                id
            }
            None => {
                sqlx::query_scalar(&format!(
                    "INSERT INTO conditional_release_scoring_ranges \
                       (rule_id, lower_bound, upper_bound, position, root_account_id, created_at, updated_at) \
                     VALUES ($1, $2, $3, $4, $5, {NOW}, {NOW}) RETURNING id"
                ))
                .bind(rule_id)
                .bind(range.lower_bound)
                .bind(range.upper_bound)
                .bind(range.position)
                .bind(root_account_id)
                .fetch_one(&mut **tx)
                .await?
            }
        };

        let Some(sets) = &range.sets else { continue };
        let kept_set_ids: Vec<i64> = sets.iter().filter_map(|set| set.id).collect();
        for id in &range.existing_set_ids {
            if !kept_set_ids.contains(id) {
                soft_delete(tx, "conditional_release_assignment_sets", *id).await?;
            }
        }

        for set in sets {
            let set_id = match set.id {
                Some(id) => {
                    if set.changed {
                        sqlx::query(&format!(
                            "UPDATE conditional_release_assignment_sets \
                             SET position = $1, updated_at = {NOW} WHERE id = $2"
                        ))
                        .bind(set.position)
                        .bind(id)
                        .execute(&mut **tx)
                        .await?;
                    }
                    id
                }
                None => {
                    sqlx::query_scalar(&format!(
                        "INSERT INTO conditional_release_assignment_sets \
                           (scoring_range_id, position, root_account_id, created_at, updated_at) \
                         VALUES ($1, $2, $3, {NOW}, {NOW}) RETURNING id"
                    ))
                    .bind(range_id)
                    .bind(set.position)
                    .bind(root_account_id)
                    .fetch_one(&mut **tx)
                    .await?
                }
            };

            let Some(associations) = &set.associations else {
                continue;
            };
            let kept_association_ids: Vec<i64> =
                associations.iter().filter_map(|assoc| assoc.id).collect();
            for id in &set.existing_association_ids {
                if !kept_association_ids.contains(id) {
                    soft_delete(tx, "conditional_release_assignment_set_associations", *id).await?;
                }
            }

            for association in associations {
                match association.id {
                    Some(id) => {
                        if association.changed {
                            sqlx::query(&format!(
                                "UPDATE conditional_release_assignment_set_associations \
                                 SET assignment_id = $1, position = $2, updated_at = {NOW} WHERE id = $3"
                            ))
                            .bind(association.assignment_id)
                            .bind(association.position)
                            .bind(id)
                            .execute(&mut **tx)
                            .await?;
                        }
                    }
                    None => {
                        let Some(assignment_id) = association.assignment_id else {
                            continue;
                        };
                        sqlx::query(&format!(
                            "INSERT INTO conditional_release_assignment_set_associations \
                               (assignment_set_id, assignment_id, position, root_account_id, created_at, updated_at) \
                             VALUES ($1, $2, $3, $4, {NOW}, {NOW})"
                        ))
                        .bind(set_id)
                        .bind(assignment_id)
                        .bind(association.position)
                        .bind(root_account_id)
                        .execute(&mut **tx)
                        .await?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// The soft delete of `ConditionalRelease::Deletion`.
async fn soft_delete(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    table: &str,
    id: i64,
) -> Result<(), ApiError> {
    let sql = format!("UPDATE {table} SET deleted_at = {NOW}, updated_at = {NOW} WHERE id = $1");
    sqlx::query(&sql).bind(id).execute(&mut **tx).await?;
    Ok(())
}
