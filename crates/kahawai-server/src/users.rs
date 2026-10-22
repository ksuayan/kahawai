//! User management (P0 Phase 3, spec §3.1–§3.4).
//!
//! The `users` / `sessions` tables live behind migration 022. Passwords
//! are Argon2id hashes (PHC strings); session tokens are 256-bit
//! CSPRNG values stored as SHA-256 hashes, so a database read never
//! yields a live credential (spec §3.2).

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};

use crate::auth::{generate_token, sha256_hex};

/// Absolute session lifetime: 30 days (spec §3.2).
pub const SESSION_TTL_MS: i64 = 30 * 24 * 3600 * 1000;

/// Argon2id at the crate's recommended parameters (RFC 9106 / OWASP:
/// m=19456 KiB, t=2, p=1) — [`Argon2::default`].
fn argon2() -> Argon2<'static> {
    Argon2::default()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub is_admin: bool,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub user_id: i64,
    pub is_admin: bool,
}

/// True once the first admin exists — i.e. Phase 3 is live and the
/// pairing-token path is retired (spec §3, §3.1).
pub async fn users_exist(pool: &sqlx::SqlitePool) -> Result<bool, sqlx::Error> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await?;
    Ok(n > 0)
}

fn hash_password_blocking(password: &str) -> Result<String, argon2::password_hash::Error> {
    let mut salt_bytes = [0u8; 16];
    getrandom::fill(&mut salt_bytes).map_err(|_| argon2::password_hash::Error::Crypto)?;
    let salt = SaltString::encode_b64(&salt_bytes)?;
    Ok(argon2()
        .hash_password(password.as_bytes(), &salt)?
        .to_string())
}

