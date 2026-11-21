//! Routes V2 — grouped job listings and search.
//!
//! Canvas: `app/controllers/jobs_v2_controller.rb`.
//! Routes:
//! - `GET /api/v1/jobs2/:bucket/by_:group`         → grouped_info
//! - `GET /api/v1/jobs2/:bucket/by_:group/search` → search
//!
//! Canvas embeds the group in a `by_:group` path segment (Rails partial
//! segments); axum cannot express that, so the routes are
//! `/jobs2/{id}/{group}` with the `by_` prefix parsed in the handlers (the
//! `{id}` name matches the other `/jobs2/*` routes for matchit
//! compatibility).

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::{require_admin, Bucket};

/// Group parameter — which column to group by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Tag,
    Strand,
    Singleton,
}

impl Group {
    /// Parse the group name out of a `by_{group}` path segment.
    fn parse(raw: &str) -> Option<Self> {
        let name = raw.strip_prefix("by_")?;
        match name {
            "tag" => Some(Self::Tag),
            "strand" => Some(Self::Strand),
            "singleton" => Some(Self::Singleton),
            _ => None,
        }
    }

    fn column(&self) -> &'static str {
        match self {
            Self::Tag => "tag",
            Self::Strand => "strand",
            Self::Singleton => "singleton",
        }
    }
}

/// Query params for `GET /jobs2/:bucket/by_:group/search`.
#[derive(Debug, Deserialize)]
pub struct GroupedSearchQuery {
    /// Required search term.
    pub term: String,
    pub start_date: Option<DateTime<Utc>>,
    pub end_date: Option<DateTime<Utc>>,
}

/// Query params for `GET /jobs2/:bucket/by_:group`.
#[derive(Debug, Deserialize)]
pub struct GroupedInfoQuery {
    pub order: Option<String>,
    pub scope: Option<String>,
    pub start_date: Option<DateTime<Utc>>,
    pub end_date: Option<DateTime<Utc>>,
    pub per_page: Option<i64>,
    pub page: Option<i64>,
}

/// Grouped row — one entry in the array response for `grouped_info`.
#[derive(Debug, Serialize)]
pub struct GroupedRow {
    pub count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strand: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub singleton: Option<String>,
    pub info: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orphaned: Option<bool>,
}

/// `GET /api/v1/jobs2/:bucket/by_:group/search`.
///
/// Canvas: `JobsV2Controller#grouped_info` → search mode.
/// Uses `ILIKE '%term%'` against the group column, groups by column,
/// orders by count DESC, limit 100.
pub async fn search_jobs(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((bucket_raw, group_raw)): Path<(String, String)>,
    Query(params): Query<GroupedSearchQuery>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&auth)?;

    let bucket = Bucket::parse(&bucket_raw)
        .ok_or_else(|| ApiError::BadRequest(format!("invalid bucket: {bucket_raw}")))?;
    let group = Group::parse(&group_raw)
        .ok_or_else(|| ApiError::BadRequest(format!("invalid group: {group_raw}")))?;

    let where_clause = bucket.where_clause();
    let col = group.column();

    let mut sql = format!(
        r#"
        SELECT {col} AS grp, COUNT(*) AS count
        FROM jobs
        WHERE {where_clause}
          AND {col} ILIKE $1
        "#,
    );

    let mut bind_idx = 2;
    if params.start_date.is_some() {
        sql.push_str(&format!(" AND run_at >= ${bind_idx}"));
        bind_idx += 1;
    }
    if params.end_date.is_some() {
        sql.push_str(&format!(" AND run_at <= ${bind_idx}"));
    }

    sql.push_str(&format!(" GROUP BY {col} ORDER BY count DESC LIMIT 100"));

    let mut qb = sqlx::query_as::<_, SearchGroupRow>(&sql)
        .bind(format!("%{}%", params.term.replace('\'', "''")));

    if let Some(start) = params.start_date {
        qb = qb.bind(start);
    }
    if let Some(end) = params.end_date {
        qb = qb.bind(end);
    }

    let rows: Vec<SearchGroupRow> = qb.fetch_all(&state.db).await?;

    // Build `{"TagName": count, ...}` map.
    let mut map = serde_json::Map::new();
    for row in rows {
        if let Some(grp) = row.grp {
            map.insert(grp, json!(row.count));
        }
    }

    Ok(Json(Value::Object(map)))
}

#[derive(Debug, sqlx::FromRow)]
struct SearchGroupRow {
    grp: Option<String>,
    count: i64,
}

