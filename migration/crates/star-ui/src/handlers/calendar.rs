//! Calendar handler — Canvas `calendar` month view, server-rendered port
//! (issue #111).
//!
//! Route (registered in `lib.rs` with the other top-level shell routes):
//! - GET /calendar — month grid (?month=YYYY-MM&contexts[]=N)
//!
//! Canvas's calendar is a React SPA; per the UI spec we port the essential
//! server-rendered month grid. Week view, mini-calendar, drag-drop, the
//! scheduler and the .ics feed are deferred.
//!
//! Read-only on purpose. Canvas' web route set has exactly one calendar
//! document at this path — `get "calendar" => "calendars#show"`
//! (`canvas-src/config/routes.rb:1103`, and the same at :267/:720) — while
//! event writes live in the per-context `resources :calendar_events` scopes
//! (`/users/:user_id/calendar_events`, `/courses/:cours_id/calendar_events`,
//! `/accounts/:account_id/calendar_events`). A `/calendar` POST or a
//! `/calendar/{id}/delete` would be a route Canvas does not have.

use askama::Template;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};
use serde::Deserialize;

use star_calendar::ui as calendar_ui;
use star_common::{ApiError, AppState};

use crate::handlers::course_context::auth_of;
use crate::handlers::error::from_api_error;
use crate::session::{ensure_csrf_cookie, SessionUser};
use crate::{assets, env::JsEnv, queries};

/// One event chip on the month grid.
pub struct EventChip {
    pub id: i64,
    pub course_id: i64,
    pub course_name: String,
    pub title: String,
    pub day: u32,
    pub location_name: Option<String>,
}

/// A course context checkbox in the sidebar.
pub struct ContextCheckbox {
    pub course_id: i64,
    pub course_name: String,
    pub checked: bool,
}

/// One cell of the month grid (a day with its event chips).
pub struct GridCell {
    pub day: u32,
    pub in_month: bool,
    pub today: bool,
    pub chips: Vec<EventChip>,
}

/// The calendar template context (global page on the base layout).
#[derive(Template)]
#[template(path = "calendar.html")]
pub struct CalendarPage {
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
    pub month_label: String,
    pub weeks: Vec<Vec<GridCell>>,
    pub contexts: Vec<ContextCheckbox>,
    pub prev_month: String,
    pub next_month: String,
    pub this_month: String,
}

impl IntoResponse for CalendarPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => {
                ApiError::Db(format!("calendar template render failed: {err}")).into_response()
            }
        }
    }
}

/// `?month=YYYY-M(M)` and `?contexts[]=N` (absent contexts = all visible).
#[derive(Deserialize, Default)]
pub struct CalendarQuery {
    pub month: Option<String>,
    /// `?contexts[]=N` filters the grid; absent = all visible contexts.
    #[serde(default, rename = "contexts")]
    pub contexts: Vec<i64>,
}

/// First and last instant of the month containing `date`, in UTC.
fn month_bounds(date: NaiveDate) -> (DateTime<Utc>, DateTime<Utc>) {
    let first = NaiveDate::from_ymd_opt(date.year(), date.month(), 1).unwrap_or(date);
    let (next_year, next_month) = if date.month() == 12 {
        (date.year() + 1, 1)
    } else {
        (date.year(), date.month() + 1)
    };
    let next_first = NaiveDate::from_ymd_opt(next_year, next_month, 1).unwrap_or(date);
    let start = Utc.from_utc_datetime(&first.and_hms_opt(0, 0, 0).unwrap_or_default());
    let end = Utc.from_utc_datetime(&next_first.and_hms_opt(0, 0, 0).unwrap_or_default());
    (start, end)
}

/// Parse `YYYY-MM` into the first-of-month date; None on garbage.
fn parse_month(raw: &str) -> Option<NaiveDate> {
    let (y, m) = raw.split_once('-')?;
    let year = y.parse::<i32>().ok()?;
    let month = m.parse::<u32>().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    NaiveDate::from_ymd_opt(year, month, 1)
}

/// The `YYYY-MM` string of the month containing `date`.
fn month_param(date: NaiveDate) -> String {
    format!("{:04}-{:02}", date.year(), date.month())
}

