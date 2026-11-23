//! Files handlers — Canvas `files#index` / `#show` server-rendered port
//! (issue #110).
//!
//! Routes (registered in `lib.rs` BEFORE the generic `/courses/{id}/{tab}`
//! route so they win the match):
//! - GET /courses/{id}/files        — index (?folder_id=N, root default)
//! - GET /courses/{id}/files/{fid}  — file metadata show page
//!
//! Read-only on purpose. The `:files` concern (`canvas-src/config/routes.rb:
//! 114-129`) also routes `POST /courses/:cours_id/files` (to `files#create`,
//! an action `FilesController` does not define — real uploads are
//! `POST /api/v1/courses/:course_id/files` → `courses#create_file`) and a
//! `/download` member whose handler ends in `Attachment` byte storage (proxy
//! or redirect to the stored file). This workspace has no byte backing store:
//! `files` rows are metadata only, so an upload would create a record no
//! reader can consume and "download" could only answer with the
//! `{public_url}` JSON of `GET /api/v1/files/{id}/public_url` — a different
//! content type from what Canvas sends. Both stay out until there is storage;
//! hidden/locked/usage-rights columns are likewise not modeled and are
//! omitted.

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;
use serde::Deserialize;

use star_common::{ApiError, AppState};
use star_files::ui as files_ui;

use crate::handlers::course_context::{auth_of, course_context};
use crate::handlers::error::from_api_error;
use crate::session::{ensure_csrf_cookie, SessionUser};
use crate::{assets, env::JsEnv};

/// A folder entry in the left tree / breadcrumb.
pub struct FolderRow {
    pub id: i64,
    pub name: String,
}

/// A file row in the index table.
pub struct FileRow {
    pub id: i64,
    pub name: String,
    pub size: i64,
    pub content_type: String,
    pub updated_at: String,
}

/// The files index template context (Canvas files#index: left folder tree,
/// breadcrumb of folder names, file table).
#[derive(Template)]
#[template(path = "files_index.html")]
pub struct FilesIndexPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    pub unread_badge: i64,
    pub tabs: Vec<star_courses::CourseTab>,
    pub active_context_tab: String,
    pub course_name: String,
    pub course_id: i64,
    pub folders: Vec<FolderRow>,
    pub trail: Vec<FolderRow>,
    pub files: Vec<FileRow>,
    pub can_manage: bool,
    pub quota_mb: i64,
    pub quota_used_mb: i64,
}

impl IntoResponse for FilesIndexPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => {
                ApiError::Db(format!("files index template render failed: {err}")).into_response()
            }
        }
    }
}

/// The file show template context (metadata + Download action).
#[derive(Template)]
#[template(path = "file_show.html")]
pub struct FileShowPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    pub unread_badge: i64,
    pub tabs: Vec<star_courses::CourseTab>,
    pub active_context_tab: String,
    pub course_name: String,
    pub course_id: i64,
    pub file_id: i64,
    pub file_name: String,
    pub content_type: String,
    pub size: i64,
    pub updated_at: String,
}

impl IntoResponse for FileShowPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => {
                ApiError::Db(format!("file show template render failed: {err}")).into_response()
            }
        }
    }
}

/// `?folder_id=N` on the index (absent = the course root folder).
#[derive(Deserialize, Default)]
pub struct FilesIndexQuery {
    pub folder_id: Option<i64>,
}

