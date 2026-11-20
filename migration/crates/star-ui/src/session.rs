//! Session authentication for the UI shell + CSRF double-submit tokens.
//!
//! `SessionUser` reads the `star_session` cookie and runs the *exact* same
//! verification path as star-auth's `AuthUser` (spec §5): HS256 decode
//! against `AppState.jwt_secret`, role reload from the DB by `sub` (the
//! token's role claim is never trusted), and rejection of tokens minted
//! before `users.last_logged_out`. The only difference is the transport
//! (cookie instead of `Authorization: Bearer`) and the rejection (303 to the
//! login page with a `next` round-trip instead of a 401), so `/api/v1`
//! clients are untouched.
//!
//! CSRF: cookie auth re-opens the CSRF hole that Bearer auth closes. Every
//! shell POST form carries a hidden `_csrf` field plus a non-HttpOnly
//! `star_csrf` cookie (double-submit); handlers compare them in constant
//! time and answer 403 on mismatch. `/api/v1` stays Bearer-only and is
//! exempt by construction.

use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use axum::response::Redirect;
use axum_extra::extract::cookie::{Cookie, SameSite};
use axum_extra::extract::CookieJar;
use chrono::{DateTime, Utc};
use jsonwebtoken::{decode, DecodingKey, Validation};
use rand::RngCore;

use star_common::{AppState, Claims};

/// Session cookie name (`star_session`, spec §5).
pub const SESSION_COOKIE: &str = "star_session";

/// CSRF double-submit cookie name.
pub const CSRF_COOKIE: &str = "star_csrf";

/// Hidden form field carrying the CSRF token.
pub const CSRF_FIELD: &str = "_csrf";

/// Token lifetime in star-auth (`create_token`); used to derive issued-at
/// from `exp` for the `last_logged_out` invalidation check.
const TOKEN_EXPIRY_SECS: i64 = 7 * 24 * 60 * 60;

/// A user authenticated through the `star_session` cookie.
#[derive(Debug, Clone)]
pub struct SessionUser {
    pub id: i64,
    pub role: String,
}

impl<S> FromRequestParts<S> for SessionUser
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    /// Rejection is a 303 redirect to `/login?next=<original path>` — the
    /// shell's equivalent of Canvas' `store_location` + `redirect_to_login`
    /// for HTML requests.
    type Rejection = Redirect;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app = AppState::from_ref(state);

        let jar = CookieJar::from_headers(&parts.headers);
        let Some(token) = jar.get(SESSION_COOKIE).map(|c| c.value().to_string()) else {
            return Err(login_redirect(parts));
        };

        // Same verification path as star-auth's AuthUser: decode against the
        // shared secret, then reload the role from the DB (never trust the
        // claim), then reject pre-logout tokens.
        let claims = decode::<Claims>(
            &token,
            &DecodingKey::from_secret(app.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|_| Redirect::to("/login"))?
        .claims;

        let row: Option<(String, Option<DateTime<Utc>>)> =
            sqlx::query_as("SELECT role, last_logged_out FROM users WHERE id = $1")
                .bind(claims.sub)
                .fetch_optional(&app.db)
                .await
                .map_err(|_| Redirect::to("/login"))?;

        let Some((role, last_logged_out)) = row else {
            return Err(Redirect::to("/login"));
        };

        // Tokens minted before the last logout are invalid. issued-at is
        // derived from exp (create_token fixes the lifetime), exactly like
        // AuthUser does.
        let issued_at = claims.exp as i64 - TOKEN_EXPIRY_SECS;
        if last_logged_out.is_some_and(|last| issued_at < last.timestamp()) {
            return Err(Redirect::to("/login"));
        }

        Ok(SessionUser {
            id: claims.sub,
            role,
        })
    }
}

/// Build the login redirect preserving the requested path (`next` round-trip,
/// acceptance criterion 9). Only same-origin paths are preserved — the value
/// is echoed back into a Location header.
fn login_redirect(parts: &Parts) -> Redirect {
    let next = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    Redirect::to(&format!("/login?next={next}"))
}

/// Sanitize a user-supplied `next` target: keep only same-origin absolute
/// paths (must start with `/` and not `//`), defaulting to `/dashboard`.
/// Mirrors Canvas' `store_location` cleaning of `return_to`.
pub fn sanitize_next(next: Option<&str>) -> String {
    match next {
        Some(path) if path.starts_with('/') && !path.starts_with("//") => path.to_string(),
        _ => "/dashboard".to_string(),
    }
}

