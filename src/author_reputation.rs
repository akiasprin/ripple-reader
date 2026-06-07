// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json;
use std::time::Duration;
use tracing::warn;

const API_TIMEOUT: Duration = Duration::from_secs(15);
const SEMANTIC_SCHOLAR_SEARCH_URL: &str = "https://api.semanticscholar.org/graph/v1/author/search";

#[derive(Debug, Deserialize)]
struct AuthorSearchResponse {
    #[serde(default)]
    data: Vec<AuthorData>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AuthorData {
    pub author_id: String,
    pub name: String,
    #[serde(default)]
    pub paper_count: Option<i64>,
    #[serde(default)]
    pub citation_count: Option<i64>,
    #[serde(default)]
    pub h_index: Option<i64>,
}

pub async fn fetch_author_semantic_scholar(name: &str) -> Result<Option<AuthorData>> {
    let client = reqwest::Client::builder()
        .timeout(API_TIMEOUT)
        .build()
        .context("Failed to build HTTP client")?;

    let api_key = std::env::var("SEMANTIC_SCHOLAR_API_KEY").ok();
    let url = format!(
        "{}?query={}&fields=name,hIndex,citationCount,paperCount&limit=3",
        SEMANTIC_SCHOLAR_SEARCH_URL,
        urlencoding::encode(name)
    );

    let mut last_error = None;
    for attempt in 0..3 {
        let mut req = client.get(&url);
        if let Some(ref key) = &api_key {
            req = req.header("x-api-key", key);
        }
        let resp = req
            .send()
            .await
            .context("Semantic Scholar API request failed")?;

        let status = resp.status();
        if status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            let result: AuthorSearchResponse = match serde_json::from_str(&body_text) {
                Ok(r) => r,
                Err(e) => {
                    warn!(
                        "[semantic_scholar] Failed to parse response for '{}': {}. body={}",
                        name, e, body_text
                    );
                    return Ok(None);
                }
            };
            let normalized = normalize_name(name);
            let best = result
                .data
                .into_iter()
                .find(|a| normalize_name(&a.name) == normalized);
            return Ok(best);
        }

        let body = resp.text().await.unwrap_or_default();
        warn!(
            "[semantic_scholar] API error for '{}': status={}, body={} (attempt {}/3)",
            name,
            status,
            body,
            attempt + 1
        );

        // Retry with exponential backoff on rate limit (429) or server errors (5xx)
        if status.as_u16() == 429 || status.is_server_error() {
            // Start at 5s for 429, up to 20s on final retry
            let backoff_secs = if status.as_u16() == 429 {
                5u64 * (attempt + 1) as u64
            } else {
                2u64.pow(attempt)
            };
            let backoff = Duration::from_secs(backoff_secs);
            warn!(
                "[semantic_scholar] Retrying {} after {:?}...",
                name, backoff
            );
            tokio::time::sleep(backoff).await;
            last_error = Some(status);
            continue;
        }

        // Client errors (4xx except 429) are not retriable
        return Ok(None);
    }

    warn!(
        "[semantic_scholar] All retries exhausted for '{}', last status={:?}",
        name, last_error
    );
    Ok(None)
}

fn normalize_name(s: &str) -> String {
    s.to_lowercase()
        .replace(|c: char| !c.is_alphabetic() && c != ' ', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Compute author reputation boost for a paper.
/// Only considers the first 3 authors plus the last author (treated as corresponding author).
/// Returns a boost in range [-1.5, 5.0].
pub fn compute_author_boost(
    first_three: &[(String, crate::db::AuthorStats)],
    corresponding_author: Option<&(String, crate::db::AuthorStats)>,
) -> f32 {
    let mut total_boost = 0.0f32;

    for (order, (_name, stats)) in first_three.iter().enumerate().take(3) {
        let order_weight = match order {
            0 => 4.0, // first
            1 => 3.0, // second
            _ => 2.0, // third
        };

        let own_quality = if stats.paper_count > 0 {
            let avg_score_boost = (stats.avg_score - 5.0) * 0.10;
            let critical_boost = (stats.critical_count as f32).min(5.0) * 0.1;
            let volume_boost = (stats.paper_count as f32).ln_1p() * 0.10;
            avg_score_boost + critical_boost + volume_boost
        } else {
            0.0
        };

        let external_quality = match (stats.h_index, stats.citation_count) {
            (Some(h), _) if h >= 50 => 0.5,
            (Some(h), _) if h >= 20 => 0.3,
            (Some(h), _) if h >= 10 => 0.15,
            (None, Some(c)) if c >= 10000 => 0.4,
            (None, Some(c)) if c >= 1000 => 0.2,
            _ => 0.0,
        };

        let author_score = own_quality + external_quality;
        total_boost += author_score * order_weight;
    }

    // Corresponding author (last author) gets a fixed weight of 2.5
    if let Some((_name, stats)) = corresponding_author {
        let order_weight = 2.5;

        let own_quality = if stats.paper_count > 0 {
            let avg_score_boost = (stats.avg_score - 5.0) * 0.10;
            let critical_boost = (stats.critical_count as f32).min(5.0) * 0.1;
            let volume_boost = (stats.paper_count as f32).ln_1p() * 0.10;
            avg_score_boost + critical_boost + volume_boost
        } else {
            0.0
        };

        let external_quality = match (stats.h_index, stats.citation_count) {
            (Some(h), _) if h >= 50 => 0.5,
            (Some(h), _) if h >= 20 => 0.3,
            (Some(h), _) if h >= 10 => 0.15,
            (None, Some(c)) if c >= 10000 => 0.4,
            (None, Some(c)) if c >= 1000 => 0.2,
            _ => 0.0,
        };

        let author_score = own_quality + external_quality;
        total_boost += author_score * order_weight;
    }

    total_boost.clamp(-1.5, 5.0)
}
