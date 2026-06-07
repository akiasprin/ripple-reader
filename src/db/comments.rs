// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::Result;
use chrono::Utc;
use sqlx::Row;

use super::Db;
use crate::db::types::PaperComment;

impl Db {
    pub async fn list_paper_comments(&self, paper_id: &str) -> Result<Vec<PaperComment>> {
        let rows = sqlx::query(
            "SELECT id, paper_id, quote, before_ctx, after_ctx, comment, ai_reply, ai_old_text, ai_new_text, ai_status, is_ai, parent_id, created_at, updated_at \
             FROM paper_comments \
             WHERE paper_id = $1 \
             ORDER BY created_at ASC",
        )
        .bind(paper_id)
        .fetch_all(&self.pool)
        .await?;

        let mut comments = Vec::with_capacity(rows.len());
        for row in &rows {
            comments.push(Self::map_comment_row(row)?);
        }
        Ok(comments)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn add_paper_comment(
        &self,
        paper_id: &str,
        quote: &str,
        before_ctx: &str,
        after_ctx: &str,
        comment: &str,
        is_ai: bool,
        ai_status: &str,
        parent_id: Option<i64>,
    ) -> Result<PaperComment> {
        let now = Utc::now();
        let row = sqlx::query(
            "INSERT INTO paper_comments (paper_id, quote, before_ctx, after_ctx, comment, is_ai, ai_status, parent_id, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
             RETURNING id, paper_id, quote, before_ctx, after_ctx, comment, ai_reply, ai_old_text, ai_new_text, ai_status, is_ai, parent_id, created_at, updated_at",
        )
        .bind(paper_id)
        .bind(quote)
        .bind(before_ctx)
        .bind(after_ctx)
        .bind(comment)
        .bind(is_ai)
        .bind(ai_status)
        .bind(parent_id)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;

        Ok(Self::map_comment_row(&row)?)
    }

    pub async fn delete_paper_comment(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM paper_comments WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Return all AI comments that are stuck in "pending" state (e.g. after a restart).
    pub async fn list_pending_ai_comments(&self) -> Result<Vec<PaperComment>> {
        let rows = sqlx::query(
            "SELECT id, paper_id, quote, before_ctx, after_ctx, comment, ai_reply, ai_old_text, ai_new_text, ai_status, is_ai, parent_id, created_at, updated_at \
             FROM paper_comments \
             WHERE is_ai = true AND ai_status = 'pending'",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut comments = Vec::with_capacity(rows.len());
        for row in &rows {
            comments.push(Self::map_comment_row(row)?);
        }
        Ok(comments)
    }

    pub async fn get_comment_by_id(&self, id: i64) -> Result<Option<PaperComment>> {
        let row = sqlx::query(
            "SELECT id, paper_id, quote, before_ctx, after_ctx, comment, ai_reply, ai_old_text, ai_new_text, ai_status, is_ai, parent_id, created_at, updated_at \
             FROM paper_comments \
             WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(r) => Ok(Some(Self::map_comment_row(&r)?)),
            None => Ok(None),
        }
    }

    /// Walk the parent_id chain from `parent_id` up to the root, returning all
    /// ancestor comments in chronological order (root first → immediate parent last).
    /// Used to reconstruct multi-turn conversation history.
    pub async fn get_comment_thread(&self, parent_id: i64) -> Result<Vec<PaperComment>> {
        let mut thread = Vec::new();
        let mut current_id = Some(parent_id);

        while let Some(id) = current_id {
            let row = sqlx::query(
                "SELECT id, paper_id, quote, before_ctx, after_ctx, comment, \
                 ai_reply, ai_old_text, ai_new_text, ai_status, is_ai, parent_id, \
                 created_at, updated_at \
                 FROM paper_comments WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

            match row {
                Some(r) => {
                    let next_id = r.try_get::<Option<i64>, _>("parent_id")?;
                    thread.push(Self::map_comment_row(&r)?);
                    current_id = next_id;
                }
                None => break,
            }
        }

        thread.reverse();
        Ok(thread)
    }

    /// Walk DOWN from `comment_id`, collecting all descendants in BFS order.
    /// Returns comments in chronological order (direct child first, then grandchildren).
    /// Used to include "what comes after" when regenerating a middle reply.
    pub async fn get_comment_subthread(&self, comment_id: i64) -> Result<Vec<PaperComment>> {
        let mut result = Vec::new();
        let mut queue: Vec<i64> = vec![comment_id];

        while !queue.is_empty() {
            let parent = queue.remove(0);
            let children = sqlx::query(
                "SELECT id, paper_id, quote, before_ctx, after_ctx, comment, \
                 ai_reply, ai_old_text, ai_new_text, ai_status, is_ai, parent_id, \
                 created_at, updated_at \
                 FROM paper_comments WHERE parent_id = $1 ORDER BY created_at ASC",
            )
            .bind(parent)
            .fetch_all(&self.pool)
            .await?;

            for row in children {
                let c = Self::map_comment_row(&row)?;
                queue.push(c.id);
                result.push(c);
            }
        }

        Ok(result)
    }

    pub async fn update_ai_comment_reply(
        &self,
        id: i64,
        ai_reply: &str,
        ai_old_text: &str,
        ai_new_text: &str,
        ai_status: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE paper_comments SET ai_reply = $1, ai_old_text = $2, ai_new_text = $3, ai_status = $4, updated_at = $5 WHERE id = $6",
        )
        .bind(ai_reply)
        .bind(ai_old_text)
        .bind(ai_new_text)
        .bind(ai_status)
        .bind(Utc::now())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn apply_ai_comment(&self, id: i64) -> Result<()> {
        sqlx::query(
            "UPDATE paper_comments SET ai_status = 'applied', updated_at = $1 WHERE id = $2",
        )
        .bind(Utc::now())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn reject_ai_comment(&self, id: i64) -> Result<()> {
        sqlx::query(
            "UPDATE paper_comments SET ai_status = 'rejected', updated_at = $1 WHERE id = $2",
        )
        .bind(Utc::now())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    fn map_comment_row(row: &sqlx::postgres::PgRow) -> Result<PaperComment, sqlx::Error> {
        Ok(PaperComment {
            id: row.try_get("id")?,
            paper_id: row.try_get("paper_id")?,
            quote: row.try_get("quote")?,
            before_ctx: row.try_get("before_ctx")?,
            after_ctx: row.try_get("after_ctx")?,
            comment: row.try_get("comment")?,
            ai_reply: row.try_get("ai_reply")?,
            ai_old_text: row.try_get("ai_old_text")?,
            ai_new_text: row.try_get("ai_new_text")?,
            ai_status: row.try_get("ai_status")?,
            is_ai: row.try_get("is_ai")?,
            parent_id: row.try_get("parent_id")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}
