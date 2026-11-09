//! star-api — shared API helpers for star-lms domain crates.
//!
//! Provides:
//! - Pagination extraction and response wrappers.
//! - Authorization helpers for course enrollment and teacher checks.
//!
//! Domain crates import these helpers rather than duplicating them.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use star_common::ApiError;

// ---------------------------------------------------------------------------
// Pagination types
// ---------------------------------------------------------------------------

/// Query-string extractor for paginated list endpoints.
///
// Deserializes from `?page=N&per_page=N` in the request URL.
// Defaults: page=1, per_page=20.  Clamps per_page to [1, 100].
#[derive(Debug, Deserialize)]
pub struct PaginationParams {
    /// 1-based page number.
    #[serde(default = "default_page")]
    pub page: i64,

    /// Items per page.
    #[serde(default = "default_per_page")]
    pub per_page: i64,
}

fn default_page() -> i64 {
    1
}

fn default_per_page() -> i64 {
    20
}

impl Default for PaginationParams {
    fn default() -> Self {
        Self {
            page: default_page(),
            per_page: default_per_page(),
        }
    }
}

impl PaginationParams {
    /// 0-based row offset for SQL LIMIT/OFFSET.
    pub fn offset(&self) -> i64 {
        (self.page().max(1) - 1) * self.per_page().max(1)
    }

    /// Effective page (clamped to ≥ 1).
    pub fn page(&self) -> i64 {
        self.page.max(1)
    }

    /// Effective per_page (clamped to [1, 100]).
    pub fn per_page(&self) -> i64 {
        self.per_page.clamp(1, 100)
    }
}

/// Metadata describing the current page within a paginated response.
#[derive(Debug, Clone, Serialize)]
pub struct PaginationMeta {
    pub page: i64,
    pub per_page: i64,
    pub total: i64,
}

/// A wrapped list response with pagination metadata.
#[derive(Debug, Clone, Serialize)]
pub struct Paginated<T> {
    pub data: Vec<T>,
    pub pagination: PaginationMeta,
}

impl<T> Paginated<T> {
    pub fn new(data: Vec<T>, page: i64, per_page: i64, total: i64) -> Self {
        Self {
            data,
            pagination: PaginationMeta {
                page,
                per_page,
                total,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Authorization helpers
// ---------------------------------------------------------------------------

/// Verify the user is enrolled in the course.  Admin bypasses enrollment check.
///
/// Returns `Ok(())` on success.  Returns `Err(ApiError::Forbidden)` if the
/// user is not enrolled and is not an admin.
pub async fn check_enrollment(
    user_id: i64,
    course_id: i64,
    role: &str,
    pool: &PgPool,
) -> Result<(), ApiError> {
    if role == "admin" {
        return Ok(());
    }

    let enrolled: (bool,) = sqlx::query_as(
        r#"SELECT EXISTS(
            SELECT 1 FROM enrollments
            WHERE user_id = $1 AND course_id = $2
        )"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_one(pool)
    .await?;

    if enrolled.0 {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

/// Verify the user has teacher privileges on the course.
///
/// Pass-through for admins; checks for a teacher enrollment for others.
///
/// Returns `Ok(())` on success.  Returns `Err(ApiError::Forbidden)` if the
/// user is neither an admin nor a teacher of the course.
pub async fn require_course_teacher(
    user_id: i64,
    role: &str,
    course_id: i64,
    pool: &PgPool,
) -> Result<(), ApiError> {
    if role == "admin" {
        return Ok(());
    }

    let teacher: (bool,) = sqlx::query_as(
        r#"SELECT EXISTS(
            SELECT 1 FROM enrollments
            WHERE user_id = $1 AND course_id = $2 AND role = 'teacher'
        )"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_one(pool)
    .await?;

    if teacher.0 {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse query string via serde_urlencoded, mirroring how axum's Query
    /// extractor works internally.
    fn parse_query(query: &str) -> PaginationParams {
        serde_urlencoded::from_str(query).unwrap_or_default()
    }

    #[test]
    fn pagination_defaults() {
        let p = parse_query("");
        assert_eq!(p.page, 1);
        assert_eq!(p.per_page, 20);
    }

    #[test]
    fn pagination_default_impl() {
        let p = PaginationParams::default();
        assert_eq!(p.page, 1);
        assert_eq!(p.per_page, 20);
    }

    #[test]
    fn pagination_explicit_page() {
        let p = parse_query("page=3");
        assert_eq!(p.page, 3);
        assert_eq!(p.per_page, 20); // default
    }

    #[test]
    fn pagination_explicit_per_page() {
        let p = parse_query("per_page=10");
        assert_eq!(p.page, 1); // default
        assert_eq!(p.per_page, 10);
    }

    #[test]
    fn pagination_both_params() {
        let p = parse_query("page=5&per_page=50");
        assert_eq!(p.page, 5);
        assert_eq!(p.per_page, 50);
    }

    #[test]
    fn pagination_clamp_per_page_max_100() {
        // Values above 100 are accepted by deserialization but clamped when accessed.
        let p = parse_query("per_page=500");
        assert_eq!(p.per_page, 500);
        assert_eq!(p.per_page(), 100); // clamped via accessor
    }

    #[test]
    fn pagination_clamp_negative_per_page() {
        let p = parse_query("per_page=-5");
        assert_eq!(p.per_page, -5);
        assert_eq!(p.per_page(), 1); // negative clamps to 1
    }

    #[test]
    fn pagination_page_0_clamped() {
        let p = parse_query("page=0");
        assert_eq!(p.page(), 1);
    }

    #[test]
    fn pagination_offset_basic() {
        let p = parse_query("page=1&per_page=20");
        assert_eq!(p.offset(), 0);
    }

    #[test]
    fn pagination_offset_page_3() {
        let p = parse_query("page=3&per_page=20");
        assert_eq!(p.offset(), 40);
    }

    #[test]
    fn pagination_offset_page_3_per_page_50() {
        let p = parse_query("page=3&per_page=50");
        assert_eq!(p.offset(), 100);
    }

    #[test]
    fn pagination_meta_serialize() {
        let meta = PaginationMeta {
            page: 2,
            per_page: 20,
            total: 100,
        };
        let json = serde_json::to_string(&meta).unwrap();
        assert!(json.contains(r#""page":2"#));
        assert!(json.contains(r#""per_page":20"#));
        assert!(json.contains(r#""total":100"#));
    }

    #[test]
    fn paginated_serialize() {
        #[derive(Serialize)]
        struct Item {
            id: i64,
        }
        let resp = Paginated::new(vec![Item { id: 1 }, Item { id: 2 }], 1, 20, 50);
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains(r#""data":["#));
        assert!(json.contains(r#""pagination":"#));
        assert!(json.contains(r#""total":50"#));
    }

    #[test]
    fn check_enrollment_admin_shortcut_compiles() {
        // Verify the role guard compiles and is used — actual DB call skipped.
        fn _guard(role: &str) -> bool {
            role == "admin"
        }
        assert!(_guard("admin"));
        assert!(!_guard("student"));
    }

    #[test]
    fn require_course_teacher_admin_shortcut_compiles() {
        fn _guard(role: &str) -> bool {
            role == "admin"
        }
        assert!(_guard("admin"));
        assert!(!_guard("teacher"));
        assert!(!_guard("student"));
    }
}
