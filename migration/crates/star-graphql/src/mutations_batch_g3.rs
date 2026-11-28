//! Batch G3 mutations: institutional tags, internal settings, inbox
//! labels/settings, speedgrader settings, allocation rules, gradebook
//! filters, learner dashboard / widget dashboards, and the split-screen
//! alert preferences (GraphQL parity gap completion).
//!
//! Same CONTRACT.md error channels as Batches A–C/G1/G2.

use async_graphql::{Context, InputObject, Object, SimpleObject, ID};
use chrono::{DateTime, Utc};

use crate::{db_err, GraphQLContext};

// ---------------------------------------------------------------------------
// Row shapes for the mutation payloads
// ---------------------------------------------------------------------------

/// A row of `institutional_tags` (0224) — the payload of the
/// `createInstitutionalTag` / `updateInstitutionalTag` mutations.
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

/// A row of `institutional_tag_categories` (0224) — the payload of the
/// `createInstitutionalTagCategory` / `updateInstitutionalTagCategory`
/// mutations.
#[derive(Debug, Clone, sqlx::FromRow)]
struct InstitutionalTagCategoryRow {
    id: i64,
    name: String,
    description: Option<String>,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

async fn is_teacher_or_admin(
    gctx: &GraphQLContext,
    course_id: i64,
) -> Result<bool, async_graphql::Error> {
    if gctx.is_admin() {
        return Ok(true);
    }
    let teaches: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments
           WHERE user_id = $1 AND course_id = $2 AND role = 'teacher'
             AND workflow_state = 'active'
           LIMIT 1"#,
    )
    .bind(gctx.auth_user.id)
    .bind(course_id)
    .fetch_optional(&gctx.pool)
    .await
    .map_err(db_err)?;
    Ok(teaches.is_some())
}

fn parse_id(raw: &ID, what: &str) -> Result<i64, async_graphql::Error> {
    raw.0
        .parse::<i64>()
        .map_err(|_| async_graphql::Error::new(format!("invalid {what}: {}", raw.0)))
}

#[derive(Debug, Clone, SimpleObject)]
pub struct G3Error {
    pub attribute: Option<String>,
    pub message: String,
}

fn validation_errors(attribute: Option<&str>, message: &str) -> Vec<G3Error> {
    vec![G3Error {
        attribute: attribute.map(str::to_string),
        message: message.to_string(),
    }]
}

#[derive(Debug, Clone, SimpleObject)]
pub struct G3SuccessPayload {
    pub success: bool,
}

