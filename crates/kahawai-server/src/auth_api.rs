//! Login / logout / first-run setup / user management HTTP handlers
//! (P0 Phase 1 §1.3, Phase 3 §3.1–§3.4).
//!
//! `/api/login` and `/api/setup*` are intentionally public (they are the
//! credential issuance endpoints — requiring a bearer there would be
//! circular); everything else here sits behind [`crate::auth::require_auth`].

use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, Extension, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::{
    api::ApiError,
    auth::{self, AuthContext},
    users::{self, UserError},
    AppState,
};

fn user_error(e: UserError) -> ApiError {
    ApiError::from(match e {
        UserError::UsernameTaken => kahawai_core::MusicError::Conflict("username is taken".into()),
        UserError::BadUsername | UserError::PasswordTooShort => {
            kahawai_core::MusicError::BadRequest(e.to_string())
        }
        UserError::NotFound => kahawai_core::MusicError::NotFound("user not found".into()),
        UserError::LastAdmin => {
            kahawai_core::MusicError::Conflict("cannot delete the last admin".into())
        }
        UserError::Db(m) | UserError::Internal(m) => kahawai_core::MusicError::Db(m),
    })
}

fn forbidden() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({ "error": "forbidden" })),
    )
        .into_response()
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": "unauthorized" })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Login / logout (Phase 3 §3.2–§3.3)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct LoginBody {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub username: String,
    pub is_admin: bool,
}

/// `POST /api/login`: username + password → session token. Uniform 401
/// for bad credentials (no user-enumeration oracle, spec §3.3); 429
/// with `Retry-After` when the per-IP throttle engages.
pub async fn login(
    State(s): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<LoginBody>,
) -> Response {
    if let Err(retry_after) = s.login_throttle.check(peer.ip()) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, retry_after.as_secs().to_string())],
            Json(serde_json::json!({ "error": "too many attempts, try again later" })),
        )
            .into_response();
    }
    match users::authenticate(&s.pool, &body.username, &body.password).await {
        Ok(Some(user)) => {
            s.login_throttle.record_success(peer.ip());
            match users::create_session(&s.pool, user.id).await {
                Ok(token) => (
                    StatusCode::OK,
                    Json(LoginResponse {
                        token,
                        username: user.username,
                        is_admin: user.is_admin,
                    }),
                )
                    .into_response(),
                Err(e) => user_error(e).into_response(),
            }
        }
        Ok(None) => {
            s.login_throttle.record_failure(peer.ip());
            unauthorized()
        }
        Err(e) => user_error(e).into_response(),
    }
}