/// `GET /api/v1/jobs2/:bucket/by_:group`.
///
/// Canvas: `JobsV2Controller#grouped_info` (non-search mode).
pub async fn grouped_info(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((bucket_raw, group_raw)): Path<(String, String)>,
    Query(params): Query<GroupedInfoQuery>,
) -> Result<Json<Vec<GroupedRow>>, ApiError> {
    require_admin(&auth)?;

    let bucket = Bucket::parse(&bucket_raw)
        .ok_or_else(|| ApiError::BadRequest(format!("invalid bucket: {bucket_raw}")))?;
    let group = Group::parse(&group_raw)
        .ok_or_else(|| ApiError::BadRequest(format!("invalid group: {group_raw}")))?;

    let order = params.order.as_deref().unwrap_or("info");
    let limit = params.per_page.unwrap_or(100).clamp(1, 100);
    let offset = ((params.page.unwrap_or(1) - 1) * limit).max(0);

    let where_clause = bucket.where_clause();
    let col = group.column();

    // For strand/singleton with queued/future bucket: check orphaned.
    let include_orphaned = matches!(group, Group::Strand | Group::Singleton)
        && matches!(bucket, Bucket::Queued | Bucket::Future);

    let mut sql = format!(
        r#"
        SELECT {col} AS grp,
               COUNT(*) AS count,
               MIN(run_at) AS earliest_run_at,
               MIN(locked_at) AS earliest_locked_at,
               MAX(failed_at) AS latest_failed_at,
               BOOL_OR(CASE WHEN next_in_strand THEN FALSE ELSE TRUE END) AS has_orphaned
        FROM jobs
        WHERE {where_clause}
        "#,
    );

    let mut bind_idx = 1;
    if params.start_date.is_some() {
        sql.push_str(&format!(" AND run_at >= ${bind_idx}"));
        bind_idx += 1;
    }
    if params.end_date.is_some() {
        sql.push_str(&format!(" AND run_at <= ${bind_idx}"));
    }

    sql.push_str(&format!(
        " GROUP BY {col} ORDER BY {} LIMIT {} OFFSET {}",
        group_order_clause(order, group),
        limit,
        offset
    ));

    let mut qb = sqlx::query_as::<_, GroupedRowDb>(&sql);

    if let Some(start) = params.start_date {
        qb = qb.bind(start);
    }
    if let Some(end) = params.end_date {
        qb = qb.bind(end);
    }

    let rows: Vec<GroupedRowDb> = qb.fetch_all(&state.db).await?;

    let result: Vec<GroupedRow> = rows
        .into_iter()
        .map(|row| {
            let info = compute_group_info(
                &bucket,
                row.earliest_run_at,
                row.earliest_locked_at,
                row.latest_failed_at,
            );
            let orphaned = if include_orphaned {
                Some(row.has_orphaned)
            } else {
                None
            };

            match group {
                Group::Tag => GroupedRow {
                    count: row.count,
                    tag: row.grp,
                    strand: None,
                    singleton: None,
                    info,
                    orphaned,
                },
                Group::Strand => GroupedRow {
                    count: row.count,
                    tag: None,
                    strand: row.grp,
                    singleton: None,
                    info,
                    orphaned,
                },
                Group::Singleton => GroupedRow {
                    count: row.count,
                    tag: None,
                    strand: None,
                    singleton: row.grp,
                    info,
                    orphaned,
                },
            }
        })
        .collect();

    Ok(Json(result))
}

#[derive(Debug, sqlx::FromRow)]
struct GroupedRowDb {
    grp: Option<String>,
    count: i64,
    earliest_run_at: Option<DateTime<Utc>>,
    earliest_locked_at: Option<DateTime<Utc>>,
    latest_failed_at: Option<DateTime<Utc>>,
    has_orphaned: bool,
}

/// Compute the `info` field for a grouped row given the bucket.
fn compute_group_info(
    bucket: &Bucket,
    earliest_run_at: Option<DateTime<Utc>>,
    earliest_locked_at: Option<DateTime<Utc>>,
    latest_failed_at: Option<DateTime<Utc>>,
) -> Value {
    match bucket {
        Bucket::Queued => {
            // Seconds since earliest run_at.
            if let Some(at) = earliest_run_at {
                json!((Utc::now() - at).num_seconds().max(0))
            } else {
                json!(0)
            }
        }
        Bucket::Running => {
            // Seconds since earliest locked_at.
            if let Some(at) = earliest_locked_at {
                json!((Utc::now() - at).num_seconds().max(0))
            } else {
                json!(0)
            }
        }
        Bucket::Future => {
            // ISO timestamp of earliest scheduled run.
            if let Some(at) = earliest_run_at {
                json!(at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            } else {
                json!(null)
            }
        }
        Bucket::Failed => {
            // ISO timestamp of last failure.
            if let Some(at) = latest_failed_at {
                json!(at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            } else {
                json!(null)
            }
        }
    }
}

/// ORDER BY clause for grouped queries.
fn group_order_clause(order: &str, group: Group) -> &'static str {
    match order {
        "count" => "count DESC",
        "tag" if group == Group::Tag => "count DESC",
        "strand" if group == Group::Strand => "count DESC",
        "singleton" if group == Group::Singleton => "count DESC",
        _ => "count DESC",
    }
}

/// Build the grouped routes sub-router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/jobs2/{id}/{group}/search", get(search_jobs))
        .route("/jobs2/{id}/{group}", get(grouped_info))
}
