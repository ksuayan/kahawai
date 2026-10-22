//! Bearer-token authentication (P0 Phase 1, spec §1.3) and session
//! authentication (P0 Phase 3, spec §3.2).
//!
//! Two credential systems, exactly one live at a time (spec §3,
//! "Composition with Phase 1"): while the `users` table is empty, the
//! pairing token in the OS keychain is the credential; once the first
//! admin exists the pairing token is retired and per-user session tokens
//! take over. The middleware enforces whichever is live.
//!
//! [`redact_access_token`] exists because the SSE/stream/artwork/export
//! endpoints accept the token as a `?access_token=` query parameter
//! (EventSource, `<audio>` and `<img>` tags, and VLC cannot set request
//! headers). URLs carry the token, so every access-log path must scrub
//! it — the tradeoff is documented on [`QUERY_AUTH_PATHS`].

#[cfg(test)]
use std::sync::Mutex;
use std::sync::{Arc, RwLock};

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use base64::Engine as _;
use serde_json::json;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Keychain service / account for the Phase 1 pairing token (spec §1.3).
pub const KEYCHAIN_SERVICE: &str = "com.suayan.kahawai-server";
pub const KEYCHAIN_ACCOUNT: &str = "api-token";

/// Query parameter carrying the token on header-incapable endpoints.
pub const ACCESS_TOKEN_PARAM: &str = "access_token";

/// Generate a fresh 256-bit token, base64url-encoded without padding
/// (43 characters). Panics only if the OS CSPRNG itself fails, which is
/// a "do not run without entropy" situation.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS CSPRNG failure");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// SHA-256 hex of a token. Session tokens are stored in this form (spec
/// §3.2: a database read must not yield live sessions); the pairing
/// token is compared in this form so its length never leaks through a
/// naive length check.
pub fn sha256_hex(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    format!("{digest:x}")
}

/// Constant-time comparison of a presented token against an expected
/// SHA-256 hex digest. Both sides are hashed first so the comparison
/// itself never branches on secret bytes.
pub fn token_matches(presented: &str, expected_hex: &str) -> bool {
    let presented_hex = sha256_hex(presented);
    bool::from(presented_hex.as_bytes().ct_eq(expected_hex.as_bytes()))
}

// ---------------------------------------------------------------------------
// Token storage (spec §1.3: keychain, never TOML/DB/logs)
// ---------------------------------------------------------------------------

/// What can go wrong talking to a token store. Never carries the token.
#[derive(Debug, thiserror::Error)]
pub enum TokenStoreError {
    #[error("keychain error: {0}")]
    Keychain(String),
    #[error("token file error: {0}")]
    File(String),
}

/// Where the pairing token lives. The trait is the seam the TODO
/// requires: production uses the OS keychain, tests use memory, and no
/// test in this repo ever touches a real keychain.
pub trait TokenStore: Send + Sync + std::fmt::Debug {
    fn get(&self) -> Result<Option<String>, TokenStoreError>;
    fn set(&self, token: &str) -> Result<(), TokenStoreError>;
    fn delete(&self) -> Result<(), TokenStoreError>;
}

/// OS keychain via the `keyring` crate (macOS Keychain, Windows
/// Credential Manager, Linux Secret Service where a D-Bus session
/// exists).
#[derive(Debug)]
pub struct KeychainTokenStore;

impl TokenStore for KeychainTokenStore {
    fn get(&self) -> Result<Option<String>, TokenStoreError> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
            .map_err(|e| TokenStoreError::Keychain(e.to_string()))?;
        match entry.get_password() {
            Ok(pw) => Ok(Some(pw)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(TokenStoreError::Keychain(e.to_string())),
        }
    }

    fn set(&self, token: &str) -> Result<(), TokenStoreError> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
            .map_err(|e| TokenStoreError::Keychain(e.to_string()))?;
        entry
            .set_password(token)
            .map_err(|e| TokenStoreError::Keychain(e.to_string()))
    }

    fn delete(&self) -> Result<(), TokenStoreError> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
            .map_err(|e| TokenStoreError::Keychain(e.to_string()))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(TokenStoreError::Keychain(e.to_string())),
        }
    }
}

/// Fallback for machines with no OS credential store (Linux dev/CI
/// boxes without a D-Bus secret service): a file with `0600`
/// permissions next to the config. This is not the TOML, the database,
/// or a log — it is a dedicated secret file, and its use is logged as
/// a warning so nobody mistakes it for the keychain.
#[derive(Debug)]
pub struct FileTokenStore {
    path: std::path::PathBuf,
}

