//! folders_api — Canvas `FoldersController` parity for the group scoped folder
//! routes.
//!
//! Routes (Canvas `config/routes.rb`, `ApiRouteSet.draw`):
//! - GET  /api/v1/groups/:group_id/folders               — `list_all_folders`
//! - POST /api/v1/groups/:group_id/folders               — `create`
//! - GET  /api/v1/groups/:group_id/folders/media         — `media_folder`
//! - GET  /api/v1/groups/:group_id/folders/:id/duplicates — `duplicates`
//!
//! Canvas behavior mirrored here (`app/controllers/folders_controller.rb`,
//! `app/models/folder.rb`, `lib/api/v1/folders.rb`):
//! - `list_all_folders` needs `:read_files` on the group, lists the flat set of
//!   the group's folders (`active_folders`), sorted by name unless
//!   `sort_by=position`, and hides hidden/locked folders from callers that may
//!   not see hidden files.
//! - `create` needs the group's `:create` right (`manage_files_add`), rejects a
//!   body that sets both a parent id and a parent path with the Canvas 400
//!   "Can't set folder path and folder id", answers 404 for a parent that does
//!   not live in the group, falls back to the Canvas `unfiled` folder when no
//!   parent is given, and creates the missing folders of a `parent_folder_path`
//!   (Canvas `Folder.assert_path`).
//! - `media_folder` needs `:read_files`; the "Uploaded Media" folder belongs to
//!   the group only for callers holding a granular file permission there, every
//!   other caller gets the media folder of their own user context.
//! - `duplicates` is gated on the two site admin feature flags
//!   (`files_a11y_rewrite` and `files_a11y_folder_duplicates`), answers 403 for
//!   a folder of another context and lists the sibling folders that share the
//!   name of the requested folder.
//!
//! The `by_path` resolvers of this slice
//! (GET /api/v1/courses|users|groups/:id/folders/by_path/*full_path) are already
//! registered by the crate router in `src/lib.rs` — they must not be registered
//! a second time: axum 0.8 panics with "Overlapping method route" when the
//! same path and method are added twice.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, ApiJson, AppState};

use crate::{
    canvas_error, canvas_not_found, dedupe_folder_name, ensure_root_folder_for,
    ensure_unfiled_folder, fetch_child_by_name, fetch_folder, fetch_sibling_names,
    find_or_create_unique_folder, insert_folder, is_admin, join_full_name, next_position,
    normalize_folder_name, pagination, user_enrolled_or_admin, user_in_group,
    user_teacher_of_course, CreateFolderRequest, DuplicatesResponse, FolderListQuery,
    FolderResponse, FolderRow, NewFolder, FOLDER_SELECT, MEDIA_TYPE,
};

/// The name of the Canvas media folder (`Folder.media_folder`).
const MEDIA_FOLDER_NAME: &str = "Uploaded Media";

// ─── group scope helpers ─────────────────────────────────────────────────────

/// A Canvas group and the course it belongs to (an account group has none).
struct GroupScope {
    id: i64,
    course_id: Option<i64>,
}