/// Generate a fresh 128-bit CSRF token (random hex, rand 0.8 OsRng).
pub fn generate_csrf_token() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time equality for the double-submit comparison.
///
/// Hand-rolled xor-fold (no `subtle` dependency): the accumulator folds the
/// length difference plus every byte pair, so the comparison time does not
/// depend on where the first difference appears.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let diff = (a.len() ^ b.len()) as u8;
    a.iter().zip(b).fold(diff, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Extract and validate the CSRF double-submit pair from a request.
///
/// Returns `true` when the non-HttpOnly `star_csrf` cookie and the hidden
/// `_csrf` form field match (constant-time); `false` means the handler must
/// answer 403.
pub fn check_csrf(jar: &CookieJar, form_token: &str) -> bool {
    let cookie_token = jar.get(CSRF_COOKIE).map(|c| c.value()).unwrap_or("");
    !cookie_token.is_empty() && !form_token.is_empty() && constant_time_eq(cookie_token, form_token)
}

/// Issue the CSRF cookie when absent, returning the (possibly updated) jar
/// and the token the form must embed. The cookie is deliberately NOT
/// HttpOnly so a future JS layer can echo it in headers (Canvas parity).
pub fn ensure_csrf_cookie(jar: CookieJar) -> (CookieJar, String) {
    if let Some(value) = jar.get(CSRF_COOKIE).map(|c| c.value().to_string()) {
        return (jar, value);
    }
    let token = generate_csrf_token();
    let jar = jar.add(
        Cookie::build((CSRF_COOKIE, token.clone()))
            .path("/")
            .same_site(SameSite::Lax),
    );
    (jar, token)
}

/// Build the `star_session` cookie for a freshly minted JWT.
///
/// `Secure` is omitted: CI and local development serve plain HTTP (spec §5
/// allows this; the flag is a deployment concern once TLS termination
/// exists). HttpOnly keeps the token out of `document.cookie`; SameSite=Lax
/// blocks cross-site POST replay.
pub fn session_cookie(jwt: &str) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, jwt.to_string()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .build()
}

/// Build the expired `star_session` cookie that clears the session client-side.
pub fn clear_session_cookie() -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, ""))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .expires(time::OffsetDateTime::UNIX_EPOCH)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_basic() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
        assert!(!constant_time_eq("", "a"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn csrf_tokens_are_128_bit_hex() {
        let t1 = generate_csrf_token();
        let t2 = generate_csrf_token();
        assert_eq!(t1.len(), 32);
        assert!(t1.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(t1, t2, "CSRF tokens must not repeat");
    }

    #[test]
    fn sanitize_next_keeps_paths_only() {
        assert_eq!(sanitize_next(Some("/courses/7")), "/courses/7");
        assert_eq!(sanitize_next(Some("/dashboard?x=1")), "/dashboard?x=1");
        assert_eq!(sanitize_next(Some("https://evil.example")), "/dashboard");
        assert_eq!(sanitize_next(Some("//evil.example")), "/dashboard");
        assert_eq!(sanitize_next(Some("")), "/dashboard");
        assert_eq!(sanitize_next(None), "/dashboard");
    }

    #[test]
    fn check_csrf_requires_both_sides() {
        let jar = CookieJar::new().add(Cookie::new(CSRF_COOKIE, "token"));
        assert!(check_csrf(&jar, "token"));
        assert!(!check_csrf(&jar, "wrong"));
        assert!(!check_csrf(&CookieJar::new(), "token"));
        assert!(!check_csrf(&jar, ""));
    }

    #[test]
    fn ensure_csrf_cookie_reuses_existing() {
        let jar = CookieJar::new().add(Cookie::new(CSRF_COOKIE, "existing"));
        let (jar, token) = ensure_csrf_cookie(jar);
        assert_eq!(token, "existing");
        assert_eq!(jar.get(CSRF_COOKIE).unwrap().value(), "existing");

        let (jar, token) = ensure_csrf_cookie(CookieJar::new());
        assert_eq!(token.len(), 32);
        assert_eq!(jar.get(CSRF_COOKIE).unwrap().value(), token);
    }

    #[test]
    fn session_cookie_attributes() {
        let cookie = session_cookie("jwt-value");
        assert_eq!(cookie.name(), SESSION_COOKIE);
        assert_eq!(cookie.value(), "jwt-value");
        assert_eq!(cookie.http_only(), Some(true));
        assert_eq!(cookie.same_site(), Some(SameSite::Lax));
        assert_eq!(cookie.path(), Some("/"));
        assert_eq!(cookie.secure(), None, "Secure omitted for plain-HTTP CI");
    }

    #[test]
    fn clear_cookie_is_expired() {
        let cookie = clear_session_cookie();
        assert_eq!(cookie.value(), "");
        // An explicit Expires in the past is what makes browsers drop it.
        let expires = cookie.expires_datetime().expect("has expiry");
        assert!(expires <= time::OffsetDateTime::now_utc());
        let header = cookie.encoded().to_string();
        assert!(header.contains("star_session=;"), "header: {header}");
    }
}