fn verify_password_blocking(hash: &str, password: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(p) => p,
        Err(_) => return false,
    };
    argon2()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Create a user. Usernames are unique (case-insensitive match rejects
/// lookalikes); the password never touches the database in plaintext.
/// Hashing runs on a blocking thread — Argon2id is deliberately slow.
pub async fn create_user(
    pool: &sqlx::SqlitePool,
    username: &str,
    password: &str,
    is_admin: bool,
) -> Result<User, UserError> {
    let username = username.trim().to_string();
    if username.is_empty() || username.len() > 64 {
        return Err(UserError::BadUsername);
    }
    if password.len() < 8 {
        return Err(UserError::PasswordTooShort);
    }
    let password = password.to_string();
    let hash = tokio::task::spawn_blocking(move || hash_password_blocking(&password))
        .await
        .map_err(|_| UserError::Internal("hash task failed".into()))?
        .map_err(|e| UserError::Internal(e.to_string()))?;
    let now = now_ms();
    let id: i64 = sqlx::query(
        "INSERT INTO users (username, password_hash, is_admin, created_at_ms) VALUES (?, ?, ?, ?)
         RETURNING id",
    )
    .bind(&username)
    .bind(&hash)
    .bind(is_admin as i64)
    .bind(now)
    .fetch_one(pool)
    .await
    .map(|r: sqlx::sqlite::SqliteRow| {
        use sqlx::Row;
        r.get::<i64, _>("id")
    })
    .map_err(|e| {
        if e.to_string().contains("UNIQUE") {
            UserError::UsernameTaken
        } else {
            UserError::Db(e.to_string())
        }
    })?;
    Ok(User {
        id,
        username,
        is_admin,
    })
}

/// Check a username/password pair. Returns the user on success, `None`
/// on any failure — callers must not distinguish "unknown user" from
/// "wrong password" (no user-enumeration oracle, spec §3.3).
pub async fn authenticate(
    pool: &sqlx::SqlitePool,
    username: &str,
    password: &str,
) -> Result<Option<User>, UserError> {
    let row: Option<(i64, String, String, i64)> = sqlx::query_as(
        "SELECT id, username, password_hash, is_admin FROM users WHERE username = ? COLLATE NOCASE",
    )
    .bind(username.trim())
    .fetch_optional(pool)
    .await
    .map_err(|e| UserError::Db(e.to_string()))?;
    let Some((id, username, hash, is_admin)) = row else {
        // Same cost as a real check: hash something so timing doesn't
        // reveal whether the user exists.
        let dummy = password.to_string();
        tokio::task::spawn_blocking(move || {
            verify_password_blocking(
                "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$RdescudvAiOluY6gYxR56Q",
                &dummy,
            )
        })
        .await
        .ok();
        return Ok(None);
    };
    let password = password.to_string();
    let ok = tokio::task::spawn_blocking(move || verify_password_blocking(&hash, &password))
        .await
        .map_err(|_| UserError::Internal("verify task failed".into()))?;
    Ok(ok.then_some(User {
        id,
        username,
        is_admin: is_admin != 0,
    }))
}

/// Issue a session: 256-bit token, SHA-256 hash stored, 30-day absolute
/// expiry. Returns the raw token — the only moment it exists in
/// plaintext outside the client's keychain.
pub async fn create_session(pool: &sqlx::SqlitePool, user_id: i64) -> Result<String, UserError> {
    create_session_with_expiry(pool, user_id, now_ms() + SESSION_TTL_MS).await
}

async fn create_session_with_expiry(
    pool: &sqlx::SqlitePool,
    user_id: i64,
    expires_at_ms: i64,
) -> Result<String, UserError> {
    let token = generate_token();
    let token_hash = sha256_hex(&token);
    let now = now_ms();
    sqlx::query(
        "INSERT INTO sessions (user_id, token_hash, created_at_ms, expires_at_ms)
         VALUES (?, ?, ?, ?)",
    )
    .bind(user_id)
    .bind(&token_hash)
    .bind(now)
    .bind(expires_at_ms)
    .execute(pool)
    .await
    .map_err(|e| UserError::Db(e.to_string()))?;
    Ok(token)
}

/// Validate a presented session token: hash lookup, expiry enforced
/// (expired sessions are deleted on sight). `None` covers unknown,
/// expired, and malformed alike.
pub async fn validate_session(
    pool: &sqlx::SqlitePool,
    presented: &str,
) -> Result<Option<Session>, UserError> {
    let token_hash = sha256_hex(presented);
    let row: Option<(i64, i64, i64)> = sqlx::query_as(
        "SELECT s.user_id, s.expires_at_ms, u.is_admin FROM sessions s
         JOIN users u ON u.id = s.user_id WHERE s.token_hash = ?",
    )
    .bind(&token_hash)
    .fetch_optional(pool)
    .await
    .map_err(|e| UserError::Db(e.to_string()))?;
    let Some((user_id, expires_at_ms, is_admin)) = row else {
        return Ok(None);
    };
    if expires_at_ms <= now_ms() {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(&token_hash)
            .execute(pool)
            .await
            .map_err(|e| UserError::Db(e.to_string()))?;
        return Ok(None);
    }
    Ok(Some(Session {
        user_id,
        is_admin: is_admin != 0,
    }))
}

/// Logout: delete one session by its presented token.
pub async fn delete_session(pool: &sqlx::SqlitePool, presented: &str) -> Result<(), UserError> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(sha256_hex(presented))
        .execute(pool)
        .await
        .map_err(|e| UserError::Db(e.to_string()))?;
    Ok(())
}

/// Revoke every session of a user (password change, admin revoke).
pub async fn revoke_user_sessions(pool: &sqlx::SqlitePool, user_id: i64) -> Result<u64, UserError> {
    let r = sqlx::query("DELETE FROM sessions WHERE user_id = ?")
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(|e| UserError::Db(e.to_string()))?;
    Ok(r.rows_affected())
}

pub async fn list_users(pool: &sqlx::SqlitePool) -> Result<Vec<User>, UserError> {
    let rows: Vec<(i64, String, i64)> =
        sqlx::query_as("SELECT id, username, is_admin FROM users ORDER BY id")
            .fetch_all(pool)
            .await
            .map_err(|e| UserError::Db(e.to_string()))?;
    Ok(rows
        .into_iter()
        .map(|(id, username, is_admin)| User {
            id,
            username,
            is_admin: is_admin != 0,
        })
        .collect())
}

/// Delete a user and all their sessions. Refuses to delete the last
/// admin — a server with no admin cannot be administered.
pub async fn delete_user(pool: &sqlx::SqlitePool, id: i64) -> Result<(), UserError> {
    let admins: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE is_admin != 0")
        .fetch_one(pool)
        .await
        .map_err(|e| UserError::Db(e.to_string()))?;
    let target: Option<(i64,)> = sqlx::query_as("SELECT is_admin FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| UserError::Db(e.to_string()))?;
    let Some((is_admin,)) = target else {
        return Err(UserError::NotFound);
    };
    if is_admin != 0 && admins.0 <= 1 {
        return Err(UserError::LastAdmin);
    }
    revoke_user_sessions(pool, id).await?;
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| UserError::Db(e.to_string()))?;
    Ok(())
}

