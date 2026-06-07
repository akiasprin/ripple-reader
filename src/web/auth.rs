// SPDX-License-Identifier: MIT OR Apache-2.0

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Json,
};
use base64::Engine;
use bcrypt::{hash, verify, DEFAULT_COST};
use rand::Rng;
use std::sync::Arc;

use super::types::{AppState, AuthRequest, AuthResponse, AuthStatus};

/// Hash a plaintext password using bcrypt. Called once at server startup.
pub(crate) fn hash_password(password: &str) -> Result<String, bcrypt::BcryptError> {
    hash(password, DEFAULT_COST)
}

/// Verify a plaintext password against a bcrypt hash.
pub(crate) fn verify_password(
    password: &str,
    password_hash: &str,
) -> Result<bool, bcrypt::BcryptError> {
    verify(password, password_hash)
}

/// Generate a random 32-byte authentication token (base64-encoded).
pub(crate) fn generate_auth_token() -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..32).map(|_| rng.gen()).collect();
    base64::prelude::BASE64_STANDARD.encode(&bytes)
}

/// Check if the request is authenticated.
/// If no auth token is configured (passwordless mode), allow all requests.
pub(crate) fn check_auth(headers: &HeaderMap, auth_token: &Option<String>) -> bool {
    let Some(expected) = auth_token else {
        return true;
    };
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    auth == format!("Bearer {}", expected)
}

pub(crate) async fn auth_login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AuthRequest>,
) -> Result<Json<AuthResponse>, StatusCode> {
    // No password configured → allow any login
    if state.password_hash.is_none() {
        return Ok(Json(AuthResponse {
            token: "authenticated".to_string(),
        }));
    }

    let Some(hash) = state.password_hash.as_deref() else {
        return Err(StatusCode::UNAUTHORIZED);
    };

    match verify_password(&req.password, hash) {
        Ok(true) => {
            let token = state
                .auth_token
                .as_ref()
                .expect("auth_token must be set when password_hash is set")
                .clone();
            Ok(Json(AuthResponse { token }))
        }
        Ok(false) => Err(StatusCode::UNAUTHORIZED),
        Err(e) => {
            tracing::error!("[auth] bcrypt verification error: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

pub(crate) async fn auth_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Json<AuthStatus> {
    let authenticated = check_auth(&headers, &state.auth_token);
    Json(AuthStatus { authenticated })
}