// ---------------------------------------------------------------------------
// Inputs + payloads
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, InputObject)]
pub struct CreateInstitutionalTagInput {
    pub account_id: ID,
    pub name: String,
    pub category_id: Option<ID>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct InstitutionalTagPayload {
    pub institutional_tag: Option<crate::root_types::InstitutionalTag>,
    pub errors: Vec<G3Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateInstitutionalTagInput {
    pub institutional_tag_id: ID,
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateInstitutionalTagArchivedStateInput {
    pub institutional_tag_id: ID,
    pub archived: bool,
}

#[derive(Debug, Clone, InputObject)]
pub struct ApplyInstitutionalTagInput {
    pub institutional_tag_id: ID,
    pub user_id: ID,
    pub course_id: ID,
}

#[derive(Debug, Clone, InputObject)]
pub struct RemoveInstitutionalTagInput {
    pub institutional_tag_id: ID,
    pub user_id: ID,
    pub course_id: ID,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct InstitutionalTagAssociationPayload {
    pub institutional_tag_association: Option<crate::entities_batch_f::InstitutionalTagAssociation>,
    pub errors: Vec<G3Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct CreateInstitutionalTagCategoryInput {
    pub account_id: ID,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct InstitutionalTagCategoryPayload {
    pub institutional_tag_category: Option<crate::root_types::InstitutionalTagCategory>,
    pub errors: Vec<G3Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateInstitutionalTagCategoryInput {
    pub institutional_tag_category_id: ID,
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateInstitutionalTagCategoryArchivedStateInput {
    pub institutional_tag_category_id: ID,
    pub archived: bool,
}

#[derive(Debug, Clone, InputObject)]
pub struct CreateUserInboxLabelInput {
    pub name: String,
    pub color: Option<String>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct UserInboxLabelPayload {
    pub inbox_label: Option<serde_json::Value>,
    pub errors: Vec<G3Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct DeleteUserInboxLabelInput {
    pub inbox_label_id: ID,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateSpeedGraderSettingsInput {
    pub course_id: Option<ID>,
    pub settings: serde_json::Value,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct SpeedGraderSettingsPayload {
    pub speed_grader_settings: Option<crate::entities_batch_f::SpeedGraderSettings>,
    pub errors: Vec<G3Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct CreateAllocationRuleInput {
    pub assignment_id: ID,
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct AllocationRulePayload {
    pub allocation_rule: Option<crate::entities_batch_f::AllocationRule>,
    pub errors: Vec<G3Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateAllocationRuleInput {
    pub allocation_rule_id: ID,
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, InputObject)]
pub struct DeleteAllocationRuleInput {
    pub allocation_rule_id: ID,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateGradebookGroupFilterInput {
    pub course_id: ID,
    pub filter: serde_json::Value,
    pub position: Option<i32>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct GradebookFilterPayload {
    pub gradebook_filter: Option<serde_json::Value>,
    pub errors: Vec<G3Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateLearnerDashboardTabSelectionInput {
    pub tab_handle: String,
    pub context_type: Option<String>,
    pub context_id: Option<ID>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct LearnerDashboardTabSelectionPayload {
    pub tab_handle: String,
    pub success: bool,
    pub errors: Vec<G3Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateWidgetDashboardConfigInput {
    pub widget: String,
    pub layout: Option<serde_json::Value>,
    pub settings: Option<serde_json::Value>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct WidgetDashboardPayload {
    pub widget_dashboards: Vec<serde_json::Value>,
    pub errors: Vec<G3Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateWidgetDashboardLayoutInput {
    pub layout: serde_json::Value,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateUserDiscussionsSplitscreenViewInput {
    pub enabled: bool,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct SplitscreenPreferencePayload {
    pub success: bool,
    pub errors: Vec<G3Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateSplitScreenViewDeeplyNestedAlertInput {
    pub dismissed: bool,
}

// ---------------------------------------------------------------------------
// Mutation container
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct BatchG3Mutations;

#[Object]
impl BatchG3Mutations {
    /// Canvas `createInstitutionalTag`
    /// (mutations/create_institutional_tag.rb; 0224).
    async fn create_institutional_tag(
        &self,
        ctx: &Context<'_>,
        input: CreateInstitutionalTagInput,
    ) -> Result<InstitutionalTagPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        if !gctx.is_admin() {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        if input.name.trim().is_empty() {
            return Ok(InstitutionalTagPayload {
                institutional_tag: None,
                errors: validation_errors(Some("name"), "name cannot be blank"),
            });
        }
        // Canvas requires `categoryId` and `description` on
        // `createInstitutionalTag`; the port's column is NOT NULL, so a
        // missing category is reported as a validation error instead of a
        // database error.
        let category_id = match input.category_id.as_ref() {
            Some(c) => parse_id(c, "category")?,
            None => {
                return Ok(InstitutionalTagPayload {
                    institutional_tag: None,
                    errors: validation_errors(Some("categoryId"), "categoryId is required"),
                });
            }
        };
        let row: Option<InstitutionalTagRow> = sqlx::query_as(
            "INSERT INTO institutional_tags (name, description, category_id) \
             VALUES ($1, COALESCE($2, ''), $3) \
             RETURNING id, name, description, category_id, workflow_state, \
                       created_at, updated_at",
        )
        .bind(input.name.trim())
        .bind(&input.description)
        .bind(category_id)
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?;
        let Some(row) = row else {
            return Err(async_graphql::Error::new("institutional tag save failed"));
        };
        Ok(InstitutionalTagPayload {
            institutional_tag: Some(crate::root_types::InstitutionalTag {
                id: row.id,
                name: row.name,
                description: row.description,
                category_id: row.category_id,
                workflow_state: row.workflow_state,
                created_at: row.created_at,
                updated_at: row.updated_at,
            }),
            errors: Vec::new(),
        })
    }

    /// Canvas `updateInstitutionalTag`
    /// (mutations/update_institutional_tag.rb).
    async fn update_institutional_tag(
        &self,
        ctx: &Context<'_>,
        input: UpdateInstitutionalTagInput,
    ) -> Result<InstitutionalTagPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        if !gctx.is_admin() {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        let tid = parse_id(&input.institutional_tag_id, "institutional tag")?;
        if let Some(n) = &input.name {
            if n.trim().is_empty() {
                return Ok(InstitutionalTagPayload {
                    institutional_tag: None,
                    errors: validation_errors(Some("name"), "name cannot be blank"),
                });
            }
        }
        let row: Option<InstitutionalTagRow> = sqlx::query_as(
            "UPDATE institutional_tags SET name = COALESCE($2, name), \
             description = COALESCE($3, description), updated_at = now() \
             WHERE id = $1 AND workflow_state <> 'deleted' \
             RETURNING id, name, description, category_id, workflow_state, \
                       created_at, updated_at",
        )
        .bind(tid)
        .bind(input.name.as_deref().map(str::trim))
        .bind(&input.description)
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?;
        let Some(row) = row else {
            return Err(async_graphql::Error::new("institutional tag not found"));
        };
        Ok(InstitutionalTagPayload {
            institutional_tag: Some(crate::root_types::InstitutionalTag {
                id: row.id,
                name: row.name,
                description: row.description,
                category_id: row.category_id,
                workflow_state: row.workflow_state,
                created_at: row.created_at,
                updated_at: row.updated_at,
            }),
            errors: Vec::new(),
        })
    }

    /// Canvas `updateInstitutionalTagArchivedState`
    /// (mutations/update_institutional_tag_archived_state.rb).
    async fn update_institutional_tag_archived_state(
        &self,
        ctx: &Context<'_>,
        input: UpdateInstitutionalTagArchivedStateInput,
    ) -> Result<G3SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        if !gctx.is_admin() {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        let tid = parse_id(&input.institutional_tag_id, "institutional tag")?;
        let res = sqlx::query(
            "UPDATE institutional_tags SET workflow_state = $2, updated_at = now() WHERE id = $1",
        )
        .bind(tid)
        .bind(if input.archived { "archived" } else { "active" })
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("institutional tag not found"));
        }
        Ok(G3SuccessPayload { success: true })
    }

    /// Canvas `applyInstitutionalTag`
    /// (mutations/apply_institutional_tag.rb).
    async fn apply_institutional_tag(
        &self,
        ctx: &Context<'_>,
        input: ApplyInstitutionalTagInput,
    ) -> Result<InstitutionalTagAssociationPayload, async_graphql::Error> {
        upsert_tag_association(ctx.data::<GraphQLContext>()?, input, true).await
    }

    /// Canvas `removeInstitutionalTag`
    /// (mutations/remove_institutional_tag.rb).
    async fn remove_institutional_tag(
        &self,
        ctx: &Context<'_>,
        input: RemoveInstitutionalTagInput,
    ) -> Result<InstitutionalTagAssociationPayload, async_graphql::Error> {
        let apply_input = ApplyInstitutionalTagInput {
            institutional_tag_id: input.institutional_tag_id,
            user_id: input.user_id,
            course_id: input.course_id,
        };
        upsert_tag_association(ctx.data::<GraphQLContext>()?, apply_input, false).await
    }

    /// Canvas `createInstitutionalTagCategory`
    /// (mutations/create_institutional_tag_category.rb).
    async fn create_institutional_tag_category(
        &self,
        ctx: &Context<'_>,
        input: CreateInstitutionalTagCategoryInput,
    ) -> Result<InstitutionalTagCategoryPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        if !gctx.is_admin() {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        if input.name.trim().is_empty() {
            return Ok(InstitutionalTagCategoryPayload {
                institutional_tag_category: None,
                errors: validation_errors(Some("name"), "name cannot be blank"),
            });
        }
        let row: Option<InstitutionalTagCategoryRow> = sqlx::query_as(
            "INSERT INTO institutional_tag_categories (account_id, name, description) \
             VALUES ($1, $2, $3) \
             RETURNING id, name, description, workflow_state, created_at, updated_at",
        )
        .bind(parse_id(&input.account_id, "account")?)
        .bind(input.name.trim())
        .bind(&input.description)
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?;
        let Some(row) = row else {
            return Err(async_graphql::Error::new(
                "institutional tag category save failed",
            ));
        };
        Ok(InstitutionalTagCategoryPayload {
            institutional_tag_category: Some(crate::root_types::InstitutionalTagCategory {
                id: row.id,
                name: row.name,
                description: row.description,
                workflow_state: row.workflow_state,
                created_at: row.created_at,
                updated_at: row.updated_at,
            }),
            errors: Vec::new(),
        })
    }

    /// Canvas `updateInstitutionalTagCategory`
    /// (mutations/update_institutional_tag_category.rb).
    async fn update_institutional_tag_category(
        &self,
        ctx: &Context<'_>,
        input: UpdateInstitutionalTagCategoryInput,
    ) -> Result<InstitutionalTagCategoryPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        if !gctx.is_admin() {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        let cid = parse_id(
            &input.institutional_tag_category_id,
            "institutional tag category",
        )?;
        if let Some(n) = &input.name {
            if n.trim().is_empty() {
                return Ok(InstitutionalTagCategoryPayload {
                    institutional_tag_category: None,
                    errors: validation_errors(Some("name"), "name cannot be blank"),
                });
            }
        }
        let row: Option<InstitutionalTagCategoryRow> = sqlx::query_as(
            "UPDATE institutional_tag_categories SET name = COALESCE($2, name), \
             description = COALESCE($3, description), updated_at = now() \
             WHERE id = $1 AND workflow_state <> 'deleted' \
             RETURNING id, name, description, workflow_state, created_at, updated_at",
        )
        .bind(cid)
        .bind(input.name.as_deref().map(str::trim))
        .bind(&input.description)
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?;
        let Some(row) = row else {
            return Err(async_graphql::Error::new(
                "institutional tag category not found",
            ));
        };
        Ok(InstitutionalTagCategoryPayload {
            institutional_tag_category: Some(crate::root_types::InstitutionalTagCategory {
                id: row.id,
                name: row.name,
                description: row.description,
                workflow_state: row.workflow_state,
                created_at: row.created_at,
                updated_at: row.updated_at,
            }),
            errors: Vec::new(),
        })
    }

    /// Canvas `updateInstitutionalTagCategoryArchivedState`
    /// (mutations/update_institutional_tag_category_archived_state.rb).
    async fn update_institutional_tag_category_archived_state(
        &self,
        ctx: &Context<'_>,
        input: UpdateInstitutionalTagCategoryArchivedStateInput,
    ) -> Result<G3SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        if !gctx.is_admin() {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        let cid = parse_id(
            &input.institutional_tag_category_id,
            "institutional tag category",
        )?;
        let res = sqlx::query(
            "UPDATE institutional_tag_categories SET workflow_state = $2, updated_at = now() \
             WHERE id = $1",
        )
        .bind(cid)
        .bind(if input.archived { "archived" } else { "active" })
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new(
                "institutional tag category not found",
            ));
        }
        Ok(G3SuccessPayload { success: true })
    }

    /// Canvas `createUserInboxLabel`
    /// (mutations/create_user_inbox_label.rb; 0230).
    async fn create_user_inbox_label(
        &self,
        ctx: &Context<'_>,
        input: CreateUserInboxLabelInput,
    ) -> Result<UserInboxLabelPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        if input.name.trim().is_empty() {
            return Ok(UserInboxLabelPayload {
                inbox_label: None,
                errors: validation_errors(Some("name"), "name cannot be blank"),
            });
        }
        let (lid,): (i64,) = sqlx::query_as(
            "INSERT INTO user_inbox_labels (user_id, name, color) \
             VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(gctx.auth_user.id)
        .bind(input.name.trim())
        .bind(&input.color)
        .fetch_one(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(UserInboxLabelPayload {
            inbox_label: Some(serde_json::json!({
                "id": lid.to_string(),
                "name": input.name.trim(),
                "color": input.color,
            })),
            errors: Vec::new(),
        })
    }

    /// Canvas `deleteUserInboxLabel`
    /// (mutations/delete_user_inbox_label.rb).
    async fn delete_user_inbox_label(
        &self,
        ctx: &Context<'_>,
        input: DeleteUserInboxLabelInput,
    ) -> Result<G3SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let lid = parse_id(&input.inbox_label_id, "inbox label")?;
        let res = sqlx::query("DELETE FROM user_inbox_labels WHERE id = $1 AND user_id = $2")
            .bind(lid)
            .bind(gctx.auth_user.id)
            .execute(&gctx.pool)
            .await
            .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("inbox label not found"));
        }
        Ok(G3SuccessPayload { success: true })
    }

    /// Canvas `updateSpeedGraderSettings`
    /// (mutations/update_speed_grader_settings.rb; 0230).
    async fn update_speed_grader_settings(
        &self,
        ctx: &Context<'_>,
        input: UpdateSpeedGraderSettingsInput,
    ) -> Result<SpeedGraderSettingsPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let course_id = match &input.course_id {
            Some(cid) => {
                let raw = parse_id(cid, "course")?;
                Some(raw)
            }
            None => None,
        };
        if let Some(cid) = course_id {
            if !is_teacher_or_admin(gctx, cid).await? {
                return Err(async_graphql::Error::new("insufficient permission"));
            }
        }
        let row: Option<(i64,)> = sqlx::query_as(
            r#"
            INSERT INTO speed_grader_settings (user_id, course_id, settings)
            VALUES ($1, $2, $3)
            ON CONFLICT (user_id, course_id)
            DO UPDATE SET settings = EXCLUDED.settings, updated_at = now()
            RETURNING id
            "#,
        )
        .bind(gctx.auth_user.id)
        .bind(course_id)
        .bind(&input.settings)
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?;
        let Some((sid,)) = row else {
            return Err(async_graphql::Error::new(
                "speed grader settings save failed",
            ));
        };
        Ok(SpeedGraderSettingsPayload {
            speed_grader_settings: Some(crate::entities_batch_f::SpeedGraderSettings {
                id: ID(sid.to_string()),
                user_id: ID(gctx.auth_user.id.to_string()),
                course_id: input.course_id.clone(),
                settings: Some(input.settings.clone()),
            }),
            errors: Vec::new(),
        })
    }

    /// Canvas `createAllocationRule`
    /// (mutations/create_allocation_rule.rb; 0224).
    async fn create_allocation_rule(
        &self,
        ctx: &Context<'_>,
        input: CreateAllocationRuleInput,
    ) -> Result<AllocationRulePayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let aid = parse_id(&input.assignment_id, "assignment")?;
        let course: Option<(i64,)> =
            sqlx::query_as("SELECT course_id FROM assignments WHERE id = $1")
                .bind(aid)
                .fetch_optional(&gctx.pool)
                .await
                .map_err(db_err)?;
        let Some((course_id,)) = course else {
            return Err(async_graphql::Error::new("assignment not found"));
        };
        if !is_teacher_or_admin(gctx, course_id).await? {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        let (rid,): (i64,) =
            sqlx::query_as("INSERT INTO allocation_rules (assignment_id) VALUES ($1) RETURNING id")
                .bind(aid)
                .fetch_one(&gctx.pool)
                .await
                .map_err(db_err)?;
        Ok(AllocationRulePayload {
            allocation_rule: Some(crate::entities_batch_f::AllocationRule {
                id: ID(rid.to_string()),
                assignment_id: Some(input.assignment_id.clone()),
                workflow_state: Some("active".to_string()),
            }),
            errors: Vec::new(),
        })
    }

    /// Canvas `updateAllocationRule`
    /// (mutations/update_allocation_rule.rb).
    async fn update_allocation_rule(
        &self,
        ctx: &Context<'_>,
        input: UpdateAllocationRuleInput,
    ) -> Result<AllocationRulePayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let rid = parse_id(&input.allocation_rule_id, "allocation rule")?;
        let row: Option<(i64, Option<i64>)> = sqlx::query_as(
            "UPDATE allocation_rules SET updated_at = now() WHERE id = $1 \
             AND workflow_state <> 'deleted' RETURNING id, assignment_id",
        )
        .bind(rid)
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?;
        let Some((id, assignment_id)) = row else {
            return Err(async_graphql::Error::new("allocation rule not found"));
        };
        let course_ok = match assignment_id {
            Some(aid) => {
                let course: Option<(i64,)> =
                    sqlx::query_as("SELECT course_id FROM assignments WHERE id = $1")
                        .bind(aid)
                        .fetch_optional(&gctx.pool)
                        .await
                        .map_err(db_err)?;
                match course {
                    Some((cid,)) => is_teacher_or_admin(gctx, cid).await?,
                    None => false,
                }
            }
            None => false,
        };
        if !course_ok {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        let _ = &input.data;
        Ok(AllocationRulePayload {
            allocation_rule: Some(crate::entities_batch_f::AllocationRule {
                id: ID(id.to_string()),
                assignment_id: assignment_id.map(|a| ID(a.to_string())),
                workflow_state: Some("active".to_string()),
            }),
            errors: Vec::new(),
        })
    }

    /// Canvas `deleteAllocationRule`
    /// (mutations/delete_allocation_rule.rb).
    async fn delete_allocation_rule(
        &self,
        ctx: &Context<'_>,
        input: DeleteAllocationRuleInput,
    ) -> Result<G3SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let rid = parse_id(&input.allocation_rule_id, "allocation rule")?;
        let res = sqlx::query(
            "UPDATE allocation_rules SET workflow_state = 'deleted', updated_at = now() \
             WHERE id = $1",
        )
        .bind(rid)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("allocation rule not found"));
        }
        Ok(G3SuccessPayload { success: true })
    }

    /// Canvas `updateGradebookGroupFilter`
    /// (mutations/update_gradebook_group_filter.rb; 0230).
    async fn update_gradebook_group_filter(
        &self,
        ctx: &Context<'_>,
        input: UpdateGradebookGroupFilterInput,
    ) -> Result<GradebookFilterPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let cid = parse_id(&input.course_id, "course")?;
        if !is_teacher_or_admin(gctx, cid).await? {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        let (fid,): (i64,) = sqlx::query_as(
            "INSERT INTO gradebook_user_filters (user_id, course_id, filter, position) \
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(gctx.auth_user.id)
        .bind(cid)
        .bind(&input.filter)
        .bind(input.position.unwrap_or(0))
        .fetch_one(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(GradebookFilterPayload {
            gradebook_filter: Some(serde_json::json!({
                "id": fid.to_string(),
                "courseId": input.course_id.0,
                "filter": input.filter,
            })),
            errors: Vec::new(),
        })
    }

    /// Canvas `updateLearnerDashboardTabSelection`
    /// (mutations/update_learner_dashboard_tab_selection.rb; 0230).
    async fn update_learner_dashboard_tab_selection(
        &self,
        ctx: &Context<'_>,
        input: UpdateLearnerDashboardTabSelectionInput,
    ) -> Result<LearnerDashboardTabSelectionPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        if input.tab_handle.trim().is_empty() {
            return Ok(LearnerDashboardTabSelectionPayload {
                tab_handle: input.tab_handle.clone(),
                success: false,
                errors: validation_errors(Some("tabHandle"), "tab handle cannot be blank"),
            });
        }
        let res = sqlx::query(
            r#"
            INSERT INTO learner_dashboard_tab_selections
                (user_id, tab_handle, context_type, context_id)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (user_id, tab_handle, context_type, context_id)
            DO UPDATE SET updated_at = now()
            "#,
        )
        .bind(gctx.auth_user.id)
        .bind(input.tab_handle.trim())
        .bind(&input.context_type)
        .bind(
            input
                .context_id
                .as_ref()
                .map(|c| parse_id(c, "context"))
                .transpose()?,
        )
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("tab selection save failed"));
        }
        Ok(LearnerDashboardTabSelectionPayload {
            tab_handle: input.tab_handle.clone(),
            success: true,
            errors: Vec::new(),
        })
    }

    /// Canvas `updateWidgetDashboardConfig`
    /// (mutations/update_widget_dashboard_config.rb; 0230).
    async fn update_widget_dashboard_config(
        &self,
        ctx: &Context<'_>,
        input: UpdateWidgetDashboardConfigInput,
    ) -> Result<WidgetDashboardPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let widget = serde_json::json!({
            "widget": input.widget,
            "layout": input.layout,
            "settings": input.settings,
        });
        let res = sqlx::query(
            r#"
            INSERT INTO widget_dashboards (user_id, context_type, context_id, layout)
            VALUES ($1, 'Course', NULL, $2::jsonb)
            ON CONFLICT (user_id, context_type, context_id)
            DO UPDATE SET layout = EXCLUDED.layout, updated_at = now()
            "#,
        )
        .bind(gctx.auth_user.id)
        .bind(&widget)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("widget dashboard save failed"));
        }
        Ok(WidgetDashboardPayload {
            widget_dashboards: vec![widget],
            errors: Vec::new(),
        })
    }

    /// Canvas `updateWidgetDashboardLayout`
    /// (mutations/update_widget_dashboard_layout.rb).
    async fn update_widget_dashboard_layout(
        &self,
        ctx: &Context<'_>,
        input: UpdateWidgetDashboardLayoutInput,
    ) -> Result<WidgetDashboardPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let res = sqlx::query(
            r#"
            INSERT INTO widget_dashboards (user_id, context_type, context_id, layout)
            VALUES ($1, 'Course', NULL, $2::jsonb)
            ON CONFLICT (user_id, context_type, context_id)
            DO UPDATE SET layout = EXCLUDED.layout, updated_at = now()
            "#,
        )
        .bind(gctx.auth_user.id)
        .bind(&input.layout)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("widget dashboard save failed"));
        }
        Ok(WidgetDashboardPayload {
            widget_dashboards: vec![input.layout.clone()],
            errors: Vec::new(),
        })
    }

    /// Canvas `updateUserDiscussionsSplitscreenView`
    /// (mutations/update_user_discussions_splitscreen_view.rb) — a
    /// per-user preference flag (user_preferences row; documented
    /// shape).
    async fn update_user_discussions_splitscreen_view(
        &self,
        ctx: &Context<'_>,
        input: UpdateUserDiscussionsSplitscreenViewInput,
    ) -> Result<SplitscreenPreferencePayload, async_graphql::Error> {
        set_user_preference(
            ctx.data::<GraphQLContext>()?,
            "discussions_splitscreen_view",
            input.enabled,
        )
        .await
    }

    /// Canvas `updateSplitScreenViewDeeplyNestedAlert`
    /// (mutations/update_split_screen_view_deeply_nested_alert.rb).
    async fn update_split_screen_view_deeply_nested_alert(
        &self,
        ctx: &Context<'_>,
        input: UpdateSplitScreenViewDeeplyNestedAlertInput,
    ) -> Result<SplitscreenPreferencePayload, async_graphql::Error> {
        set_user_preference(
            ctx.data::<GraphQLContext>()?,
            "split_screen_view_deeply_nested_alert",
            input.dismissed,
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// Implementations
// ---------------------------------------------------------------------------

async fn upsert_tag_association(
    gctx: &GraphQLContext,
    input: ApplyInstitutionalTagInput,
    apply: bool,
) -> Result<InstitutionalTagAssociationPayload, async_graphql::Error> {
    let tid = parse_id(&input.institutional_tag_id, "institutional tag")?;
    let uid = parse_id(&input.user_id, "user")?;
    let cid = parse_id(&input.course_id, "course")?;
    if !gctx.is_admin() {
        return Err(async_graphql::Error::new("insufficient permission"));
    }
    if apply {
        let (aid,): (i64,) = sqlx::query_as(
            r#"
            INSERT INTO institutional_tag_associations
                (institutional_tag_id, user_id, course_id)
            VALUES ($1, $2, $3)
            ON CONFLICT (institutional_tag_id, user_id, course_id)
            DO UPDATE SET workflow_state = 'active', updated_at = now()
            RETURNING id
            "#,
        )
        .bind(tid)
        .bind(uid)
        .bind(cid)
        .fetch_one(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(InstitutionalTagAssociationPayload {
            institutional_tag_association: Some(
                crate::entities_batch_f::InstitutionalTagAssociation {
                    id: ID(aid.to_string()),
                    institutional_tag_id: input.institutional_tag_id.clone(),
                    user_id: Some(input.user_id.clone()),
                    course_id: Some(input.course_id.clone()),
                    workflow_state: Some("active".to_string()),
                },
            ),
            errors: Vec::new(),
        })
    } else {
        sqlx::query(
            "UPDATE institutional_tag_associations SET workflow_state = 'deleted', \
             updated_at = now() \
             WHERE institutional_tag_id = $1 AND user_id = $2 AND course_id = $3",
        )
        .bind(tid)
        .bind(uid)
        .bind(cid)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(InstitutionalTagAssociationPayload {
            institutional_tag_association: None,
            errors: Vec::new(),
        })
    }
}

async fn set_user_preference(
    gctx: &GraphQLContext,
    key: &str,
    value: bool,
) -> Result<SplitscreenPreferencePayload, async_graphql::Error> {
    let res = sqlx::query(
        r#"
        INSERT INTO user_preferences (user_id, preferences)
        VALUES ($1, jsonb_build_object($2, $3))
        ON CONFLICT (user_id)
        DO UPDATE SET
            preferences = user_preferences.preferences
                || jsonb_build_object($2, $3),
            updated_at = now()
        "#,
    )
    .bind(gctx.auth_user.id)
    .bind(key)
    .bind(value)
    .execute(&gctx.pool)
    .await
    .map_err(db_err)?;
    if res.rows_affected() == 0 {
        return Err(async_graphql::Error::new("preference save failed"));
    }
    Ok(SplitscreenPreferencePayload {
        success: true,
        errors: Vec::new(),
    })
}
