// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::{Context, Result};
use serde::Deserialize;
use tracing::{info, warn};

use super::Paper;

#[derive(Debug, Deserialize)]
struct OrNote {
    id: String,
    #[serde(default)]
    forum: Option<String>,
    #[serde(default)]
    content: OrContent,
    #[serde(default)]
    tcdate: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct OrContent {
    #[serde(default)]
    title: Option<serde_json::Value>,
    #[serde(default)]
    authors: Option<serde_json::Value>,
    #[serde(default)]
    pdf: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct OrResponse {
    #[serde(default)]
    notes: Vec<OrNote>,
}

/// Fetch papers from OpenReview API for a given venue.
///
/// `venue` should be something like `NeurIPS.cc/2024/Conference`.
/// Returns arxiv::Paper structs with bare forum IDs and `source_type: "openreview"`.
pub async fn fetch_papers(venue: &str, year: i32) -> Result<Vec<Paper>> {
    let invitation = format!("{}/-/Submission", venue);
    let url = format!(
        "https://api2.openreview.net/notes?invitation={}",
        urlencoding::encode(&invitation)
    );

    info!("[openreview] Fetching {}", url);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .context("Failed to build HTTP client")?;

    let resp: OrResponse = client
        .get(&url)
        .send()
        .await
        .context("OpenReview API request failed")?
        .json()
        .await
        .context("Failed to parse OpenReview response")?;

    let mut papers = Vec::new();
    for note in resp.notes {
        let forum = note.forum.as_ref().unwrap_or(&note.id).clone();

        let title = extract_string(&note.content.title).unwrap_or_default();
        if title.is_empty() {
            warn!("[openreview] Skipping note {} with empty title", forum);
            continue;
        }

        let authors = extract_string_list(&note.content.authors);

        let pdf_url = extract_string(&note.content.pdf)
            .map(|p| {
                if p.starts_with("http") {
                    p
                } else {
                    format!("https://openreview.net{}", p)
                }
            })
            .unwrap_or_else(|| format!("https://openreview.net/pdf?id={}", forum));

        let published = note.tcdate.map_or_else(
            || format!("{}-01-01T00:00:00Z", year),
            |ts| {
                let dt =
                    chrono::DateTime::from_timestamp_millis(ts).unwrap_or_else(chrono::Utc::now);
                dt.to_rfc3339()
            },
        );

        papers.push(Paper {
            id: forum.clone(),
            title,
            authors,
            summary: String::new(),
            pdf_url,
            published,
            source_type: Some("openreview".to_string()),
            source_url: Some(format!("https://openreview.net/forum?id={}", forum)),
            external_id: Some(forum),
        });
    }

    info!(
        "[openreview] Fetched {} papers for {} {}",
        papers.len(),
        venue,
        year
    );
    Ok(papers)
}

fn extract_string(v: &Option<serde_json::Value>) -> Option<String> {
    match v {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Object(map)) => map
            .get("value")
            .and_then(|v| v.as_str().map(|s| s.to_string())),
        _ => None,
    }
}

fn extract_string_list(v: &Option<serde_json::Value>) -> Vec<String> {
    match v {
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        Some(serde_json::Value::Object(map)) => map
            .get("value")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Fetch a single paper from OpenReview by forum id.
/// Returns an arxiv::Paper with bare forum id and `source_type: "openreview"`.
pub async fn fetch_paper_by_forum(forum: &str) -> Result<Paper> {
    let url = format!("https://api2.openreview.net/notes?id={}", forum);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to build HTTP client")?;

    let resp: OrResponse = client
        .get(&url)
        .send()
        .await
        .context("OpenReview API request failed")?
        .json()
        .await
        .context("Failed to parse OpenReview response")?;

    let note = resp
        .notes
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No note found for forum {}", forum))?;

    let forum_id = note.forum.as_ref().unwrap_or(&note.id).clone();
    let title = extract_string(&note.content.title).unwrap_or_default();
    if title.is_empty() {
        anyhow::bail!("Paper {} has empty title", forum);
    }

    let authors = extract_string_list(&note.content.authors);
    let pdf_url = extract_string(&note.content.pdf)
        .map(|p| {
            if p.starts_with("http") {
                p
            } else {
                format!("https://openreview.net{}", p)
            }
        })
        .unwrap_or_else(|| format!("https://openreview.net/pdf?id={}", forum_id));

    let published = note.tcdate.map_or_else(
        || chrono::Utc::now().to_rfc3339(),
        |ts| {
            chrono::DateTime::from_timestamp_millis(ts)
                .unwrap_or_else(chrono::Utc::now)
                .to_rfc3339()
        },
    );

    Ok(Paper {
        id: forum_id.clone(),
        title,
        authors,
        summary: String::new(),
        pdf_url,
        published,
        source_type: Some("openreview".to_string()),
        source_url: Some(format!("https://openreview.net/forum?id={}", forum_id)),
        external_id: Some(forum_id),
    })
}
