//! Data access for the Canvas GraphQL query root.
//!
//! One function per Canvas root field (`app/graphql/types/query_type.rb`),
//! each returning the Canvas-shaped object or null. Unknown ids resolve to
//! null, exactly like Canvas' `GraphQLNodeLoader`.
//!
//! Authorization lives in the resolvers (`query_root.rs`), not here: this
//! module only knows how to read a row.

use async_graphql::ID;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::{
    db_err,
    root_types::{
        AccountNotification, AssignmentGroup, EnrollmentRole, InstitutionalTag,
        InstitutionalTagCategory, InstitutionalTagConnection, InstitutionalTagEdge,
        InstructorCourseInfo, InstructorEnrollmentInfo, InstructorUserInfo,
        InstructorWithEnrollments, InstructorWithEnrollmentsConnection, InternalSetting,
        LearningOutcome, LearningOutcomeGroup, OutcomeCalculationMethod, OutcomeProficiency,
        PeerReviewSubAssignment, ProficiencyRating, ProficiencyRatingConnection,
        ProficiencyRatingEdge, Rubric, RubricCriterion, RubricRating, TotalCountPageInfo,
    },
    Course, Enrollment, PageInfo,
};

/// The connection type Canvas returns from `courseInstructorsConnection`.
pub(crate) type InstructorConnection = InstructorWithEnrollmentsConnection;

/// Default page size for connections when `first` is omitted.
const DEFAULT_PAGE_SIZE: i32 = 10;

/// Maximum accepted page size for connections.
const MAX_PAGE_SIZE: i32 = 100;

fn page_size(first: Option<i32>) -> usize {
    first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize
}

fn after_id(after: Option<&str>) -> Result<Option<i64>, async_graphql::Error> {
    match after {
        None => Ok(None),
        Some(cursor) => crate::decode_cursor(cursor)
            .map(Some)
            .ok_or_else(|| async_graphql::Error::new("invalid cursor")),
    }
}