/// Canvas `require_context`: an unknown or deleted group is a 404.
async fn load_group(db: &PgPool, group_id: i64) -> Result<GroupScope, ApiError> {
    let row: Option<(i64, Option<i64>)> = sqlx::query_as(
        "SELECT id, course_id FROM groups WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(group_id)
    .fetch_optional(db)
    .await?;
    let (id, course_id) = row.ok_or(ApiError::NotFound)?;
    Ok(GroupScope { id, course_id })
}

/// The group rights of a group context come from the course the group belongs
/// to: `manage_files_add` (and with it `:create` and `:manage_contents`) is
/// held by the teachers of that course, an account admin holds every right.
async fn group_can_manage(
    db: &PgPool,
    auth: &AuthUser,
    group: &GroupScope,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    match group.course_id {
        Some(course_id) => user_teacher_of_course(db, auth.id, course_id, &auth.role).await,
        None => Ok(false),
    }
}

/// Whether the caller may read the group context.
///
/// Canvas' `Group` policy (`app/models/group.rb:662`) grants the group's
/// permissions when the group's *context* grants the user `:read`:
///
/// ```ruby
/// given { |user, session| !non_collaborative? && (context.is_a?(Account) || context&.grants_right?(user, session, :read) || false) }
/// ```
///
/// so a member of the group's course may read the group without being a group
/// member.
async fn group_can_read(
    db: &PgPool,
    auth: &AuthUser,
    group: &GroupScope,
) -> Result<bool, ApiError> {
    if group_can_manage(db, auth, group).await? {
        return Ok(true);
    }
    if user_in_group(db, auth.id, group.id).await? {
        return Ok(true);
    }
    match group.course_id {
        Some(course_id) => user_enrolled_or_admin(db, auth.id, course_id, &auth.role).await,
        None => Ok(false),
    }
}

// ─── list_all_folders ────────────────────────────────────────────────────────

/// GET /groups/{groupId}/folders — Canvas FoldersController#list_all_folders.
async fn list_group_folders(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    Query(params): Query<FolderListQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<FolderResponse>>, ApiError> {
    let group = load_group(&state.db, group_id).await?;
    let can_manage = group_can_manage(&state.db, &auth, &group).await?;
    let can_read = group_can_read(&state.db, &auth, &group).await?;
    let _ = can_manage;
    if !can_read {
        return Err(ApiError::Forbidden);
    }

    let (per_page, offset) = pagination(params.page, params.per_page);
    let by_position = params.sort_by.as_deref() == Some("position");

    // Canvas `folder_index_scope`: @context.active_folders, reduced to the
    // visible and unlocked folders for callers that may not see hidden files.
    let mut sql = format!(
        "{FOLDER_SELECT} WHERE f.context_type = 'Group' AND f.context_id = $1 \
         AND f.workflow_state <> 'deleted'"
    );
    if !can_manage {
        sql.push_str(
            " AND f.workflow_state <> 'hidden' AND f.locked = FALSE \
             AND (f.lock_at IS NULL OR f.lock_at > now() \
                  OR (f.unlock_at IS NOT NULL AND f.unlock_at < now()))",
        );
    }
    sql.push_str(if by_position {
        " ORDER BY f.position ASC, f.id ASC"
    } else {
        " ORDER BY f.name ASC, f.id ASC"
    });
    sql.push_str(" LIMIT $2 OFFSET $3");

    let rows = sqlx::query_as::<_, FolderRow>(&sql)
        .bind(group.id)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;

    Ok(Json(
        rows.into_iter()
            .map(|row| FolderResponse::from_row(row, can_manage))
            .collect(),
    ))
}

// ─── create ──────────────────────────────────────────────────────────────────

/// POST /groups/{groupId}/folders — Canvas FoldersController#create.
async fn create_group_folder(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
    ApiJson(payload): ApiJson<CreateFolderRequest>,
) -> Result<Response, ApiError> {
    let group = load_group(&state.db, group_id).await?;

    let body_parent_id = payload.folder_id.or(payload.parent_folder_id);
    // Canvas rejects mixing parent_folder_path with an explicit parent id.
    if payload.parent_folder_path.is_some() && body_parent_id.is_some() {
        return Ok(canvas_error(
            StatusCode::BAD_REQUEST,
            "Can't set folder path and folder id",
        ));
    }

    // Canvas: authorized_action(@folder, @current_user, :create) — the
    // `manage_files_add` right of the group.
    let can_manage = group_can_manage(&state.db, &auth, &group).await?;
    if !can_manage {
        return Err(ApiError::Forbidden);
    }

    let root = ensure_root_folder_for(&state.db, "Group", group.id).await?;

    let parent: FolderRow = if let Some(parent_id) = body_parent_id {
        let Some(folder) = fetch_folder(&state.db, parent_id).await? else {
            return Ok(canvas_not_found());
        };
        if folder.context_type != "Group" || folder.context_id != group.id {
            return Ok(canvas_not_found());
        }
        folder
    } else if let Some(path) = payload.parent_folder_path.as_deref() {
        ensure_group_path(&state.db, &group, &root, path).await?
    } else {
        // Canvas Folder.unfiled_folder(context).
        ensure_unfiled_folder(&state.db, root).await?
    };

    let requested_name = normalize_folder_name(payload.name.as_deref());
    let siblings = fetch_sibling_names(&state.db, parent.id).await?;
    let name = dedupe_folder_name(&requested_name, &siblings);
    let full_name = join_full_name(&parent.full_name, &name);
    let position = match payload.position {
        Some(position) => position,
        None => next_position(&state.db, parent.id).await?,
    };
    // Canvas Folder#hidden= maps the hidden param onto workflow_state
    // 'hidden'/'visible' (app/models/folder.rb), never a raw boolean.
    let workflow_state = if payload.hidden.unwrap_or(false) {
        "hidden"
    } else {
        "visible"
    };

    let new_folder = NewFolder {
        name,
        full_name,
        context_type: "Group".to_string(),
        context_id: group.id,
        parent_id: parent.id,
        workflow_state: workflow_state.to_string(),
        position,
        locked: payload.locked.unwrap_or(false),
        lock_at: payload.lock_at,
        unlock_at: payload.unlock_at,
        for_submissions: false,
    };
    let row = insert_folder(&state.db, new_folder).await?;

    Ok(Json(FolderResponse::from_row(row, can_manage)).into_response())
}

/// Canvas `Folder.assert_path`: walk a slash separated path below the context
/// root and create the folders that do not exist yet.
async fn ensure_group_path(
    db: &PgPool,
    group: &GroupScope,
    root: &FolderRow,
    path: &str,
) -> Result<FolderRow, ApiError> {
    let components: Vec<&str> = path
        .split('/')
        .map(str::trim)
        .filter(|component| !component.is_empty() && *component != ".")
        .collect();

    let root_name = root.name.clone();
    let mut current = root.clone();
    let mut first = true;
    for component in components {
        if first {
            first = false;
            // Canvas drops a leading root folder name ("files/notes").
            if component == root_name {
                continue;
            }
        }
        let parent_id = current.id;
        let parent_full_name = current.full_name.clone();
        match fetch_child_by_name(db, parent_id, component).await? {
            Some(child) => current = child,
            None => {
                let position = next_position(db, parent_id).await?;
                let new_folder = NewFolder {
                    name: component.to_string(),
                    full_name: join_full_name(&parent_full_name, component),
                    context_type: "Group".to_string(),
                    context_id: group.id,
                    parent_id,
                    workflow_state: "visible".to_string(),
                    position,
                    locked: false,
                    lock_at: None,
                    unlock_at: None,
                    for_submissions: false,
                };
                current = insert_folder(db, new_folder).await?;
            }
        }
    }

    Ok(current)
}

// ─── media_folder ────────────────────────────────────────────────────────────

/// GET /groups/{groupId}/folders/media — Canvas FoldersController#media_folder.
async fn media_folder_group(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<FolderResponse>, ApiError> {
    let group = load_group(&state.db, group_id).await?;
    let can_manage = group_can_manage(&state.db, &auth, &group).await?;
    let can_read = group_can_read(&state.db, &auth, &group).await?;
    let _ = can_manage;
    if !can_read {
        return Err(ApiError::Forbidden);
    }

    // Canvas: the media folder belongs to the group only for callers holding a
    // granular file permission there, every other caller gets the media
    // folder of their own user context.
    let (context_type, context_id, root) = if can_manage {
        let root = ensure_root_folder_for(&state.db, "Group", group.id).await?;
        ("Group", group.id, root)
    } else {
        let root = ensure_root_folder_for(&state.db, "User", auth.id).await?;
        ("User", auth.id, root)
    };

    let folder = find_or_create_unique_folder(
        &state.db,
        context_type,
        context_id,
        root.id,
        MEDIA_TYPE,
        MEDIA_FOLDER_NAME,
    )
    .await?;

    Ok(Json(FolderResponse::from_row(folder, can_manage)))
}

// ─── duplicates ──────────────────────────────────────────────────────────────

/// GET /groups/{groupId}/folders/{id}/duplicates — Canvas
/// FoldersController#duplicates: the sibling folders that share the name of
/// the requested folder.
async fn list_group_folder_duplicates(
    State(state): State<AppState>,
    Path((group_id, folder_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<DuplicatesResponse>, ApiError> {
    let group = load_group(&state.db, group_id).await?;

    // Canvas: the endpoint needs both a11y feature flags on the site admin.
    if !duplicates_feature_enabled(&state.db).await? {
        return Err(ApiError::Forbidden);
    }

    let Some(folder) = fetch_folder(&state.db, folder_id).await? else {
        return Err(ApiError::NotFound);
    };
    if folder.context_type != "Group" || folder.context_id != group.id {
        // Canvas render_unauthorized_action.
        return Err(ApiError::Forbidden);
    }

    let can_manage = group_can_manage(&state.db, &auth, &group).await?;
    let can_read = group_can_read(&state.db, &auth, &group).await?;
    let _ = can_manage;
    if !can_read {
        return Err(ApiError::Forbidden);
    }

    // Canvas find_duplicate_folders: a root folder has no duplicates.
    let Some(parent_id) = folder.parent_folder_id else {
        return Ok(Json(DuplicatesResponse {
            duplicates: Vec::new(),
        }));
    };
    let sql = format!(
        "{FOLDER_SELECT} WHERE f.parent_folder_id = $1 AND f.name = $2 \
         AND f.id <> $3 AND f.workflow_state <> 'deleted' ORDER BY f.id ASC"
    );
    let rows = sqlx::query_as::<_, FolderRow>(&sql)
        .bind(parent_id)
        .bind(&folder.name)
        .bind(folder.id)
        .fetch_all(&state.db)
        .await?;

    Ok(Json(DuplicatesResponse {
        duplicates: rows
            .into_iter()
            .map(|row| FolderResponse::from_row(row, can_manage))
            .collect(),
    }))
}

/// Canvas `Account.site_admin.feature_enabled?` for the two flags that gate
/// the duplicates endpoint.
async fn duplicates_feature_enabled(db: &PgPool) -> Result<bool, ApiError> {
    let root: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL AND workflow_state <> 'deleted' \
         ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(db)
    .await?;
    let Some((root_id,)) = root else {
        return Ok(false);
    };

    let (count,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM feature_flags \
         WHERE context_type = 'Account' AND context_id = $1 \
           AND feature IN ('files_a11y_rewrite', 'files_a11y_folder_duplicates') \
           AND state = 'enabled'",
    )
    .bind(root_id)
    .fetch_one(db)
    .await?;
    Ok(count == 2)
}

// ─── router ──────────────────────────────────────────────────────────────────

/// Build the folders API router.
///
/// The group scoped routes of the Canvas folders API. The `by_path` resolvers
/// are registered by the crate router (see the module docs).
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/groups/{groupId}/folders",
            get(list_group_folders).post(create_group_folder),
        )
        .route("/groups/{groupId}/folders/media", get(media_folder_group))
        .route(
            "/groups/{groupId}/folders/{id}/duplicates",
            get(list_group_folder_duplicates),
        )
}
