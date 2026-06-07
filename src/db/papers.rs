// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::{Context, Result};
use sqlx::Row;

use super::{Db, DbPaper, DbPaperUpdate};
use crate::db::types::normalize_insight_heading;

/// Shared filter logic for `count_papers` and `list_papers_light`.
///
/// Pushes JOIN clauses, WHERE conditions for mark/insight/tag/checked/source/search
/// filters onto the builder. Both count and list queries use identical WHERE clauses,
/// so extracting this avoids duplication and keeps them in sync.
fn push_paper_filters<'a>(
    builder: &mut sqlx::QueryBuilder<'a, sqlx::Postgres>,
    search: &'a Option<String>,
    mark_filter: &'a Option<String>,
    insight_filter: &'a Option<String>,
    tag_filter: &'a Option<String>,
    checked_filter: &'a Option<String>,
    source_type_filter: &'a Option<String>,
) {
    match mark_filter.as_deref() {
        Some("none") => {
            builder.push(" LEFT JOIN paper_marks pm ON p.id = pm.paper_id ");
        }
        Some(_) => {
            builder.push(" JOIN paper_marks pm ON p.id = pm.paper_id ");
        }
        None => {}
    }

    builder.push(" WHERE NOT EXISTS (SELECT 1 FROM deleted_papers dp WHERE dp.id = p.id) ");

    if let Some("none") = mark_filter.as_deref() {
        builder.push(" AND pm.paper_id IS NULL ");
    } else if let Some(m) = mark_filter {
        builder.push(" AND pm.mark = ").push_bind(m);
    }

    match insight_filter.as_deref() {
        Some("has") => {
            builder.push(" AND pi.insight != '' AND pi.insight != '__ANALYZING__' ");
            builder.push(" AND pi.insight NOT LIKE 'Figure extraction failed%' ");
            builder.push(" AND pi.insight NOT LIKE 'Insight failed%' ");
            builder.push(" AND pi.insight NOT LIKE 'MinerU API not configured%' ");
        }
        Some("none") => {
            builder.push(
                " AND (pi.insight IS NULL OR pi.insight = '' OR pi.insight = '__ANALYZING__' ",
            );
            builder.push(" OR pi.insight LIKE 'Figure extraction failed%' ");
            builder.push(" OR pi.insight LIKE 'Insight failed%' ");
            builder.push(" OR pi.insight LIKE 'MinerU API not configured%') ");
        }
        _ => {}
    }

    if let Some(t) = tag_filter {
        builder
            .push(" AND EXISTS (SELECT 1 FROM paper_tags pt WHERE pt.paper_id = p.id AND pt.tag = ")
            .push_bind(t)
            .push(") ");
    }

    match checked_filter.as_deref() {
        Some("checked" | "has") => {
            builder.push(" AND pi.checked_at IS NOT NULL ");
        }
        Some("unchecked" | "none") => {
            builder.push(" AND pi.checked_at IS NULL ");
        }
        _ => {}
    }

    if let Some(st) = source_type_filter {
        builder.push(" AND p.source_type = ").push_bind(st);
    }

    if let Some(s) = search {
        let pattern = format!("%{}%", s);
        builder.push(" AND (p.id ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR p.title ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR p.summary ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR p.abstract ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR p.authors::text ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR pi.insight ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR pi.review ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(
            " OR EXISTS (SELECT 1 FROM paper_tags pt WHERE pt.paper_id = p.id AND pt.tag ILIKE ",
        );
        builder.push_bind(pattern);
        builder.push(")) ");
    }
}

impl Db {
    pub async fn get_paper(&self, id: &str) -> Result<Option<DbPaper>> {
        let row = sqlx::query(
            "SELECT p.id, p.title, p.authors, p.published, p.score, p.paper_type,
                    p.summary, p.abstract, p.processed_at, p.source_type, p.source_url, p.external_id,
                    pi.insight, pi.processed_at as insight_processed_at,
                    pi.review as insight_review, pi.reviewed_at as insight_reviewed_at, pi.checked_at
             FROM papers p
             LEFT JOIN paper_insights pi ON p.id = pi.paper_id
             WHERE p.id = $1 AND NOT EXISTS (SELECT 1 FROM deleted_papers dp WHERE dp.id = p.id)",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(r) => Ok(Some(Self::map_paper_row(&r)?)),
            None => Ok(None),
        }
    }

