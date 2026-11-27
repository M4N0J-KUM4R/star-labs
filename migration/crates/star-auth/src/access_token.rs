//! Canvas' access-token model.
//!
//! Canvas does not authenticate its API with a session token. It mints an
//! opaque token, stores it hashed in `access_tokens.crypted_token`, and
//! resolves the caller from that row:
//!
//! * `app/models/access_token.rb:312-315` — the `token=` setter writes the
//!   hash and the hint, and keeps the clear value only in memory.
//! * `app/models/access_token.rb:151-168` — `AccessToken.authenticate` looks
//!   the row up by the hash.
//! * `app/models/access_token.rb:176-180` — `AccessToken.hashed_token` is an
//!   HMAC-SHA1 of the token, keyed with the app secret.
//! * `app/models/access_token.rb:238-247` — `usable?` decides whether the row
//!   may act: not expired, not pending, developer key usable, and a user on
//!   the row.
//! * `lib/authentication_methods.rb:144-177` — the caller is the user of the
//!   row, and the row's ids go into the `x-canvas-meta` header.
//!
//! The port resolves the caller the same way, so it knows the access token's
//! global id (`at`) and the developer key's (`dk`) — the two fields the
//! recorded corpus carries.

use chrono::Utc;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sqlx::PgPool;

use star_common::ApiError;

/// Canvas' `AccessToken::HINT_LENGTH`.
pub const HINT_LENGTH: usize = 5;

/// Canvas' `AccessToken.hashed_token`
/// (`app/models/access_token.rb:176-180`): an HMAC-SHA1 of the token, keyed
/// with the app secret.
///
/// Canvas keys it with `Canvas::Security.encryption_key`. The port has one
/// secret — `AppState::jwt_secret` — and keys the hash with it, so a token
/// minted by the port is the token the port accepts.
pub fn hashed_token(token: &str, secret: &str) -> String {
    let mut mac =
        Hmac::<Sha1>::new_from_slice(secret.as_bytes()).expect("HMAC accepts a key of any length");
    mac.update(token.as_bytes());
    let digest = mac.finalize().into_bytes();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// The first five characters of a token — Canvas' `token_hint`
/// (`app/models/access_token.rb:314`).
pub fn token_hint(token: &str) -> String {
    token.chars().take(HINT_LENGTH).collect()
}

/// Whether a request is an API request.
///
/// Canvas loads an access token only for API requests
/// (`lib/authentication_methods.rb:144-147`), which are the paths matching
/// `API_REQUEST_REGEX` (`app/controllers/application_controller.rb:2357-2359`).
pub fn is_api_request(path: &str) -> bool {
    path.starts_with("/api/")
}

/// The Canvas actions that are answered without a caller.
///
/// `UsersController` skips `load_user` for `create_self_registered_user`
/// (`app/controllers/users_controller.rb:106`): the request is anonymous, so
/// Canvas records no access token — and no user — for it.
pub fn skips_authentication(path: &str) -> bool {
    path.ends_with("/self_registration")
}

/// The caller an access token resolves to.
#[derive(Debug, Clone)]
pub struct AccessTokenAuth {
    /// The `access_tokens` row id — the `at` of the `x-canvas-meta` header.
    pub id: i64,
    /// The user the token acts for.
    pub user_id: i64,
    /// The developer key the token was issued for — the `dk` of the header.
    pub developer_key_id: Option<i64>,
    /// The user's current role, read from the `users` table.
    pub role: String,
}

/// The row of `access_tokens` the caller is resolved from.
#[derive(Debug, sqlx::FromRow)]
struct AccessTokenRow {
    id: i64,
    user_id: i64,
    developer_key_id: Option<i64>,
    workflow_state: String,
    permanent_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    role: String,
    key_workflow_state: Option<String>,
    auto_expire_tokens: Option<bool>,
}

/// Resolve a bearer token to the `access_tokens` row that carries it.
///
/// The token is hashed and looked up the way Canvas looks it up
/// (`app/models/access_token.rb:151-168`), and the row has to be usable
/// (`app/models/access_token.rb:238-247`):
///
/// * not revoked and not pending — `workflow_state` must be `active`
///   (`access_token.rb:239`, `access_token.rb:300-302`);
/// * not expired — `permanent_expires_at` must be in the future
///   (`access_token.rb:300-302`);
/// * its developer key must be usable, which for Canvas is `active?`
///   (`app/models/developer_key.rb:157-162`);
/// * and it must carry a user (`access_token.rb:242`).
///
/// A row that fails any of those is not a caller: Canvas answers 401
/// (`lib/authentication_methods.rb:153-157`).
pub async fn authenticate(
    pool: &PgPool,
    secret: &str,
    token: &str,
) -> Result<Option<AccessTokenAuth>, ApiError> {
    let hash = hashed_token(token, secret);

    // Canvas orders the candidates by `workflow_state = 'active' DESC,
    // workflow_state` (app/models/access_token.rb:158-159).
    let row: Option<AccessTokenRow> = sqlx::query_as(
        "SELECT t.id, t.user_id, t.developer_key_id, t.workflow_state, \
                t.permanent_expires_at, t.expires_at, u.role, \
                k.workflow_state AS key_workflow_state, k.auto_expire_tokens \
         FROM access_tokens t \
         JOIN users u ON u.id = t.user_id \
         LEFT JOIN developer_keys k ON k.id = t.developer_key_id \
         WHERE t.crypted_token = $1 \
         ORDER BY (t.workflow_state = 'active') DESC, t.workflow_state \
         LIMIT 1",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?;

    let Some(row) = row else {
        return Ok(None);
    };

    if row.workflow_state != "active" {
        return Ok(None);
    }

    if row.permanent_expires_at.is_some_and(|at| at <= Utc::now()) {
        return Ok(None);
    }

    if row.developer_key_id.is_some() {
        // `DeveloperKey#usable?` is `active?`.
        if row.key_workflow_state.as_deref() != Some("active") {
            return Ok(None);
        }
        // `AccessToken#needs_refresh?` (app/models/access_token.rb:304-310).
        if row.auto_expire_tokens.unwrap_or(false)
            && row.expires_at.is_some_and(|at| at <= Utc::now())
        {
            return Ok(None);
        }
    }

    Ok(Some(AccessTokenAuth {
        id: row.id,
        user_id: row.user_id,
        developer_key_id: row.developer_key_id,
        role: row.role,
    }))
}
