// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::Result;
use std::collections::HashMap;

use super::Db;
use crate::db::types::{DbPaper, TagsDashboard};

impl Db {
    pub async fn save_paper_tags(&self, paper_id: &str, tags: &[(String, f32)]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM paper_tags WHERE paper_id = $1")
            .bind(paper_id)
            .execute(&mut *tx)
            .await?;
        if !tags.is_empty() {
            let tag_names: Vec<&str> = tags.iter().map(|(t, _)| t.as_str()).collect();
            let weights: Vec<f32> = tags.iter().map(|(_, w)| *w).collect();
            sqlx::query(
                "INSERT INTO paper_tags (paper_id, tag, weight) \
                 SELECT $1, unnest($2::text[]), unnest($3::real[])",
            )
            .bind(paper_id)
            .bind(&tag_names[..])
            .bind(&weights[..])
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn get_paper_tags(&self, paper_id: &str) -> Result<Vec<(String, f32)>> {
        let tags: Vec<(String, f32)> = sqlx::query_as(
            "SELECT tag, weight FROM paper_tags WHERE paper_id = $1 ORDER BY weight DESC",
        )
        .bind(paper_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(tags)
    }

    pub async fn list_papers_tags(
        &self,
        paper_ids: &[String],
    ) -> Result<HashMap<String, Vec<(String, f32)>>> {
        let mut map: HashMap<String, Vec<(String, f32)>> = HashMap::new();
        if paper_ids.is_empty() {
            return Ok(map);
        }
        let rows: Vec<(String, String, f32)> = sqlx::query_as(
            "SELECT paper_id, tag, weight FROM paper_tags WHERE paper_id = ANY($1) ORDER BY weight DESC",
        )
        .bind(paper_ids)
        .fetch_all(&self.pool)
        .await?;

        for (id, tag, weight) in rows {
            map.entry(id).or_default().push((tag, weight));
        }
        Ok(map)
    }

    pub async fn find_related_papers(
        &self,
        paper_id: &str,
        limit: usize,
    ) -> Result<Vec<(String, f32)>> {
        let self_tags: Vec<(String, f32)> =
            sqlx::query_as("SELECT tag, weight FROM paper_tags WHERE paper_id = $1")
                .bind(paper_id)
                .fetch_all(&self.pool)
                .await?;

        if self_tags.is_empty() {
            return Ok(Vec::new());
        }

        let rows: Vec<(String, String, f32)> = sqlx::query_as(
            "SELECT paper_id, tag, weight FROM paper_tags WHERE paper_id != $1 AND paper_id NOT IN (SELECT id FROM deleted_papers) AND tag IN (SELECT tag FROM paper_tags WHERE paper_id = $1)",
        )
        .bind(paper_id)
        .fetch_all(&self.pool)
        .await?;

        let mut other_papers: HashMap<String, Vec<(String, f32)>> = HashMap::new();
        for (id, tag, weight) in rows {
            other_papers.entry(id).or_default().push((tag, weight));
        }

        let self_map: HashMap<String, f32> = self_tags.into_iter().collect();
        let mut similarities: Vec<(String, f32)> = Vec::new();

        for (other_id, other_tags) in other_papers {
            let other_map: HashMap<String, f32> = other_tags.into_iter().collect();
            let all_tags: std::collections::HashSet<_> =
                self_map.keys().chain(other_map.keys()).collect();
            let mut dot = 0.0f32;
            let mut norm1 = 0.0f32;
            let mut norm2 = 0.0f32;
            for tag in &all_tags {
                let v1 = *self_map.get(*tag).unwrap_or(&0.0);
                let v2 = *other_map.get(*tag).unwrap_or(&0.0);
                dot += v1 * v2;
                norm1 += v1 * v1;
                norm2 += v2 * v2;
            }
            let sim = if norm1 > 0.0 && norm2 > 0.0 {
                dot / (norm1.sqrt() * norm2.sqrt())
            } else {
                0.0
            };
            if sim > 0.0 {
                similarities.push((other_id, sim));
            }
        }

        similarities.sort_by(|a, b| b.1.total_cmp(&a.1));
        similarities.truncate(limit);
        Ok(similarities)
    }

    pub async fn list_tags_with_counts(&self) -> Result<Vec<(String, usize)>> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "WITH ranked AS ( \
                SELECT paper_id, tag, weight, \
                       ROW_NUMBER() OVER (PARTITION BY paper_id ORDER BY weight DESC) as rn \
                FROM paper_tags \
                WHERE paper_id NOT IN (SELECT id FROM deleted_papers) \
            ) \
            SELECT tag, COUNT(*) as cnt \
            FROM ranked \
            WHERE rn <= 3 \
            GROUP BY tag \
            ORDER BY cnt DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(t, c)| (t, c as usize)).collect())
    }

    pub async fn get_tags_dashboard(&self) -> Result<TagsDashboard> {
        let tags: Vec<(String, i64)> = sqlx::query_as(
            "WITH ranked AS ( \
                SELECT paper_id, tag, weight, \
                       ROW_NUMBER() OVER (PARTITION BY paper_id ORDER BY weight DESC) as rn \
                FROM paper_tags \
                WHERE paper_id NOT IN (SELECT id FROM deleted_papers) \
            ) \
            SELECT tag, COUNT(*) as cnt \
            FROM ranked \
            WHERE rn <= 3 \
            GROUP BY tag \
            ORDER BY cnt DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        let tags: Vec<(String, usize)> = tags.into_iter().map(|(t, c)| (t, c as usize)).collect();

        let orphan_count: i64 = sqlx::query_scalar(
            "WITH tag_counts AS ( \
                SELECT tag, COUNT(DISTINCT paper_id) as paper_cnt \
                FROM paper_tags \
                WHERE paper_id NOT IN (SELECT id FROM deleted_papers) \
                GROUP BY tag \
            ) \
            SELECT COUNT(DISTINCT p.id) \
            FROM papers p \
            WHERE p.id NOT IN (SELECT id FROM deleted_papers) \
              AND EXISTS (SELECT 1 FROM paper_tags pt WHERE pt.paper_id = p.id) \
              AND NOT EXISTS ( \
                  SELECT 1 FROM paper_tags pt \
                  JOIN tag_counts tc ON pt.tag = tc.tag \
                  WHERE pt.paper_id = p.id AND tc.paper_cnt > 1 \
              )",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);

        let disinterest_cfg_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM user_tag_preferences WHERE user_id = 1 AND preference_type = 'disinterest'",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);

        let uninteresting_count = if disinterest_cfg_count == 0 {
            0i64
        } else {
            sqlx::query_scalar(
                "WITH disinterest_tags AS ( \
                    SELECT tag FROM user_tag_preferences WHERE user_id = 1 AND preference_type = 'disinterest' \
                ), \
                interest_tags AS ( \
                    SELECT tag FROM user_tag_preferences WHERE user_id = 1 AND preference_type = 'interest' \
                ), \
                has_disinterest AS ( \
                    SELECT DISTINCT p.id \
                    FROM papers p \
                    JOIN paper_tags pt ON p.id = pt.paper_id \
                    JOIN disinterest_tags dt ON pt.tag LIKE '%' || dt.tag || '%' \
                    WHERE p.id NOT IN (SELECT id FROM deleted_papers) \
                ), \
                has_interest AS ( \
                    SELECT DISTINCT p.id \
                    FROM papers p \
                    JOIN paper_tags pt ON p.id = pt.paper_id \
                    JOIN interest_tags it ON pt.tag LIKE '%' || it.tag || '%' \
                    WHERE p.id NOT IN (SELECT id FROM deleted_papers) \
                ) \
                SELECT COUNT(*) \
                FROM papers p \
                WHERE p.id IN (SELECT id FROM has_disinterest) \
                  AND p.id NOT IN (SELECT id FROM has_interest) \
                  AND p.id NOT IN (SELECT id FROM deleted_papers)",
            )
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0)
        };

        let disinterest_tags: Vec<(String, i64)> = sqlx::query_as(
            "WITH disinterest_tags AS ( \
                SELECT tag FROM user_tag_preferences WHERE user_id = 1 AND preference_type = 'disinterest' \
            ), \
            interest_tags AS ( \
                SELECT tag FROM user_tag_preferences WHERE user_id = 1 AND preference_type = 'interest' \
            ), \
            has_interest AS ( \
                SELECT DISTINCT p.id \
                FROM papers p \
                JOIN paper_tags pt ON p.id = pt.paper_id \
                JOIN interest_tags it ON pt.tag LIKE '%' || it.tag || '%' \
                WHERE p.id NOT IN (SELECT id FROM deleted_papers) \
            ) \
            SELECT dt.tag, COUNT(DISTINCT pt.paper_id) as cnt \
            FROM disinterest_tags dt \
            JOIN paper_tags pt ON pt.tag LIKE '%' || dt.tag || '%' \
            JOIN papers p ON pt.paper_id = p.id \
            WHERE p.id NOT IN (SELECT id FROM deleted_papers) \
              AND p.id NOT IN (SELECT id FROM has_interest) \
            GROUP BY dt.tag \
            ORDER BY cnt DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        let disinterest_tags: Vec<(String, usize)> = disinterest_tags
            .into_iter()
            .map(|(t, c)| (t, c as usize))
            .collect();

        let interest_tags: Vec<String> = sqlx::query_scalar(
            "SELECT tag FROM user_tag_preferences WHERE user_id = 1 AND preference_type = 'interest' ORDER BY tag",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(TagsDashboard {
            tags,
            orphan_count: orphan_count as usize,
            uninteresting_count: uninteresting_count as usize,
            disinterest_tags,
            interest_tags,
        })
    }

    pub async fn get_paper_ids_by_tag(&self, tag: &str) -> Result<Vec<String>> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT paper_id FROM paper_tags WHERE tag = $1 AND paper_id NOT IN (SELECT id FROM deleted_papers)",
        )
        .bind(tag)
        .fetch_all(&self.pool)
        .await?;
        Ok(ids)
    }

    pub async fn sync_interest_from_critical_paper(&self, id: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO user_tag_preferences (user_id, tag, preference_type) \
             SELECT 1, tag, 'interest' FROM paper_tags WHERE paper_id = $1 \
             ON CONFLICT (user_id, tag) DO UPDATE SET preference_type = 'interest'",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_tag_preferences(&self, preference_type: &str) -> Result<Vec<String>> {
        let tags: Vec<String> = sqlx::query_scalar(
            "SELECT tag FROM user_tag_preferences WHERE user_id = 1 AND preference_type = $1 ORDER BY tag",
        )
        .bind(preference_type)
        .fetch_all(&self.pool)
        .await?;
        Ok(tags)
    }

    pub async fn ensure_tag_preferences(&self) -> Result<()> {
        let paper_ids: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT pt.paper_id FROM paper_tags pt \
             JOIN paper_marks pm ON pt.paper_id = pm.paper_id \
             WHERE pm.mark = 'critical' \
             AND pt.paper_id NOT IN (SELECT id FROM deleted_papers)",
        )
        .fetch_all(&self.pool)
        .await?;

        for pid in paper_ids {
            let tags: Vec<String> = sqlx::query_scalar(
                "SELECT tag FROM paper_tags WHERE paper_id = $1 ORDER BY weight DESC",
            )
            .bind(&pid)
            .fetch_all(&self.pool)
            .await?;

            for tag in tags {
                let _ = sqlx::query(
                    "INSERT INTO user_tag_preferences (user_id, tag, preference_type) VALUES (1, $1, 'interest') \
                     ON CONFLICT (user_id, tag) DO UPDATE SET preference_type = 'interest'",
                )
                .bind(&tag)
                .execute(&self.pool)
                .await;
            }
        }
        Ok(())
    }

    pub async fn list_orphan_tag_papers(&self) -> Result<Vec<DbPaper>> {
        let rows = sqlx::query(
            "WITH tag_counts AS ( \
                SELECT tag, COUNT(DISTINCT paper_id) as paper_cnt \
                FROM paper_tags \
                WHERE paper_id NOT IN (SELECT id FROM deleted_papers) \
                GROUP BY tag \
            ) \
            SELECT p.id, p.title, p.authors, p.published, p.score, p.paper_type, p.summary, p.abstract, p.processed_at, \
                   pi.insight, pi.processed_at as insight_processed_at, pi.review as insight_review, pi.reviewed_at as insight_reviewed_at, pi.checked_at \
            FROM papers p \
            LEFT JOIN paper_insights pi ON p.id = pi.paper_id \
            WHERE p.id NOT IN (SELECT id FROM deleted_papers) \
              AND EXISTS (SELECT 1 FROM paper_tags pt WHERE pt.paper_id = p.id) \
              AND NOT EXISTS ( \
                  SELECT 1 FROM paper_tags pt \
                  JOIN tag_counts tc ON pt.tag = tc.tag \
                  WHERE pt.paper_id = p.id AND tc.paper_cnt > 1 \
              ) \
            ORDER BY p.id",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut papers = Vec::with_capacity(rows.len());
        for row in &rows {
            papers.push(Self::map_paper_row(row)?);
        }
        Ok(papers)
    }

    pub async fn list_uninteresting_papers(&self) -> Result<Vec<DbPaper>> {
        let disinterest_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM user_tag_preferences WHERE user_id = 1 AND preference_type = 'disinterest'",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);

        if disinterest_count == 0 {
            return Ok(Vec::new());
        }

        let rows = sqlx::query(
            "WITH disinterest_tags AS ( \
                SELECT tag FROM user_tag_preferences WHERE user_id = 1 AND preference_type = 'disinterest' \
            ), \
            interest_tags AS ( \
                SELECT tag FROM user_tag_preferences WHERE user_id = 1 AND preference_type = 'interest' \
            ), \
            has_disinterest AS ( \
                SELECT DISTINCT p.id \
                FROM papers p \
                JOIN paper_tags pt ON p.id = pt.paper_id \
                JOIN disinterest_tags dt ON pt.tag LIKE '%' || dt.tag || '%' \
                WHERE p.id NOT IN (SELECT id FROM deleted_papers) \
            ), \
            has_interest AS ( \
                SELECT DISTINCT p.id \
                FROM papers p \
                JOIN paper_tags pt ON p.id = pt.paper_id \
                JOIN interest_tags it ON pt.tag LIKE '%' || it.tag || '%' \
                WHERE p.id NOT IN (SELECT id FROM deleted_papers) \
            ) \
            SELECT p.id, p.title, p.authors, p.published, p.score, p.paper_type, p.summary, p.abstract, p.processed_at, \
                   pi.insight, pi.processed_at as insight_processed_at, pi.review as insight_review, pi.reviewed_at as insight_reviewed_at, pi.checked_at \
            FROM papers p \
            LEFT JOIN paper_insights pi ON p.id = pi.paper_id \
            WHERE p.id IN (SELECT id FROM has_disinterest) \
              AND p.id NOT IN (SELECT id FROM has_interest) \
              AND p.id NOT IN (SELECT id FROM deleted_papers) \
            ORDER BY p.id",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut papers = Vec::with_capacity(rows.len());
        for row in &rows {
            papers.push(Self::map_paper_row(row)?);
        }
        Ok(papers)
    }

    pub async fn list_disinterest_tags_with_counts(&self) -> Result<Vec<(String, usize)>> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "WITH disinterest_tags AS ( \
                SELECT tag FROM user_tag_preferences WHERE user_id = 1 AND preference_type = 'disinterest' \
            ), \
            interest_tags AS ( \
                SELECT tag FROM user_tag_preferences WHERE user_id = 1 AND preference_type = 'interest' \
            ), \
            has_interest AS ( \
                SELECT DISTINCT p.id \
                FROM papers p \
                JOIN paper_tags pt ON p.id = pt.paper_id \
                JOIN interest_tags it ON pt.tag LIKE '%' || it.tag || '%' \
                WHERE p.id NOT IN (SELECT id FROM deleted_papers) \
            ) \
            SELECT dt.tag, COUNT(DISTINCT pt.paper_id) as cnt \
            FROM disinterest_tags dt \
            JOIN paper_tags pt ON pt.tag LIKE '%' || dt.tag || '%' \
            JOIN papers p ON pt.paper_id = p.id \
            WHERE p.id NOT IN (SELECT id FROM deleted_papers) \
              AND p.id NOT IN (SELECT id FROM has_interest) \
            GROUP BY dt.tag \
            ORDER BY cnt DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(t, c)| (t, c as usize)).collect())
    }

    pub async fn list_uninteresting_papers_by_tags(
        &self,
        selected_tags: &[String],
    ) -> Result<Vec<DbPaper>> {
        if selected_tags.is_empty() {
            return Ok(Vec::new());
        }

        let mut builder = sqlx::QueryBuilder::new(
            "WITH interest_tags AS ( \
                SELECT tag FROM user_tag_preferences WHERE user_id = 1 AND preference_type = 'interest' \
            ), \
            has_interest AS ( \
                SELECT DISTINCT p.id \
                FROM papers p \
                JOIN paper_tags pt ON p.id = pt.paper_id \
                JOIN interest_tags it ON pt.tag LIKE '%' || it.tag || '%' \
                WHERE p.id NOT IN (SELECT id FROM deleted_papers) \
            ) \
            SELECT DISTINCT p.id, p.title, p.authors, p.published, p.score, p.paper_type, p.summary, p.abstract, p.processed_at, \
                   pi.insight, pi.processed_at as insight_processed_at, pi.review as insight_review, pi.reviewed_at as insight_reviewed_at, pi.checked_at \
            FROM papers p \
            LEFT JOIN paper_insights pi ON p.id = pi.paper_id \
            JOIN paper_tags pt ON p.id = pt.paper_id \
            WHERE (",
        );

        for (i, tag) in selected_tags.iter().enumerate() {
            if i > 0 {
                builder.push(" OR ");
            }
            let pattern = format!("%{}%", tag);
            builder.push("pt.tag ILIKE ");
            builder.push_bind(pattern);
        }

        builder.push(
            ") AND p.id NOT IN (SELECT id FROM deleted_papers) \
            AND p.id NOT IN (SELECT id FROM has_interest) \
            ORDER BY p.id",
        );

        let rows = builder.build().fetch_all(&self.pool).await?;
        let mut papers = Vec::with_capacity(rows.len());
        for row in &rows {
            papers.push(Self::map_paper_row(row)?);
        }
        Ok(papers)
    }
}
