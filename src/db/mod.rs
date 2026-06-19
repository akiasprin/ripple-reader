// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::{Context, Result};
use sqlx::{ConnectOptions, PgPool, Row};
use std::str::FromStr;
use std::time::Duration;

mod authors;
mod cache;
mod comments;
mod dates;
mod deleted;
mod insights;
mod marks;
mod papers;
mod tags;
pub mod types;
mod users;

pub use types::{
    is_valid_insight, normalize_insight_heading, normalize_text, normalize_text_convert_quotes,
    AuthorStats, DateTreeNode, DbPaper, DbPaperListItem, DbPaperUpdate, InsightBackupRow,
    PaperByDate, PaperComment, TagsDashboard,
};

#[derive(Clone)]
pub struct Db {
    pub(crate) pool: PgPool,
}

impl Db {
    pub async fn new(database_url: &str) -> Result<Self> {
        let slow_ms: u64 = std::env::var("DB_SLOW_QUERY_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(100);
        let max_conns: u32 = std::env::var("DB_MAX_CONNECTIONS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(50);
        let opts = sqlx::postgres::PgConnectOptions::from_str(database_url)
            .context("Failed to parse DATABASE_URL")?
            .log_statements(log::LevelFilter::Off)
            .log_slow_statements(log::LevelFilter::Info, Duration::from_millis(slow_ms));
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(max_conns)
            .connect_with(opts)
            .await
            .context("Failed to connect to PostgreSQL")?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("Failed to run migrations")?;
        Ok(Self { pool })
    }

    pub(crate) fn map_paper_row(row: &sqlx::postgres::PgRow) -> Result<DbPaper, sqlx::Error> {
        let authors_json: sqlx::types::Json<Vec<String>> = row.try_get("authors")?;
        Ok(DbPaper {
            id: row.try_get("id")?,
            title: row.try_get("title")?,
            authors: authors_json.0,
            published: row.try_get("published")?,
            score: row.try_get("score")?,
            paper_type: row.try_get("paper_type")?,
            summary: row.try_get("summary")?,
            abstract_zh: row.try_get("abstract_zh")?,
            abstract_en: row.try_get("abstract_en").unwrap_or_default(),
            processed_at: row.try_get("processed_at")?,
            insight: row.try_get("insight").unwrap_or_default(),
            insight_processed_at: row.try_get("insight_processed_at").ok(),
            insight_review: row.try_get("insight_review").unwrap_or_default(),
            insight_reviewed_at: row.try_get("insight_reviewed_at").ok(),
            checked_at: row.try_get("checked_at").ok(),
            source_type: row
                .try_get("source_type")
                .unwrap_or_else(|_| "arxiv".to_string()),
            source_url: row.try_get("source_url").ok(),
            external_id: row.try_get("external_id").ok(),
        })
    }

    pub(crate) fn map_paper_list_row(
        row: &sqlx::postgres::PgRow,
    ) -> Result<types::DbPaperListItem, sqlx::Error> {
        use types::DbPaperListItem;
        let authors_json: sqlx::types::Json<Vec<String>> = row.try_get("authors")?;
        Ok(DbPaperListItem {
            id: row.try_get("id")?,
            title: row.try_get("title")?,
            authors: authors_json.0,
            published: row.try_get("published")?,
            score: row.try_get("score")?,
            paper_type: row.try_get("paper_type")?,
            summary: row.try_get("summary")?,
            abstract_zh: row.try_get("abstract_zh")?,
            processed_at: row.try_get("processed_at")?,
            insight_processed_at: row.try_get("insight_processed_at").ok(),
            insight_reviewed_at: row.try_get("insight_reviewed_at").ok(),
            checked_at: row.try_get("checked_at").ok(),
            source_type: row
                .try_get("source_type")
                .unwrap_or_else(|_| "arxiv".to_string()),
            source_url: row.try_get("source_url").ok(),
            external_id: row.try_get("external_id").ok(),
            is_analyzing: row.try_get("is_analyzing").unwrap_or(false),
            is_reviewing: row.try_get("is_reviewing").unwrap_or(false),
            has_insight: row.try_get("has_insight").unwrap_or(false),
            has_review: row.try_get("has_review").unwrap_or(false),
        })
    }
}
