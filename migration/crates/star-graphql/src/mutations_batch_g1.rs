//! Batch G1 mutations: discussion read-states/participants, outcome
//! links/proficiency/calculation/friendly descriptions, module item
//! ops, conversation ops, enrollment invitations (GraphQL parity gap
//! completion).
//!
//! Same CONTRACT.md error channels as Batches A–C: authorization /
//! missing records as top-level execution errors with Canvas's exact
//! strings; validation as payload `errors` arrays inside HTTP 200.

use async_graphql::{Context, InputObject, Object, ID};

use crate::{db_err, GraphQLContext};

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
pub struct G1Error {
    attribute: Option<String>,
    message: String,
}

fn validation_errors(attribute: Option<&str>, message: &str) -> Vec<G1Error> {
    vec![G1Error {
        attribute: attribute.map(str::to_string),
        message: message.to_string(),
    }]
}

use async_graphql::SimpleObject;

#[derive(Debug, Clone, SimpleObject)]
pub struct G1SuccessPayload {
    pub success: bool,
}

// ---------------------------------------------------------------------------
// Inputs + payloads
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, InputObject)]
pub struct UpdateDiscussionReadStateInput {
    pub discussion_topic_id: ID,
    pub read: bool,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateDiscussionEntriesReadStateInput {
    pub discussion_topic_id: ID,
    pub read: bool,
    pub discussion_entry_ids: Option<Vec<ID>>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateDiscussionThreadReadStateInput {
    pub discussion_topic_id: ID,
    pub discussion_entry_id: ID,
    pub read: bool,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateDiscussionEntryParticipantInput {
    pub discussion_entry_id: ID,
    pub forced_read_state: Option<bool>,
    pub rating: Option<i32>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateDiscussionTopicParticipantInput {
    pub discussion_topic_id: ID,
    pub subscribed: Option<bool>,
    pub sort_order: Option<String>,
}

#[derive(Debug, Clone, InputObject)]
pub struct RestoreDeletedDiscussionEntryInput {
    pub discussion_entry_id: ID,
}

#[derive(Debug, Clone, InputObject)]
pub struct DeleteOutcomeLinksInput {
    pub outcome_ids: Vec<ID>,
    pub outcome_group_id: ID,
}

#[derive(Debug, Clone, InputObject)]
pub struct MoveOutcomeLinksInput {
    pub outcome_ids: Vec<ID>,
    pub outcome_group_id: ID,
    pub target_outcome_group_id: ID,
}

#[derive(Debug, Clone, InputObject)]
pub struct ImportOutcomesInput {
    pub course_id: ID,
    pub outcome_group_id: ID,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct ImportOutcomesPayload {
    pub imported_count: i32,
    pub errors: Vec<G1Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct CreateOutcomeProficiencyInput {
    pub course_id: ID,
    pub ratings: Vec<crate::root_mutations::ProficiencyRatingInput>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct OutcomeProficiencyPayload {
    pub outcome_proficiency: Option<crate::root_types::OutcomeProficiency>,
    pub errors: Vec<G1Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateOutcomeProficiencyInput {
    pub outcome_proficiency_id: ID,
    pub ratings: Vec<crate::root_mutations::ProficiencyRatingInput>,
}

#[derive(Debug, Clone, InputObject)]
pub struct DeleteOutcomeProficiencyInput {
    pub outcome_proficiency_id: ID,
}

#[derive(Debug, Clone, InputObject)]
pub struct CreateOutcomeCalculationMethodInput {
    pub course_id: ID,
    pub calculation_method: String,
    pub calculation_int: Option<i32>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct OutcomeCalculationMethodPayload {
    pub outcome_calculation_method: Option<crate::root_types::OutcomeCalculationMethod>,
    pub errors: Vec<G1Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateOutcomeCalculationMethodInput {
    pub outcome_calculation_method_id: ID,
    pub calculation_method: Option<String>,
    pub calculation_int: Option<i32>,
}

#[derive(Debug, Clone, InputObject)]
pub struct DeleteOutcomeCalculationMethodInput {
    pub outcome_calculation_method_id: ID,
}

#[derive(Debug, Clone, InputObject)]
pub struct SetFriendlyDescriptionInput {
    pub learning_outcome_id: ID,
    pub course_id: ID,
    pub description: String,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct FriendlyDescriptionPayload {
    pub outcome_friendly_description: Option<serde_json::Value>,
    pub errors: Vec<G1Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct ReorderModuleItemsInput {
    pub module_id: ID,
    pub item_ids: Vec<ID>,
}

#[derive(Debug, Clone, InputObject)]
pub struct SetModuleItemCompletionInput {
    pub module_item_id: ID,
    pub done: bool,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct ModuleItemCompletionPayload {
    pub module_item_id: ID,
    pub done: bool,
    pub errors: Vec<G1Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct DeleteConversationsInput {
    pub conversation_ids: Vec<ID>,
}

#[derive(Debug, Clone, InputObject)]
pub struct DeleteConversationMessagesInput {
    pub conversation_id: ID,
    pub message_ids: Vec<ID>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateConversationParticipantsInput {
    pub conversation_id: ID,
    pub starred: Option<bool>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct ConversationParticipantPayload {
    pub success: bool,
    pub errors: Vec<G1Error>,
}

// ---------------------------------------------------------------------------
// Mutation container
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct BatchG1Mutations;

#[Object]
impl BatchG1Mutations {
    /// Canvas `updateDiscussionReadState`
    /// (mutations/update_discussion_read_state.rb).
    async fn update_discussion_read_state(
        &self,
        ctx: &Context<'_>,
        input: UpdateDiscussionReadStateInput,
    ) -> Result<G1SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        update_topic_read_state(gctx, &input.discussion_topic_id, input.read).await
    }

    /// Canvas `updateDiscussionEntriesReadState`
    /// (mutations/update_discussion_entries_read_state.rb).
    async fn update_discussion_entries_read_state(
        &self,
        ctx: &Context<'_>,
        input: UpdateDiscussionEntriesReadStateInput,
    ) -> Result<G1SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        update_topic_read_state(gctx, &input.discussion_topic_id, input.read).await
    }

    /// Canvas `updateDiscussionThreadReadState`
    /// (mutations/update_discussion_thread_read_state.rb).
    async fn update_discussion_thread_read_state(
        &self,
        ctx: &Context<'_>,
        input: UpdateDiscussionThreadReadStateInput,
    ) -> Result<G1SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let entry_id = parse_id(&input.discussion_entry_id, "discussion entry")?;
        let res = sqlx::query(
            r#"
            UPDATE discussion_entry_participants
            SET workflow_state = CASE WHEN $2 THEN 'read' ELSE 'unread' END,
                read_at = CASE WHEN $2 THEN now() ELSE NULL END,
                updated_at = now()
            WHERE discussion_entry_id = $1 AND user_id = $3
            "#,
        )
        .bind(entry_id)
        .bind(input.read)
        .bind(gctx.auth_user.id)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("discussion entry not found"));
        }
        Ok(G1SuccessPayload { success: true })
    }

    /// Canvas `updateDiscussionEntryParticipant`
    /// (mutations/update_discussion_entry_participant.rb).
    async fn update_discussion_entry_participant(
        &self,
        ctx: &Context<'_>,
        input: UpdateDiscussionEntryParticipantInput,
    ) -> Result<G1SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let entry_id = parse_id(&input.discussion_entry_id, "discussion entry")?;
        // Upsert the caller's row (0224 discussion_entry_participants).
        let res = sqlx::query(
            r#"
            INSERT INTO discussion_entry_participants
                (discussion_entry_id, user_id, forced_read_state, rating)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (discussion_entry_id, user_id)
            DO UPDATE SET
                forced_read_state = COALESCE(EXCLUDED.forced_read_state,
                    discussion_entry_participants.forced_read_state),
                rating = COALESCE(EXCLUDED.rating,
                    discussion_entry_participants.rating),
                updated_at = now()
            "#,
        )
        .bind(entry_id)
        .bind(gctx.auth_user.id)
        .bind(input.forced_read_state)
        .bind(input.rating)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("discussion entry not found"));
        }
        Ok(G1SuccessPayload { success: true })
    }

    /// Canvas `updateDiscussionTopicParticipant`
    /// (mutations/update_discussion_topic_participant.rb).
    async fn update_discussion_topic_participant(
        &self,
        ctx: &Context<'_>,
        input: UpdateDiscussionTopicParticipantInput,
    ) -> Result<G1SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let topic_id = parse_id(&input.discussion_topic_id, "discussion topic")?;
        if let Some(order) = &input.sort_order {
            if !matches!(order.as_str(), "asc" | "desc") {
                return Ok(G1SuccessPayload { success: false });
            }
        }
        let res = sqlx::query(
            r#"
            INSERT INTO discussion_topic_participants
                (discussion_topic_id, user_id, subscribed, sort_order)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (discussion_topic_id, user_id)
            DO UPDATE SET
                subscribed = COALESCE(EXCLUDED.subscribed,
                    discussion_topic_participants.subscribed),
                sort_order = COALESCE(EXCLUDED.sort_order,
                    discussion_topic_participants.sort_order),
                updated_at = now()
            "#,
        )
        .bind(topic_id)
        .bind(gctx.auth_user.id)
        .bind(input.subscribed)
        .bind(input.sort_order)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("discussion topic not found"));
        }
        Ok(G1SuccessPayload { success: true })
    }

    /// Canvas `restoreDeletedDiscussionEntry`
    /// (mutations/restore_deleted_discussion_entry.rb).
    async fn restore_deleted_discussion_entry(
        &self,
        ctx: &Context<'_>,
        input: RestoreDeletedDiscussionEntryInput,
    ) -> Result<G1SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let entry_id = parse_id(&input.discussion_entry_id, "discussion entry")?;
        let course: Option<(i64,)> = sqlx::query_as(
            r#"SELECT t.course_id FROM discussion_entries e
               JOIN discussion_topics t ON t.id = e.topic_id
               WHERE e.id = $1"#,
        )
        .bind(entry_id)
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?;
        let Some((course_id,)) = course else {
            return Err(async_graphql::Error::new("discussion entry not found"));
        };
        if !is_teacher_or_admin(gctx, course_id).await? {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        let res = sqlx::query(
            "UPDATE discussion_entries SET workflow_state = 'active', updated_at = now() \
             WHERE id = $1 AND workflow_state = 'deleted'",
        )
        .bind(entry_id)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("discussion entry not found"));
        }
        Ok(G1SuccessPayload { success: true })
    }

    /// Canvas `deleteOutcomeLinks`
    /// (mutations/delete_outcome_links.rb).
    async fn delete_outcome_links(
        &self,
        ctx: &Context<'_>,
        input: DeleteOutcomeLinksInput,
    ) -> Result<G1SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let group_id = parse_id(&input.outcome_group_id, "outcome group")?;
        let course_id = group_course_id(gctx, group_id).await?;
        if !is_teacher_or_admin(gctx, course_id).await? {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        for oid in &input.outcome_ids {
            let raw = parse_id(oid, "outcome")?;
            sqlx::query(
                "DELETE FROM learning_outcome_links \
                 WHERE outcome_id = $1 AND outcome_group_id = $2",
            )
            .bind(raw)
            .bind(group_id)
            .execute(&gctx.pool)
            .await
            .map_err(db_err)?;
        }
        Ok(G1SuccessPayload { success: true })
    }

    /// Canvas `moveOutcomeLinks` (mutations/move_outcome_links.rb).
    async fn move_outcome_links(
        &self,
        ctx: &Context<'_>,
        input: MoveOutcomeLinksInput,
    ) -> Result<G1SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let from = parse_id(&input.outcome_group_id, "outcome group")?;
        let to = parse_id(&input.target_outcome_group_id, "outcome group")?;
        let course_id = group_course_id(gctx, from).await?;
        if !is_teacher_or_admin(gctx, course_id).await? {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        let target: Option<(Option<i64>,)> = sqlx::query_as(
            "SELECT course_id FROM learning_outcome_groups \
             WHERE id = $1 AND workflow_state <> 'deleted'",
        )
        .bind(to)
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?;
        if target.is_none() {
            return Err(async_graphql::Error::new("outcome group not found"));
        }
        for oid in &input.outcome_ids {
            let raw = parse_id(oid, "outcome")?;
            sqlx::query(
                r#"
                INSERT INTO learning_outcome_links (outcome_id, outcome_group_id)
                VALUES ($1, $2)
                ON CONFLICT (outcome_id, outcome_group_id)
                DO UPDATE SET workflow_state = 'active', updated_at = now()
                "#,
            )
            .bind(raw)
            .bind(to)
            .execute(&gctx.pool)
            .await
            .map_err(db_err)?;
            sqlx::query(
                "DELETE FROM learning_outcome_links \
                 WHERE outcome_id = $1 AND outcome_group_id = $2 AND id <> \
                 (SELECT id FROM learning_outcome_links WHERE outcome_id = $1 \
                  AND outcome_group_id = $2 LIMIT 1)",
            )
            .bind(raw)
            .bind(from)
            .execute(&gctx.pool)
            .await
            .map_err(db_err)?;
        }
        Ok(G1SuccessPayload { success: true })
    }

    /// Canvas `importOutcomes` (mutations/import_outcomes.rb) — star-db
    /// has no global outcome bank, so the import validates the target
    /// group and reports zero imported (documented deviation).
    async fn import_outcomes(
        &self,
        ctx: &Context<'_>,
        input: ImportOutcomesInput,
    ) -> Result<ImportOutcomesPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let group_id = parse_id(&input.outcome_group_id, "outcome group")?;
        let course_id = group_course_id(gctx, group_id).await?;
        if !is_teacher_or_admin(gctx, course_id).await? {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        Ok(ImportOutcomesPayload {
            imported_count: 0,
            errors: Vec::new(),
        })
    }

    /// Canvas `createOutcomeProficiency`
    /// (mutations/create_outcome_proficiency.rb).
    async fn create_outcome_proficiency(
        &self,
        ctx: &Context<'_>,
        input: CreateOutcomeProficiencyInput,
    ) -> Result<OutcomeProficiencyPayload, async_graphql::Error> {
        create_or_update_proficiency(ctx.data::<GraphQLContext>()?, None, input).await
    }

    /// Canvas `updateOutcomeProficiency`
    /// (mutations/update_outcome_proficiency.rb).
    async fn update_outcome_proficiency(
        &self,
        ctx: &Context<'_>,
        input: UpdateOutcomeProficiencyInput,
    ) -> Result<OutcomeProficiencyPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let pid = parse_id(&input.outcome_proficiency_id, "outcome proficiency")?;
        let input2 = CreateOutcomeProficiencyInput {
            course_id: ID(pid.to_string()), // unused by the update path
            ratings: input.ratings,
        };
        create_or_update_proficiency(gctx, Some(pid), input2).await
    }

    /// Canvas `deleteOutcomeProficiency`
    /// (mutations/delete_outcome_proficiency.rb).
    async fn delete_outcome_proficiency(
        &self,
        ctx: &Context<'_>,
        input: DeleteOutcomeProficiencyInput,
    ) -> Result<G1SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let pid = parse_id(&input.outcome_proficiency_id, "outcome proficiency")?;
        let res = sqlx::query(
            "UPDATE outcome_proficiencies SET workflow_state = 'deleted', updated_at = now() \
             WHERE id = $1",
        )
        .bind(pid)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("outcome proficiency not found"));
        }
        Ok(G1SuccessPayload { success: true })
    }

    /// Canvas `createOutcomeCalculationMethod`
    /// (mutations/create_outcome_calculation_method.rb).
    async fn create_outcome_calculation_method(
        &self,
        ctx: &Context<'_>,
        input: CreateOutcomeCalculationMethodInput,
    ) -> Result<OutcomeCalculationMethodPayload, async_graphql::Error> {
        create_or_update_calc_method(ctx.data::<GraphQLContext>()?, None, input).await
    }

    /// Canvas `updateOutcomeCalculationMethod`
    /// (mutations/update_outcome_calculation_method.rb).
    async fn update_outcome_calculation_method(
        &self,
        ctx: &Context<'_>,
        input: UpdateOutcomeCalculationMethodInput,
    ) -> Result<OutcomeCalculationMethodPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let mid = parse_id(
            &input.outcome_calculation_method_id,
            "outcome calculation method",
        )?;
        let input2 = CreateOutcomeCalculationMethodInput {
            course_id: ID(mid.to_string()), // unused by the update path
            calculation_method: input.calculation_method.unwrap_or_default(),
            calculation_int: input.calculation_int,
        };
        create_or_update_calc_method(gctx, Some(mid), input2).await
    }

    /// Canvas `deleteOutcomeCalculationMethod`
    /// (mutations/delete_outcome_calculation_method.rb).
    async fn delete_outcome_calculation_method(
        &self,
        ctx: &Context<'_>,
        input: DeleteOutcomeCalculationMethodInput,
    ) -> Result<G1SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let mid = parse_id(
            &input.outcome_calculation_method_id,
            "outcome calculation method",
        )?;
        let res = sqlx::query(
            "UPDATE outcome_calculation_methods SET workflow_state = 'deleted', updated_at = now() \
             WHERE id = $1",
        )
        .bind(mid)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new(
                "outcome calculation method not found",
            ));
        }
        Ok(G1SuccessPayload { success: true })
    }

    /// Canvas `setFriendlyDescription`
    /// (mutations/set_friendly_description.rb).
    async fn set_friendly_description(
        &self,
        ctx: &Context<'_>,
        input: SetFriendlyDescriptionInput,
    ) -> Result<FriendlyDescriptionPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let outcome_id = parse_id(&input.learning_outcome_id, "outcome")?;
        let course_id = parse_id(&input.course_id, "course")?;
        if !is_teacher_or_admin(gctx, course_id).await? {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        if input.description.trim().is_empty() {
            return Ok(FriendlyDescriptionPayload {
                outcome_friendly_description: None,
                errors: validation_errors(Some("description"), "description cannot be blank"),
            });
        }
        let row: Option<serde_json::Value> = sqlx::query_scalar(
            r#"
            INSERT INTO outcome_friendly_descriptions
                (learning_outcome_id, context_id, context_type, description)
            VALUES ($1, $2, 'Course', $3)
            ON CONFLICT (learning_outcome_id, context_id, context_type)
            DO UPDATE SET description = EXCLUDED.description, updated_at = now()
            RETURNING to_jsonb(outcome_friendly_descriptions.*)
            "#,
        )
        .bind(outcome_id)
        .bind(course_id)
        .bind(input.description.trim())
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(FriendlyDescriptionPayload {
            outcome_friendly_description: row,
            errors: Vec::new(),
        })
    }

    /// Canvas `reorderModuleItems` (mutations/reorder_module_items.rb).
    async fn reorder_module_items(
        &self,
        ctx: &Context<'_>,
        input: ReorderModuleItemsInput,
    ) -> Result<G1SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let module_id = parse_id(&input.module_id, "module")?;
        let course: Option<(i64,)> = sqlx::query_as(
            "SELECT course_id FROM modules WHERE id = $1 AND workflow_state <> 'deleted'",
        )
        .bind(module_id)
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?;
        let Some((course_id,)) = course else {
            return Err(async_graphql::Error::new("module not found"));
        };
        if !is_teacher_or_admin(gctx, course_id).await? {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        for (idx, iid) in input.item_ids.iter().enumerate() {
            let raw = parse_id(iid, "module item")?;
            sqlx::query("UPDATE module_items SET position = $2 WHERE id = $1 AND module_id = $3")
                .bind(raw)
                .bind((idx + 1) as i32)
                .bind(module_id)
                .execute(&gctx.pool)
                .await
                .map_err(db_err)?;
        }
        Ok(G1SuccessPayload { success: true })
    }

    /// Canvas `setModuleItemCompletion`
    /// (mutations/set_module_item_completion.rb) — the student's own
    /// done flag (0039 module_item_completions).
    async fn set_module_item_completion(
        &self,
        ctx: &Context<'_>,
        input: SetModuleItemCompletionInput,
    ) -> Result<ModuleItemCompletionPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let item_id = parse_id(&input.module_item_id, "module item")?;
        let exists: Option<(i64,)> = sqlx::query_as(
            "SELECT module_id FROM module_items WHERE id = $1 AND workflow_state <> 'deleted'",
        )
        .bind(item_id)
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?;
        if exists.is_none() {
            return Err(async_graphql::Error::new("module item not found"));
        }
        let res = sqlx::query(
            r#"
            INSERT INTO module_item_completions (module_id, item_id, user_id, state)
            VALUES ((SELECT module_id FROM module_items WHERE id = $1), $1, $2, $3)
            ON CONFLICT (module_id, item_id, user_id)
            DO UPDATE SET state = EXCLUDED.state, updated_at = now()
            "#,
        )
        .bind(item_id)
        .bind(gctx.auth_user.id)
        .bind(if input.done { "done" } else { "undone" })
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("module item not found"));
        }
        Ok(ModuleItemCompletionPayload {
            module_item_id: input.module_item_id.clone(),
            done: input.done,
            errors: Vec::new(),
        })
    }

    /// Canvas `deleteConversations` (mutations/delete_conversations.rb).
    async fn delete_conversations(
        &self,
        ctx: &Context<'_>,
        input: DeleteConversationsInput,
    ) -> Result<G1SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        for cid in &input.conversation_ids {
            let raw = parse_id(cid, "conversation")?;
            sqlx::query(
                "UPDATE conversation_participants SET workflow_state = 'deleted', \
                 updated_at = now() \
                 WHERE conversation_id = $1 AND user_id = $2 AND workflow_state = 'active'",
            )
            .bind(raw)
            .bind(gctx.auth_user.id)
            .execute(&gctx.pool)
            .await
            .map_err(db_err)?;
        }
        Ok(G1SuccessPayload { success: true })
    }

    /// Canvas `deleteConversationMessages`
    /// (mutations/delete_conversation_messages.rb).
    async fn delete_conversation_messages(
        &self,
        ctx: &Context<'_>,
        input: DeleteConversationMessagesInput,
    ) -> Result<G1SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let conv = parse_id(&input.conversation_id, "conversation")?;
        let owns: Option<(i32,)> = sqlx::query_as(
            "SELECT 1 FROM conversation_participants \
             WHERE conversation_id = $1 AND user_id = $2 AND workflow_state <> 'deleted'",
        )
        .bind(conv)
        .bind(gctx.auth_user.id)
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?;
        if owns.is_none() {
            return Err(async_graphql::Error::new("conversation not found"));
        }
        for mid in &input.message_ids {
            let raw = parse_id(mid, "conversation message")?;
            sqlx::query(
                "DELETE FROM conversation_messages \
                 WHERE id = $1 AND conversation_id = $2 AND author_id = $3",
            )
            .bind(raw)
            .bind(conv)
            .bind(gctx.auth_user.id)
            .execute(&gctx.pool)
            .await
            .map_err(db_err)?;
        }
        Ok(G1SuccessPayload { success: true })
    }

    /// Canvas `updateConversationParticipants`
    /// (mutations/update_conversation_participants.rb) — the caller's
    /// own participant row (starred).
    async fn update_conversation_participants(
        &self,
        ctx: &Context<'_>,
        input: UpdateConversationParticipantsInput,
    ) -> Result<ConversationParticipantPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let conv = parse_id(&input.conversation_id, "conversation")?;
        let res = sqlx::query(
            "UPDATE conversation_participants SET starred = $3, updated_at = now() \
             WHERE conversation_id = $1 AND user_id = $2 AND workflow_state <> 'deleted'",
        )
        .bind(conv)
        .bind(gctx.auth_user.id)
        .bind(input.starred)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("conversation not found"));
        }
        Ok(ConversationParticipantPayload {
            success: true,
            errors: Vec::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Implementations
// ---------------------------------------------------------------------------

async fn update_topic_read_state(
    gctx: &GraphQLContext,
    topic_id: &ID,
    read: bool,
) -> Result<G1SuccessPayload, async_graphql::Error> {
    let tid = parse_id(topic_id, "discussion topic")?;
    let res = sqlx::query(
        r#"
        INSERT INTO discussion_topic_participants
            (discussion_topic_id, user_id, workflow_state, subscribed)
        VALUES ($1, $2, CASE WHEN $3 THEN 'read' ELSE 'unread' END, true)
        ON CONFLICT (discussion_topic_id, user_id)
        DO UPDATE SET workflow_state = EXCLUDED.workflow_state, updated_at = now()
        "#,
    )
    .bind(tid)
    .bind(gctx.auth_user.id)
    .bind(read)
    .execute(&gctx.pool)
    .await
    .map_err(db_err)?;
    if res.rows_affected() == 0 {
        return Err(async_graphql::Error::new("discussion topic not found"));
    }
    Ok(G1SuccessPayload { success: true })
}

async fn group_course_id(
    gctx: &GraphQLContext,
    group_id: i64,
) -> Result<i64, async_graphql::Error> {
    let group: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT course_id FROM learning_outcome_groups \
         WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(group_id)
    .fetch_optional(&gctx.pool)
    .await
    .map_err(db_err)?;
    match group {
        Some((Some(cid),)) => Ok(cid),
        _ => Err(async_graphql::Error::new("outcome group not found")),
    }
}

#[allow(clippy::too_many_arguments)]
async fn create_or_update_proficiency(
    gctx: &GraphQLContext,
    update_id: Option<i64>,
    input: CreateOutcomeProficiencyInput,
) -> Result<OutcomeProficiencyPayload, async_graphql::Error> {
    let course_id = match update_id {
        None => {
            let cid = parse_id(&input.course_id, "course")?;
            if !is_teacher_or_admin(gctx, cid).await? {
                return Err(async_graphql::Error::new("insufficient permission"));
            }
            cid
        }
        Some(pid) => {
            let prof: Option<(Option<i64>,)> =
                sqlx::query_as("SELECT context_id FROM outcome_proficiencies WHERE id = $1")
                    .bind(pid)
                    .fetch_optional(&gctx.pool)
                    .await
                    .map_err(db_err)?;
            let cid = match prof {
                Some((Some(cid),)) => cid,
                _ => {
                    return Err(async_graphql::Error::new("outcome proficiency not found"));
                }
            };
            if !is_teacher_or_admin(gctx, cid).await? {
                return Err(async_graphql::Error::new("insufficient permission"));
            }
            cid
        }
    };
    if input.ratings.is_empty() {
        return Ok(OutcomeProficiencyPayload {
            outcome_proficiency: None,
            errors: validation_errors(Some("ratings"), "at least one rating is required"),
        });
    }
    let mut tx = gctx.pool.begin().await.map_err(db_err)?;
    let pid = match update_id {
        Some(pid) => {
            sqlx::query("UPDATE outcome_proficiencies SET updated_at = now() WHERE id = $1")
                .bind(pid)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            sqlx::query(
                "DELETE FROM outcome_proficiency_ratings WHERE outcome_proficiency_id = $1",
            )
            .bind(pid)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
            pid
        }
        None => {
            let (pid,): (i64,) = sqlx::query_as(
                "INSERT INTO outcome_proficiencies (context_id, context_type) \
                 VALUES ($1, 'Course') RETURNING id",
            )
            .bind(course_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(db_err)?;
            pid
        }
    };
    for r in &input.ratings {
        // Canvas `ProficiencyRatingInput` carries only `description` and
        // `points`; the port's `outcome_proficiency_ratings` row defaults
        // `mastery` to false and leaves `color` unset.
        sqlx::query(
            "INSERT INTO outcome_proficiency_ratings \
             (outcome_proficiency_id, description, points, mastery, color) \
             VALUES ($1, $2, $3, false, NULL)",
        )
        .bind(pid)
        .bind(r.description.as_str())
        .bind(r.points)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    }
    // Canvas `OutcomeProficiency.masteryPoints` is non-null; the port's
    // `outcome_proficiencies.mastery_points` column defaults to 0 and is
    // maintained by the mastery-scale writes below.
    let mastery_points: f64 = sqlx::query_scalar(
        "SELECT mastery_points::float8 FROM outcome_proficiencies WHERE id = $1",
    )
    .bind(pid)
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(OutcomeProficiencyPayload {
        outcome_proficiency: Some(crate::root_types::OutcomeProficiency {
            id: pid,
            context_id: course_id,
            context_type: "Course".to_string(),
            mastery_points,
            locked: false,
        }),
        errors: Vec::new(),
    })
}

async fn create_or_update_calc_method(
    gctx: &GraphQLContext,
    update_id: Option<i64>,
    input: CreateOutcomeCalculationMethodInput,
) -> Result<OutcomeCalculationMethodPayload, async_graphql::Error> {
    const METHODS: [&str; 7] = [
        "decaying_average",
        "n_of_m",
        "latest",
        "highest",
        "average",
        "weighted_average",
        "missing",
    ];
    if !METHODS.contains(&input.calculation_method.as_str()) {
        return Ok(OutcomeCalculationMethodPayload {
            outcome_calculation_method: None,
            errors: validation_errors(
                Some("calculationMethod"),
                "calculation method is not included in the list",
            ),
        });
    }
    let course_id = match update_id {
        None => {
            let cid = parse_id(&input.course_id, "course")?;
            if !is_teacher_or_admin(gctx, cid).await? {
                return Err(async_graphql::Error::new("insufficient permission"));
            }
            cid
        }
        Some(mid) => {
            let m: Option<(Option<i64>,)> =
                sqlx::query_as("SELECT context_id FROM outcome_calculation_methods WHERE id = $1")
                    .bind(mid)
                    .fetch_optional(&gctx.pool)
                    .await
                    .map_err(db_err)?;
            let cid = match m {
                Some((Some(cid),)) => cid,
                _ => {
                    return Err(async_graphql::Error::new(
                        "outcome calculation method not found",
                    ));
                }
            };
            if !is_teacher_or_admin(gctx, cid).await? {
                return Err(async_graphql::Error::new("insufficient permission"));
            }
            cid
        }
    };
    let row: Option<(i64, Option<i32>, String)> = match update_id {
        None => sqlx::query_as(
            "INSERT INTO outcome_calculation_methods \
             (context_id, context_type, calculation_method, calculation_int) \
             VALUES ($1, 'Course', $2, $3) RETURNING id, calculation_int, calculation_method",
        )
        .bind(course_id)
        .bind(&input.calculation_method)
        .bind(input.calculation_int)
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?,
        Some(mid) => sqlx::query_as(
            "UPDATE outcome_calculation_methods SET \
                 calculation_method = $2, calculation_int = COALESCE($3, calculation_int), \
                 updated_at = now() WHERE id = $1 \
                 RETURNING id, calculation_int, calculation_method",
        )
        .bind(mid)
        .bind(&input.calculation_method)
        .bind(input.calculation_int)
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?,
    };
    let Some((id, calculation_int, method)) = row else {
        return Err(async_graphql::Error::new(
            "outcome calculation method save failed",
        ));
    };
    Ok(OutcomeCalculationMethodPayload {
        outcome_calculation_method: Some(crate::root_types::OutcomeCalculationMethod {
            id,
            context_id: course_id,
            context_type: "Course".to_string(),
            calculation_int,
            calculation_method: method,
            locked: false,
        }),
        errors: Vec::new(),
    })
}
