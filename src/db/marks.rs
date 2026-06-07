// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::{Context, Result};
use std::collections::HashMap;

use super::Db;

const VALID_MARKS: &[&str] = &["critical", "supporting", "marginal"];

impl Db {
    pub async fn get_mark(&self, paper_id: &str) -> Result<Option<String>> {
        let mark: Option<String> =
            sqlx::query_scalar("SELECT mark FROM paper_marks WHERE paper_id = $1")
                .bind(paper_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(mark)
    }

    pub async fn set_mark(&self, paper_id: &str, mark: Option<&str>) -> Result<()> {
        if let Some(m) = mark {
            if !VALID_MARKS.contains(&m) {
                anyhow::bail!(
                    "Invalid mark: {}. Valid marks: critical, supporting, marginal",
                    m
                );
            }
            sqlx::query(
                "INSERT INTO paper_marks (paper_id, mark) VALUES ($1, $2) \
                 ON CONFLICT (paper_id) DO UPDATE SET mark = EXCLUDED.mark",
            )
            .bind(paper_id)
            .bind(m)
            .execute(&self.pool)
            .await
            .context("Failed to set mark")?;
        } else {
            sqlx::query("DELETE FROM paper_marks WHERE paper_id = $1")
                .bind(paper_id)
                .execute(&self.pool)
                .await
                .context("Failed to delete mark")?;
        }
        Ok(())
    }

    pub async fn list_marks(&self, paper_ids: Vec<String>) -> Result<HashMap<String, String>> {
        if paper_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT paper_id, mark FROM paper_marks WHERE paper_id = ANY($1)")
                .bind(&paper_ids)
                .fetch_all(&self.pool)
                .await?;

        let mut map = HashMap::new();
        for (id, mark) in rows {
            map.insert(id, mark);
        }
        Ok(map)
    }
}
