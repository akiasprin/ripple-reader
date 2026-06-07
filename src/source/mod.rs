// SPDX-License-Identifier: MIT OR Apache-2.0

pub mod arxiv;
pub mod openreview;

/// A paper fetched from any source (arXiv, OpenReview, etc.).
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct Paper {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    #[allow(dead_code)]
    pub summary: String,
    pub pdf_url: String,
    pub published: String,
    #[serde(default)]
    pub source_type: Option<String>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub external_id: Option<String>,
}
