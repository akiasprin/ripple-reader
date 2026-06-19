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
    pub abstract_zh: String,
    pub abstract_en: String,
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
    pub abstract_zh: String,
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
    pub abstract_zh: String,
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
    pub abstract_zh: Option<String>,
    pub abstract_en: Option<String>,
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

/// Normalize text without converting quotes.
///
/// Performs whitespace cleanup (collapses three-or-more newlines to two),
/// inserts a space between `)` and a following CJK character, and adds
/// separating spaces between CJK and ASCII/alphanumeric runs. Leaves all
/// quote characters untouched.
///
/// This is the default normalizer for most fields: titles, paper types,
/// insights, reviews, revisions, etc.
pub fn normalize_text(s: &str) -> String {
    normalize_text_impl(s, false)
}

/// Normalize text and convert English/curly double quotes to CJK corner brackets.
///
/// Use this only for **summary and abstract fields** where the product rule
/// requires quotes to be rendered as `「 」`. All other fields should use the
/// default [`normalize_text`].
pub fn normalize_text_convert_quotes(s: &str) -> String {
    normalize_text_impl(s, true)
}

fn normalize_text_impl(s: &str, convert_quotes: bool) -> String {
    let mut s = s.replace("\r\n", "\n");
    while s.contains("\n\n\n") {
        s = s.replace("\n\n\n", "\n\n");
    }
    // Replace English/curly double quotes with Chinese corner brackets 「 」.
    if convert_quotes {
        s = replace_double_quotes(&s);
    }
    // Insert space between ) and following CJK character: "Google (Alphabet)完成了" → "Google (Alphabet) 完成了"
    s = insert_space_after_close_paren(&s);
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

/// Replace English/curly double quotes with Chinese corner brackets 「 」.
///
/// Handles three patterns:
/// - `"..."` — straight ASCII double quotes
/// - `\u{201C}...\u{201D}` — curly left/right double quotes
/// - Unmatched opening quote → `「` at end of run
///
/// Quotes appearing inside HTML tags (`<...>`) are left untouched, since they
/// are attribute delimiters, not body punctuation. Without this guard, a
/// `<div style="text-align:right">` would be mangled into
/// `<div style=「text-align:right」>` whenever a user re-saves an insight
/// that already contains our auto-appended footer.
fn replace_double_quotes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 16);
    let mut open = false;
    let mut in_tag = false;
    for &ch in &chars {
        match ch {
            '<' => {
                in_tag = true;
                out.push(ch);
            }
            '>' if in_tag => {
                in_tag = false;
                out.push(ch);
            }
            // Straight ASCII double quote
            '"' if !in_tag => {
                out.push(if open { '\u{300D}' } else { '\u{300C}' });
                open = !open;
            }
            // Curly left double quote \u{201C}
            '\u{201C}' if !in_tag => {
                out.push('\u{300C}');
                open = true;
            }
            // Curly right double quote \u{201D}
            '\u{201D}' if !in_tag => {
                out.push('\u{300D}');
                open = false;
            }
            _ => out.push(ch),
        }
    }
    out
}

/// Insert a space between `)` and a following CJK character.
///
/// Handles: `Google (Alphabet)完成了` → `Google (Alphabet) 完成了`
fn insert_space_after_close_paren(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 16);
    for i in 0..chars.len() {
        out.push(chars[i]);
        if chars[i] == ')' && i + 1 < chars.len() && is_cjk(chars[i + 1]) {
            out.push(' ');
        }
    }
    out
}