/// `POST /api/logout`: delete the presenting session. Idempotent — a
/// stale or already-dead token still gets a 200.
pub async fn logout(State(s): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = auth::bearer_from_headers(&headers) {
        let _ = users::delete_session(&s.pool, &token).await;
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({ "status": "logged out" })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// First-run setup (Phase 3 §3.1)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct SetupStatus {
    pub needs_setup: bool,
}

/// `GET /api/setup/status`: does this server still need its first admin?
pub async fn setup_status(State(s): State<AppState>) -> Result<Json<SetupStatus>, ApiError> {
    let needs_setup = !users::users_exist(&s.pool)
        .await
        .map_err(|e| ApiError::from(kahawai_core::MusicError::Db(e.to_string())))?;
    Ok(Json(SetupStatus { needs_setup }))
}

#[derive(Debug, Deserialize)]
pub struct SetupBody {
    pub username: String,
    pub password: String,
    /// The Phase 1 pairing token — the bootstrap secret (spec §3.1).
    pub pairing_token: String,
}

/// `POST /api/setup`: create the first admin. Requires the pairing
/// token; once any user exists this endpoint is 404 (spec §3.1). On
/// success the pairing token is deleted from the keychain and retired,
/// so the single-token auth path dies at the same moment the first
/// session-capable credential is born.
pub async fn setup(State(s): State<AppState>, Json(body): Json<SetupBody>) -> Response {
    let exists = users::users_exist(&s.pool).await.unwrap_or(true);
    if exists {
        return StatusCode::NOT_FOUND.into_response();
    }
    let expected = s.auth.pairing_token_hash.read().unwrap().clone();
    match expected {
        Some(expected_hex) if auth::token_matches(&body.pairing_token, &expected_hex) => {}
        _ => return unauthorized(),
    }
    let user = match users::create_user(&s.pool, &body.username, &body.password, true).await {
        Ok(user) => user,
        Err(e) => return user_error(e).into_response(),
    };
    // Retire the pairing token: keychain entry deleted, in-memory hash
    // cleared. The hash retirement is what actually kills the token;
    // a keychain failure here is hygiene, not security — warn, continue.
    if let Err(e) = s.token_store.delete() {
        tracing::warn!(error = %e, "could not delete the retired pairing token from the store");
    }
    s.auth.retire_pairing_token();
    s.token_shown
        .store(false, std::sync::atomic::Ordering::SeqCst);
    (
        StatusCode::CREATED,
        Json(serde_json::json!({ "username": user.username })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Pairing-token rotation (Phase 1 §1.3)
// ---------------------------------------------------------------------------

/// `POST /api/token/regenerate`: new 256-bit token, keychain updated,
/// old token invalid immediately. Only in pairing-token mode (Phase 3
/// retires this path); the caller must present the current token.
pub async fn regenerate_token(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Response {
    if !ctx.via_pairing_token {
        return forbidden();
    }
    let token = auth::generate_token();
    if let Err(e) = s.token_store.set(&token) {
        return ApiError::from(kahawai_core::MusicError::Db(format!(
            "could not store the new token: {e}"
        )))
        .into_response();
    }
    s.auth.rotate_pairing_token(&token);
    // The web UI's show-once flag resets so the new token is displayed
    // once (spec §1.3: display exactly once).
    s.token_shown
        .store(false, std::sync::atomic::Ordering::SeqCst);
    (StatusCode::OK, Json(serde_json::json!({ "token": token }))).into_response()
}

// ---------------------------------------------------------------------------
// User management (Phase 3 §3.4; admin only)
// ---------------------------------------------------------------------------

/// Returns `Some(403)` when the caller is not an admin.
fn require_admin(ctx: &AuthContext) -> Option<Response> {
    (!ctx.is_admin).then(forbidden)
}

#[derive(Debug, Serialize)]
pub struct UserView {
    pub id: i64,
    pub username: String,
    pub is_admin: bool,
}

/// `GET /api/users`: list users (admin only).
pub async fn list_users(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Response {
    if let Some(r) = require_admin(&ctx) {
        return r;
    }
    match users::list_users(&s.pool).await {
        Ok(list) => Json(
            list.into_iter()
                .map(|u| UserView {
                    id: u.id,
                    username: u.username,
                    is_admin: u.is_admin,
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => user_error(e).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateUserBody {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub is_admin: bool,
}

/// `POST /api/users`: create a user (admin only).
pub async fn create_user(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(body): Json<CreateUserBody>,
) -> Response {
    if let Some(r) = require_admin(&ctx) {
        return r;
    }
    match users::create_user(&s.pool, &body.username, &body.password, body.is_admin).await {
        Ok(user) => (
            StatusCode::CREATED,
            Json(UserView {
                id: user.id,
                username: user.username,
                is_admin: user.is_admin,
            }),
        )
            .into_response(),
        Err(e) => user_error(e).into_response(),
    }
}

/// `DELETE /api/users/{id}`: revoke a user and all their sessions
/// (admin only; cannot delete the last admin).
pub async fn delete_user(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<i64>,
) -> Response {
    if let Some(r) = require_admin(&ctx) {
        return r;
    }
    // An admin cannot delete themselves out from under their own
    // session via this path — they would lock the server into an
    // admin-less state only if they were the last admin, which
    // `users::delete_user` already refuses.
    match users::delete_user(&s.pool, id).await {
        Ok(()) => Json(serde_json::json!({ "status": "deleted" })).into_response(),
        Err(e) => user_error(e).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordBody {
    pub new_password: String,
}

/// `POST /api/users/me/password`: change your own password. Revokes
/// every session of the user (spec §3.2) — including the one making
/// the call, so the client should re-login.
pub async fn change_password(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(body): Json<ChangePasswordBody>,
) -> Response {
    let Some(user_id) = ctx.user_id else {
        // Pairing-token mode has no users yet.
        return ApiError::from(kahawai_core::MusicError::BadRequest(
            "no user accounts exist yet".into(),
        ))
        .into_response();
    };
    match users::set_password(&s.pool, user_id, &body.new_password).await {
        Ok(()) => Json(serde_json::json!({ "status": "password changed; all sessions revoked" }))
            .into_response(),
        Err(e) => user_error(e).into_response(),
    }
}