impl FileTokenStore {
    pub fn new(path: std::path::PathBuf) -> Self {
        Self { path }
    }
}

impl TokenStore for FileTokenStore {
    fn get(&self) -> Result<Option<String>, TokenStoreError> {
        match std::fs::read_to_string(&self.path) {
            Ok(s) => Ok(Some(s.trim().to_string()).filter(|t| !t.is_empty())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(TokenStoreError::File(e.to_string())),
        }
    }

    fn set(&self, token: &str) -> Result<(), TokenStoreError> {
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| TokenStoreError::File(e.to_string()))?;
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true).mode(0o600);
        use std::io::Write as _;
        let mut f = opts
            .open(&self.path)
            .map_err(|e| TokenStoreError::File(e.to_string()))?;
        f.write_all(token.as_bytes())
            .map_err(|e| TokenStoreError::File(e.to_string()))?;
        Ok(())
    }

    fn delete(&self) -> Result<(), TokenStoreError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(TokenStoreError::File(e.to_string())),
        }
    }
}

/// In-memory store for tests. No real keychain is ever touched by tests.
#[cfg(test)]
#[derive(Debug)]
pub struct MemoryTokenStore {
    inner: Mutex<Option<String>>,
}

#[cfg(test)]
impl MemoryTokenStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }
}

#[cfg(test)]
impl Default for MemoryTokenStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl TokenStore for MemoryTokenStore {
    fn get(&self) -> Result<Option<String>, TokenStoreError> {
        Ok(self.inner.lock().unwrap().clone())
    }

    fn set(&self, token: &str) -> Result<(), TokenStoreError> {
        *self.inner.lock().unwrap() = Some(token.to_string());
        Ok(())
    }

    fn delete(&self) -> Result<(), TokenStoreError> {
        *self.inner.lock().unwrap() = None;
        Ok(())
    }
}

/// Probe the OS keychain: a clean read (or a clean "no entry") means it
/// works; anything else means there is no usable backend on this
/// machine and the caller should fall back to [`FileTokenStore`].
fn keychain_usable() -> bool {
    KeychainTokenStore.get().is_ok()
}

/// The production token store: OS keychain where available, the
/// `0600` file fallback elsewhere. `config_dir` is the directory
/// holding the server config (the fallback file is `api-token` in it).
pub fn default_token_store(config_dir: &std::path::Path) -> Box<dyn TokenStore> {
    if keychain_usable() {
        Box::new(KeychainTokenStore)
    } else {
        tracing::warn!("no OS keychain backend available; the API token falls back to a 0600 file");
        Box::new(FileTokenStore::new(config_dir.join("api-token")))
    }
}

/// Load the pairing token, generating and storing one on first run.
/// Returns the token and whether it was just generated (the caller
/// displays a fresh token exactly once). The token is never logged
/// here — the caller decides the single display.
pub fn ensure_pairing_token(store: &dyn TokenStore) -> Result<(String, bool), TokenStoreError> {
    if let Some(token) = store.get()? {
        return Ok((token, false));
    }
    let token = generate_token();
    store.set(&token)?;
    Ok((token, true))
}

// ---------------------------------------------------------------------------
// Log hygiene (spec §1.3, TODO SEC-3): the token must never appear in logs
// ---------------------------------------------------------------------------