/// The files form payloads: exactly one of `new_folder` (name) or the upload
/// fields is filled, matching the two forms on the page.
#[derive(Deserialize)]
/// GET /courses/{id}/files — the index (root folder or `?folder_id=N`).
pub async fn files_index(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    Query(query): Query<FilesIndexQuery>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match course_context(&app, &user, course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };

    let root = match files_ui::load_root_folder_for_ui(&app.db, &auth_of(&user), course_id).await {
        Ok(f) => f,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    let folder_id = query.folder_id.unwrap_or(root.id);

    let trail = match files_ui::load_breadcrumb_for_ui(
        &app.db,
        &auth_of(&user),
        course_id,
        folder_id,
    )
    .await
    {
        Ok(t) => t,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    let folders = match files_ui::load_subfolders_for_ui(
        &app.db,
        &auth_of(&user),
        course_id,
        folder_id,
    )
    .await
    {
        Ok(f) => f,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    let file_rows =
        match files_ui::load_files_for_ui(&app.db, &auth_of(&user), course_id, folder_id).await {
            Ok(f) => f,
            Err(e) => return from_api_error(&e, &origin).into_response(),
        };

    let folders_view: Vec<FolderRow> = folders
        .into_iter()
        .map(|f| FolderRow {
            id: f.id,
            name: f.name,
        })
        .collect();
    let trail_view: Vec<FolderRow> = trail
        .into_iter()
        .map(|f| FolderRow {
            id: f.id,
            name: f.name,
        })
        .collect();
    let files_view: Vec<FileRow> = file_rows
        .into_iter()
        .map(|f| FileRow {
            id: f.id,
            name: f.display_name,
            size: f.size,
            content_type: f.content_type,
            updated_at: f.updated_at.format("%b %-d, %Y").to_string(),
        })
        .collect();

    // Usage quota (Canvas shows a usage bar on the files page); bytes to MB
    // for the template, matching Canvas's api_quota rounding at whole MBs.
    let (quota_bytes, used_bytes) =
        match files_ui::load_quota_for_ui(&app.db, &auth_of(&user), course_id).await {
            Ok(q) => q,
            Err(e) => return from_api_error(&e, &origin).into_response(),
        };
    let quota_mb = quota_bytes / (1024 * 1024);
    let quota_used_mb = used_bytes / (1024 * 1024);

    let context_asset_string = format!("course_{course_id}");
    let env = JsEnv::for_user(
        user.id,
        &user.role,
        ctx.preferences,
        ctx.use_high_contrast,
        &origin,
        Some("files"),
        &context_asset_string,
        Vec::new(),
    );

    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let page = FilesIndexPage {
        title: format!("Files - {}", ctx.course.name),
        body_class: format!(
            "with-left-side course-menu-expanded context-{context_asset_string} files"
        ),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: Vec::new(),
        user_name: ctx.user_name,
        active_path: "courses",
        breadcrumb: vec![
            ("Dashboard".to_string(), "/dashboard".to_string()),
            (ctx.course.name.clone(), format!("/courses/{course_id}")),
            ("Files".to_string(), format!("/courses/{course_id}/files")),
        ],
        csrf_token,
        unread_badge: ctx.unread_badge,
        tabs: ctx.tabs,
        active_context_tab: "files".to_string(),
        course_name: ctx.course.name.clone(),
        course_id,
        folders: folders_view,
        trail: trail_view,
        files: files_view,
        can_manage: ctx.is_teacher,
        quota_mb,
        quota_used_mb,
    };
    (jar, page).into_response()
}

/// GET /courses/{id}/files/{fid} — file metadata show page.
pub async fn file_show(
    State(app): State<AppState>,
    Path((course_id, file_id)): Path<(i64, i64)>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match course_context(&app, &user, course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };

    let file = match files_ui::load_file_for_ui(&app.db, &auth_of(&user), course_id, file_id).await
    {
        Ok(f) => f,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    let context_asset_string = format!("course_{course_id}");
    let env = JsEnv::for_user(
        user.id,
        &user.role,
        ctx.preferences,
        ctx.use_high_contrast,
        &origin,
        Some("files"),
        &context_asset_string,
        Vec::new(),
    );

    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let page = FileShowPage {
        title: format!("{} - Files", file.display_name),
        body_class: format!(
            "with-left-side course-menu-expanded context-{context_asset_string} files"
        ),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: Vec::new(),
        user_name: ctx.user_name,
        active_path: "courses",
        breadcrumb: vec![
            ("Dashboard".to_string(), "/dashboard".to_string()),
            (ctx.course.name.clone(), format!("/courses/{course_id}")),
            ("Files".to_string(), format!("/courses/{course_id}/files")),
        ],
        csrf_token,
        unread_badge: ctx.unread_badge,
        tabs: ctx.tabs,
        active_context_tab: "files".to_string(),
        course_name: ctx.course.name.clone(),
        course_id,
        file_id,
        file_name: file.display_name,
        content_type: file.content_type,
        size: file.size,
        updated_at: file.updated_at.format("%b %-d, %Y").to_string(),
    };
    (jar, page).into_response()
}
