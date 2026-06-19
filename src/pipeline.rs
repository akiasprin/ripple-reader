// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::author_reputation::compute_author_boost;
use crate::db::Db;
use crate::processor::{Processor, Summary};
use crate::source::Paper;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

pub async fn process_paper_with_cache(
    processor: &Processor,
    db: &Db,
    paper: &Paper,
    pdf_semaphore: &Arc<Semaphore>,
    force_refresh: bool,
    skip_author_stats: bool,
) -> Result<(Summary, String, bool)> {
    // Skip deleted papers
    if db.is_deleted(&paper.id).await.unwrap_or(false) {
        info!(
            "[process_paper_with_cache] Skipping deleted paper {}",
            paper.id
        );
        anyhow::bail!("Paper {} has been deleted", paper.id);
    }

    if !force_refresh {
        if let Some(cached) = db
            .get_paper(&paper.id)
            .await
            .with_context(|| format!("DB lookup failed for {}", paper.id))?
        {
            debug!("[process_paper_with_cache] Cache hit for {}", paper.id);
            let tags = db.get_paper_tags(&paper.id).await.unwrap_or_default();
            let summary = Summary {
                score: cached.score,
                paper_type: cached.paper_type,
                summary: cached.summary,
                tags,
                raw: String::new(),
            };
            return Ok((summary, cached.abstract_zh, false));
        }
    } else {
        info!(
            "[process_paper_with_cache] Force refresh enabled, skipping cache for {}",
            paper.id
        );
    }

    info!(
        "[process_paper_with_cache] Cache miss for {}, processing",
        paper.id
    );
    let (mut summary, abs) = process_paper(processor, paper, pdf_semaphore).await?;

    // Apply author reputation boost
    // Only considers first 3 authors + last author (corresponding author)
    // Unknown authors get a default stats to avoid being penalized
    let default_stats = crate::db::AuthorStats {
        paper_count: 1,
        avg_score: 6.5,
        critical_count: 0,
        citation_count: None,
        h_index: None,
    };

    let mut author_stats = Vec::new();
    for author in paper.authors.iter().take(3) {
        let author = author.trim();
        if author.len() < 2 {
            warn!(
                "[author_boost] Skipping short author name '{}' from paper {}",
                author, paper.id
            );
            continue;
        }
        match db.get_author_stats(author).await {
            Ok(Some(stats)) => author_stats.push((author.to_string(), stats)),
            Ok(None) => {
                info!(
                    "[author_boost] New author '{}', using default stats",
                    author
                );
                author_stats.push((author.to_string(), default_stats.clone()));
            }
            Err(e) => warn!("[author_boost] Failed to get stats for {}: {}", author, e),
        }
    }

    let mut corresponding_stats = None;
    if paper.authors.len() > 3 {
        if let Some(author) = paper.authors.last() {
            let author = author.trim();
            if author.len() >= 2 {
                let already_in_first_three = author_stats.iter().any(|(name, _)| name == author);
                if !already_in_first_three {
                    match db.get_author_stats(author).await {
                        Ok(Some(stats)) => corresponding_stats = Some((author.to_string(), stats)),
                        Ok(None) => {
                            info!(
                                "[author_boost] New corresponding author '{}', using default stats",
                                author
                            );
                            corresponding_stats = Some((author.to_string(), default_stats.clone()));
                        }
                        Err(e) => warn!(
                            "[author_boost] Failed to get stats for corresponding author {}: {}",
                            author, e
                        ),
                    }
                }
            } else {
                warn!(
                    "[author_boost] Skipping short corresponding author name '{}' from paper {}",
                    author, paper.id
                );
            }
        }
    }

    let boost = compute_author_boost(&author_stats, corresponding_stats.as_ref());
    let original_score = summary.score;
    summary.score = (summary.score + boost).clamp(0.0, 10.0);
    info!(
        "[process_paper_with_cache] Author boost for {}: {:.2} ({} authors, {}->{:.1})",
        paper.id,
        boost,
        author_stats.len(),
        original_score,
        summary.score
    );

    // Preserve existing insight fields (single DB read)
    let existing_paper = db.get_paper(&paper.id).await.ok().flatten();
    let existing_insight = existing_paper
        .as_ref()
        .map(|p| p.insight.clone())
        .unwrap_or_default();
    let existing_review = existing_paper
        .as_ref()
        .map(|p| p.insight_review.clone())
        .unwrap_or_default();
    let existing_insight_processed_at =
        existing_paper.as_ref().and_then(|p| p.insight_processed_at);
    let existing_reviewed_at = existing_paper.as_ref().and_then(|p| p.insight_reviewed_at);

    let (source_type, source_url, external_id) = match paper.source_type.as_ref() {
        Some(st) => (
            st.clone(),
            paper.source_url.clone(),
            paper.external_id.clone(),
        ),
        None => (
            "arxiv".to_string(),
            Some(format!("https://arxiv.org/abs/{}", paper.id)),
            Some(paper.id.clone()),
        ),
    };

    let db_paper = crate::db::DbPaper {
        id: paper.id.clone(),
        title: crate::db::normalize_text(&paper.title),
        authors: paper.authors.clone(),
        published: DateTime::parse_from_rfc3339(&paper.published)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        score: summary.score,
        paper_type: crate::db::normalize_text(&summary.paper_type),
        summary: crate::db::normalize_text_convert_quotes(&summary.summary),
        abstract_zh: crate::db::normalize_text_convert_quotes(&abs),
        abstract_en: crate::db::normalize_text_convert_quotes(&paper.summary),
        processed_at: Utc::now(),
        insight: existing_insight,
        insight_processed_at: existing_insight_processed_at,
        insight_review: existing_review,
        insight_reviewed_at: existing_reviewed_at,
        checked_at: None,
        source_type,
        source_url,
        external_id,
    };
    db.save_paper(&db_paper)
        .await
        .with_context(|| format!("DB save failed for {}", paper.id))?;
    if !summary.tags.is_empty() {
        db.save_paper_tags(&paper.id, &summary.tags)
            .await
            .with_context(|| format!("DB save tags failed for {}", paper.id))?;
    }

    // Update author stats after successful processing
    if !skip_author_stats {
        let mark = db.get_mark(&paper.id).await.ok().flatten();
        let is_critical = mark.as_deref() == Some("critical");
        for author in paper.authors.iter().take(20) {
            let author = author.trim();
            if author.len() < 2 {
                warn!(
                    "[author_stats] Skipping short author name '{}' from paper {}",
                    author, paper.id
                );
                continue;
            }
            if let Err(e) = db
                .upsert_author_stats(author, summary.score, is_critical)
                .await
            {
                warn!("[author_stats] Failed to update {}: {}", author, e);
            }
        }
    }

    info!(
        "[process_paper_with_cache] Saved {} to database with {} tags",
        paper.id,
        summary.tags.len()
    );

    Ok((summary, abs, true))
}