/// Scrub `?access_token=<secret>` / `&access_token=<secret>` from a URI
/// string for access logging. Used by the HTTP span maker in `main.rs`
/// and anywhere else a request URI is logged.
pub fn redact_access_token(uri: &str) -> String {
    let mut out = uri.to_string();
    for sep in ["?access_token=", "&access_token="] {
        let mut start = 0;
        while let Some(idx) = out[start..].find(sep) {
            let abs = start + idx;
            let val_start = abs + sep.len();
            let val_end = out[val_start..]
                .find('&')
                .map(|i| val_start + i)
                .unwrap_or(out.len());
            out.replace_range(val_start..val_end, "[REDACTED]");
            start = val_start + "[REDACTED]".len();
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

/// Paths that never require a bearer token:
/// - `/api/health`, `/api/identity`: documented in `api.rs` as
///   intentionally unauthenticated and cheap (reachability probes that
///   return no catalog data).
/// - `/api/login`, `/api/setup`, `/api/setup/status`: the credential
///   issuance endpoints — requiring a token here would be circular.
/// - `/api/shutdown`: keeps its existing triple guard (loopback-only,
///   no `Origin` header, `x-kahawai-shutdown: yes` header) instead of
///   the bearer; it is unreachable off-machine by construction.
const PUBLIC_PATHS: &[&str] = &[
    "/api/health",
    "/api/identity",
    "/api/login",
    "/api/shutdown",
    "/api/setup",
    "/api/setup/status",
];

/// Endpoints that accept `?access_token=` because their clients cannot
/// set request headers (spec §1.3 names `/api/events`; the same
/// constraint covers the rest):
/// - `/api/events`: `EventSource` (spec §1.3).
/// - `/stream/…`: `<audio>` tags and range-requesting players.
/// - `/api/artwork/…`: `<img>` tags.
/// - `…/export`: VLC / other players opening M3U links.
///
/// Tradeoff (documented per the TODO): URL-borne secrets can leak via
/// history, logs, or proxies; the token is 256-bit high-entropy and
/// every log path redacts the parameter ([`redact_access_token`]),
/// which bounds the exposure.
const QUERY_AUTH_PREFIXES: &[&str] = &["/stream/", "/api/artwork/", "/api/events"];
const QUERY_AUTH_SUFFIXES: &[&str] = &["/export"];

fn query_auth_allowed(path: &str) -> bool {
    QUERY_AUTH_PREFIXES.iter().any(|p| path.starts_with(p))
        || QUERY_AUTH_SUFFIXES.iter().any(|s| path.ends_with(s))
}

/// Who the request authenticated as, for downstream handlers.
#[derive(Debug, Clone)]
pub struct AuthContext {
    /// `Some` once Phase 3 is active (first admin exists).
    pub user_id: Option<i64>,
    pub is_admin: bool,
    /// True while the Phase 1 pairing token is the live credential.
    pub via_pairing_token: bool,
}

/// Shared auth state for the middleware: the pairing-token hash (None
/// once Phase 3 retires it) plus the DB pool for session lookups.
#[derive(Debug)]
pub struct AuthState {
    pub pool: sqlx::SqlitePool,
    /// SHA-256 hex of the live pairing token; `None` after the first
    /// admin is created (spec §3.1: the single-token path is retired).
    pub pairing_token_hash: RwLock<Option<String>>,
}

impl AuthState {
    pub fn new(pool: sqlx::SqlitePool, pairing_token_hash: Option<String>) -> Self {
        Self {
            pool,
            pairing_token_hash: RwLock::new(pairing_token_hash),
        }
    }

    /// Rotate the pairing token (the "Regenerate token" UI action): the
    /// old token stops working the moment the hash is swapped.
    pub fn rotate_pairing_token(&self, new_token: &str) {
        *self.pairing_token_hash.write().unwrap() = Some(sha256_hex(new_token));
    }

    /// Retire the pairing token (Phase 3 bootstrap complete).
    pub fn retire_pairing_token(&self) {
        *self.pairing_token_hash.write().unwrap() = None;
    }
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "unauthorized" })),
    )
        .into_response()
}

/// Extract a bearer token from request headers (shared by the
/// middleware and the logout handler, which needs the raw token to
/// delete the presenting session).
pub fn bearer_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    let value = headers.get(axum::http::header::AUTHORIZATION)?;
    let value = value.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    if token.is_empty() || token.contains(char::is_whitespace) {
        return None;
    }
    Some(token.to_string())
}

fn bearer_token(req: &Request) -> Option<String> {
    bearer_from_headers(req.headers())
}

fn query_token(req: &Request) -> Option<String> {
    let query = req.uri().query()?;
    for pair in query.split('&') {
        let prefix = format!("{ACCESS_TOKEN_PARAM}=");
        if let Some(value) = pair.strip_prefix(&prefix) {
            // URL-decode best-effort; a token that fails to decode is
            // simply not the token.
            let decoded = percent_decode(value);
            if !decoded.is_empty() {
                return Some(decoded);
            }
        }
    }
    None
}

fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((h << 4 | l) as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Axum middleware: every `/api/*` and `/stream/*` request carries a
/// valid bearer token or it gets a uniform
/// `401 {"error":"unauthorized"}` — missing, malformed, and wrong are
/// indistinguishable (no oracle). Wire with
/// `axum::middleware::from_fn_with_state(Arc<AuthState>, require_auth)`.
pub async fn require_auth(
    State(auth): State<Arc<AuthState>>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();

    if PUBLIC_PATHS.contains(&path.as_str()) {
        return next.run(req).await;
    }
    let protected = path.starts_with("/api/") || path.starts_with("/stream/") || path == "/";
    if !protected {
        return next.run(req).await;
    }
    // `/` (the home page) is the pairing/onboarding surface — the token
    // is shown there once, the setup flow lives there. It stays public.
    if path == "/" {
        return next.run(req).await;
    }

    let presented = bearer_token(&req).or_else(|| {
        if query_auth_allowed(&path) {
            query_token(&req)
        } else {
            None
        }
    });
    let Some(presented) = presented else {
        return unauthorized();
    };

    // Phase 3 active (at least one user): sessions only. The pairing
    // token hash is None by then, so the old path is dead even if a
    // stale hash somehow lingered — sessions are the only check.
    let users_exist = crate::users::users_exist(&auth.pool).await.unwrap_or(false);
    if users_exist {
        match crate::users::validate_session(&auth.pool, &presented).await {
            Ok(Some(session)) => {
                let mut req = req;
                req.extensions_mut().insert(AuthContext {
                    user_id: Some(session.user_id),
                    is_admin: session.is_admin,
                    via_pairing_token: false,
                });
                return next.run(req).await;
            }
            _ => return unauthorized(),
        }
    }

    // Phase 1: the pairing token, constant-time against its hash.
    let expected = auth.pairing_token_hash.read().unwrap().clone();
    match expected {
        Some(expected_hex) if token_matches(&presented, &expected_hex) => {
            let mut req = req;
            req.extensions_mut().insert(AuthContext {
                user_id: None,
                is_admin: true,
                via_pairing_token: true,
            });
            next.run(req).await
        }
        _ => unauthorized(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_43_char_base64url() {
        for _ in 0..10 {
            let t = generate_token();
            assert_eq!(t.len(), 43, "256 bits, no padding");
            assert!(
                t.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "base64url alphabet: {t}"
            );
        }
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b, "CSPRNG output must differ");
    }

    #[test]
    fn token_matches_is_exact() {
        let token = generate_token();
        let hex = sha256_hex(&token);
        assert!(token_matches(&token, &hex));
        assert!(!token_matches("wrong", &hex));
        assert!(!token_matches("", &hex));
        assert!(!token_matches(&format!("{token}x"), &hex));
    }

    #[test]
    fn memory_store_round_trips() {
        let store = MemoryTokenStore::new();
        assert_eq!(store.get().unwrap(), None);
        store.set("secret").unwrap();
        assert_eq!(store.get().unwrap(), Some("secret".to_string()));
        store.delete().unwrap();
        assert_eq!(store.get().unwrap(), None);
    }

    #[test]
    fn ensure_generates_once_then_reuses() {
        let store = MemoryTokenStore::new();
        let (t1, fresh1) = ensure_pairing_token(&store).unwrap();
        assert!(fresh1);
        let (t2, fresh2) = ensure_pairing_token(&store).unwrap();
        assert!(!fresh2);
        assert_eq!(t1, t2);
    }

    #[test]
    fn file_store_round_trips_with_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join("kahawai-auth-test-file-store");
        std::fs::create_dir_all(&dir).unwrap();
        let store = FileTokenStore::new(dir.join("api-token"));
        assert_eq!(store.get().unwrap(), None);
        store.set("file-secret").unwrap();
        assert_eq!(store.get().unwrap(), Some("file-secret".to_string()));
        let mode = std::fs::metadata(dir.join("api-token"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "secret file must not be group/other readable");
        store.delete().unwrap();
        assert_eq!(store.get().unwrap(), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn redact_access_token_scrubs_both_forms() {
        let token = generate_token();
        let uri = format!("/api/events?access_token={token}");
        let redacted = redact_access_token(&uri);
        assert!(
            !redacted.contains(&token),
            "token must not survive: {redacted}"
        );
        assert!(redacted.contains("access_token=[REDACTED]"));

        let uri2 = format!("/stream/5?format=flac&access_token={token}");
        let redacted2 = redact_access_token(&uri2);
        assert!(!redacted2.contains(&token));
        assert!(redacted2.contains("format=flac"));

        // No token present: unchanged.
        assert_eq!(
            redact_access_token("/api/albums?page=2"),
            "/api/albums?page=2"
        );
    }

    /// Log-hygiene (TODO SEC-3): a token-shaped secret pushed through
    /// every log path we control must never appear in log output. The
    /// controlled paths are the redacted request URI (span) and the
    /// structured fields below — none of which take the raw token.
    #[test]
    fn token_shaped_secrets_never_reach_logs() {
        let token = generate_token();
        // The access-log path:
        let logged_uri = redact_access_token(&format!("/api/events?access_token={token}"));
        assert!(!logged_uri.contains(&token));
        // Structured fields carry hashes or lengths, never the secret:
        let fields = format!(
            "token_len={} token_hash={}",
            token.len(),
            &sha256_hex(&token)[..12]
        );
        assert!(!fields.contains(&token));
    }
}
