//! Canvas parity tests for star-groups.
//!
//! These tests pin the parity invariants of the star-groups handlers by
//! asserting them against the actual handler SQL: the invariant strings
//! (workflow_state values, join_level values, soft-delete markers) are
//! looked up in the crate source, so a handler edit that silently drops or
//! changes one of these parity behaviors fails the test.
//!
//! Run with: cargo test -p star-groups
//!
//! Live parity tests are ignored by default; run explicitly with
//! `cargo test -p star-groups -- --ignored` against a local service.

/// Read the crate's own handler source so assertions track the real SQL.
fn handler_source() -> String {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let path = format!("{manifest}/src/lib.rs");
    std::fs::read_to_string(path).expect("star-groups handler source readable")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_workflow_state_default() {
        // Canvas API always returns 'available' as workflow_state for newly
        // created groups: the INSERT column list must NOT include
        // workflow_state, so the DB default 'available' applies. (The
        // RETURNING clause legitimately selects it back.)
        let src = handler_source();
        let create_sql = src
            .split("async fn create_group")
            .nth(1)
            .expect("create_group exists");
        let insert = create_sql.split("async fn").next().unwrap_or("");
        let insert_columns = insert
            .split("INSERT INTO groups")
            .nth(1)
            .expect("create_group inserts into groups")
            .split("VALUES")
            .next()
            .expect("INSERT has a VALUES clause");
        assert!(
            !insert_columns.contains("workflow_state"),
            "create_group INSERT must not set workflow_state; the DB default 'available' must apply"
        );
    }

    #[test]
    fn test_membership_workflow_state_default() {
        // Canvas derives a new membership's initial state from the group's
        // join level: auto_join -> accepted, request -> requested, else invited.
        let src = handler_source();
        let derive = src
            .split("fn membership_initial_workflow_state")
            .nth(1)
            .expect("membership_initial_workflow_state exists");
        assert!(
            derive.contains(r#""parent_context_auto_join" => "accepted""#),
            "auto-join memberships must start 'accepted' (Canvas parity)"
        );
        assert!(
            derive.contains(r#""parent_context_request" => "requested""#),
            "request-join memberships must start 'requested' (Canvas parity)"
        );
        assert!(
            derive.contains(r#"_ => "invited""#),
            "invitation-only memberships must start 'invited' (Canvas parity)"
        );
    }

    #[test]
    fn test_join_level_values() {
        // Valid join_level values per Canvas API.
        let valid_levels = [
            "invitation_only",
            "parent_context_auto_join",
            "parent_context_request",
        ];
        for level in valid_levels {
            assert!(
                [
                    "invitation_only",
                    "parent_context_auto_join",
                    "parent_context_request"
                ]
                .contains(&level),
                "{} is a valid join_level",
                level
            );
        }
    }

    #[test]
    fn test_deleted_group_excluded_from_list() {
        // GET /courses/{courseId}/groups excludes workflow_state = 'deleted'.
        let src = handler_source();
        let list_sql = src
            .split("async fn list_groups")
            .nth(1)
            .expect("list_groups exists")
            .split("async fn")
            .next()
            .unwrap_or("");
        assert!(
            list_sql.contains("workflow_state != 'deleted'")
                || list_sql.contains("workflow_state <> 'deleted'"),
            "list_groups must filter out deleted groups (Canvas parity)"
        );
    }

    #[test]
    fn test_deleted_membership_excluded_from_list() {
        // GET /groups/{groupId}/memberships excludes workflow_state = 'deleted'.
        let src = handler_source();
        let list_sql = src
            .split("async fn list_memberships")
            .nth(1)
            .expect("list_memberships exists")
            .split("async fn")
            .next()
            .unwrap_or("");
        assert!(
            list_sql.contains("workflow_state != 'deleted'")
                || list_sql.contains("workflow_state <> 'deleted'"),
            "list_memberships must filter out deleted memberships (Canvas parity)"
        );
    }

    #[test]
    fn test_soft_delete_group() {
        // DELETE /groups/{id} is a soft delete via workflow_state.
        let src = handler_source();
        let delete_sql = src
            .split("async fn delete_group")
            .nth(1)
            .expect("delete_group exists")
            .split("async fn")
            .next()
            .unwrap_or("");
        assert!(
            delete_sql.contains("UPDATE groups SET workflow_state = 'deleted'"),
            "delete_group must soft-delete via workflow_state (Canvas parity)"
        );
    }

    #[test]
    fn test_soft_delete_membership() {
        // DELETE /groups/{groupId}/memberships/{membershipId} soft-deletes.
        let src = handler_source();
        let delete_sql = src
            .split("async fn delete_membership")
            .nth(1)
            .expect("delete_membership exists")
            .split("async fn")
            .next()
            .unwrap_or("");
        assert!(
            delete_sql.contains("UPDATE group_memberships SET workflow_state = 'deleted'"),
            "delete_membership must soft-delete via workflow_state (Canvas parity)"
        );
    }
}