pub async fn process_paper(
    processor: &Processor,
    paper: &Paper,
    pdf_semaphore: &Arc<Semaphore>,
) -> Result<(Summary, String)> {
    info!(
        "[process_paper] Starting paper id='{}', title='{}'",
        paper.id, paper.title
    );

    let source = paper
        .source_type
        .as_deref()
        .context("paper missing source_type")?;
    let text_path = format!("text/{}/{}.txt", source, paper.id);

    let text = match tokio::fs::read_to_string(&text_path).await {
        Ok(cached) if cached.len() >= 200 => {
            info!(
                "[process_paper] Text cache hit ({} chars) for {}",
                cached.len(),
                paper.id
            );
            cached
        }
        _ => {
            let _permit = pdf_semaphore
                .acquire()
                .await
                .context("Failed to acquire PDF semaphore")?;
            let result = match crate::pdf::download_pdf(source, &paper.id, &paper.pdf_url).await {
                Ok(bytes) => {
                    info!(
                        "[process_paper] PDF downloaded ({} bytes) for {}",
                        bytes.len(),
                        paper.id
                    );
                    let extracted = tokio::task::spawn_blocking({
                        let id = paper.id.clone();
                        move || {
                            info!(
                                "[process_paper] Extracting text from PDF for {} in blocking thread",
                                id
                            );
                            crate::extractor::extract_text(&bytes)
                        }
                    })
                    .await??;
                    info!(
                        "[process_paper] PDF text extracted ({} chars) for {}",
                        extracted.len(),
                        paper.id
                    );
                    if extracted.len() < 200 {
                        warn!(
                            "[process_paper] Skipping {} - PDF appears to be scanned (only {} chars extracted)",
                            paper.id,
                            extracted.len()
                        );
                        anyhow::bail!(
                            "PDF appears to be scanned (only {} chars extracted, need >= 200)",
                            extracted.len()
                        );
                    }
                    if let Some(parent) = std::path::Path::new(&text_path).parent() {
                        let _ = tokio::fs::create_dir_all(parent).await;
                    }
                    let tmp_path = format!("{}.tmp", text_path);
                    tokio::fs::write(&tmp_path, &extracted).await?;
                    tokio::fs::rename(&tmp_path, &text_path).await?;
                    info!("[process_paper] Text cache written to {}", text_path);
                    extracted
                }
                Err(e) => {
                    anyhow::bail!(
                        "[process_paper] PDF download failed for {}: {:#}",
                        paper.id,
                        e
                    );
                }
            };
            drop(_permit);
            info!("[process_paper] PDF semaphore released for {}", paper.id);
            result
        }
    };

    info!("[process_paper] Extracting sections for {}", paper.id);
    let _text_permit = processor
        .text_sem
        .acquire()
        .await
        .context("Failed to acquire text postprocess semaphore")?;
    let sections = tokio::task::spawn_blocking({
        let text = text.clone();
        let id = paper.id.clone();
        move || {
            info!(
                "[process_paper] Running extract_sections for {} in blocking thread",
                id
            );
            crate::extractor::extract_sections(&text)
        }
    })
    .await?;
    let combined_len = sections.intro.len() + sections.conclusion.len();
    info!("[process_paper] Section extraction complete for {} (intro={} chars, conclusion={} chars, header_len={}, combined={})",
        paper.id, sections.intro.len(), sections.conclusion.len(),
        sections.header.len(), combined_len);
    let combined = format!(
        "HEADER (title, **authors**, extract **affiliations** or **institutions**):\n{}\n\nINTRODUCTION/BACKGROUND:\n{}\n\nCONCLUSION:\n{}",
        sections.header, sections.intro, sections.conclusion
    );

    info!("[process_paper] Calling translator for {}", paper.id);
    // Translate first so the Chinese abstract can feed into summarize.
    let abs = processor.translate(&paper.summary).await?;
    info!(
        "[process_paper] Translation done for {} (abstract_len={})",
        paper.id,
        abs.len()
    );

    let summarize_input = format!(
        "{}\n\nREFERENCE STYLE (translated abstract, align expression style with this):\n{}",
        combined, abs
    );
    let summary = processor.summarize(&paper.title, &summarize_input).await?;
    info!(
        "[process_paper] Paper {} completed (score={:.1}/10.0, digest_len={}, abstract_len={})",
        paper.id,
        summary.score,
        summary.summary.len(),
        abs.len()
    );
    Ok((summary, abs))
}
