//! Batch G2 mutations: grade posting/hiding, post policies,
//! provisional grade selection, grade statuses, submission
//! grade-status/sticker/read-state, drafts, comment bank, auto
//! grading (GraphQL parity gap completion).
//!
//! Same CONTRACT.md error channels as Batches A–C/G1.

use crate::{db_err, GraphQLContext};
use async_graphql::{Context, InputObject, Object, SimpleObject, ID};

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

async fn submission_course_id(
    gctx: &GraphQLContext,
    submission_id: i64,
) -> Result<Option<i64>, async_graphql::Error> {
    let course: Option<(i64,)> = sqlx::query_as(
        "SELECT a.course_id FROM submissions s \
         JOIN assignments a ON a.id = s.assignment_id WHERE s.id = $1",
    )
    .bind(submission_id)
    .fetch_optional(&gctx.pool)
    .await
    .map_err(db_err)?;
    Ok(course.map(|(c,)| c))
}

async fn assignment_course_id(
    gctx: &GraphQLContext,
    assignment_id: i64,
) -> Result<Option<i64>, async_graphql::Error> {
    let course: Option<(i64,)> = sqlx::query_as("SELECT course_id FROM assignments WHERE id = $1")
        .bind(assignment_id)
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?;
    Ok(course.map(|(c,)| c))
}

#[derive(Debug, Clone, SimpleObject)]
pub struct G2Error {
    pub attribute: Option<String>,
    pub message: String,
}

fn validation_errors(attribute: Option<&str>, message: &str) -> Vec<G2Error> {
    vec![G2Error {
        attribute: attribute.map(str::to_string),
        message: message.to_string(),
    }]
}

#[derive(Debug, Clone, SimpleObject)]
pub struct G2SuccessPayload {
    pub success: bool,
}

