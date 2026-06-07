// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::Result;

use super::Db;

impl Db {
    pub async fn get_insight_html_cache(
        &self,
        paper_id: &str,
    ) -> Result<Option<(String, Vec<crate::web::TocItem>)>> {
        let row = sqlx::query_as::<_, (String, String)>(
            "SELECT html, toc FROM _cache_insight_html WHERE paper_id = $1",
        )
        .bind(paper_id)
        .fetch_optional(&self.pool)
        .await?;

        let (html, toc_str) = match row {
            Some(r) => r,
            None => return Ok(None),
        };
        let toc: Vec<crate::web::TocItem> = serde_json::from_str(&toc_str).unwrap_or_default();
        Ok(Some((html, toc)))
    }

    pub async fn upsert_insight_html_cache(
        &self,
        paper_id: &str,
        html: &str,
        toc: &[crate::web::TocItem],
    ) -> Result<()> {
        let toc_str = serde_json::to_string(toc).unwrap_or_default();
        sqlx::query(
            "INSERT INTO _cache_insight_html (paper_id, html, toc, updated_at) \
             VALUES ($1, $2, $3, NOW()) \
             ON CONFLICT (paper_id) DO UPDATE SET html = $2, toc = $3, updated_at = NOW()",
        )
        .bind(paper_id)
        .bind(html)
        .bind(toc_str)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_insight_html_cache(&self, paper_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM _cache_insight_html WHERE paper_id = $1")
            .bind(paper_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
