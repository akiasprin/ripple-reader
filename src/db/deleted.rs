// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::{Context, Result};
use chrono::Utc;

use super::Db;

impl Db {
    pub async fn is_deleted(&self, id: &str) -> Result<bool> {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM deleted_papers WHERE id = $1)")
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
        Ok(exists)
    }

    pub async fn list_deleted_ids(&self) -> Result<Vec<String>> {
        let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM deleted_papers")
            .fetch_all(&self.pool)
            .await?;
        Ok(ids)
    }

    pub async fn cleanup_deleted_papers(&self) -> Result<usize> {
        let mut tx = self.pool.begin().await?;

        let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM deleted_papers")
            .fetch_all(&mut *tx)
            .await?;

        if ids.is_empty() {
            tx.commit().await?;
            return Ok(0);
        }

        sqlx::query("DELETE FROM paper_marks WHERE paper_id = ANY($1)")
            .bind(&ids)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM paper_tags WHERE paper_id = ANY($1)")
            .bind(&ids)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM paper_comments WHERE paper_id = ANY($1)")
            .bind(&ids)
            .execute(&mut *tx)
            .await?;

        let result = sqlx::query("DELETE FROM papers WHERE id = ANY($1)")
            .bind(&ids)
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM deleted_papers")
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(result.rows_affected() as usize)
    }

    pub async fn restore_paper(&self, id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM deleted_papers WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .context("Failed to restore paper")?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn delete_paper(&self, id: &str) -> Result<bool> {
        let result = sqlx::query(
            "WITH top_tags AS ( \
                SELECT tag FROM paper_tags WHERE paper_id = $1 ORDER BY weight DESC LIMIT 2 \
            ), \
            non_interest AS ( \
                SELECT tt.tag \
                FROM top_tags tt \
                WHERE NOT EXISTS ( \
                    SELECT 1 FROM user_tag_preferences \
                    WHERE user_id = 1 AND tag = tt.tag AND preference_type = 'interest' \
                ) \
            ), \
            insert_disinterest AS ( \
                INSERT INTO user_tag_preferences (user_id, tag, preference_type) \
                SELECT 1, tag, 'disinterest' FROM non_interest \
                ON CONFLICT (user_id, tag) DO UPDATE SET preference_type = 'disinterest' \
                RETURNING tag \
            ) \
            INSERT INTO deleted_papers (id, deleted_at) VALUES ($1, $2) \
            ON CONFLICT (id) DO UPDATE SET deleted_at = EXCLUDED.deleted_at",
        )
        .bind(id)
        .bind(Utc::now())
        .execute(&self.pool)
        .await
        .context("Failed to delete paper")?;
        Ok(result.rows_affected() > 0)
    }
}
