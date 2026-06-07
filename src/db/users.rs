// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::{Context, Result};

use super::Db;

impl Db {
    /// Ensure the default user (id=1) exists.
    /// Creates the user if not present, or updates the password hash if provided.
    pub async fn ensure_default_user(&self, password_hash: Option<String>) -> Result<i32> {
        let user_id: i32 = sqlx::query_scalar(
            "INSERT INTO users (id, username, password_hash, auth_token) \
             VALUES (1, 'default', $1, NULL) \
             ON CONFLICT (id) DO UPDATE SET \
                 password_hash = COALESCE(EXCLUDED.password_hash, users.password_hash) \
             RETURNING id",
        )
        .bind(&password_hash)
        .fetch_one(&self.pool)
        .await
        .context("Failed to ensure default user exists")?;

        Ok(user_id)
    }

    /// Get the default user's auth token from the database.
    pub async fn get_default_auth_token(&self) -> Result<Option<String>> {
        let token: Option<String> = sqlx::query_scalar("SELECT auth_token FROM users WHERE id = 1")
            .fetch_optional(&self.pool)
            .await
            .context("Failed to get default auth token")?;

        Ok(token)
    }

    /// Store an auth token for the default user.
    pub async fn store_auth_token(&self, token: &str) -> Result<()> {
        sqlx::query("UPDATE users SET auth_token = $1 WHERE id = 1")
            .bind(token)
            .execute(&self.pool)
            .await
            .context("Failed to store auth token")?;
        Ok(())
    }

    /// Get the default user's password hash from the database.
    pub async fn get_default_password_hash(&self) -> Result<Option<String>> {
        let hash: Option<String> =
            sqlx::query_scalar("SELECT password_hash FROM users WHERE id = 1")
                .fetch_optional(&self.pool)
                .await
                .context("Failed to get default password hash")?;

        Ok(hash)
    }
}
