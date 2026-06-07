// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::Row;
use std::collections::HashMap;

use super::Db;
use crate::db::types::AuthorStats;

impl Db {
    pub async fn get_author_stats(&self, name: &str) -> Result<Option<AuthorStats>> {
        let row = sqlx::query(
            "SELECT paper_count, avg_score, critical_count, citation_count, h_index \
             FROM author_stats WHERE name = $1",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(r) => Ok(Some(AuthorStats {
                paper_count: r.try_get(0)?,
                avg_score: r.try_get(1)?,
                critical_count: r.try_get(2)?,
                citation_count: r.try_get(3).ok(),
                h_index: r.try_get(4).ok(),
            })),
            None => Ok(None),
        }
    }

    pub async fn upsert_author_stats(
        &self,
        name: &str,
        score: f32,
        is_critical: bool,
    ) -> Result<()> {
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO author_stats (name, paper_count, avg_score, critical_count, first_seen, last_updated) \
             VALUES ($1, 1, $2, $3, $4, $4) \
             ON CONFLICT (name) DO UPDATE SET \
             paper_count = author_stats.paper_count + 1, \
             avg_score = (author_stats.avg_score * author_stats.paper_count + EXCLUDED.avg_score) / (author_stats.paper_count + 1), \
             critical_count = author_stats.critical_count + EXCLUDED.critical_count, \
             last_updated = EXCLUDED.last_updated",
        )
        .bind(name)
        .bind(score)
        .bind(if is_critical { 1i64 } else { 0i64 })
        .bind(now)
        .execute(&self.pool)
        .await
        .context("Failed to upsert author stats")?;
        Ok(())
    }

    pub async fn update_author_external_stats(
        &self,
        name: &str,
        citation_count: Option<i64>,
        h_index: Option<i64>,
    ) -> Result<()> {
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO author_stats (name, paper_count, avg_score, critical_count, citation_count, h_index, first_seen, last_updated) \
             VALUES ($1, 0, 0.0, 0, $2, $3, $4, $4) \
             ON CONFLICT (name) DO UPDATE SET \
             citation_count = COALESCE(EXCLUDED.citation_count, author_stats.citation_count), \
             h_index = COALESCE(EXCLUDED.h_index, author_stats.h_index), \
             last_updated = EXCLUDED.last_updated",
        )
        .bind(name)
        .bind(citation_count)
        .bind(h_index)
        .bind(now)
        .execute(&self.pool)
        .await
        .context("Failed to update author external stats")?;
        Ok(())
    }

    pub async fn estimate_local_stats(&self) -> Result<usize> {
        let now = Utc::now();
        let result = sqlx::query(
            "UPDATE author_stats \
             SET paper_count = CASE \
                 WHEN h_index >= 50 THEN h_index * 6 \
                 WHEN h_index >= 20 THEN h_index * 5 \
                 WHEN h_index >= 10 THEN h_index * 4 \
                 WHEN h_index > 0  THEN h_index * 3 \
                 ELSE 0 \
             END, \
             avg_score = CASE \
                 WHEN h_index >= 50 THEN 7.0 \
                 WHEN h_index >= 20 THEN 6.5 \
                 WHEN h_index >= 10 THEN 6.0 \
                 WHEN h_index > 0  THEN 5.5 \
                 ELSE 5.0 \
             END, \
             last_updated = $1 \
             WHERE paper_count = 0 AND h_index IS NOT NULL",
        )
        .bind(now)
        .execute(&self.pool)
        .await
        .context("Failed to estimate local stats")?;
        Ok(result.rows_affected() as usize)
    }

    pub async fn build_author_stats_from_papers(&self) -> Result<usize> {
        let mut tx = self.pool.begin().await?;
        let now = Utc::now();

        sqlx::query("DELETE FROM author_stats")
            .execute(&mut *tx)
            .await?;

        let rows = sqlx::query(
            "SELECT id, authors, score FROM papers WHERE id NOT IN (SELECT id FROM deleted_papers)",
        )
        .fetch_all(&mut *tx)
        .await?;

        let mut papers = Vec::with_capacity(rows.len());
        for row in &rows {
            let id: String = row.try_get(0)?;
            let authors_json: sqlx::types::Json<Vec<String>> = row.try_get(1)?;
            let score: f32 = row.try_get(2)?;
            papers.push((id, authors_json.0, score));
        }

        let mut author_scores: HashMap<String, Vec<f32>> = HashMap::new();
        let mut author_critical: HashMap<String, i64> = HashMap::new();

        for (paper_id, authors, score) in papers {
            let is_critical: bool =
                sqlx::query("SELECT 1 FROM paper_marks WHERE paper_id = $1 AND mark = 'critical'")
                    .bind(&paper_id)
                    .fetch_optional(&mut *tx)
                    .await?
                    .is_some();

            for author in authors.iter().take(20) {
                let author = author.trim().to_string();
                if author.len() < 2 {
                    tracing::warn!(
                        "[build_author_stats] Skipping short author name '{}' from paper {}",
                        author,
                        paper_id
                    );
                    continue;
                }

                author_scores.entry(author.clone()).or_default().push(score);
                if is_critical {
                    *author_critical.entry(author).or_insert(0) += 1;
                }
            }
        }

        for (author, scores) in author_scores {
            let count = scores.len() as i64;
            let avg = scores.iter().sum::<f32>() / scores.len() as f32;
            let critical = author_critical.get(&author).copied().unwrap_or(0);

            sqlx::query(
                "INSERT INTO author_stats (name, paper_count, avg_score, critical_count, first_seen, last_updated) \
                 VALUES ($1, $2, $3, $4, $5, $5)",
            )
            .bind(&author)
            .bind(count)
            .bind(avg)
            .bind(critical)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM author_stats")
            .fetch_one(&self.pool)
            .await?;
        Ok(count as usize)
    }

    pub async fn get_authors_without_external_stats(&self, limit: usize) -> Result<Vec<String>> {
        let names: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM author_stats WHERE h_index IS NULL AND paper_count >= 0 ORDER BY paper_count DESC LIMIT $1",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(names)
    }
}
