// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::Row;

use super::Db;
use crate::db::types::{normalize_insight_heading, InsightBackupRow};

impl Db {
    pub async fn migrate_insight_headings(&self) -> Result<usize> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT paper_id, insight FROM paper_insights WHERE insight LIKE '# %'")
                .fetch_all(&self.pool)
                .await?;

        let mut count = 0;
        for (id, insight) in rows {
            let normalized = normalize_insight_heading(&insight);
            if normalized != insight {
                sqlx::query("UPDATE paper_insights SET insight = $1 WHERE paper_id = $2")
                    .bind(&normalized)
                    .bind(&id)
                    .execute(&self.pool)
                    .await?;
                count += 1;
            }
        }
        Ok(count)
    }

    pub async fn insight_progress(&self) -> Result<Vec<(String, String, String)>> {
        let titles: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT p.id, p.title, p.source_type FROM papers p \
             JOIN paper_insights pi ON p.id = pi.paper_id \
             WHERE p.id NOT IN (SELECT id FROM deleted_papers) AND pi.insight = '__ANALYZING__' \
             ORDER BY p.processed_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(titles)
    }

    pub async fn recent_completed_insights(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, String, Option<DateTime<Utc>>, String)>> {
        let rows: Vec<(String, String, Option<DateTime<Utc>>, String)> = sqlx::query_as(
            "SELECT p.id, p.title, COALESCE(pi.processed_at, p.processed_at) as ts, p.source_type \
             FROM papers p \
             JOIN paper_insights pi ON p.id = pi.paper_id \
             WHERE p.id NOT IN (SELECT id FROM deleted_papers) \
               AND pi.insight != '' \
               AND pi.insight != '__ANALYZING__' \
               AND NOT (pi.insight LIKE 'Figure extraction failed%' \
                        OR pi.insight LIKE 'Insight failed%' \
                        OR pi.insight LIKE 'MinerU API not configured%') \
             ORDER BY ts DESC \
             LIMIT $1",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get_insight_neighbors(
        &self,
        id: &str,
    ) -> Result<(
        Option<(String, String, String)>,
        Option<(String, String, String)>,
    )> {
        // Two index-range queries instead of a full-table window function scan.
        // papers(id) is the PK, so id < $1 / id > $1 with LIMIT 1 is instant.
        let filter = "FROM papers p \
            JOIN paper_insights pi ON p.id = pi.paper_id \
            WHERE p.id NOT IN (SELECT id FROM deleted_papers) \
              AND pi.insight != '' \
              AND pi.insight != '__ANALYZING__' \
              AND pi.insight NOT LIKE 'Figure extraction failed%' \
              AND pi.insight NOT LIKE 'Insight failed%' \
              AND pi.insight NOT LIKE 'MinerU API not configured%'";

        let prev_sql = format!(
            "SELECT p.id, p.title, p.source_type {filter} AND p.id < $1 ORDER BY p.id DESC LIMIT 1"
        );
        let next_sql = format!(
            "SELECT p.id, p.title, p.source_type {filter} AND p.id > $1 ORDER BY p.id ASC LIMIT 1"
        );

        let (prev, next) = tokio::join!(
            sqlx::query(&prev_sql).bind(id).fetch_optional(&self.pool),
            sqlx::query(&next_sql).bind(id).fetch_optional(&self.pool),
        );

        let prev = prev?.map(|r| {
            (
                r.try_get::<String, _>(0).unwrap_or_default(),
                r.try_get::<String, _>(1).unwrap_or_default(),
                r.try_get::<String, _>(2).unwrap_or_default(),
            )
        });
        let next = next?.map(|r| {
            (
                r.try_get::<String, _>(0).unwrap_or_default(),
                r.try_get::<String, _>(1).unwrap_or_default(),
                r.try_get::<String, _>(2).unwrap_or_default(),
            )
        });
        Ok((prev, next))
    }

    pub async fn clear_all_insights(&self) -> Result<usize> {
        let result = sqlx::query(
            "UPDATE paper_insights SET insight = '', processed_at = NULL, review = '', reviewed_at = NULL WHERE insight != '' AND checked_at IS NULL",
        )
        .execute(&self.pool)
        .await
        .context("Failed to clear all insights")?;
        Ok(result.rows_affected() as usize)
    }

    pub async fn backup_insight(
        &self,
        paper_id: &str,
        insight: &str,
        insight_processed_at: Option<&str>,
        insight_review: &str,
        insight_reviewed_at: Option<&str>,
    ) -> Result<()> {
        let insight_processed_at = insight_processed_at
            .map(|s| DateTime::parse_from_rfc3339(s).map(|dt| dt.with_timezone(&Utc)))
            .transpose()
            .context("Invalid insight_processed_at")?;
        let insight_reviewed_at = insight_reviewed_at
            .map(|s| DateTime::parse_from_rfc3339(s).map(|dt| dt.with_timezone(&Utc)))
            .transpose()
            .context("Invalid insight_reviewed_at")?;
        let created_at = Utc::now();

        sqlx::query(
            "INSERT INTO paper_insight_backups (paper_id, insight, insight_processed_at, insight_review, insight_reviewed_at, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(paper_id)
        .bind(insight)
        .bind(insight_processed_at)
        .bind(insight_review)
        .bind(insight_reviewed_at)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .context("Failed to backup insight")?;
        Ok(())
    }

    pub async fn list_paper_insight_backups(
        &self,
        paper_id: &str,
    ) -> Result<Vec<InsightBackupRow>> {
        let rows: Vec<InsightBackupRow> = sqlx::query_as(
            "SELECT id, insight, insight_processed_at, insight_review, insight_reviewed_at, created_at \
             FROM paper_insight_backups \
             WHERE paper_id = $1 \
             ORDER BY created_at DESC",
        )
        .bind(paper_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn delete_insight_backup(&self, paper_id: &str, backup_id: i64) -> Result<bool> {
        let result =
            sqlx::query("DELETE FROM paper_insight_backups WHERE id = $1 AND paper_id = $2")
                .bind(backup_id)
                .bind(paper_id)
                .execute(&self.pool)
                .await
                .context("Failed to delete insight backup")?;
        Ok(result.rows_affected() > 0)
    }
}