/// Change a password and revoke every session of that user (spec §3.2).
pub async fn set_password(
    pool: &sqlx::SqlitePool,
    id: i64,
    new_password: &str,
) -> Result<(), UserError> {
    if new_password.len() < 8 {
        return Err(UserError::PasswordTooShort);
    }
    let password = new_password.to_string();
    let hash = tokio::task::spawn_blocking(move || hash_password_blocking(&password))
        .await
        .map_err(|_| UserError::Internal("hash task failed".into()))?
        .map_err(|e| UserError::Internal(e.to_string()))?;
    let r = sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(&hash)
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| UserError::Db(e.to_string()))?;
    if r.rows_affected() == 0 {
        return Err(UserError::NotFound);
    }
    revoke_user_sessions(pool, id).await?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum UserError {
    #[error("database error: {0}")]
    Db(String),
    #[error("username is taken")]
    UsernameTaken,
    #[error("username must be 1-64 characters")]
    BadUsername,
    #[error("password must be at least 8 characters")]
    PasswordTooShort,
    #[error("user not found")]
    NotFound,
    #[error("cannot delete the last admin")]
    LastAdmin,
    #[error("internal error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// Login throttling (spec §3.3): 5 attempts per minute per IP, exponential
// backoff thereafter. Uniform 401 for bad credentials either way.
// ---------------------------------------------------------------------------

/// Attempts are counted per client IP; the backoff doubles every time
/// the 5-per-minute budget is exceeded, capped at 5 minutes, and resets
/// on a successful login.
#[derive(Debug)]
pub struct LoginThrottle {
    inner: std::sync::Mutex<HashMap<IpAddr, ThrottleEntry>>,
}

#[derive(Debug)]
struct ThrottleEntry {
    failures: VecDeque<Instant>,
    blocked_until: Option<Instant>,
    backoff_secs: u64,
}

const THROTTLE_WINDOW: Duration = Duration::from_secs(60);
const THROTTLE_MAX_ATTEMPTS: usize = 5;
const THROTTLE_MAX_BACKOFF_SECS: u64 = 300;

impl LoginThrottle {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// `Ok(())` if the attempt may proceed, `Err(retry_after)` if the IP
    /// is currently backed off.
    pub fn check(&self, ip: IpAddr) -> Result<(), Duration> {
        let mut map = self.inner.lock().unwrap();
        let now = Instant::now();
        let entry = map.entry(ip).or_insert_with(|| ThrottleEntry {
            failures: VecDeque::new(),
            blocked_until: None,
            backoff_secs: 1,
        });
        if let Some(until) = entry.blocked_until {
            if now < until {
                return Err(until - now);
            }
            entry.blocked_until = None;
        }
        entry
            .failures
            .retain(|t| now.duration_since(*t) < THROTTLE_WINDOW);
        if entry.failures.len() >= THROTTLE_MAX_ATTEMPTS {
            let backoff = entry.backoff_secs;
            entry.backoff_secs = (backoff * 2).min(THROTTLE_MAX_BACKOFF_SECS);
            entry.blocked_until = Some(now + Duration::from_secs(backoff));
            return Err(Duration::from_secs(backoff));
        }
        Ok(())
    }

    pub fn record_failure(&self, ip: IpAddr) {
        let mut map = self.inner.lock().unwrap();
        let entry = map.entry(ip).or_insert_with(|| ThrottleEntry {
            failures: VecDeque::new(),
            blocked_until: None,
            backoff_secs: 1,
        });
        entry.failures.push_back(Instant::now());
    }

    /// A successful login clears the IP's history and resets the backoff.
    pub fn record_success(&self, ip: IpAddr) {
        self.inner.lock().unwrap().remove(&ip);
    }
}

impl Default for LoginThrottle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(include_str!("../migrations/022_users.sql"))
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    #[tokio::test]
    async fn user_lifecycle() {
        let pool = test_pool().await;
        assert!(!users_exist(&pool).await.unwrap());

        let user = create_user(&pool, "kyo", "correct-horse-123", true)
            .await
            .unwrap();
        assert!(user.is_admin);
        assert!(users_exist(&pool).await.unwrap());

        // Duplicate username (case-insensitive) is rejected.
        assert!(matches!(
            create_user(&pool, "KYO", "another-pass-1", false).await,
            Err(UserError::UsernameTaken)
        ));
        // Short passwords are rejected.
        assert!(matches!(
            create_user(&pool, "anna", "short", false).await,
            Err(UserError::PasswordTooShort)
        ));

        // Authentication: right password works, wrong doesn't, unknown
        // user is indistinguishable from wrong password.
        assert!(authenticate(&pool, "kyo", "correct-horse-123")
            .await
            .unwrap()
            .is_some());
        assert!(authenticate(&pool, "kyo", "wrong-password")
            .await
            .unwrap()
            .is_none());
        assert!(authenticate(&pool, "nobody", "whatever")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn password_hash_is_argon2id_not_plaintext() {
        let pool = test_pool().await;
        create_user(&pool, "kyo", "correct-horse-123", true)
            .await
            .unwrap();
        let (hash,): (String,) =
            sqlx::query_as("SELECT password_hash FROM users WHERE username = 'kyo'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            hash.starts_with("$argon2id$"),
            "must be a PHC string: {hash}"
        );
        assert!(!hash.contains("correct-horse-123"));
    }

    #[tokio::test]
    async fn session_round_trip_and_expiry() {
        let pool = test_pool().await;
        let user = create_user(&pool, "kyo", "correct-horse-123", true)
            .await
            .unwrap();

        let token = create_session(&pool, user.id).await.unwrap();
        let session = validate_session(&pool, &token).await.unwrap().unwrap();
        assert_eq!(session.user_id, user.id);
        assert!(session.is_admin);

        // The database holds a hash, not the token.
        let (stored,): (String,) = sqlx::query_as("SELECT token_hash FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, sha256_hex(&token));
        assert_ne!(stored, token);

        // Logout kills it.
        delete_session(&pool, &token).await.unwrap();
        assert!(validate_session(&pool, &token).await.unwrap().is_none());

        // Expired sessions are rejected (and pruned).
        let old = create_session_with_expiry(&pool, user.id, now_ms() - 1000)
            .await
            .unwrap();
        assert!(validate_session(&pool, &old).await.unwrap().is_none());
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "expired session pruned on sight");
    }

    #[tokio::test]
    async fn password_change_revokes_sessions() {
        let pool = test_pool().await;
        let user = create_user(&pool, "kyo", "correct-horse-123", true)
            .await
            .unwrap();
        let t1 = create_session(&pool, user.id).await.unwrap();
        let t2 = create_session(&pool, user.id).await.unwrap();
        set_password(&pool, user.id, "new-password-456")
            .await
            .unwrap();
        assert!(validate_session(&pool, &t1).await.unwrap().is_none());
        assert!(validate_session(&pool, &t2).await.unwrap().is_none());
        // New password works, old doesn't.
        assert!(authenticate(&pool, "kyo", "new-password-456")
            .await
            .unwrap()
            .is_some());
        assert!(authenticate(&pool, "kyo", "correct-horse-123")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn last_admin_is_protected() {
        let pool = test_pool().await;
        let admin = create_user(&pool, "kyo", "correct-horse-123", true)
            .await
            .unwrap();
        assert!(matches!(
            delete_user(&pool, admin.id).await,
            Err(UserError::LastAdmin)
        ));
        let admin2 = create_user(&pool, "anna", "another-pass-1", true)
            .await
            .unwrap();
        delete_user(&pool, admin.id).await.unwrap();
        let users = list_users(&pool).await.unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].username, "anna");
        let _ = admin2;
    }

    #[test]
    fn throttle_engages_after_five_and_resets_on_success() {
        let throttle = LoginThrottle::new();
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        for _ in 0..5 {
            assert!(throttle.check(ip).is_ok());
            throttle.record_failure(ip);
        }
        // Sixth attempt inside the window is backed off.
        assert!(throttle.check(ip).is_err());
        // Success clears the slate.
        throttle.record_success(ip);
        assert!(throttle.check(ip).is_ok());
    }
}
