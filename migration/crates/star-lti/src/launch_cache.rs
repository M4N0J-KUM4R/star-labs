//! Cached LTI 1.3 launches and once-only nonces.
//!
//! Canvas caches the launch message at login-initiation time and consumes it
//! exactly once during the authentication request step
//! (`app/controllers/lti/ims/authentication_controller.rb`,
//! `cached_launch_with_nonce` → `Lti::RedisMessageClient#fetch_and_delete_launch`),
//! and stores deep-linking data nonces once-only
//! (`lib/lti/deep_linking_data.rb`, `Lti::Security.check_and_store_nonce`).
//! Canvas backs both with Redis; the port keeps them in process memory with
//! the same single-use semantics and a one-hour TTL (Canvas launch validity:
//! `Lti::KeyStorage`/`deep_linking_data` use a 1 hour window).
//!
//! `seed_launch` is the port of the message factory that Canvas runs when a
//! user starts a launch (see `app/models/lti/lti_advantage_adapter.rb`):
//! it stores the tool identity and message claims under the `verifier` that
//! the `lti_message_hint` JWT carries. Tests (and future launch-initiation
//! endpoints) call it directly, mirroring how Canvas writes to Redis outside
//! of the authentication HTTP surface.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a cached launch stays valid (Canvas: 1 hour).
const LAUNCH_TTL: Duration = Duration::from_secs(60 * 60);

/// The cached launch payload: the port of Canvas'
/// `{ post_payload:, assoc_tool_data: }` cache entry. The tool identity is
/// stored separately so `authorize` can resolve the developer key (client
/// id) and the LTI deployment id at launch-consumption time.
#[derive(Debug, Clone)]
pub struct CachedLaunch {
    /// `external_tools.id` of the tool being launched.
    pub tool_id: i64,
    /// `LtiResourceLinkRequest` or `LtiDeepLinkingRequest`.
    pub message_type: String,
    /// Where the tool expects to receive the launch.
    pub target_link_uri: String,
    /// Extra message claims merged into the id token
    /// (`https://purl.imsglobal.org/spec/lti/claim/custom`,
    /// `.../resource_link`, `.../deep_linking`, `.../context`, ...).
    pub extra_claims: Value,
}

struct CacheInner {
    launches: HashMap<(String, i64, String), (CachedLaunch, Instant)>,
    nonces: HashMap<String, Instant>,
}

fn cache() -> &'static Mutex<CacheInner> {
    static CACHE: OnceLock<Mutex<CacheInner>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(CacheInner {
            launches: HashMap::new(),
            nonces: HashMap::new(),
        })
    })
}

fn sweep_expired(inner: &mut CacheInner) {
    let now = Instant::now();
    inner
        .launches
        .retain(|_, (_, at)| now.duration_since(*at) < LAUNCH_TTL);
    inner
        .nonces
        .retain(|_, at| now.duration_since(*at) < LAUNCH_TTL);
}

/// Store a launch under `(context_type, context_id, verifier)` — the port of
/// Canvas' Redis key (`context.global_asset_string` + verifier).
pub fn seed_launch(context_type: &str, context_id: i64, verifier: &str, launch: CachedLaunch) {
    let mut inner = cache().lock().expect("launch cache poisoned");
    sweep_expired(&mut inner);
    inner.launches.insert(
        (context_type.to_string(), context_id, verifier.to_string()),
        (launch, Instant::now()),
    );
}

/// Fetch and delete a cached launch — single use, exactly like Canvas'
/// `fetch_and_delete_launch`.
pub fn fetch_and_delete_launch(
    context_type: &str,
    context_id: i64,
    verifier: &str,
) -> Option<CachedLaunch> {
    let mut inner = cache().lock().expect("launch cache poisoned");
    sweep_expired(&mut inner);
    let key = (context_type.to_string(), context_id, verifier.to_string());
    match inner.launches.remove(&key) {
        Some((launch, at)) if at.elapsed() < LAUNCH_TTL => Some(launch),
        _ => None,
    }
}

/// Check and store a nonce once-only — the port of Canvas'
/// `Lti::Security.check_and_store_nonce` (Redis SETNX, 1 hour TTL).
/// Returns `true` when the nonce was fresh, `false` when it was already used.
pub fn check_and_store_nonce(key: &str) -> bool {
    let mut inner = cache().lock().expect("launch cache poisoned");
    sweep_expired(&mut inner);
    if inner.nonces.contains_key(key) {
        return false;
    }
    inner.nonces.insert(key.to_string(), Instant::now());
    true
}
