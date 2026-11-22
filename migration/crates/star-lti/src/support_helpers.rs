//! Slice `lti-3` — star-lti support helpers: Turnitin (7 routes).
//!
//! Canvas parity: `app/controllers/support_helpers/turnitin_controller.rb`
//! (routes registered in Canvas under `/api/v1/support_helpers/turnitin/*`).
//!
//! Routes (mounted under `/api/v1` by star-http):
//!
//! - `GET /support_helpers/turnitin/md5`
//! - `GET /support_helpers/turnitin/error2305`
//! - `GET /support_helpers/turnitin/shard`
//! - `GET /support_helpers/turnitin/assignment`
//! - `GET /support_helpers/turnitin/pending`
//! - `GET /support_helpers/turnitin/expired`
//! - `GET /support_helpers/turnitin/refresh_lti_attachment`
//!
//! Slice composition note: the other half of this slice's nominal scope —
//! the 11 `lti/ims/dynamic_registration` routes — was already implemented
//! and merged by slice `lti-4` (PR #163, `dynamic_registration.rs`); those
//! routes are NOT re-implemented here.
//!
//! Behavior contract (see PR body for the full assumptions list; the
//! implementation brief for this slice was absent):
//!
//! - These are **support-tool endpoints**, not product surface: they exist so
//!   an operator with support tooling can inspect a Turnitin install. Canvas
//!   requires site-admin-equivalent rights; the workspace mapping (matching
//!   the spec's proposal for `registration_token`) is the global `admin`
//!   role. Non-admins get `403`, anonymous callers `401`.
//! - The workspace has **no Turnitin schema** (no turnitin columns on
//!   `assignments`/`submissions`, no settings table) and phase-5 lanes do not
//!   add product schema, so the data-dependent endpoints follow the crate's
//!   established stub pattern (`external_tools.rs` routes 7-8 and 17:
//!   `sessionless_launch` / `eula_launch`): when the underlying integration
//!   is not configured in the workspace, respond `503 Service Unavailable`
//!   with a JSON body explaining the gap.
//! - `md5` is the one endpoint that is fully implementable without Turnitin
//!   infrastructure: it echoes an MD5 digest of the `object_id`/message
//!   parameters — the classical Turnitin "shared secret check" helper. It is
//!   implemented for real (crate-local MD5) and is the negative-auth test
//!   target.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the support-helpers (Turnitin) router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/support_helpers/turnitin/md5", get(turnitin_md5))
        .route(
            "/support_helpers/turnitin/error2305",
            get(turnitin_error2305),
        )
        .route("/support_helpers/turnitin/shard", get(turnitin_shard))
        .route(
            "/support_helpers/turnitin/assignment",
            get(turnitin_assignment),
        )
        .route("/support_helpers/turnitin/pending", get(turnitin_pending))
        .route("/support_helpers/turnitin/expired", get(turnitin_expired))
        .route(
            "/support_helpers/turnitin/refresh_lti_attachment",
            get(turnitin_refresh_lti_attachment),
        )
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Query parameters accepted by the support-helper endpoints. Canvas' support
/// helpers take free-form lookup params (`object_id`, `course_id`,
/// `assignment_id`, `user_id`); every field is optional and unknown params
/// are ignored, matching Canvas' permissive params handling.
#[derive(Debug, Default, Deserialize)]
pub struct TurnitinQuery {
    pub object_id: Option<String>,
}