// ---------------------------------------------------------------------------
// AccountNotification
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct AccountNotificationRow {
    id: i64,
    account_id: i64,
    account_name: Option<String>,
    message: String,
    subject: String,
    icon: Option<String>,
    workflow_state: String,
    start_at: DateTime<Utc>,
    end_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const ACCOUNT_NOTIFICATION_COLUMNS: &str = r#"
    n.id, n.account_id, a.name AS account_name, n.message, n.subject, n.icon,
    n.workflow_state, n.start_at, n.end_at, n.created_at, n.updated_at
"#;

/// Canvas `Query.accountNotifications` — the notifications for the current
/// user, optionally scoped to one account.
///
/// Canvas (`app/graphql/types/query_type.rb` →
/// `AccountNotification.for_user_and_account`):
/// * a site administrator sees every notification of the account;
/// * anybody else sees only the notifications addressed to them
///   (`user_id = caller`);
/// * only notifications inside their window (`start_at <= now < end_at`) and
///   not deleted are listed.
pub(crate) async fn fetch_account_notifications(
    pool: &PgPool,
    user_id: i64,
    account_id: Option<i64>,
    is_admin: bool,
) -> Result<Vec<AccountNotification>, async_graphql::Error> {
    let rows: Vec<AccountNotificationRow> = sqlx::query_as(&format!(
        r#"
        SELECT {ACCOUNT_NOTIFICATION_COLUMNS}
        FROM account_notifications n
        LEFT JOIN accounts a ON a.id = n.account_id
        WHERE n.workflow_state <> 'deleted'
          AND n.start_at <= now()
          AND n.end_at >= now()
          AND ($3::boolean OR n.user_id = $1)
          AND ($2::bigint IS NULL OR n.account_id = $2)
        ORDER BY n.id
        "#
    ))
    .bind(user_id)
    .bind(account_id)
    .bind(is_admin)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    Ok(rows
        .into_iter()
        .map(|row| AccountNotification {
            id: row.id,
            account_id: row.account_id,
            account_name: row.account_name,
            message: row.message,
            subject: row.subject,
            icon: row.icon,
            notification_type: Some(row.workflow_state),
            site_admin: false,
            start_at: row.start_at,
            end_at: row.end_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
        .collect())
}

// ---------------------------------------------------------------------------
// AssignmentGroup
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct AssignmentGroupRow {
    id: i64,
    course_id: i64,
    name: String,
    position: i32,
    group_weight: f64,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    drop_lowest: Option<i32>,
    drop_highest: Option<i32>,
    never_drop: Option<Vec<i64>>,
}

/// Canvas `Query.assignmentGroup`.
pub(crate) async fn fetch_assignment_group(
    pool: &PgPool,
    id: i64,
) -> Result<Option<AssignmentGroup>, async_graphql::Error> {
    let row: Option<AssignmentGroupRow> = sqlx::query_as(
        r#"
        SELECT g.id, g.course_id, g.name, g.position, g.group_weight,
               g.workflow_state, g.created_at, g.updated_at,
               r.drop_lowest, r.drop_highest, r.never_drop
        FROM assignment_groups g
        LEFT JOIN assignment_group_rules r ON r.assignment_group_id = g.id
        WHERE g.id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.map(|row| AssignmentGroup {
        id: row.id,
        course_id: row.course_id,
        name: row.name,
        position: row.position,
        group_weight: row.group_weight,
        workflow_state: row.workflow_state,
        created_at: row.created_at,
        updated_at: row.updated_at,
        drop_lowest: row.drop_lowest,
        drop_highest: row.drop_highest,
        never_drop: row.never_drop,
    }))
}

/// The assignments that belong to an assignment group.
pub(crate) async fn fetch_group_assignments(
    pool: &PgPool,
    course_id: i64,
    group_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<crate::assignment::AssignmentConnection, async_graphql::Error> {
    let size = page_size(first);
    let after_id = after_id(after)?;
    let rows: Vec<crate::assignment::AssignmentRow> = sqlx::query_as(
        r#"
        SELECT id, course_id, title AS name, description,
               points_possible::float8 AS points_possible, due_at,
               created_at, updated_at
        FROM assignments
        WHERE course_id = $1 AND assignment_group_id = $2
          AND workflow_state <> 'deleted'
          AND ($3::bigint IS NULL OR id > $3)
        ORDER BY id
        LIMIT $4
        "#,
    )
    .bind(course_id)
    .bind(group_id)
    .bind(after_id)
    .bind((size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let has_next_page = rows.len() > size;
    let edges: Vec<crate::assignment::AssignmentEdge> = rows
        .into_iter()
        .take(size)
        .map(|row| crate::assignment::AssignmentEdge {
            cursor: crate::encode_cursor(row.id),
            node: row.into(),
        })
        .collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(crate::assignment::AssignmentConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

// ---------------------------------------------------------------------------
// InstitutionalTag / InstitutionalTagCategory
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct InstitutionalTagRow {
    id: i64,
    name: String,
    description: String,
    category_id: i64,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Canvas `Query.institutionalTag`.
pub(crate) async fn fetch_institutional_tag(
    pool: &PgPool,
    id: i64,
) -> Result<Option<InstitutionalTag>, async_graphql::Error> {
    let row: Option<InstitutionalTagRow> = sqlx::query_as(
        r#"
        SELECT id, name, description, category_id, workflow_state,
               created_at, updated_at
        FROM institutional_tags
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.map(|row| InstitutionalTag {
        id: row.id,
        name: row.name,
        description: row.description,
        category_id: row.category_id,
        workflow_state: row.workflow_state,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct InstitutionalTagCategoryRow {
    id: i64,
    name: String,
    description: Option<String>,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Canvas `Query.institutionalTagCategory`.
pub(crate) async fn fetch_institutional_tag_category(
    pool: &PgPool,
    id: i64,
) -> Result<Option<InstitutionalTagCategory>, async_graphql::Error> {
    let row: Option<InstitutionalTagCategoryRow> = sqlx::query_as(
        r#"
        SELECT id, name, description, workflow_state, created_at, updated_at
        FROM institutional_tag_categories
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.map(|row| InstitutionalTagCategory {
        id: row.id,
        name: row.name,
        description: row.description,
        workflow_state: row.workflow_state,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

/// Canvas `InstitutionalTagCategory.tagsConnection`.
pub(crate) async fn fetch_institutional_tag_connection(
    pool: &PgPool,
    category_id: Option<i64>,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<InstitutionalTagConnection, async_graphql::Error> {
    let size = page_size(first);
    let after_id = after_id(after)?;
    let rows: Vec<InstitutionalTagRow> = sqlx::query_as(
        r#"
        SELECT id, name, description, category_id, workflow_state,
               created_at, updated_at
        FROM institutional_tags
        WHERE ($1::bigint IS NULL OR category_id = $1)
          AND workflow_state <> 'deleted'
          AND ($2::bigint IS NULL OR id > $2)
        ORDER BY id
        LIMIT $3
        "#,
    )
    .bind(category_id)
    .bind(after_id)
    .bind((size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let has_next_page = rows.len() > size;
    let edges: Vec<InstitutionalTagEdge> = rows
        .into_iter()
        .take(size)
        .map(|row| InstitutionalTagEdge {
            cursor: crate::encode_cursor(row.id),
            node: InstitutionalTag {
                id: row.id,
                name: row.name,
                description: row.description,
                category_id: row.category_id,
                workflow_state: row.workflow_state,
                created_at: row.created_at,
                updated_at: row.updated_at,
            },
        })
        .collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(InstitutionalTagConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

/// Canvas `InstitutionalTag.associationsCount`.
pub(crate) async fn count_institutional_tag_associations(
    pool: &PgPool,
    tag_id: i64,
) -> Result<i32, async_graphql::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT COUNT(*)::int FROM institutional_tag_associations
           WHERE institutional_tag_id = $1"#,
    )
    .bind(tag_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row.map(|r| r.0).unwrap_or(0))
}

/// Canvas `InstitutionalTag.usersConnection` — the users the tag is
/// associated with (star-db's `institutional_tag_associations`).
pub(crate) async fn fetch_institutional_tag_users(
    pool: &PgPool,
    tag_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<crate::course_user_fields::UserConnection, async_graphql::Error> {
    #[derive(Debug, Clone, sqlx::FromRow)]
    struct TagUserRow {
        id: i64,
        name: String,
        email: String,
        sortable_name: String,
        avatar_url: Option<String>,
        created_at: DateTime<Utc>,
    }

    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => Some(
            crate::decode_cursor(cursor)
                .ok_or_else(|| async_graphql::Error::new("invalid cursor"))?,
        ),
    };

    let rows: Vec<TagUserRow> = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (u.id)
               u.id, u.name, u.email,
               COALESCE(u.sortable_name, u.name) AS sortable_name,
               up.avatar_url,
               u.created_at
        FROM institutional_tag_associations a
        JOIN users u ON u.id = a.user_id
        LEFT JOIN user_profiles up ON up.user_id = u.id
        WHERE a.institutional_tag_id = $1
          AND a.workflow_state <> 'deleted'
          AND ($2::bigint IS NULL OR u.id > $2)
        ORDER BY u.id
        LIMIT $3
        "#,
    )
    .bind(tag_id)
    .bind(after_id)
    .bind((page_size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let has_next_page = rows.len() > page_size;
    let mut rows = rows;
    rows.truncate(page_size);

    let edges: Vec<crate::course_user_fields::UserEdge> = rows
        .into_iter()
        .map(|row| crate::course_user_fields::UserEdge {
            cursor: crate::encode_cursor(row.id),
            node: crate::User {
                id: row.id,
                name: row.name,
                email: row.email,
                sis_user_id: None,
                sortable_name: row.sortable_name,
                avatar_url: row.avatar_url,
                created_at: row.created_at,
            },
        })
        .collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(crate::course_user_fields::UserConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

/// Canvas `InstitutionalTagCategory.associationsCount`.
pub(crate) async fn count_institutional_tag_category_associations(
    pool: &PgPool,
    category_id: i64,
) -> Result<i32, async_graphql::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT COUNT(*)::int FROM institutional_tag_associations
           WHERE institutional_tag_id IN (
               SELECT id FROM institutional_tags WHERE category_id = $1
           )"#,
    )
    .bind(category_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row.map(|r| r.0).unwrap_or(0))
}

// ---------------------------------------------------------------------------
// InternalSetting
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct InternalSettingRow {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) value: Option<String>,
    pub(crate) secret: bool,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

pub(crate) fn internal_setting_from_row(row: InternalSettingRow) -> InternalSetting {
    InternalSetting {
        id: row.id,
        name: row.name,
        value: row.value,
        secret: row.secret,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// Canvas `Query.internalSetting` — by id.
pub(crate) async fn fetch_internal_setting(
    pool: &PgPool,
    id: i64,
) -> Result<Option<InternalSetting>, async_graphql::Error> {
    let row: Option<InternalSettingRow> = sqlx::query_as(
        r#"SELECT id, name, value, secret, created_at, updated_at
           FROM settings WHERE id = $1"#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row.map(internal_setting_from_row))
}

/// Canvas `Query.internalSetting` — by name.
pub(crate) async fn fetch_internal_setting_by_name(
    pool: &PgPool,
    name: &str,
) -> Result<Option<InternalSetting>, async_graphql::Error> {
    let row: Option<InternalSettingRow> = sqlx::query_as(
        r#"SELECT id, name, value, secret, created_at, updated_at
           FROM settings WHERE name = $1"#,
    )
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row.map(internal_setting_from_row))
}

/// Canvas `Query.internalSettings` — every setting.
pub(crate) async fn fetch_internal_settings(
    pool: &PgPool,
) -> Result<Vec<InternalSetting>, async_graphql::Error> {
    let rows: Vec<InternalSettingRow> = sqlx::query_as(
        r#"SELECT id, name, value, secret, created_at, updated_at
           FROM settings ORDER BY id"#,
    )
    .fetch_all(pool)
    .await
    .map_err(db_err)?;
    Ok(rows.into_iter().map(internal_setting_from_row).collect())
}

// ---------------------------------------------------------------------------
// LearningOutcome / LearningOutcomeGroup
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct LearningOutcomeRow {
    pub(crate) id: i64,
    pub(crate) title: String,
    pub(crate) description: Option<String>,
    pub(crate) display_name: Option<String>,
    pub(crate) context_id: Option<i64>,
    pub(crate) context_type: Option<String>,
    pub(crate) calculation_int: Option<i32>,
    pub(crate) calculation_method: Option<String>,
    pub(crate) mastery_points: Option<f64>,
    pub(crate) points_possible: Option<f64>,
    pub(crate) assessed: bool,
    pub(crate) vendor_guid: Option<String>,
    pub(crate) workflow_state: String,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

/// Map an outcome row onto the GraphQL object.
pub(crate) fn learning_outcome_from_row(row: LearningOutcomeRow) -> LearningOutcome {
    LearningOutcome {
        id: row.id,
        title: row.title,
        description: row.description,
        display_name: row.display_name,
        context_id: row.context_id,
        context_type: row.context_type,
        calculation_int: row.calculation_int,
        calculation_method: row.calculation_method,
        mastery_points: row.mastery_points,
        points_possible: row.points_possible,
        assessed: row.assessed,
        vendor_guid: row.vendor_guid,
        workflow_state: row.workflow_state,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// Map an outcome group row onto the GraphQL object.
pub(crate) fn learning_outcome_group_from_row(
    row: LearningOutcomeGroupRow,
) -> LearningOutcomeGroup {
    LearningOutcomeGroup {
        id: row.id,
        title: row.title,
        description: row.description,
        context_id: row.context_id,
        context_type: row.context_type,
        vendor_guid: row.vendor_guid,
        workflow_state: row.workflow_state,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// Map an inbox settings row onto the GraphQL object.
pub(crate) fn inbox_settings_from_row(
    row: InboxSettingsRow,
) -> Option<crate::root_types::InboxSettings> {
    row.user_id
        .parse::<i64>()
        .ok()
        .map(|user_id| crate::root_types::InboxSettings {
            id: row.id,
            user_id,
            use_signature: row.use_signature,
            signature: row.signature,
            use_out_of_office: row.use_out_of_office,
            out_of_office_first_date: row.out_of_office_first_date,
            out_of_office_last_date: row.out_of_office_last_date,
            out_of_office_subject: row.out_of_office_subject,
            out_of_office_message: row.out_of_office_message,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
}

/// Canvas `Query.learningOutcome`.
pub(crate) async fn fetch_learning_outcome(
    pool: &PgPool,
    id: i64,
) -> Result<Option<LearningOutcome>, async_graphql::Error> {
    let row: Option<LearningOutcomeRow> = sqlx::query_as(
        r#"
        SELECT id, title, description, display_name, context_id, context_type,
               calculation_int, calculation_method, mastery_points,
               points_possible, assessed, vendor_guid, workflow_state,
               created_at, updated_at
        FROM learning_outcomes
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.map(learning_outcome_from_row))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct LearningOutcomeGroupRow {
    pub(crate) id: i64,
    pub(crate) title: String,
    pub(crate) description: Option<String>,
    pub(crate) context_id: Option<i64>,
    pub(crate) context_type: Option<String>,
    pub(crate) vendor_guid: Option<String>,
    pub(crate) workflow_state: String,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

/// Canvas `Query.learningOutcomeGroup`.
pub(crate) async fn fetch_learning_outcome_group(
    pool: &PgPool,
    id: i64,
) -> Result<Option<LearningOutcomeGroup>, async_graphql::Error> {
    let row: Option<LearningOutcomeGroupRow> = sqlx::query_as(
        r#"
        SELECT id, title, description, context_id, context_type, vendor_guid,
               workflow_state, created_at, updated_at
        FROM learning_outcome_groups
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.map(learning_outcome_group_from_row))
}

/// Canvas `LearningOutcomeGroup.outcomesCount`.
pub(crate) async fn count_group_outcomes(
    pool: &PgPool,
    group_id: i64,
) -> Result<i32, async_graphql::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT COUNT(*)::int FROM learning_outcome_links
           WHERE outcome_group_id = $1 AND workflow_state <> 'deleted'"#,
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row.map(|r| r.0).unwrap_or(0))
}

/// Canvas `Rubric.rubricAssociationForContext` — the rubric's association
/// with the course it belongs to (star-db's `rubric_associations`).
pub(crate) async fn fetch_rubric_association(
    pool: &PgPool,
    rubric_id: i64,
    course_id: i64,
) -> Result<Option<crate::root_types::RubricAssociation>, async_graphql::Error> {
    #[derive(Debug, Clone, sqlx::FromRow)]
    struct RubricAssociationRow {
        id: i64,
        association_id: i64,
        association_type: String,
        use_for_grading: bool,
        hide_score_total: bool,
        hide_points: bool,
        hide_outcome_results: bool,
    }

    let row: Option<RubricAssociationRow> = sqlx::query_as(
        r#"
        SELECT id, association_id, association_type, use_for_grading,
               hide_score_total, hide_points, hide_outcome_results
        FROM rubric_associations
        WHERE rubric_id = $1 AND context_id = $2 AND context_type = 'Course'
        ORDER BY id
        LIMIT 1
        "#,
    )
    .bind(rubric_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.map(|row| crate::root_types::RubricAssociation {
        id: row.id,
        association_id: row.association_id.to_string(),
        association_type: row.association_type,
        use_for_grading: row.use_for_grading,
        hide_score_total: row.hide_score_total,
        hide_points: row.hide_points,
        hide_outcome_results: row.hide_outcome_results,
    }))
}

/// Canvas `AssignmentGroup.gradesConnection` — the students' grade rollups
/// for an assignment group (star-db's `scores` table).
pub(crate) async fn fetch_grades_connection(
    pool: &PgPool,
    assignment_group_id: i64,
    filter: Option<&crate::root_types::GradesEnrollmentFilter>,
    group: crate::root_types::AssignmentGroup,
) -> Result<crate::root_types::GradesConnection, async_graphql::Error> {
    #[derive(Debug, Clone, sqlx::FromRow)]
    struct ScoreRow {
        id: i64,
        enrollment_id: i64,
        grading_period_id: Option<i64>,
        current_score: Option<f64>,
        final_score: Option<f64>,
        override_score: Option<f64>,
        unposted_current_score: Option<f64>,
        unposted_final_score: Option<f64>,
        custom_grade_status_id: Option<i64>,
        workflow_state: String,
    }

    let enrollment_ids: Option<Vec<i64>> =
        filter.and_then(|f| f.enrollment_ids.as_ref()).map(|ids| {
            ids.iter()
                .filter_map(|id| id.parse::<i64>().ok())
                .collect::<Vec<i64>>()
        });

    let rows: Vec<ScoreRow> = sqlx::query_as(
        r#"
        SELECT id, enrollment_id, grading_period_id, current_score, final_score,
               override_score, unposted_current_score, unposted_final_score,
               custom_grade_status_id, workflow_state
        FROM scores
        WHERE assignment_group_id = $1
          AND ($2::bigint[] IS NULL OR enrollment_id = ANY($2))
        ORDER BY enrollment_id, id
        "#,
    )
    .bind(assignment_group_id)
    .bind(enrollment_ids)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let mut edges = Vec::with_capacity(rows.len());
    let mut nodes = Vec::with_capacity(rows.len());
    for row in rows {
        let node = crate::root_types::Grades {
            enrollment_id: row.enrollment_id,
            grading_period_id: row.grading_period_id,
            current_score: row.current_score,
            final_score: row.final_score,
            override_score: row.override_score,
            unposted_current_score: row.unposted_current_score,
            unposted_final_score: row.unposted_final_score,
            custom_grade_status_id: row.custom_grade_status_id,
            state: if row.workflow_state == "deleted" {
                crate::graphql_enums_inputs::GradeState::Deleted
            } else {
                crate::graphql_enums_inputs::GradeState::Active
            },
            assignment_group: Some(group.clone()),
        };
        nodes.push(node.clone());
        edges.push(crate::root_types::GradesEdge {
            cursor: crate::encode_cursor(row.id),
            node,
        });
    }
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(crate::root_types::GradesConnection {
        edges,
        nodes,
        page_info: PageInfo {
            has_next_page: false,
            end_cursor,
        },
    })
}

/// Canvas `LearningOutcomeGroup.outcomes` — the group's outcomes as a
/// `ContentTagConnection` (the link row is the `ContentTag`).
pub(crate) async fn fetch_content_tag_connection(
    pool: &PgPool,
    group_id: i64,
) -> Result<crate::root_types::ContentTagConnection, async_graphql::Error> {
    #[derive(Debug, Clone, sqlx::FromRow)]
    struct ContentTagRow {
        link_id: i64,
        #[sqlx(flatten)]
        outcome: LearningOutcomeRow,
    }

    let rows: Vec<ContentTagRow> = sqlx::query_as(
        r#"
        SELECT l.id AS link_id, o.id, o.title, o.description, o.display_name,
               o.context_id, o.context_type, o.calculation_int, o.calculation_method,
               o.mastery_points, o.points_possible, o.assessed, o.vendor_guid,
               o.workflow_state, o.created_at, o.updated_at
        FROM learning_outcome_links l
        JOIN learning_outcomes o ON o.id = l.outcome_id
        WHERE l.outcome_group_id = $1
          AND l.workflow_state <> 'deleted'
          AND o.workflow_state <> 'deleted'
        ORDER BY o.id
        "#,
    )
    .bind(group_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let mut edges = Vec::with_capacity(rows.len());
    let mut nodes = Vec::with_capacity(rows.len());
    for row in rows {
        let link_id = row.link_id;
        let outcome = learning_outcome_from_row(row.outcome);
        nodes.push(crate::root_types::ContentTagContent::LearningOutcome(
            outcome.clone(),
        ));
        edges.push(crate::root_types::ContentTag {
            id: async_graphql::ID(link_id.to_string()),
            legacy_id: async_graphql::ID(link_id.to_string()),
            cursor: crate::encode_cursor(link_id),
            node: Some(crate::root_types::ContentTagContent::LearningOutcome(
                outcome,
            )),
            group: None,
            can_unlink: None,
            created_at: None,
            updated_at: None,
        });
    }
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(crate::root_types::ContentTagConnection {
        edges,
        nodes,
        page_info: PageInfo {
            has_next_page: false,
            end_cursor,
        },
    })
}

// ---------------------------------------------------------------------------
// InboxSettings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct InboxSettingsRow {
    pub(crate) id: i64,
    pub(crate) user_id: String,
    pub(crate) use_signature: bool,
    pub(crate) signature: Option<String>,
    pub(crate) use_out_of_office: bool,
    pub(crate) out_of_office_first_date: Option<DateTime<Utc>>,
    pub(crate) out_of_office_last_date: Option<DateTime<Utc>>,
    pub(crate) out_of_office_subject: Option<String>,
    pub(crate) out_of_office_message: Option<String>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

/// Canvas `Query.myInboxSettings` — the caller's own inbox settings.
pub(crate) async fn fetch_inbox_settings(
    pool: &PgPool,
    user_id: i64,
) -> Result<Option<crate::root_types::InboxSettings>, async_graphql::Error> {
    let row: Option<InboxSettingsRow> = sqlx::query_as(
        r#"
        SELECT id, user_id, use_signature, signature, use_out_of_office,
               out_of_office_first_date, out_of_office_last_date,
               out_of_office_subject, out_of_office_message,
               created_at, updated_at
        FROM inbox_settings
        WHERE user_id = $1
        "#,
    )
    .bind(user_id.to_string())
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.and_then(inbox_settings_from_row))
}

// ---------------------------------------------------------------------------
// OutcomeCalculationMethod / OutcomeProficiency / ProficiencyRating
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct OutcomeCalculationMethodRow {
    id: i64,
    context_id: i64,
    context_type: String,
    calculation_int: Option<i32>,
    calculation_method: String,
}

/// Canvas `Query.outcomeCalculationMethod`.
pub(crate) async fn fetch_outcome_calculation_method(
    pool: &PgPool,
    id: i64,
) -> Result<Option<OutcomeCalculationMethod>, async_graphql::Error> {
    let row: Option<OutcomeCalculationMethodRow> = sqlx::query_as(
        r#"
        SELECT id, context_id, context_type, calculation_int, calculation_method
        FROM outcome_calculation_methods
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.map(|row| OutcomeCalculationMethod {
        id: row.id,
        context_id: row.context_id,
        context_type: row.context_type,
        calculation_int: row.calculation_int,
        calculation_method: row.calculation_method,
        locked: false,
    }))
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct OutcomeProficiencyRow {
    id: i64,
    context_id: i64,
    context_type: String,
    mastery_points: f64,
}

/// Canvas `Query.outcomeProficiency`.
pub(crate) async fn fetch_outcome_proficiency(
    pool: &PgPool,
    id: i64,
) -> Result<Option<OutcomeProficiency>, async_graphql::Error> {
    let row: Option<OutcomeProficiencyRow> = sqlx::query_as(
        r#"
        SELECT id, context_id, context_type, mastery_points
        FROM outcome_proficiencies
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.map(|row| OutcomeProficiency {
        id: row.id,
        context_id: row.context_id,
        context_type: row.context_type,
        mastery_points: row.mastery_points,
        locked: false,
    }))
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ProficiencyRatingRow {
    id: i64,
    description: String,
    points: f64,
    mastery: bool,
}

/// A row of `outcome_proficiency_ratings` (migration 0225) — Canvas' rating
/// scale for an `OutcomeProficiency`. Carries the colour column that
/// `proficiency_ratings` (the learning-outcome scale) does not have.
#[derive(Debug, Clone, sqlx::FromRow)]
struct OutcomeProficiencyRatingRow {
    id: i64,
    description: String,
    points: f64,
    mastery: bool,
    color: Option<String>,
}

/// Canvas `LearningOutcome.ratings`.
pub(crate) async fn fetch_proficiency_ratings(
    pool: &PgPool,
    outcome_id: i64,
) -> Result<Vec<ProficiencyRating>, async_graphql::Error> {
    let rows: Vec<ProficiencyRatingRow> = sqlx::query_as(
        r#"
        SELECT id, description, points, mastery
        FROM proficiency_ratings
        WHERE learning_outcome_id = $1 AND workflow_state <> 'deleted'
        ORDER BY points DESC, id
        "#,
    )
    .bind(outcome_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    Ok(rows
        .into_iter()
        .map(|row| ProficiencyRating {
            id: row.id,
            description: row.description,
            points: row.points,
            mastery: row.mastery,
            color: None,
        })
        .collect())
}

/// Canvas `OutcomeProficiency.proficiencyRatingsConnection`.
///
/// Canvas' `OutcomeProficiency` owns its rating scale through
/// `outcome_proficiency_ratings` (migration 0225), so the connection reads
/// that table by `outcome_proficiency_id`.
pub(crate) async fn fetch_proficiency_rating_connection(
    pool: &PgPool,
    outcome_proficiency_id: i64,
) -> Result<ProficiencyRatingConnection, async_graphql::Error> {
    let rows: Vec<OutcomeProficiencyRatingRow> = sqlx::query_as(
        r#"
        SELECT id, description, points, mastery, color
        FROM outcome_proficiency_ratings
        WHERE outcome_proficiency_id = $1
          AND workflow_state <> 'deleted'
        ORDER BY points DESC, id
        "#,
    )
    .bind(outcome_proficiency_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let edges: Vec<ProficiencyRatingEdge> = rows
        .into_iter()
        .map(|row| ProficiencyRatingEdge {
            cursor: crate::encode_cursor(row.id),
            node: ProficiencyRating {
                id: row.id,
                description: row.description,
                points: row.points,
                mastery: row.mastery,
                color: row.color,
            },
        })
        .collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(ProficiencyRatingConnection {
        edges,
        page_info: PageInfo {
            has_next_page: false,
            end_cursor,
        },
    })
}

// ---------------------------------------------------------------------------
// Rubric
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct RubricRow {
    id: i64,
    course_id: i64,
    title: String,
    points_possible: f64,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct RubricCriterionRow {
    id: i64,
    rubric_id: i64,
    description: String,
    points: f64,
    position: i32,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct RubricRatingRow {
    id: i64,
    criterion_id: i64,
    description: String,
    points: f64,
    position: i32,
}

/// Canvas `Query.rubric` — a rubric with its criteria and their ratings.
pub(crate) async fn fetch_rubric(
    pool: &PgPool,
    id: i64,
) -> Result<Option<Rubric>, async_graphql::Error> {
    let row: Option<RubricRow> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, points_possible::float8 AS points_possible,
               workflow_state, created_at, updated_at
        FROM rubrics
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    let Some(row) = row else {
        return Ok(None);
    };

    let criterion_rows: Vec<RubricCriterionRow> = sqlx::query_as(
        r#"
        SELECT id, rubric_id, description, points::float8 AS points, position
        FROM rubric_criteria
        WHERE rubric_id = $1
        ORDER BY position, id
        "#,
    )
    .bind(row.id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let rating_rows: Vec<RubricRatingRow> = sqlx::query_as(
        r#"
        SELECT id, criterion_id, description, points::float8 AS points, position
        FROM rubric_ratings
        WHERE criterion_id IN (
            SELECT id FROM rubric_criteria WHERE rubric_id = $1
        )
        ORDER BY position, id
        "#,
    )
    .bind(row.id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let criteria = criterion_rows
        .into_iter()
        .map(|criterion| RubricCriterion {
            ratings: rating_rows
                .iter()
                .filter(|rating| rating.criterion_id == criterion.id)
                .map(|rating| RubricRating {
                    id: rating.id,
                    rubric_id: criterion.rubric_id,
                    criterion_id: rating.criterion_id,
                    description: rating.description.clone(),
                    points: rating.points,
                    position: rating.position,
                })
                .collect(),
            id: criterion.id,
            rubric_id: criterion.rubric_id,
            description: criterion.description,
            points: criterion.points,
            position: criterion.position,
        })
        .collect();

    Ok(Some(Rubric {
        id: row.id,
        course_id: row.course_id,
        title: row.title,
        points_possible: row.points_possible,
        workflow_state: row.workflow_state,
        criteria,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

// ---------------------------------------------------------------------------
// PeerReviewSubAssignment
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct PeerReviewRow {
    id: i64,
    course_id: i64,
    title: String,
    description: Option<String>,
    points_possible: f64,
    due_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Canvas `Query.peerReviewSubAssignment` — an assignment row flagged
/// `assignment_type = 'peer_review'`.
pub(crate) async fn fetch_peer_review_sub_assignment(
    pool: &PgPool,
    id: i64,
) -> Result<Option<PeerReviewSubAssignment>, async_graphql::Error> {
    let row: Option<PeerReviewRow> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, description,
               points_possible::float8 AS points_possible, due_at,
               created_at, updated_at
        FROM assignments
        WHERE id = $1 AND assignment_type = 'peer_review'
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.map(|row| PeerReviewSubAssignment {
        id: row.id,
        course_id: row.course_id,
        name: row.title,
        description: row.description,
        points_possible: row.points_possible,
        due_at: row.due_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

// ---------------------------------------------------------------------------
// Assignment (root lookup, honouring includeTypes)
// ---------------------------------------------------------------------------

/// Canvas `Query.assignment` — the assignment with the requested types.
///
/// `includeTypes` defaults to `[ASSIGNMENT]`; a peer-review sub-assignment is
/// only returned when `PEER_REVIEW_SUB_ASSIGNMENT` is asked for, exactly
/// like Canvas' `AbstractAssignment` loader.
pub(crate) async fn fetch_assignment_by_id(
    pool: &PgPool,
    id: i64,
    include_types: &[crate::root_types::AssignmentTypeEnum],
) -> Result<Option<crate::assignment::Assignment>, async_graphql::Error> {
    let wants_peer =
        include_types.contains(&crate::root_types::AssignmentTypeEnum::PeerReviewSubAssignment);

    let row: Option<crate::assignment::AssignmentRow> = sqlx::query_as(
        r#"
        SELECT id, course_id, title AS name, description,
               points_possible::float8 AS points_possible, due_at,
               created_at, updated_at
        FROM assignments
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    let Some(row) = row else {
        return Ok(None);
    };

    let is_peer = crate::root_data::assignment_is_peer_review(pool, row.id).await?;
    if is_peer && !wants_peer {
        return Ok(None);
    }
    if !is_peer && !include_types.is_empty() {
        // `includeTypes` was given but only asks for the peer-review type.
        if include_types
            .iter()
            .all(|t| *t == crate::root_types::AssignmentTypeEnum::PeerReviewSubAssignment)
        {
            return Ok(None);
        }
    }

    Ok(Some(row.into()))
}

/// Whether an assignment row is a peer-review sub-assignment.
pub(crate) async fn assignment_is_peer_review(
    pool: &PgPool,
    id: i64,
) -> Result<bool, async_graphql::Error> {
    let row: Option<(String,)> =
        sqlx::query_as(r#"SELECT assignment_type FROM assignments WHERE id = $1"#)
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(db_err)?;
    Ok(row.map(|r| r.0 == "peer_review").unwrap_or(false))
}

// ---------------------------------------------------------------------------
// Courses (allCourses / courses)
// ---------------------------------------------------------------------------

/// Canvas `Query.allCourses` — the courses the caller participates in.
pub(crate) async fn fetch_courses_for_user(
    pool: &PgPool,
    user_id: i64,
) -> Result<Vec<Course>, async_graphql::Error> {
    let ids: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT DISTINCT course_id
        FROM enrollments
        WHERE user_id = $1 AND workflow_state <> 'deleted'
        ORDER BY course_id
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    crate::fetch_courses_by_ids(pool, &ids).await
}

// ---------------------------------------------------------------------------
// Enrollment invitations
// ---------------------------------------------------------------------------

/// Canvas `Query.enrollmentInvitations` — the caller's pending invitations.
pub(crate) async fn fetch_enrollment_invitations(
    pool: &PgPool,
    user_id: i64,
    include_enrollment_uuid: Option<&str>,
) -> Result<Vec<Enrollment>, async_graphql::Error> {
    let rows: Vec<crate::EnrollmentRow> = sqlx::query_as(
        r#"
        SELECT id, user_id, course_id, role, workflow_state, created_at, updated_at
        FROM enrollments
        WHERE user_id = $1
          AND workflow_state = 'invited'
          AND ($2::text IS NULL OR id::text = $2)
        ORDER BY id
        "#,
    )
    .bind(user_id)
    .bind(include_enrollment_uuid)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    Ok(rows.into_iter().map(Enrollment::from).collect())
}

// ---------------------------------------------------------------------------
// courseInstructorsConnection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct InstructorRow {
    course_id: i64,
    course_name: String,
    course_code: Option<String>,
    enrollment_role: String,
    enrollment_type: Option<String>,
    enrollment_state: String,
    user_id: i64,
    user_name: String,
    user_email: String,
    sortable_name: Option<String>,
    avatar_url: Option<String>,
}

/// Canvas `Query.courseInstructorsConnection` — the instructors of the given
/// courses, with their enrollments.
pub(crate) async fn fetch_course_instructors(
    pool: &PgPool,
    course_ids: &[i64],
    enrollment_types: &[String],
    first: Option<i32>,
    after: Option<&str>,
) -> Result<InstructorWithEnrollmentsConnection, async_graphql::Error> {
    let size = page_size(first);
    let after_id = after_id(after)?;

    let rows: Vec<InstructorRow> = sqlx::query_as(
        r#"
        SELECT e.course_id, c.name AS course_name,
               c.course_code, e.role AS enrollment_role, e.type AS enrollment_type,
               e.workflow_state AS enrollment_state,
               u.id AS user_id, u.name AS user_name, u.email AS user_email,
               u.sortable_name, up.avatar_url
        FROM enrollments e
        JOIN courses c ON c.id = e.course_id
        JOIN users u ON u.id = e.user_id
        LEFT JOIN user_profiles up ON up.user_id = u.id
        WHERE e.workflow_state <> 'deleted'
          AND e.role = 'teacher'
          AND e.course_id = ANY($1)
          AND ($2::bigint IS NULL OR e.id > $2)
        ORDER BY e.id
        LIMIT $3
        "#,
    )
    .bind(course_ids)
    .bind(after_id)
    .bind((size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let filtered: Vec<InstructorRow> = if enrollment_types.is_empty() {
        rows
    } else {
        rows.into_iter()
            .filter(|row| {
                row.enrollment_type
                    .as_deref()
                    .map(|t| enrollment_types.iter().any(|want| want == t))
                    .unwrap_or(false)
            })
            .collect()
    };

    let has_next_page = filtered.len() > size;
    let page: Vec<InstructorRow> = filtered.into_iter().take(size).collect();

    // Group the enrollments by instructor, keeping the first-seen order.
    let mut order: Vec<i64> = Vec::new();
    let mut grouped: std::collections::HashMap<i64, Vec<InstructorRow>> =
        std::collections::HashMap::new();
    for row in &page {
        if !grouped.contains_key(&row.user_id) {
            order.push(row.user_id);
        }
        grouped.entry(row.user_id).or_default().push(row.clone());
    }

    let nodes: Vec<InstructorWithEnrollments> = order
        .into_iter()
        .filter_map(|user_id| {
            let enrollments = grouped.get(&user_id)?;
            let first_row = enrollments.first()?;
            let user = InstructorUserInfo {
                id: ID(user_id.to_string()),
                legacy_id: ID(user_id.to_string()),
                name: first_row.user_name.clone(),
                email: Some(first_row.user_email.clone()),
                short_name: None,
                sortable_name: first_row.sortable_name.clone(),
                avatar_url: first_row.avatar_url.clone(),
            };
            Some(InstructorWithEnrollments {
                user: user.clone(),
                enrollments: enrollments
                    .iter()
                    .map(|row| InstructorEnrollmentInfo {
                        course: InstructorCourseInfo {
                            id: ID(row.course_id.to_string()),
                            legacy_id: ID(row.course_id.to_string()),
                            name: row.course_name.clone(),
                            course_code: row.course_code.clone(),
                        },
                        enrollment_state: row.enrollment_state.clone(),
                        role: EnrollmentRole {
                            legacy_id: None,
                            name: Some(row.enrollment_role.clone()),
                        },
                        enrollment_type: row
                            .enrollment_type
                            .clone()
                            .unwrap_or_else(|| "TeacherEnrollment".to_string()),
                        user: user.clone(),
                    })
                    .collect(),
            })
        })
        .collect();

    let edges: Vec<crate::root_types::InstructorWithEnrollmentsEdge> = nodes
        .iter()
        .enumerate()
        .map(
            |(index, node)| crate::root_types::InstructorWithEnrollmentsEdge {
                cursor: crate::encode_cursor(index as i64 + 1),
                node: Some(node.clone()),
            },
        )
        .collect();

    let end_cursor = edges.last().map(|edge| edge.cursor.clone());
    let total_count = nodes.len() as i32;

    let start_cursor = edges.first().map(|edge| edge.cursor.clone());
    Ok(InstructorWithEnrollmentsConnection {
        edges,
        nodes,
        page_info: TotalCountPageInfo {
            has_next_page,
            has_previous_page: false,
            end_cursor,
            start_cursor,
            total_count: Some(total_count),
            total_nr_of_pages: Some(1),
        },
    })
}

// ---------------------------------------------------------------------------
// Node dispatch
// ---------------------------------------------------------------------------

/// Resolve a Canvas node by its legacy id and type.
///
/// Canvas' `GraphQLNodeLoader.load` maps the type name to a loader and
/// applies the read permission; the permission checks live in
/// `query_root.rs`, this only loads the row.
pub(crate) async fn load_node(
    pool: &PgPool,
    node_type: crate::node::NodeType,
    id: i64,
) -> Result<Option<crate::node::Node>, async_graphql::Error> {
    use crate::node::Node;

    Ok(match node_type {
        crate::node::NodeType::Account => crate::course_user_fields::fetch_account(pool, id)
            .await?
            .map(Node::Account),
        crate::node::NodeType::Assignment => crate::root_data::fetch_assignment_by_id(
            pool,
            id,
            &[
                crate::root_types::AssignmentTypeEnum::Assignment,
                crate::root_types::AssignmentTypeEnum::PeerReviewSubAssignment,
            ],
        )
        .await?
        .map(Node::Assignment),
        crate::node::NodeType::AssignmentGroup => fetch_assignment_group(pool, id)
            .await?
            .map(Node::AssignmentGroup),
        crate::node::NodeType::Conversation => crate::conversations::fetch_conversation(pool, id)
            .await?
            .map(Node::Conversation),
        crate::node::NodeType::Course => crate::fetch_course(pool, id).await?.map(Node::Course),
        crate::node::NodeType::Discussion => {
            crate::discussion_topic::fetch_discussion_topic(pool, id)
                .await?
                .map(Node::Discussion)
        }
        crate::node::NodeType::Enrollment => crate::fetch_enrollment(pool, id)
            .await?
            .map(Node::Enrollment),
        crate::node::NodeType::File => crate::files::fetch_file(pool, id).await?.map(Node::File),
        crate::node::NodeType::Folder => crate::files::fetch_folder(pool, id)
            .await?
            .map(Node::Folder),
        crate::node::NodeType::Group => {
            crate::groups::fetch_group(pool, id).await?.map(Node::Group)
        }
        crate::node::NodeType::GroupSet => crate::groups::fetch_group_set(pool, id)
            .await?
            .map(Node::GroupSet),
        crate::node::NodeType::InstitutionalTag => fetch_institutional_tag(pool, id)
            .await?
            .map(Node::InstitutionalTag),
        crate::node::NodeType::InstitutionalTagCategory => {
            fetch_institutional_tag_category(pool, id)
                .await?
                .map(Node::InstitutionalTagCategory)
        }
        crate::node::NodeType::InternalSetting => fetch_internal_setting(pool, id)
            .await?
            .map(Node::InternalSetting),
        crate::node::NodeType::LearningOutcomeGroup => fetch_learning_outcome_group(pool, id)
            .await?
            .map(Node::LearningOutcomeGroup),
        crate::node::NodeType::Module => crate::modules::fetch_module(pool, id)
            .await?
            .map(Node::Module),
        crate::node::NodeType::ModuleItem => crate::modules::fetch_module_item_unchecked(pool, id)
            .await?
            .map(Node::ModuleItem),
        crate::node::NodeType::OutcomeCalculationMethod => {
            fetch_outcome_calculation_method(pool, id)
                .await?
                .map(Node::OutcomeCalculationMethod)
        }
        crate::node::NodeType::OutcomeProficiency => fetch_outcome_proficiency(pool, id)
            .await?
            .map(Node::OutcomeProficiency),
        crate::node::NodeType::Page => crate::pages::fetch_page(pool, id).await?.map(Node::Page),
        crate::node::NodeType::PeerReviewSubAssignment => {
            fetch_peer_review_sub_assignment(pool, id)
                .await?
                .map(Node::PeerReviewSubAssignment)
        }
        crate::node::NodeType::Progress => crate::pages::fetch_progress(pool, id)
            .await?
            .map(Node::Progress),
        crate::node::NodeType::Rubric => fetch_rubric(pool, id).await?.map(Node::Rubric),
        crate::node::NodeType::Section => crate::terms_sections::fetch_section_unchecked(pool, id)
            .await?
            .map(Node::Section),
        crate::node::NodeType::Submission => crate::submission::fetch_submission(pool, id)
            .await?
            .map(Node::Submission),
        crate::node::NodeType::Term => crate::terms_sections::fetch_term(pool, id)
            .await?
            .map(Node::Term),
        crate::node::NodeType::User => crate::fetch_user(pool, id).await?.map(Node::User),
        // The remaining NodeType members have no port type yet (allocation
        // rules, discussion entries, grading periods, media objects, module
        // progressions, post policies, usage rights, institutional tag
        // associations, sub-assignments). Canvas resolves them to null when
        // the record is missing; the port has no row to read.
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// Course visibility
// ---------------------------------------------------------------------------

/// The course ids the user participates in.
pub(crate) async fn fetch_course_ids_for_user(
    pool: &PgPool,
    user_id: i64,
) -> Result<Vec<i64>, async_graphql::Error> {
    let ids: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT DISTINCT course_id
        FROM enrollments
        WHERE user_id = $1 AND workflow_state <> 'deleted'
        ORDER BY course_id
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;
    Ok(ids)
}

/// The courses among `ids` the user may see.
///
/// Canvas' `Query.courses` returns "courses by IDs that are viewable by
/// the current user" — a course the caller is not enrolled in is dropped.
pub(crate) async fn fetch_visible_courses(
    pool: &PgPool,
    user_id: i64,
    ids: &[i64],
) -> Result<Vec<Course>, async_graphql::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let visible = fetch_course_ids_for_user(pool, user_id).await?;
    let wanted: Vec<i64> = ids
        .iter()
        .copied()
        .filter(|id| visible.contains(id))
        .collect();
    crate::fetch_courses_by_ids(pool, &wanted).await
}

/// Canvas `Query.submission` — by assignment and anonymous id.
pub(crate) async fn fetch_submission_by_assignment_and_anonymous_id(
    pool: &PgPool,
    assignment_id: i64,
    anonymous_id: &str,
) -> Result<Option<crate::submission::Submission>, async_graphql::Error> {
    let row: Option<crate::submission::SubmissionRow> = sqlx::query_as(
        r#"
        SELECT id, assignment_id, user_id, attempt, body, score::float8 AS score,
               submission_type, workflow_state, submitted_at, graded_at, excused,
               late_policy_status, points_deducted::float8 AS points_deducted,
               created_at, updated_at
        FROM submissions
        WHERE assignment_id = $1 AND anonymous_id = $2
        LIMIT 1
        "#,
    )
    .bind(assignment_id)
    .bind(anonymous_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.map(Into::into))
}

// ---------------------------------------------------------------------------
// SubmissionComment
// ---------------------------------------------------------------------------

/// Row of the `submission_comments` table (migration 0026).
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct SubmissionCommentRow {
    pub(crate) id: i64,
    pub(crate) submission_id: i64,
    pub(crate) author_id: i64,
    pub(crate) comment: String,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

/// Map a submission comment row onto the GraphQL object.
pub(crate) fn submission_comment_from_row(
    row: SubmissionCommentRow,
) -> crate::root_types::SubmissionComment {
    crate::root_types::SubmissionComment {
        id: row.id,
        submission_id: row.submission_id,
        author_id: row.author_id,
        comment: Some(row.comment),
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}
