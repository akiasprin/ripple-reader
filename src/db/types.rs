// SPDX-License-Identifier: MIT OR Apache-2.0

use chrono::{DateTime, Utc};

pub type InsightBackupRow = (
    i64,
    String,
    Option<DateTime<Utc>>,
    String,
    Option<DateTime<Utc>>,
    DateTime<Utc>,
);

#[derive(Debug, Clone)]
pub struct DbPaper {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub published: DateTime<Utc>,
    pub score: f32,
    pub paper_type: String,
    pub summary: String,
    pub r#abstract: String,
    pub insight: String,
    pub processed_at: DateTime<Utc>,
    pub insight_processed_at: Option<DateTime<Utc>>,
    pub insight_review: String,
    pub insight_reviewed_at: Option<DateTime<Utc>>,
    pub checked_at: Option<DateTime<Utc>>,
    pub source_type: String,
    pub source_url: Option<String>,
    pub external_id: Option<String>,
}

/// Lightweight paper struct for list queries.
/// Does not carry full insight/review text; uses boolean flags instead.
#[derive(Debug, Clone)]
pub struct DbPaperListItem {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub published: DateTime<Utc>,
    pub score: f32,
    pub paper_type: String,
    pub summary: String,
    pub r#abstract: String,
    pub processed_at: DateTime<Utc>,
    pub insight_processed_at: Option<DateTime<Utc>>,
    pub insight_reviewed_at: Option<DateTime<Utc>>,
    pub checked_at: Option<DateTime<Utc>>,
    pub source_type: String,
    pub source_url: Option<String>,
    pub external_id: Option<String>,
    pub is_analyzing: bool,
    pub is_reviewing: bool,
    pub has_insight: bool,
    pub has_review: bool,
}

#[derive(Debug, Clone)]
pub struct AuthorStats {
    pub paper_count: i64,
    pub avg_score: f32,
    pub critical_count: i64,
    pub citation_count: Option<i64>,
    pub h_index: Option<i64>,
}

#[derive(Debug)]
pub struct TagsDashboard {
    pub tags: Vec<(String, usize)>,
    pub orphan_count: usize,
    pub uninteresting_count: usize,
    pub disinterest_tags: Vec<(String, usize)>,
    pub interest_tags: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct PaperComment {
    pub id: i64,
    pub paper_id: String,
    pub quote: String,
    pub before_ctx: String,
    pub after_ctx: String,
    pub comment: String,
    pub ai_reply: String,
    pub ai_old_text: String,
    pub ai_new_text: String,
    pub ai_status: String,
    pub is_ai: bool,
    pub parent_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct DateTreeNode {
    pub label: String,
    pub count: usize,
    pub children: Vec<DateTreeNode>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct PaperByDate {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub r#abstract: String,
    pub score: f32,
    pub mark: Option<String>,
    pub source_type: Option<String>,
    pub tags: Vec<(String, f32)>,
}

#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
pub struct DbPaperUpdate {
    pub title: Option<String>,
    pub score: Option<f32>,
    pub paper_type: Option<String>,
    pub summary: Option<String>,
    pub r#abstract: Option<String>,
    pub insight: Option<String>,
    pub insight_processed_at: Option<DateTime<Utc>>,
    pub insight_review: Option<String>,
    pub insight_reviewed_at: Option<DateTime<Utc>>,
    pub checked_at: Option<DateTime<Utc>>,
    /// When true, explicitly set `checked_at = NULL` in the DB.
    /// Needed because `update_paper` skips `None` values, so toggling
    /// checked off (None) would otherwise be a no-op.
    #[serde(default)]
    pub clear_checked_at: bool,
}

/// Normalize insight heading: if it starts with a markdown H1 "# ", convert to blockquote "> "
pub fn normalize_insight_heading(s: &str) -> String {
    let trimmed = s.trim_start();
    if let Some(stripped) = trimmed.strip_prefix("# ") {
        format!("> {}", stripped)
    } else {
        s.to_string()
    }
}

/// Check whether an insight contains valid content (not an error placeholder).
pub fn is_valid_insight(insight: &str) -> bool {
    !insight.is_empty()
        && insight != "__ANALYZING__"
        && !insight.starts_with("Figure extraction failed")
        && !insight.starts_with("Insight failed")
        && !insight.starts_with("MinerU API not configured")
}

pub fn cleanup_text(s: &str) -> String {
    let mut s = s.replace("\r\n", "\n");
    while s.contains("\n\n\n") {
        s = s.replace("\n\n\n", "\n\n");
    }
    let s = s.trim();
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() * 2);
    for i in 0..chars.len() {
        out.push(chars[i]);
        if i + 1 < chars.len() {
            let curr = chars[i];
            let next = chars[i + 1];
            let curr_cjk = is_cjk(curr);
            let next_cjk = is_cjk(next);
            let needs_space = (curr_cjk && next.is_ascii_alphanumeric())
                || (curr.is_ascii_alphanumeric() && next_cjk);
            if needs_space && !out.ends_with(' ') && next != ' ' {
                out.push(' ');
            }
        }
    }
    out
}

pub(super) fn is_cjk(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c)
}