/// Gate every support-helper route: anonymous callers get `401` (from the
/// [`AuthUser`] extractor), authenticated non-admins get `403` — the
/// workspace mapping of Canvas' site-admin requirement for support tooling.
fn require_support_admin(auth: &AuthUser) -> Result<(), ApiError> {
    if auth.role == "admin" {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

/// The standard `503` stub body shared by the data-dependent endpoints.
///
/// Mirrors the crate's existing stub convention (external_tools.rs
/// `sessionless_launch` / `eula_launch`): a JSON payload that names the
/// missing integration so operators see *why* the endpoint is unavailable
/// rather than an empty error page. Phase-5 lanes cannot add Turnitin
/// product schema, so until the schema lane lands there is no row source
/// these queries could read.
fn turnitin_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": "turnitin_integration_not_configured",
            "message": "Turnitin support-tool data is unavailable: the workspace has no Turnitin schema or configured integration (stub matching external_tools sessionless_launch/eula_launch).",
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /support_helpers/turnitin/md5` — compute the MD5 digest used by the
/// classical Turnitin integration to verify shared-secret material.
///
/// Fully implemented: no Turnitin infrastructure is needed, only a digest of
/// the request parameters. With `object_id` present the digest covers
/// `object_id` (the support-tool "check this id" use); with no parameters the
/// digest covers the empty string, matching the helper's echo semantics.
async fn turnitin_md5(
    State(_state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<TurnitinQuery>,
) -> Result<Json<Value>, ApiError> {
    require_support_admin(&auth)?;

    let message = params.object_id.unwrap_or_default();
    let digest = md5_hex(message.as_bytes());

    Ok(Json(json!({
        "object_id": message,
        "md5": digest,
    })))
}

/// `GET /support_helpers/turnitin/error2305` — inspect the recorded Turnitin
/// error-2305 (token mismatch) condition. Stub: 503 until the integration
/// schema lands.
async fn turnitin_error2305(
    State(_state): State<AppState>,
    auth: AuthUser,
    Query(_params): Query<TurnitinQuery>,
) -> Result<Response, ApiError> {
    require_support_admin(&auth)?;
    Ok(turnitin_unavailable())
}

/// `GET /support_helpers/turnitin/shard` — report which Turnitin shard the
/// install is homed on. Stub: 503.
async fn turnitin_shard(
    State(_state): State<AppState>,
    auth: AuthUser,
    Query(_params): Query<TurnitinQuery>,
) -> Result<Response, ApiError> {
    require_support_admin(&auth)?;
    Ok(turnitin_unavailable())
}

/// `GET /support_helpers/turnitin/assignment` — dump the Turnitin assignment
/// mapping for a course/assignment pair. Stub: 503.
async fn turnitin_assignment(
    State(_state): State<AppState>,
    auth: AuthUser,
    Query(_params): Query<TurnitinQuery>,
) -> Result<Response, ApiError> {
    require_support_admin(&auth)?;
    Ok(turnitin_unavailable())
}

/// `GET /support_helpers/turnitin/pending` — list submissions still pending
/// Turnitin processing. Stub: 503.
async fn turnitin_pending(
    State(_state): State<AppState>,
    auth: AuthUser,
    Query(_params): Query<TurnitinQuery>,
) -> Result<Response, ApiError> {
    require_support_admin(&auth)?;
    Ok(turnitin_unavailable())
}

/// `GET /support_helpers/turnitin/expired` — list submissions whose Turnitin
/// data has expired. Stub: 503.
async fn turnitin_expired(
    State(_state): State<AppState>,
    auth: AuthUser,
    Query(_params): Query<TurnitinQuery>,
) -> Result<Response, ApiError> {
    require_support_admin(&auth)?;
    Ok(turnitin_unavailable())
}

/// `GET /support_helpers/turnitin/refresh_lti_attachment` — re-upload an LTI
/// attachment to Turnitin. Stub: 503 (no outbound HTTP in phase-5 handlers).
async fn turnitin_refresh_lti_attachment(
    State(_state): State<AppState>,
    auth: AuthUser,
    Query(_params): Query<TurnitinQuery>,
) -> Result<Response, ApiError> {
    require_support_admin(&auth)?;
    Ok(turnitin_unavailable())
}

// ---------------------------------------------------------------------------
// MD5 (crate-local, no external dependency)
// ---------------------------------------------------------------------------

/// Hex-encode bytes (lowercase, like Rust's `hex` crate formatting used
/// elsewhere in the workspace).
fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from_digit((b >> 4) as u32, 16).expect("high nibble"));
        out.push(char::from_digit((b & 0x0f) as u32, 16).expect("low nibble"));
    }
    out
}

/// MD5 of `data`, returned as a lowercase 32-char hex string.
///
/// Crate-local RFC 1321 implementation: adding a digest crate would require
/// syncing `Cargo.lock` for every parallel lane on this crate, and `md-5` is
/// only a transitive dependency of sqlx (not in `[workspace.dependencies]`).
/// RFC 1321 is short and fully testable, and CI exercises it via the md5
/// integration tests.
fn md5_hex(data: &[u8]) -> String {
    to_hex(&md5_raw(data))
}

fn md5_raw(data: &[u8]) -> [u8; 16] {
    // --- RFC 1321 constants -------------------------------------------------
    const S: [[u32; 4]; 4] = [
        [7, 12, 17, 22],
        [5, 9, 14, 20],
        [4, 11, 16, 23],
        [6, 10, 15, 21],
    ];
    // floor(abs(sin(i + 1)) * 2^32) for i in 0..64
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;

    // --- Padding ------------------------------------------------------------
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    // --- Process each 512-bit block ------------------------------------------
    for chunk in msg.as_chunks::<64>().0 {
        let mut m = [0u32; 16];
        for (i, word) in m.iter_mut().enumerate() {
            *word = u32::from_le_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }

        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);

        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f2 = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f2.rotate_left(S[i / 16][i % 4]));
        }

        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

// ---------------------------------------------------------------------------
// Tests (unit — no DB required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_rfc1321_vectors() {
        // The official RFC 1321 test suite.
        let cases: &[(&str, &str)] = &[
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                "abcdefghijklmnopqrstuvwxyz",
                "c3fcd3d76192e4007dfb496cca67e13b",
            ),
            (
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(&md5_hex(input.as_bytes()), expected, "input: {input:?}");
        }
    }

    #[test]
    fn md5_long_input_multi_block() {
        // > 2 blocks of padding exercise the length-extension path.
        let input = "x".repeat(1000);
        assert_eq!(
            md5_hex(input.as_bytes()),
            "398533d48111e9f664b1f64cb10c4b63"
        );
    }

    #[test]
    fn hex_encoding_matches_lowercase_convention() {
        assert_eq!(to_hex(&[0x0f, 0xa0, 0x07]), "0fa007");
    }
}