    pub async fn save_paper(&self, paper: &DbPaper) -> Result<()> {
        let mut tx = self
            .pool
            .begin()
            .await
            .context("Failed to begin transaction")?;

        sqlx::query(
            "INSERT INTO papers (id, title, authors, published, score, paper_type, summary, abstract, processed_at, source_type, source_url, external_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
             ON CONFLICT (id) DO UPDATE SET
               title = EXCLUDED.title,
               authors = EXCLUDED.authors,
               published = EXCLUDED.published,
               score = EXCLUDED.score,
               paper_type = EXCLUDED.paper_type,
               summary = EXCLUDED.summary,
               abstract = EXCLUDED.abstract,
               processed_at = EXCLUDED.processed_at,
               source_type = EXCLUDED.source_type,
               source_url = EXCLUDED.source_url,
               external_id = EXCLUDED.external_id",
        )
        .bind(&paper.id)
        .bind(&paper.title)
        .bind(sqlx::types::Json(&paper.authors))
        .bind(paper.published)
        .bind(paper.score)
        .bind(&paper.paper_type)
        .bind(&paper.summary)
        .bind(&paper.r#abstract)
        .bind(paper.processed_at)
        .bind(&paper.source_type)
        .bind(&paper.source_url)
        .bind(&paper.external_id)
        .execute(&mut *tx)
        .await
        .context("Failed to save paper")?;

        let has_insight_data = !paper.insight.is_empty()
            || !paper.insight_review.is_empty()
            || paper.insight_processed_at.is_some()
            || paper.insight_reviewed_at.is_some()
            || paper.checked_at.is_some();

        if has_insight_data {
            sqlx::query(
                "INSERT INTO paper_insights (paper_id, insight, processed_at, review, reviewed_at, checked_at)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (paper_id) DO UPDATE SET
                   insight = EXCLUDED.insight,
                   processed_at = EXCLUDED.processed_at,
                   review = EXCLUDED.review,
                   reviewed_at = EXCLUDED.reviewed_at,
                   checked_at = EXCLUDED.checked_at",
            )
            .bind(&paper.id)
            .bind(&paper.insight)
            .bind(paper.insight_processed_at)
            .bind(&paper.insight_review)
            .bind(paper.insight_reviewed_at)
            .bind(paper.checked_at)
            .execute(&mut *tx)
            .await
            .context("Failed to save paper insight")?;
        }

        tx.commit().await.context("Failed to commit transaction")?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn list_papers(
        &self,
        limit: usize,
        offset: usize,
        search: Option<String>,
        mark_filter: Option<String>,
        insight_filter: Option<String>,
        tag_filter: Option<String>,
        checked_filter: Option<String>,
        sort: Option<String>,
        order: Option<String>,
        source_type_filter: Option<String>,
    ) -> Result<Vec<DbPaper>> {
        let mut builder: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
            "SELECT p.id, p.title, p.authors, p.published, p.score, p.paper_type, \
             p.summary, p.abstract, p.processed_at, p.source_type, p.source_url, p.external_id, \
             pi.insight, pi.processed_at as insight_processed_at, \
             pi.review as insight_review, pi.reviewed_at as insight_reviewed_at, pi.checked_at \
             FROM papers p \
             LEFT JOIN paper_insights pi ON p.id = pi.paper_id",
        );

        match mark_filter.as_deref() {
            Some("none") => {
                builder.push(" LEFT JOIN paper_marks pm ON p.id = pm.paper_id ");
            }
            Some(_) => {
                builder.push(" JOIN paper_marks pm ON p.id = pm.paper_id ");
            }
            None => {}
        }

        builder.push(" WHERE NOT EXISTS (SELECT 1 FROM deleted_papers dp WHERE dp.id = p.id) ");

        if let Some("none") = mark_filter.as_deref() {
            builder.push(" AND pm.paper_id IS NULL ");
        } else if let Some(m) = &mark_filter {
            builder.push(" AND pm.mark = ").push_bind(m);
        }

        match insight_filter.as_deref() {
            Some("has") => {
                builder.push(" AND pi.insight != '' AND pi.insight != '__ANALYZING__' ");
                builder.push(" AND pi.insight NOT LIKE 'Figure extraction failed%' ");
                builder.push(" AND pi.insight NOT LIKE 'Insight failed%' ");
                builder.push(" AND pi.insight NOT LIKE 'MinerU API not configured%' ");
            }
            Some("none") => {
                builder.push(
                    " AND (pi.insight IS NULL OR pi.insight = '' OR pi.insight = '__ANALYZING__' ",
                );
                builder.push(" OR pi.insight LIKE 'Figure extraction failed%' ");
                builder.push(" OR pi.insight LIKE 'Insight failed%' ");
                builder.push(" OR pi.insight LIKE 'MinerU API not configured%') ");
            }
            _ => {}
        }

        if let Some(t) = &tag_filter {
            builder.push(" AND EXISTS (SELECT 1 FROM paper_tags pt WHERE pt.paper_id = p.id AND pt.tag = ").push_bind(t).push(") ");
        }

        match checked_filter.as_deref() {
            Some("checked") => {
                builder.push(" AND pi.checked_at IS NOT NULL ");
            }
            Some("unchecked") => {
                builder.push(" AND pi.checked_at IS NULL ");
            }
            _ => {}
        }

        if let Some(st) = &source_type_filter {
            builder.push(" AND p.source_type = ").push_bind(st);
        }

        if let Some(s) = &search {
            let pattern = format!("%{}%", s);
            builder.push(" AND (p.id ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(" OR p.title ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(" OR p.summary ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(" OR p.abstract ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(" OR p.authors::text ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(" OR pi.insight ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(" OR pi.review ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(" OR EXISTS (SELECT 1 FROM paper_tags pt WHERE pt.paper_id = p.id AND pt.tag ILIKE ");
            builder.push_bind(pattern);
            builder.push(")) ");
        }

        let sort_col = match sort.as_deref() {
            Some("processed_at") => "p.processed_at",
            Some("score") => "p.score",
            Some("checked") => "pi.checked_at",
            _ => "p.published",
        };
        let sort_order = if order.as_deref() == Some("asc") {
            "ASC"
        } else {
            "DESC"
        };
        builder.push(format!(
            " ORDER BY {} {}, p.id {}",
            sort_col, sort_order, sort_order
        ));
        builder.push(" LIMIT ").push_bind(limit as i64);
        builder.push(" OFFSET ").push_bind(offset as i64);

        let rows = builder.build().fetch_all(&self.pool).await?;
        let mut papers = Vec::with_capacity(rows.len());
        for row in &rows {
            papers.push(Self::map_paper_row(row)?);
        }
        Ok(papers)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn list_papers_light(
        &self,
        limit: usize,
        offset: usize,
        search: Option<String>,
        mark_filter: Option<String>,
        insight_filter: Option<String>,
        tag_filter: Option<String>,
        checked_filter: Option<String>,
        sort: Option<String>,
        order: Option<String>,
        source_type_filter: Option<String>,
    ) -> Result<Vec<super::types::DbPaperListItem>> {
        let mut builder: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
            "SELECT p.id, p.title, p.authors, p.published, p.score, p.paper_type, \
             p.summary, p.abstract, p.processed_at, p.source_type, p.source_url, p.external_id, \
             pi.processed_at as insight_processed_at, \
             pi.reviewed_at as insight_reviewed_at, pi.checked_at, \
             pi.insight = '__ANALYZING__' as is_analyzing, \
             pi.review = '__REVIEWING__' as is_reviewing, \
             pi.insight IS NOT NULL AND pi.insight != '' AND pi.insight != '__ANALYZING__' as has_insight, \
             pi.review IS NOT NULL AND pi.review != '' AND pi.review != '__REVIEWING__' as has_review \
             FROM papers p \
             LEFT JOIN paper_insights pi ON p.id = pi.paper_id",
        );

        push_paper_filters(
            &mut builder,
            &search,
            &mark_filter,
            &insight_filter,
            &tag_filter,
            &checked_filter,
            &source_type_filter,
        );

        let sort_col = match sort.as_deref() {
            Some("processed_at") => "p.processed_at",
            Some("score") => "p.score",
            Some("checked") => "pi.checked_at",
            _ => "p.published",
        };
        let sort_order = if order.as_deref() == Some("asc") {
            "ASC"
        } else {
            "DESC"
        };
        builder.push(format!(
            " ORDER BY {} {}, p.id {}",
            sort_col, sort_order, sort_order
        ));
        builder.push(" LIMIT ").push_bind(limit as i64);
        builder.push(" OFFSET ").push_bind(offset as i64);

        let rows = builder.build().fetch_all(&self.pool).await?;
        let mut papers = Vec::with_capacity(rows.len());
        for row in &rows {
            papers.push(Self::map_paper_list_row(row)?);
        }
        Ok(papers)
    }

    pub async fn get_papers_by_ids(&self, ids: &[String]) -> Result<Vec<DbPaper>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT p.id, p.title, p.authors, p.published, p.score, p.paper_type, \
             p.summary, p.abstract, p.processed_at, p.source_type, p.source_url, p.external_id, \
             pi.insight, pi.processed_at as insight_processed_at, \
             pi.review as insight_review, pi.reviewed_at as insight_reviewed_at, pi.checked_at \
             FROM papers p \
             LEFT JOIN paper_insights pi ON p.id = pi.paper_id \
             WHERE p.id = ANY($1) AND p.id NOT IN (SELECT id FROM deleted_papers)",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await?;

        let mut papers = Vec::with_capacity(rows.len());
        for row in &rows {
            papers.push(Self::map_paper_row(row)?);
        }
        Ok(papers)
    }

    pub async fn count_papers(
        &self,
        search: Option<String>,
        mark_filter: Option<String>,
        insight_filter: Option<String>,
        tag_filter: Option<String>,
        checked_filter: Option<String>,
        source_type_filter: Option<String>,
    ) -> Result<usize> {
        let mut builder: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
            "SELECT COUNT(DISTINCT p.id) FROM papers p LEFT JOIN paper_insights pi ON p.id = pi.paper_id",
        );

        push_paper_filters(
            &mut builder,
            &search,
            &mark_filter,
            &insight_filter,
            &tag_filter,
            &checked_filter,
            &source_type_filter,
        );

        let row = builder.build().fetch_one(&self.pool).await?;
        let count: i64 = row.try_get(0)?;
        Ok(count as usize)
    }

    pub async fn list_all_paper_ids(&self) -> Result<Vec<String>> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM papers WHERE id NOT IN (SELECT id FROM deleted_papers) ORDER BY published DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(ids)
    }

    pub async fn list_papers_without_marks(&self) -> Result<Vec<DbPaper>> {
        let rows = sqlx::query(
            "SELECT p.id, p.title, p.authors, p.published, p.score, p.paper_type, \
             p.summary, p.abstract, p.processed_at, p.source_type, p.source_url, p.external_id, \
             pi.insight, pi.processed_at as insight_processed_at, \
             pi.review as insight_review, pi.reviewed_at as insight_reviewed_at, pi.checked_at \
             FROM papers p \
             LEFT JOIN paper_marks pm ON p.id = pm.paper_id \
             LEFT JOIN paper_insights pi ON p.id = pi.paper_id \
             WHERE p.id NOT IN (SELECT id FROM deleted_papers) AND pm.paper_id IS NULL \
             ORDER BY p.published DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut papers = Vec::with_capacity(rows.len());
        for row in &rows {
            papers.push(Self::map_paper_row(row)?);
        }
        Ok(papers)
    }

    pub async fn list_paper_ids_without_tags(&self) -> Result<Vec<String>> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT p.id FROM papers p \
             LEFT JOIN paper_tags pt ON p.id = pt.paper_id \
             WHERE p.id NOT IN (SELECT id FROM deleted_papers) AND pt.paper_id IS NULL \
             ORDER BY p.published DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(ids)
    }

    pub async fn list_analyzing_papers(&self) -> Result<Vec<DbPaper>> {
        let rows = sqlx::query(
            "SELECT p.id, p.title, p.authors, p.published, p.score, p.paper_type, \
             p.summary, p.abstract, p.processed_at, p.source_type, p.source_url, p.external_id, \
             pi.insight, pi.processed_at as insight_processed_at, \
             pi.review as insight_review, pi.reviewed_at as insight_reviewed_at, pi.checked_at \
             FROM papers p \
             LEFT JOIN paper_insights pi ON p.id = pi.paper_id \
             WHERE p.id NOT IN (SELECT id FROM deleted_papers) AND pi.insight = '__ANALYZING__'",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut papers = Vec::with_capacity(rows.len());
        for row in &rows {
            papers.push(Self::map_paper_row(row)?);
        }
        Ok(papers)
    }

    pub async fn list_reviewing_papers(&self) -> Result<Vec<DbPaper>> {
        let rows = sqlx::query(
            "SELECT p.id, p.title, p.authors, p.published, p.score, p.paper_type, \
             p.summary, p.abstract, p.processed_at, p.source_type, p.source_url, p.external_id, \
             pi.insight, pi.processed_at as insight_processed_at, \
             pi.review as insight_review, pi.reviewed_at as insight_reviewed_at, pi.checked_at \
             FROM papers p \
             LEFT JOIN paper_insights pi ON p.id = pi.paper_id \
             WHERE p.id NOT IN (SELECT id FROM deleted_papers) AND pi.review = '__REVIEWING__'",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut papers = Vec::with_capacity(rows.len());
        for row in &rows {
            papers.push(Self::map_paper_row(row)?);
        }
        Ok(papers)
    }

    pub async fn update_paper(&self, id: &str, updates: &DbPaperUpdate) -> Result<bool> {
        let mut paper_builder = sqlx::QueryBuilder::new("UPDATE papers SET ");
        let mut has_paper = false;

        if let Some(title) = &updates.title {
            if has_paper {
                paper_builder.push(", ");
            }
            paper_builder.push("title = ").push_bind(title);
            has_paper = true;
        }
        if let Some(score) = updates.score {
            if has_paper {
                paper_builder.push(", ");
            }
            paper_builder.push("score = ").push_bind(score);
            has_paper = true;
        }
        if let Some(paper_type) = &updates.paper_type {
            if has_paper {
                paper_builder.push(", ");
            }
            paper_builder.push("paper_type = ").push_bind(paper_type);
            has_paper = true;
        }
        if let Some(summary) = &updates.summary {
            if has_paper {
                paper_builder.push(", ");
            }
            paper_builder.push("summary = ").push_bind(summary);
            has_paper = true;
        }
        if let Some(abstract_) = &updates.r#abstract {
            if has_paper {
                paper_builder.push(", ");
            }
            paper_builder.push("abstract = ").push_bind(abstract_);
            has_paper = true;
        }

        let mut n = 0usize;
        if has_paper {
            paper_builder.push(" WHERE id = ").push_bind(id);
            let result = paper_builder.build().execute(&self.pool).await?;
            n = result.rows_affected() as usize;
        }

        let mut insight_builder = sqlx::QueryBuilder::new("UPDATE paper_insights SET ");
        let mut has_insight = false;

        if let Some(insight) = &updates.insight {
            if has_insight {
                insight_builder.push(", ");
            }
            insight_builder
                .push("insight = ")
                .push_bind(normalize_insight_heading(insight));
            has_insight = true;
        }
        if let Some(processed_at) = updates.insight_processed_at {
            if has_insight {
                insight_builder.push(", ");
            }
            insight_builder
                .push("processed_at = ")
                .push_bind(processed_at);
            has_insight = true;
        }
        if let Some(review) = &updates.insight_review {
            if has_insight {
                insight_builder.push(", ");
            }
            insight_builder.push("review = ").push_bind(review);
            has_insight = true;
        }
        if let Some(reviewed_at) = updates.insight_reviewed_at {
            if has_insight {
                insight_builder.push(", ");
            }
            insight_builder
                .push("reviewed_at = ")
                .push_bind(reviewed_at);
            has_insight = true;
        }
        if updates.clear_checked_at {
            if has_insight {
                insight_builder.push(", ");
            }
            insight_builder.push("checked_at = NULL");
            has_insight = true;
        } else if let Some(checked_at) = updates.checked_at {
            if has_insight {
                insight_builder.push(", ");
            }
            insight_builder.push("checked_at = ").push_bind(checked_at);
            has_insight = true;
        }

        if has_insight {
            sqlx::query("INSERT INTO paper_insights (paper_id) VALUES ($1) ON CONFLICT DO NOTHING")
                .bind(id)
                .execute(&self.pool)
                .await?;

            insight_builder.push(" WHERE paper_id = ").push_bind(id);
            let is_content_update = updates.insight.is_some()
                || updates.insight_processed_at.is_some()
                || updates.insight_review.is_some()
                || updates.insight_reviewed_at.is_some();
            if is_content_update && !updates.clear_checked_at {
                insight_builder.push(" AND checked_at IS NULL");
            }
            let result = insight_builder.build().execute(&self.pool).await?;
            n = result.rows_affected() as usize;
        }

        Ok(n > 0)
    }

    pub async fn list_paper_ids_with_insight(&self) -> Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT paper_id FROM paper_insights WHERE insight IS NOT NULL AND insight != '' AND insight != '__ANALYZING__'",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| r.try_get::<String, _>(0).unwrap_or_default())
            .collect())
    }
}