/// A human label like "November 2026" for the month of `date`.
fn month_label(date: NaiveDate) -> String {
    date.format("%B %Y").to_string()
}

/// Month grid (Sunday-first weeks like Canvas) with event chips per day.
fn build_grid(
    month: NaiveDate,
    events: Vec<calendar_ui::UiCalendarEvent>,
    today: NaiveDate,
) -> Vec<Vec<GridCell>> {
    let mut by_day: std::collections::BTreeMap<u32, Vec<calendar_ui::UiCalendarEvent>> =
        std::collections::BTreeMap::new();
    for event in events {
        by_day.entry(event.start_at.day()).or_default().push(event);
    }

    // Weeks start on Sunday: back up from the 1st to the previous Sunday.
    let first = NaiveDate::from_ymd_opt(month.year(), month.month(), 1).unwrap_or(month);
    let grid_start = first - Duration::days(first.weekday().num_days_from_sunday() as i64);

    let mut weeks: Vec<Vec<GridCell>> = Vec::new();
    let mut cursor = grid_start;
    // 6 fixed weeks of 7 days always cover every month from its first Sunday.
    for _ in 0..6 {
        let mut week: Vec<GridCell> = Vec::with_capacity(7);
        for _ in 0..7 {
            let in_month = cursor.month() == month.month();
            let day = cursor.day();
            let chips = if in_month {
                by_day
                    .remove(&day)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|e| EventChip {
                        id: e.id,
                        course_id: e.course_id,
                        course_name: e.course_name,
                        title: e.title,
                        day,
                        location_name: e.location_name,
                    })
                    .collect()
            } else {
                Vec::new()
            };
            week.push(GridCell {
                day,
                in_month,
                today: cursor == today,
                chips,
            });
            cursor += Duration::days(1);
        }
        weeks.push(week);
    }
    weeks
}

/// GET /calendar — the month grid (?month=YYYY-MM&contexts[]=N).
pub async fn calendar_index(
    State(app): State<AppState>,
    Query(query): Query<CalendarQuery>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let contexts = match calendar_ui::load_visible_contexts_for_ui(&app.db, &auth_of(&user)).await {
        Ok(c) => c,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    let today = Utc::now().date_naive();
    let month = query
        .month
        .as_deref()
        .and_then(parse_month)
        .unwrap_or_else(|| {
            NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap_or(today)
        });
    let (start, end) = month_bounds(month);

    let events = match calendar_ui::load_events_for_range_for_ui(
        &app.db,
        &auth_of(&user),
        start,
        end,
        &query.contexts,
    )
    .await
    {
        Ok(events) => events,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    let weeks = build_grid(month, events, today);

    let (preferences, use_high_contrast) = match queries::load_preferences(&app.db, user.id).await {
        Ok(loaded) => loaded,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    let user_name = match queries::load_user_name(&app.db, user.id).await {
        Ok(name) => name.unwrap_or_default(),
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    let unread_badge = match star_conversations::ui::load_unread_count(&app.db, user.id).await {
        Ok(count) => count,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    let env = JsEnv::for_user(
        user.id,
        &user.role,
        preferences,
        use_high_contrast,
        &origin,
        Some("calendar"),
        "",
        Vec::new(),
    );

    let selected = &query.contexts;
    let context_boxes: Vec<ContextCheckbox> = contexts
        .into_iter()
        .map(|c| ContextCheckbox {
            course_id: c.course_id,
            course_name: c.course_name,
            checked: selected.is_empty() || selected.contains(&c.course_id),
        })
        .collect();

    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let page = CalendarPage {
        title: "Calendar".to_string(),
        body_class: "calendar".to_string(),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: Vec::new(),
        user_name,
        active_path: "calendar",
        breadcrumb: vec![(
            "Calendar".to_string(),
            format!("/calendar?month={}", month_param(month)),
        )],
        csrf_token,
        unread_badge,
        month_label: month_label(month),
        weeks,
        contexts: context_boxes,
        prev_month: month_param(month.pred_opt().unwrap_or(month)),
        next_month: month_param(month.succ_opt().unwrap_or(month)),
        this_month: month_param(today),
    };
    (jar, page).into_response()
}
