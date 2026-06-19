// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::Result;
use sqlx::Row;
use std::collections::BTreeMap;

use super::Db;
use crate::db::types::{DateTreeNode, PaperByDate};

impl Db {
    pub async fn list_paper_dates_tree(&self) -> Result<Vec<DateTreeNode>> {
        let rows = sqlx::query(
            "SELECT published::date::text as date, COUNT(*) as cnt \
             FROM papers \
             WHERE id NOT IN (SELECT id FROM deleted_papers) \
             GROUP BY date \
             ORDER BY date DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut tree: BTreeMap<String, BTreeMap<String, Vec<(String, usize)>>> = BTreeMap::new();
        for row in rows {
            let date: String = row.try_get("date")?;
            let count: i64 = row.try_get("cnt")?;
            let year = date[..4].to_string();
            let month = date[5..7].to_string();
            let day = date[8..10].to_string();
            tree.entry(year)
                .or_default()
                .entry(month)
                .or_default()
                .push((day, count as usize));
        }

        let mut result = Vec::new();
        for (year, months_map) in tree.into_iter().rev() {
            let mut month_nodes = Vec::new();
            for (month, days) in months_map.into_iter().rev() {
                let day_nodes: Vec<DateTreeNode> = days
                    .into_iter()
                    .rev()
                    .map(|(day, count)| DateTreeNode {
                        label: day,
                        count,
                        children: vec![],
                    })
                    .collect();
                let total: usize = day_nodes.iter().map(|n| n.count).sum();
                month_nodes.push(DateTreeNode {
                    label: month,
                    count: total,
                    children: day_nodes,
                });
            }
            let total: usize = month_nodes.iter().map(|n| n.count).sum();
            result.push(DateTreeNode {
                label: year,
                count: total,
                children: month_nodes,
            });
        }
        Ok(result)
    }

    pub async fn list_papers_by_date(&self, date_prefix: &str) -> Result<Vec<PaperByDate>> {
        let pattern = format!("{}%", date_prefix);
        let rows = sqlx::query(
            "SELECT p.id, p.title, p.authors, p.abstract_zh, p.score, pm.mark, p.source_type \
             FROM papers p \
             LEFT JOIN paper_marks pm ON p.id = pm.paper_id \
             WHERE p.published::date::text LIKE $1 \
               AND p.id NOT IN (SELECT id FROM deleted_papers) \
             ORDER BY p.score DESC \
             LIMIT 5000",
        )
        .bind(&pattern)
        .fetch_all(&self.pool)
        .await?;

        let mut result = Vec::with_capacity(rows.len());
        let mut ids = Vec::with_capacity(rows.len());
        for row in &rows {
            let authors_json: sqlx::types::Json<Vec<String>> = row.try_get("authors")?;
            let id: String = row.try_get("id")?;
            ids.push(id.clone());
            result.push(PaperByDate {
                id,
                title: row.try_get("title")?,
                authors: authors_json.0,
                abstract_zh: row.try_get("abstract_zh")?,
                score: row.try_get("score")?,
                mark: row.try_get("mark").ok(),
                source_type: row.try_get("source_type").ok(),
                tags: Vec::new(),
            });
        }

        let mut tags_map = self.list_papers_tags(&ids).await?;
        for paper in result.iter_mut() {
            if let Some(tags) = tags_map.remove(&paper.id) {
                paper.tags = tags;
            }
        }
        Ok(result)
    }
}