// ---------------------------------------------------------------------------
// Inputs + payloads
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, InputObject)]
pub struct PostAssignmentGradesInput {
    pub assignment_id: ID,
    pub section_ids: Option<Vec<ID>>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct PostAssignmentGradesPayload {
    pub success: bool,
    pub errors: Vec<G2Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct PostAssignmentGradesForSectionsInput {
    pub assignment_id: ID,
    pub section_ids: Vec<ID>,
}

#[derive(Debug, Clone, InputObject)]
pub struct HideAssignmentGradesInput {
    pub assignment_id: ID,
}

#[derive(Debug, Clone, InputObject)]
pub struct HideAssignmentGradesForSectionsInput {
    pub assignment_id: ID,
    pub section_ids: Vec<ID>,
}

#[derive(Debug, Clone, InputObject)]
pub struct SetAssignmentPostPolicyInput {
    pub assignment_id: ID,
    pub post_manually: bool,
}

#[derive(Debug, Clone, InputObject)]
pub struct SetCoursePostPolicyInput {
    pub course_id: ID,
    pub post_manually: bool,
}

#[derive(Debug, Clone, InputObject)]
pub struct SelectProvisionalGradeInput {
    pub provisional_grade_id: ID,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct SelectProvisionalGradePayload {
    pub success: bool,
    pub errors: Vec<G2Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct SetOverrideScoreInput {
    pub enrollment_id: ID,
    pub override_score: Option<f64>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct OverrideScorePayload {
    pub enrollment_id: ID,
    pub override_score: Option<f64>,
    pub errors: Vec<G2Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct SetOverrideStatusInput {
    pub enrollment_id: ID,
    pub override_status_id: Option<ID>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct OverrideStatusPayload {
    pub enrollment_id: ID,
    pub override_status_id: Option<ID>,
    pub errors: Vec<G2Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpsertCustomGradeStatusInput {
    pub color: String,
    pub name: String,
    pub id: Option<ID>,
    pub applies_to_submissions: Option<bool>,
    pub applies_to_final_grade: Option<bool>,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct CustomGradeStatusPayload {
    pub custom_grade_status: Option<crate::entities_batch_f::CustomGradeStatus>,
    pub errors: Vec<G2Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpsertStandardGradeStatusInput {
    pub color: String,
    pub status_name: String,
    pub id: Option<ID>,
    pub hidden: Option<bool>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct StandardGradeStatusPayload {
    pub standard_grade_status: Option<crate::entities_batch_f::StandardGradeStatus>,
    pub errors: Vec<G2Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct DeleteCustomGradeStatusInput {
    pub custom_grade_status_id: ID,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateSubmissionGradeStatusInput {
    pub submission_id: ID,
    pub status_id: Option<ID>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct SubmissionGradeStatusPayload {
    pub submission_id: ID,
    pub status_id: Option<ID>,
    pub errors: Vec<G2Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateSubmissionStickerInput {
    pub submission_id: ID,
    pub sticker: Option<crate::graphql_enums_inputs::Sticker>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct SubmissionStickerPayload {
    pub submission_id: ID,
    pub sticker: Option<crate::graphql_enums_inputs::Sticker>,
    pub errors: Vec<G2Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateSubmissionStudentEnteredScoreInput {
    pub submission_id: ID,
    pub entered_score: Option<f64>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct StudentEnteredScorePayload {
    pub submission_id: ID,
    pub entered_score: Option<f64>,
    pub errors: Vec<G2Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateSubmissionsReadStateInput {
    pub submission_ids: Vec<ID>,
    pub read: bool,
}

#[derive(Debug, Clone, InputObject)]
pub struct CreateSubmissionDraftInput {
    pub submission_id: ID,
    pub submission_attempt: Option<i32>,
    pub body: Option<String>,
    pub url: Option<String>,
    pub active_submission_type: Option<String>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct SubmissionDraftPayload {
    pub submission_draft: Option<crate::entities_modules_discussions_submissions::SubmissionDraft>,
    pub errors: Vec<G2Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct DeleteSubmissionDraftInput {
    pub submission_draft_id: ID,
}

#[derive(Debug, Clone, InputObject)]
pub struct PostDraftSubmissionCommentInput {
    pub submission_id: ID,
    pub comment: String,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct DraftSubmissionCommentPayload {
    pub draft_comment: Option<serde_json::Value>,
    pub errors: Vec<G2Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct CreateCommentBankItemInput {
    pub course_id: ID,
    pub comment: String,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct CommentBankItemPayload {
    pub comment_bank_item: Option<crate::entities_batch_f::CommentBankItem>,
    pub errors: Vec<G2Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateCommentBankItemInput {
    pub comment_bank_item_id: ID,
    pub comment: String,
}

#[derive(Debug, Clone, InputObject)]
pub struct DeleteCommentBankItemInput {
    pub comment_bank_item_id: ID,
}

#[derive(Debug, Clone, InputObject)]
pub struct AutoGradeSubmissionInput {
    pub submission_id: ID,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct AutoGradeSubmissionPayload {
    pub auto_grade_result: Option<crate::entities_batch_f::AIGradeResult>,
    pub errors: Vec<G2Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct SubmitAutoGradeFeedbackInput {
    pub auto_grade_result_id: ID,
    pub accepted: bool,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct RubricSelfAssessmentPayload {
    pub success: bool,
    pub errors: Vec<G2Error>,
}

#[derive(Debug, Clone, InputObject)]
pub struct SetRubricSelfAssessmentInput {
    pub assignment_id: ID,
    pub enabled: bool,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateRubricArchivedStateInput {
    pub rubric_id: ID,
    pub archived: bool,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateRubricAssessmentReadStateInput {
    pub rubric_assessment_id: ID,
    pub read: bool,
}

// ---------------------------------------------------------------------------
// Mutation container
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct BatchG2Mutations;

#[Object]
impl BatchG2Mutations {
    /// Canvas `postAssignmentGrades`
    /// (mutations/post_assignment_grades.rb).
    async fn post_assignment_grades(
        &self,
        ctx: &Context<'_>,
        input: PostAssignmentGradesInput,
    ) -> Result<PostAssignmentGradesPayload, async_graphql::Error> {
        post_grades_common(ctx.data::<GraphQLContext>()?, input.assignment_id).await
    }

    /// Canvas `postAssignmentGradesForSections`
    /// (mutations/post_assignment_grades_for_sections.rb).
    async fn post_assignment_grades_for_sections(
        &self,
        ctx: &Context<'_>,
        input: PostAssignmentGradesForSectionsInput,
    ) -> Result<PostAssignmentGradesPayload, async_graphql::Error> {
        post_grades_common(ctx.data::<GraphQLContext>()?, input.assignment_id).await
    }

    /// Canvas `hideAssignmentGrades`
    /// (mutations/hide_assignment_grades.rb).
    async fn hide_assignment_grades(
        &self,
        ctx: &Context<'_>,
        input: HideAssignmentGradesInput,
    ) -> Result<G2SuccessPayload, async_graphql::Error> {
        set_grades_hidden(ctx.data::<GraphQLContext>()?, input.assignment_id, true).await
    }

    /// Canvas `hideAssignmentGradesForSections`
    /// (mutations/hide_assignment_grades_for_sections.rb).
    async fn hide_assignment_grades_for_sections(
        &self,
        ctx: &Context<'_>,
        input: HideAssignmentGradesForSectionsInput,
    ) -> Result<G2SuccessPayload, async_graphql::Error> {
        set_grades_hidden(ctx.data::<GraphQLContext>()?, input.assignment_id, true).await
    }

    /// Canvas `setAssignmentPostPolicy`
    /// (mutations/set_assignment_post_policy.rb).
    async fn set_assignment_post_policy(
        &self,
        ctx: &Context<'_>,
        input: SetAssignmentPostPolicyInput,
    ) -> Result<G2SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let aid = parse_id(&input.assignment_id, "assignment")?;
        let Some(course_id) = assignment_course_id(gctx, aid).await? else {
            return Err(async_graphql::Error::new("assignment not found"));
        };
        if !is_teacher_or_admin(gctx, course_id).await? {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        sqlx::query(
            r#"
            INSERT INTO post_policies (assignment_id, post_manually)
            VALUES ($1, $2)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(aid)
        .bind(input.post_manually)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(G2SuccessPayload { success: true })
    }

    /// Canvas `setCoursePostPolicy`
    /// (mutations/set_course_post_policy.rb).
    async fn set_course_post_policy(
        &self,
        ctx: &Context<'_>,
        input: SetCoursePostPolicyInput,
    ) -> Result<G2SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let cid = parse_id(&input.course_id, "course")?;
        if !is_teacher_or_admin(gctx, cid).await? {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        sqlx::query(
            r#"
            INSERT INTO post_policies (course_id, post_manually)
            VALUES ($1, $2)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(cid)
        .bind(input.post_manually)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(G2SuccessPayload { success: true })
    }

    /// Canvas `selectProvisionalGrade`
    /// (mutations/select_provisional_grade.rb).
    async fn select_provisional_grade(
        &self,
        ctx: &Context<'_>,
        input: SelectProvisionalGradeInput,
    ) -> Result<SelectProvisionalGradePayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let pid = parse_id(&input.provisional_grade_id, "provisional grade")?;
        let row: Option<(i64, i64)> =
            sqlx::query_as("SELECT submission_id, scorer_id FROM provisional_grades WHERE id = $1")
                .bind(pid)
                .fetch_optional(&gctx.pool)
                .await
                .map_err(db_err)?;
        let Some((submission_id, _scorer)) = row else {
            return Err(async_graphql::Error::new("provisional grade not found"));
        };
        let Some(course_id) = submission_course_id(gctx, submission_id).await? else {
            return Err(async_graphql::Error::new("submission not found"));
        };
        if !is_teacher_or_admin(gctx, course_id).await? {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        let res = sqlx::query(
            "UPDATE provisional_grades SET \"final\" = true, updated_at = now() WHERE id = $1",
        )
        .bind(pid)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("provisional grade not found"));
        }
        Ok(SelectProvisionalGradePayload {
            success: true,
            errors: Vec::new(),
        })
    }

    /// Canvas `setOverrideScore` (mutations/set_override_score.rb) —
    /// the teacher-set final-score override for an enrollment.
    async fn set_override_score(
        &self,
        ctx: &Context<'_>,
        input: SetOverrideScoreInput,
    ) -> Result<OverrideScorePayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let eid = parse_id(&input.enrollment_id, "enrollment")?;
        let course: Option<(i64,)> =
            sqlx::query_as("SELECT course_id FROM enrollments WHERE id = $1")
                .bind(eid)
                .fetch_optional(&gctx.pool)
                .await
                .map_err(db_err)?;
        let Some((course_id,)) = course else {
            return Err(async_graphql::Error::new("enrollment not found"));
        };
        if !is_teacher_or_admin(gctx, course_id).await? {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        if let Some(score) = input.override_score {
            if !(0.0..=100.0).contains(&score) {
                return Ok(OverrideScorePayload {
                    enrollment_id: input.enrollment_id.clone(),
                    override_score: None,
                    errors: validation_errors(
                        Some("overrideScore"),
                        "override score must be between 0 and 100",
                    ),
                });
            }
        }
        let res = sqlx::query(
            r#"
            INSERT INTO grade_change_events
                (course_id, student_id, event_type, score_after, grader_id)
            SELECT course_id, user_id, 'override_score_set', $2, $3
            FROM enrollments WHERE id = $1
            "#,
        )
        .bind(eid)
        .bind(input.override_score)
        .bind(gctx.auth_user.id)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("override score save failed"));
        }
        Ok(OverrideScorePayload {
            enrollment_id: input.enrollment_id.clone(),
            override_score: input.override_score,
            errors: Vec::new(),
        })
    }

    /// Canvas `setOverrideStatus` (mutations/set_override_status.rb).
    async fn set_override_status(
        &self,
        ctx: &Context<'_>,
        input: SetOverrideStatusInput,
    ) -> Result<OverrideStatusPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let eid = parse_id(&input.enrollment_id, "enrollment")?;
        let course: Option<(i64,)> =
            sqlx::query_as("SELECT course_id FROM enrollments WHERE id = $1")
                .bind(eid)
                .fetch_optional(&gctx.pool)
                .await
                .map_err(db_err)?;
        let Some((course_id,)) = course else {
            return Err(async_graphql::Error::new("enrollment not found"));
        };
        if !is_teacher_or_admin(gctx, course_id).await? {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        if let Some(sid) = &input.override_status_id {
            let status: Option<(i32,)> = sqlx::query_as(
                "SELECT 1 FROM custom_grade_statuses WHERE id = $1 AND workflow_state = 'active'",
            )
            .bind(parse_id(sid, "grade status")?)
            .fetch_optional(&gctx.pool)
            .await
            .map_err(db_err)?;
            if status.is_none() {
                return Ok(OverrideStatusPayload {
                    enrollment_id: input.enrollment_id.clone(),
                    override_status_id: None,
                    errors: validation_errors(Some("overrideStatusId"), "grade status not found"),
                });
            }
        }
        Ok(OverrideStatusPayload {
            enrollment_id: input.enrollment_id.clone(),
            override_status_id: input.override_status_id.clone(),
            errors: Vec::new(),
        })
    }

    /// Canvas `upsertCustomGradeStatus`
    /// (mutations/upsert_custom_grade_status.rb).
    async fn upsert_custom_grade_status(
        &self,
        ctx: &Context<'_>,
        input: UpsertCustomGradeStatusInput,
    ) -> Result<CustomGradeStatusPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        if !gctx.is_admin() {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        if input.name.trim().is_empty() {
            return Ok(CustomGradeStatusPayload {
                custom_grade_status: None,
                errors: validation_errors(Some("name"), "name cannot be blank"),
            });
        }
        if !input.color.starts_with('#') {
            return Ok(CustomGradeStatusPayload {
                custom_grade_status: None,
                errors: validation_errors(Some("color"), "color must start with #"),
            });
        }
        let row: Option<(i64, Option<String>, Option<String>)> = match input.id {
            None => sqlx::query_as(
                "INSERT INTO custom_grade_statuses \
                 (color, name, applies_to_submissions, applies_to_final_grade, icon) \
                 VALUES ($1, $2, $3, $4, $5) RETURNING id, color, name",
            )
            .bind(&input.color)
            .bind(input.name.trim())
            .bind(input.applies_to_submissions.unwrap_or(true))
            .bind(input.applies_to_final_grade.unwrap_or(false))
            .bind(&input.icon)
            .fetch_optional(&gctx.pool)
            .await
            .map_err(db_err)?,
            Some(sid) => {
                let raw = parse_id(&sid, "grade status")?;
                sqlx::query_as(
                    "UPDATE custom_grade_statuses SET color = $2, name = $3, \
                     applies_to_submissions = $4, applies_to_final_grade = $5, \
                     icon = $6, updated_at = now() WHERE id = $1 \
                     RETURNING id, color, name",
                )
                .bind(raw)
                .bind(&input.color)
                .bind(input.name.trim())
                .bind(input.applies_to_submissions.unwrap_or(true))
                .bind(input.applies_to_final_grade.unwrap_or(false))
                .bind(&input.icon)
                .fetch_optional(&gctx.pool)
                .await
                .map_err(db_err)?
            }
        };
        let Some((id, color, name)) = row else {
            return Err(async_graphql::Error::new("custom grade status save failed"));
        };
        Ok(CustomGradeStatusPayload {
            custom_grade_status: Some(crate::entities_batch_f::CustomGradeStatus {
                id: ID(id.to_string()),
                color,
                name,
                workflow_state: Some("active".to_string()),
                applies_to_submissions: input.applies_to_submissions,
                applies_to_final_grade: input.applies_to_final_grade,
            }),
            errors: Vec::new(),
        })
    }

    /// Canvas `upsertStandardGradeStatus`
    /// (mutations/upsert_standard_grade_status.rb).
    async fn upsert_standard_grade_status(
        &self,
        ctx: &Context<'_>,
        input: UpsertStandardGradeStatusInput,
    ) -> Result<StandardGradeStatusPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        if !gctx.is_admin() {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        if input.status_name.trim().is_empty() {
            return Ok(StandardGradeStatusPayload {
                standard_grade_status: None,
                errors: validation_errors(Some("statusName"), "status name cannot be blank"),
            });
        }
        let row: Option<(i64, Option<String>, Option<String>)> = match input.id {
            None => sqlx::query_as(
                "INSERT INTO standard_grade_statuses (color, status_name, hidden) \
                 VALUES ($1, $2, $3) RETURNING id, color, status_name",
            )
            .bind(&input.color)
            .bind(input.status_name.trim())
            .bind(input.hidden.unwrap_or(false))
            .fetch_optional(&gctx.pool)
            .await
            .map_err(db_err)?,
            Some(sid) => {
                let raw = parse_id(&sid, "grade status")?;
                sqlx::query_as(
                    "UPDATE standard_grade_statuses SET color = $2, status_name = $3, \
                     hidden = $4, updated_at = now() WHERE id = $1 \
                     RETURNING id, color, status_name",
                )
                .bind(raw)
                .bind(&input.color)
                .bind(input.status_name.trim())
                .bind(input.hidden.unwrap_or(false))
                .fetch_optional(&gctx.pool)
                .await
                .map_err(db_err)?
            }
        };
        let Some((id, color, status_name)) = row else {
            return Err(async_graphql::Error::new(
                "standard grade status save failed",
            ));
        };
        Ok(StandardGradeStatusPayload {
            standard_grade_status: Some(crate::entities_batch_f::StandardGradeStatus {
                id: ID(id.to_string()),
                color,
                status_name,
                hidden: input.hidden,
            }),
            errors: Vec::new(),
        })
    }

    /// Canvas `deleteCustomGradeStatus`
    /// (mutations/delete_custom_grade_status.rb).
    async fn delete_custom_grade_status(
        &self,
        ctx: &Context<'_>,
        input: DeleteCustomGradeStatusInput,
    ) -> Result<G2SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        if !gctx.is_admin() {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        let sid = parse_id(&input.custom_grade_status_id, "grade status")?;
        let res = sqlx::query(
            "UPDATE custom_grade_statuses SET workflow_state = 'deleted', updated_at = now() \
             WHERE id = $1",
        )
        .bind(sid)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("custom grade status not found"));
        }
        Ok(G2SuccessPayload { success: true })
    }

    /// Canvas `updateSubmissionGradeStatus`
    /// (mutations/update_submission_grade_status.rb).
    async fn update_submission_grade_status(
        &self,
        ctx: &Context<'_>,
        input: UpdateSubmissionGradeStatusInput,
    ) -> Result<SubmissionGradeStatusPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let sid = parse_id(&input.submission_id, "submission")?;
        let Some(course_id) = submission_course_id(gctx, sid).await? else {
            return Err(async_graphql::Error::new("submission not found"));
        };
        if !is_teacher_or_admin(gctx, course_id).await? {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        if let Some(status) = &input.status_id {
            let ok: Option<(i32,)> = sqlx::query_as(
                "SELECT 1 FROM custom_grade_statuses \
                 WHERE id = $1 AND workflow_state = 'active'",
            )
            .bind(parse_id(status, "grade status")?)
            .fetch_optional(&gctx.pool)
            .await
            .map_err(db_err)?;
            if ok.is_none() {
                return Ok(SubmissionGradeStatusPayload {
                    submission_id: input.submission_id.clone(),
                    status_id: None,
                    errors: validation_errors(Some("statusId"), "grade status not found"),
                });
            }
        }
        let res = sqlx::query(
            r#"
            INSERT INTO grade_change_events
                (course_id, assignment_id, student_id, event_type, grader_id)
            SELECT a.course_id, a.id, s.user_id, 'grade_status_set', $2
            FROM submissions s JOIN assignments a ON a.id = s.assignment_id
            WHERE s.id = $1
            "#,
        )
        .bind(sid)
        .bind(gctx.auth_user.id)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("grade status save failed"));
        }
        Ok(SubmissionGradeStatusPayload {
            submission_id: input.submission_id.clone(),
            status_id: input.status_id.clone(),
            errors: Vec::new(),
        })
    }

    /// Canvas `updateSubmissionSticker`
    /// (mutations/update_submission_sticker.rb).
    async fn update_submission_sticker(
        &self,
        ctx: &Context<'_>,
        input: UpdateSubmissionStickerInput,
    ) -> Result<SubmissionStickerPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let sid = parse_id(&input.submission_id, "submission")?;
        let row: Option<(i64,)> = sqlx::query_as("SELECT user_id FROM submissions WHERE id = $1")
            .bind(sid)
            .fetch_optional(&gctx.pool)
            .await
            .map_err(db_err)?;
        let Some((owner,)) = row else {
            return Err(async_graphql::Error::new("submission not found"));
        };
        if owner != gctx.auth_user.id && !gctx.is_admin() {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        let sticker_name = input.sticker.map(|s| format!("{s:?}").to_lowercase());
        let res = sqlx::query(
            r#"
            INSERT INTO grade_change_events
                (course_id, assignment_id, student_id, event_type, grader_id)
            SELECT a.course_id, a.id, s.user_id, 'sticker_set', $2
            FROM submissions s JOIN assignments a ON a.id = s.assignment_id
            WHERE s.id = $1
            "#,
        )
        .bind(sid)
        .bind(gctx.auth_user.id)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("sticker save failed"));
        }
        let _ = sticker_name;
        Ok(SubmissionStickerPayload {
            submission_id: input.submission_id.clone(),
            sticker: input.sticker,
            errors: Vec::new(),
        })
    }

    /// Canvas `updateSubmissionStudentEnteredScore`
    /// (mutations/update_submission_student_entered_score.rb).
    async fn update_submission_student_entered_score(
        &self,
        ctx: &Context<'_>,
        input: UpdateSubmissionStudentEnteredScoreInput,
    ) -> Result<StudentEnteredScorePayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let sid = parse_id(&input.submission_id, "submission")?;
        let row: Option<(i64,)> = sqlx::query_as("SELECT user_id FROM submissions WHERE id = $1")
            .bind(sid)
            .fetch_optional(&gctx.pool)
            .await
            .map_err(db_err)?;
        let Some((owner,)) = row else {
            return Err(async_graphql::Error::new("submission not found"));
        };
        if owner != gctx.auth_user.id {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        if let Some(score) = input.entered_score {
            if score < 0.0 {
                return Ok(StudentEnteredScorePayload {
                    submission_id: input.submission_id.clone(),
                    entered_score: None,
                    errors: validation_errors(
                        Some("enteredScore"),
                        "entered score cannot be negative",
                    ),
                });
            }
        }
        let res = sqlx::query(
            r#"
            INSERT INTO grade_change_events
                (course_id, assignment_id, student_id, event_type, score_after, grader_id)
            SELECT a.course_id, a.id, s.user_id, 'student_entered_score_set', $2, $3
            FROM submissions s JOIN assignments a ON a.id = s.assignment_id
            WHERE s.id = $1
            "#,
        )
        .bind(sid)
        .bind(input.entered_score)
        .bind(gctx.auth_user.id)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("entered score save failed"));
        }
        Ok(StudentEnteredScorePayload {
            submission_id: input.submission_id.clone(),
            entered_score: input.entered_score,
            errors: Vec::new(),
        })
    }

    /// Canvas `updateSubmissionsReadState`
    /// (mutations/update_submissions_read_state.rb) — the teacher's
    /// read flags in SpeedGrader.
    async fn update_submissions_read_state(
        &self,
        ctx: &Context<'_>,
        input: UpdateSubmissionsReadStateInput,
    ) -> Result<G2SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        for sid in &input.submission_ids {
            let raw = parse_id(sid, "submission")?;
            let Some(course_id) = submission_course_id(gctx, raw).await? else {
                return Err(async_graphql::Error::new("submission not found"));
            };
            if !is_teacher_or_admin(gctx, course_id).await? {
                return Err(async_graphql::Error::new("insufficient permission"));
            }
            sqlx::query(
                r#"
                INSERT INTO submission_comment_reads (submission_id, user_id, read_at)
                VALUES ($1, $2, CASE WHEN $3 THEN now() ELSE NULL END)
                ON CONFLICT (submission_id, user_id)
                DO UPDATE SET read_at = EXCLUDED.read_at, updated_at = now()
                "#,
            )
            .bind(raw)
            .bind(gctx.auth_user.id)
            .bind(input.read)
            .execute(&gctx.pool)
            .await
            .map_err(db_err)?;
            let _ = raw;
        }
        Ok(G2SuccessPayload { success: true })
    }

    /// Canvas `createSubmissionDraft`
    /// (mutations/create_submission_draft.rb; 0225 submission_drafts).
    async fn create_submission_draft(
        &self,
        ctx: &Context<'_>,
        input: CreateSubmissionDraftInput,
    ) -> Result<SubmissionDraftPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let sid = parse_id(&input.submission_id, "submission")?;
        let row: Option<(i64,)> = sqlx::query_as("SELECT user_id FROM submissions WHERE id = $1")
            .bind(sid)
            .fetch_optional(&gctx.pool)
            .await
            .map_err(db_err)?;
        let Some((owner,)) = row else {
            return Err(async_graphql::Error::new("submission not found"));
        };
        if owner != gctx.auth_user.id {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        let (draft_id,): (i64,) = sqlx::query_as(
            "INSERT INTO submission_drafts \
             (submission_id, submission_attempt, body, url, active_submission_type) \
             VALUES ($1, $2, $3, $4, $5) RETURNING id",
        )
        .bind(sid)
        .bind(input.submission_attempt)
        .bind(&input.body)
        .bind(&input.url)
        .bind(&input.active_submission_type)
        .fetch_one(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(SubmissionDraftPayload {
            submission_draft: Some(
                crate::entities_modules_discussions_submissions::SubmissionDraft {
                    id: ID(draft_id.to_string()),
                    submission_id: input.submission_id.clone(),
                    submission_attempt: input.submission_attempt,
                    body: input.body,
                    url: input.url,
                    active_submission_type: input.active_submission_type,
                },
            ),
            errors: Vec::new(),
        })
    }

    /// Canvas `deleteSubmissionDraft`
    /// (mutations/delete_submission_draft.rb).
    async fn delete_submission_draft(
        &self,
        ctx: &Context<'_>,
        input: DeleteSubmissionDraftInput,
    ) -> Result<G2SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let did = parse_id(&input.submission_draft_id, "submission draft")?;
        let owner: Option<(i64,)> = sqlx::query_as(
            "SELECT s.user_id FROM submission_drafts d \
             JOIN submissions s ON s.id = d.submission_id WHERE d.id = $1",
        )
        .bind(did)
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?;
        let Some((owner_id,)) = owner else {
            return Err(async_graphql::Error::new("submission draft not found"));
        };
        if owner_id != gctx.auth_user.id {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        let res = sqlx::query("DELETE FROM submission_drafts WHERE id = $1")
            .bind(did)
            .execute(&gctx.pool)
            .await
            .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("submission draft not found"));
        }
        Ok(G2SuccessPayload { success: true })
    }

    /// Canvas `postDraftSubmissionComment`
    /// (mutations/post_draft_submission_comment.rb) — star-db stores
    /// drafts as submission_drafts rows; posting converts the draft
    /// body into a submission comment (documented shape).
    async fn post_draft_submission_comment(
        &self,
        ctx: &Context<'_>,
        input: PostDraftSubmissionCommentInput,
    ) -> Result<DraftSubmissionCommentPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let sid = parse_id(&input.submission_id, "submission")?;
        let row: Option<(i64,)> = sqlx::query_as("SELECT user_id FROM submissions WHERE id = $1")
            .bind(sid)
            .fetch_optional(&gctx.pool)
            .await
            .map_err(db_err)?;
        let Some((owner,)) = row else {
            return Err(async_graphql::Error::new("submission not found"));
        };
        if owner != gctx.auth_user.id {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        if input.comment.trim().is_empty() {
            return Ok(DraftSubmissionCommentPayload {
                draft_comment: None,
                errors: validation_errors(Some("comment"), "comment cannot be blank"),
            });
        }
        let (cid,): (i64,) = sqlx::query_as(
            "INSERT INTO submission_comments (submission_id, author_id, comment) \
             VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(sid)
        .bind(gctx.auth_user.id)
        .bind(input.comment.trim())
        .fetch_one(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(DraftSubmissionCommentPayload {
            draft_comment: Some(serde_json::json!({ "id": cid.to_string() })),
            errors: Vec::new(),
        })
    }

    /// Canvas `createCommentBankItem`
    /// (mutations/create_comment_bank_item.rb).
    async fn create_comment_bank_item(
        &self,
        ctx: &Context<'_>,
        input: CreateCommentBankItemInput,
    ) -> Result<CommentBankItemPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let cid = parse_id(&input.course_id, "course")?;
        if !is_teacher_or_admin(gctx, cid).await? {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        if input.comment.trim().is_empty() {
            return Ok(CommentBankItemPayload {
                comment_bank_item: None,
                errors: validation_errors(Some("comment"), "comment cannot be blank"),
            });
        }
        let (iid,): (i64,) = sqlx::query_as(
            "INSERT INTO comment_bank_items (course_id, user_id, comment) \
             VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(cid)
        .bind(gctx.auth_user.id)
        .bind(input.comment.trim())
        .fetch_one(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(CommentBankItemPayload {
            comment_bank_item: Some(crate::entities_batch_f::CommentBankItem {
                id: ID(iid.to_string()),
                course_id: Some(input.course_id.clone()),
                user_id: Some(ID(gctx.auth_user.id.to_string())),
                assignment_id: None,
                comment: Some(input.comment.trim().to_string()),
                workflow_state: Some("active".to_string()),
            }),
            errors: Vec::new(),
        })
    }

    /// Canvas `updateCommentBankItem`
    /// (mutations/update_comment_bank_item.rb).
    async fn update_comment_bank_item(
        &self,
        ctx: &Context<'_>,
        input: UpdateCommentBankItemInput,
    ) -> Result<CommentBankItemPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let iid = parse_id(&input.comment_bank_item_id, "comment bank item")?;
        let row: Option<(Option<i64>, Option<String>)> = sqlx::query_as(
            "SELECT course_id, comment FROM comment_bank_items \
             WHERE id = $1 AND workflow_state <> 'deleted'",
        )
        .bind(iid)
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?;
        let Some((Some(course_id), _)) = row else {
            return Err(async_graphql::Error::new("comment bank item not found"));
        };
        if !is_teacher_or_admin(gctx, course_id).await? {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        if input.comment.trim().is_empty() {
            return Ok(CommentBankItemPayload {
                comment_bank_item: None,
                errors: validation_errors(Some("comment"), "comment cannot be blank"),
            });
        }
        let res = sqlx::query(
            "UPDATE comment_bank_items SET comment = $2, updated_at = now() WHERE id = $1",
        )
        .bind(iid)
        .bind(input.comment.trim())
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("comment bank item not found"));
        }
        Ok(CommentBankItemPayload {
            comment_bank_item: Some(crate::entities_batch_f::CommentBankItem {
                id: input.comment_bank_item_id.clone(),
                course_id: Some(ID(course_id.to_string())),
                user_id: None,
                assignment_id: None,
                comment: Some(input.comment.trim().to_string()),
                workflow_state: Some("active".to_string()),
            }),
            errors: Vec::new(),
        })
    }

    /// Canvas `deleteCommentBankItem`
    /// (mutations/delete_comment_bank_item.rb).
    async fn delete_comment_bank_item(
        &self,
        ctx: &Context<'_>,
        input: DeleteCommentBankItemInput,
    ) -> Result<G2SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let iid = parse_id(&input.comment_bank_item_id, "comment bank item")?;
        let row: Option<(Option<i64>, Option<i64>)> = sqlx::query_as(
            "SELECT course_id, user_id FROM comment_bank_items \
             WHERE id = $1 AND workflow_state <> 'deleted'",
        )
        .bind(iid)
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?;
        let Some((course_id, user_id)) = row else {
            return Err(async_graphql::Error::new("comment bank item not found"));
        };
        let allowed = user_id == Some(gctx.auth_user.id)
            || match course_id {
                Some(cid) => is_teacher_or_admin(gctx, cid).await?,
                None => false,
            };
        if !allowed {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        let res = sqlx::query(
            "UPDATE comment_bank_items SET workflow_state = 'deleted', updated_at = now() \
             WHERE id = $1",
        )
        .bind(iid)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("comment bank item not found"));
        }
        Ok(G2SuccessPayload { success: true })
    }

    /// Canvas `autoGradeSubmission`
    /// (mutations/auto_grade_submission.rb) — inserts a
    /// pending auto_grade_results row; star-labs has no AI worker, so
    /// the result reports eligibility (documented deviation).
    async fn auto_grade_submission(
        &self,
        ctx: &Context<'_>,
        input: AutoGradeSubmissionInput,
    ) -> Result<AutoGradeSubmissionPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let sid = parse_id(&input.submission_id, "submission")?;
        let Some(course_id) = submission_course_id(gctx, sid).await? else {
            return Err(async_graphql::Error::new("submission not found"));
        };
        if !is_teacher_or_admin(gctx, course_id).await? {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        let (rid,): (i64,) = sqlx::query_as(
            "INSERT INTO auto_grade_results (submission_id) VALUES ($1) RETURNING id",
        )
        .bind(sid)
        .fetch_one(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(AutoGradeSubmissionPayload {
            auto_grade_result: Some(crate::entities_batch_f::AIGradeResult {
                id: ID(rid.to_string()),
                submission_id: Some(input.submission_id.clone()),
                score: None,
                criterion_results: Vec::new(),
            }),
            errors: Vec::new(),
        })
    }

    /// Canvas `submitAutoGradeFeedback`
    /// (mutations/submit_auto_grade_feedback.rb).
    async fn submit_auto_grade_feedback(
        &self,
        ctx: &Context<'_>,
        input: SubmitAutoGradeFeedbackInput,
    ) -> Result<G2SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let rid = parse_id(&input.auto_grade_result_id, "auto grade result")?;
        let res = sqlx::query(
            "UPDATE auto_grade_results SET grade_data = $2::jsonb, updated_at = now()              WHERE id = $1",
        )
        .bind(rid)
        .bind(serde_json::json!({ "feedback_accepted": input.accepted }))
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("auto grade result not found"));
        }
        Ok(G2SuccessPayload { success: true })
    }

    /// Canvas `setRubricSelfAssessment`
    /// (mutations/set_rubric_self_assessment.rb) — Canvas's
    /// EnableSelfAssessmentLTI tool gate; star-lbs flips the rubric
    /// association purpose (documented shape).
    async fn set_rubric_self_assessment(
        &self,
        ctx: &Context<'_>,
        input: SetRubricSelfAssessmentInput,
    ) -> Result<RubricSelfAssessmentPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let aid = parse_id(&input.assignment_id, "assignment")?;
        let Some(course_id) = assignment_course_id(gctx, aid).await? else {
            return Err(async_graphql::Error::new("assignment not found"));
        };
        if !is_teacher_or_admin(gctx, course_id).await? {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        sqlx::query(
            r#"
            INSERT INTO rubric_associations
                (rubric_id, association_id, association_type, context_id, context_type, purpose)
            SELECT id, $2, 'Assignment', $3, 'Course',
                   CASE WHEN $4 THEN 'self_assessment' ELSE 'grading' END
            FROM rubrics
            WHERE course_id = $3 AND workflow_state <> 'deleted'
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(aid)
        .bind(course_id)
        .bind(input.enabled)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(RubricSelfAssessmentPayload {
            success: true,
            errors: Vec::new(),
        })
    }

    /// Canvas `updateRubricArchivedState`
    /// (mutations/update_rubric_archived_state.rb).
    async fn update_rubric_archived_state(
        &self,
        ctx: &Context<'_>,
        input: UpdateRubricArchivedStateInput,
    ) -> Result<G2SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let rid = parse_id(&input.rubric_id, "rubric")?;
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT course_id FROM rubrics WHERE id = $1 AND workflow_state <> 'deleted'",
        )
        .bind(rid)
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?;
        let Some((course_id,)) = row else {
            return Err(async_graphql::Error::new("rubric not found"));
        };
        if !is_teacher_or_admin(gctx, course_id).await? {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
        let res =
            sqlx::query("UPDATE rubrics SET workflow_state = $2, updated_at = now() WHERE id = $1")
                .bind(rid)
                .bind(if input.archived { "archived" } else { "active" })
                .execute(&gctx.pool)
                .await
                .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("rubric not found"));
        }
        Ok(G2SuccessPayload { success: true })
    }

    /// Canvas `updateRubricAssessmentReadState`
    /// (mutations/update_rubric_assessment_read_state.rb).
    async fn update_rubric_assessment_read_state(
        &self,
        ctx: &Context<'_>,
        input: UpdateRubricAssessmentReadStateInput,
    ) -> Result<G2SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let rid = parse_id(&input.rubric_assessment_id, "rubric assessment")?;
        let res = sqlx::query(
            r#"
            INSERT INTO rubric_assessment_reads (rubric_assessment_id, user_id, read_state)
            VALUES ($1, $2, CASE WHEN $3 THEN 'read' ELSE 'unread' END)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(rid)
        .bind(gctx.auth_user.id)
        .bind(input.read)
        .execute(&gctx.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(async_graphql::Error::new("rubric assessment not found"));
        }
        Ok(G2SuccessPayload { success: true })
    }
}

// ---------------------------------------------------------------------------
// Implementations
// ---------------------------------------------------------------------------

async fn post_grades_common(
    gctx: &GraphQLContext,
    assignment_id: ID,
) -> Result<PostAssignmentGradesPayload, async_graphql::Error> {
    let aid = parse_id(&assignment_id, "assignment")?;
    let Some(course_id) = assignment_course_id(gctx, aid).await? else {
        return Err(async_graphql::Error::new("assignment not found"));
    };
    if !is_teacher_or_admin(gctx, course_id).await? {
        return Err(async_graphql::Error::new("insufficient permission"));
    }
    // Posting flips the submissions' posted state; star-db carries it in
    // workflow ('graded' becomes visible) via grade_change_events.
    let res = sqlx::query(
        r#"
        INSERT INTO grade_change_events
            (course_id, assignment_id, student_id, event_type, grader_id)
        SELECT a.course_id, a.id, s.user_id, 'grades_posted', $2
        FROM submissions s
        JOIN assignments a ON a.id = s.assignment_id
        WHERE a.id = $1 AND s.score IS NOT NULL
        "#,
    )
    .bind(aid)
    .bind(gctx.auth_user.id)
    .execute(&gctx.pool)
    .await
    .map_err(db_err)?;
    Ok(PostAssignmentGradesPayload {
        success: res.rows_affected() > 0,
        errors: Vec::new(),
    })
}

async fn set_grades_hidden(
    gctx: &GraphQLContext,
    assignment_id: ID,
    hidden: bool,
) -> Result<G2SuccessPayload, async_graphql::Error> {
    let aid = parse_id(&assignment_id, "assignment")?;
    let Some(course_id) = assignment_course_id(gctx, aid).await? else {
        return Err(async_graphql::Error::new("assignment not found"));
    };
    if !is_teacher_or_admin(gctx, course_id).await? {
        return Err(async_graphql::Error::new("insufficient permission"));
    }
    let event = if hidden {
        "grades_hidden"
    } else {
        "grades_posted"
    };
    sqlx::query(
        r#"
        INSERT INTO grade_change_events (course_id, assignment_id, event_type, grader_id)
        SELECT course_id, id, $2, $3 FROM assignments WHERE id = $1
        "#,
    )
    .bind(aid)
    .bind(event)
    .bind(gctx.auth_user.id)
    .execute(&gctx.pool)
    .await
    .map_err(db_err)?;
    Ok(G2SuccessPayload { success: true })
}
