// SPDX-License-Identifier: MIT OR Apache-2.0

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, Sse},
        Json,
    },
};
use base64::Engine as _;
use chrono::Utc;
use futures::channel::mpsc;
use futures::stream::BoxStream;
use futures::StreamExt;
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use super::auth::check_auth;
use super::insight_html::refresh_insight_html_cache;
use super::types::*;
use crate::db::DbPaperUpdate;
use crate::processor::Processor;

pub(crate) async fn insight_progress(
    State(state): State<Arc<AppState>>,
) -> Result<Json<InsightProgressResponse>, StatusCode> {
    let analyzing = match state.db.insight_progress().await {
        Ok(titles) => titles
            .into_iter()
            .map(|(id, title, source_type)| InsightProgressItem {
                id,
                title,
                processed_at: None,
                source_type,
            })
            .collect(),
        Err(e) => {
            error!("{}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    let completed = match state.db.recent_completed_insights(5).await {
        Ok(titles) => titles
            .into_iter()
            .map(|(id, title, ts, source_type)| InsightProgressItem {
                id,
                title,
                processed_at: ts.map(|dt| dt.timestamp_millis()),
                source_type,
            })
            .collect(),
        Err(e) => {
            error!("{}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    Ok(Json(InsightProgressResponse {
        titles: analyzing,
        completed,
    }))
}

pub(crate) async fn list_insight_providers(
    State(state): State<Arc<AppState>>,
) -> Result<Json<InsightProvidersResponse>, StatusCode> {
    let processors = state.insight_processors.read().await;
    let providers: Vec<String> = processors.iter().map(|(name, _)| name.clone()).collect();
    Ok(Json(InsightProvidersResponse { providers }))
}

pub(crate) async fn insight_analyze(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((source, id)): Path<(String, String)>,
    Json(req): Json<InsightRequest>,
) -> Result<Json<PaperResponse>, StatusCode> {
    if !check_auth(&headers, &state.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let processor = if let Some(ref name) = req.provider {
        let processors = state.insight_processors.read().await;
        processors
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| Arc::clone(s))
            .ok_or(StatusCode::BAD_REQUEST)?
    } else {
        let insight_proc = state.comment_processor.read().await;
        insight_proc
            .clone()
            .ok_or(StatusCode::SERVICE_UNAVAILABLE)?
    };

    // Fetch paper info from DB
    let (title, fallback_abstract, has_existing_insight, is_checked) = {
        let db = &state.db;
        let paper = db
            .get_paper(&id)
            .await
            .map_err(|e| {
                error!("{}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            .ok_or(StatusCode::NOT_FOUND)?;
        // Centralized insight validity check (replaces scattered starts_with checks)
        let has_existing = crate::db::is_valid_insight(&paper.insight);
        let is_checked = paper.checked_at.is_some();
        (
            paper.title.clone(),
            paper.abstract_zh.clone(),
            has_existing,
            is_checked,
        )
    };

    // Skip analysis if paper is already verified
    if is_checked {
        let db = &state.db;
        let paper = db
            .get_paper(&id)
            .await
            .map_err(|e| {
                error!("{}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            .ok_or(StatusCode::NOT_FOUND)?;
        let mark = db.get_mark(&id).await.map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        let tags_raw = db.get_paper_tags(&id).await.map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        let tags: Vec<TagItem> = tags_raw
            .into_iter()
            .map(|(tag, weight)| TagItem { tag, weight })
            .collect();
        return Ok(Json(PaperResponse::from_db(paper, mark, tags)));
    }

    // Only mark as analyzing if there's no existing valid insight
    if !has_existing_insight {
        let db = &state.db;
        let updates = DbPaperUpdate {
            insight: Some("__ANALYZING__".to_string()),
            ..Default::default()
        };
        db.update_paper(&id, &updates).await.map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    }

    // Parse split_figures from request (comma-separated, e.g. "F:7,F:8")
    let split_figures: Vec<String> = req
        .split_figures
        .as_ref()
        .map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_default();

    // Register cancellation token for this analysis
    let cancel_flag = Arc::new(AtomicBool::new(false));

    // Register streaming state early so clients can connect immediately
    let stream_state = InsightStreamState::new();
    {
        let mut streams = state.insight_streams.write().await;
        streams.insert(id.clone(), stream_state.clone());
    }

    // Spawn background task for actual analysis
    let state_clone = Arc::clone(&state);
    let state_for_cleanup = Arc::clone(&state);
    let processor_clone = Arc::clone(&processor);
    let id_bg = id.clone();
    let id_cleanup = id.clone();
    let prompt = req.prompt;
    let auto_review = req.auto_review;
    let skip_validation = req.skip_validation;
    let keep_caption = req.keep_caption;
    let stream_state_for_task = stream_state.clone();
    let source_bg = source.clone();
    let handle = tokio::spawn(async move {
        run_insight_analysis(
            state_clone,
            processor_clone,
            source_bg,
            id_bg,
            title,
            fallback_abstract,
            prompt,
            split_figures,
            auto_review,
            skip_validation,
            keep_caption,
            stream_state_for_task,
        )
        .await;
        // Unregister cancellation token when done
        {
            let mut cancels = state_for_cleanup.insight_cancellations.write().await;
            cancels.remove(&id_cleanup);
        }
    });
    {
        let mut cancels = state.insight_cancellations.write().await;
        cancels.insert(id.clone(), (cancel_flag.clone(), handle));
    }

    // Return current paper state with __ANALYZING__
    let db = &state.db;
    let paper = db
        .get_paper(&id)
        .await
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;
    let mark = db.get_mark(&id).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let tags_raw = db.get_paper_tags(&id).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let tags: Vec<TagItem> = tags_raw
        .into_iter()
        .map(|(tag, weight)| TagItem { tag, weight })
        .collect();
    Ok(Json(PaperResponse::from_db(paper, mark, tags)))
}

pub(crate) async fn insight_stream(
    State(state): State<Arc<AppState>>,
    Path((_source, id)): Path<(String, String)>,
) -> Result<Sse<BoxStream<'static, Result<Event, Infallible>>>, StatusCode> {
    let stream_state = {
        let streams = state.insight_streams.read().await;
        streams.get(&id).cloned()
    };

    let stream_state = match stream_state {
        Some(s) => s,
        None => {
            let empty_stream =
                futures::stream::iter(Vec::<Result<Event, Infallible>>::new()).boxed();
            return Ok(Sse::new(empty_stream));
        }
    };

    let accumulated = stream_state.accumulated.lock().unwrap().clone();
    let completed = stream_state
        .completed
        .load(std::sync::atomic::Ordering::Relaxed);
    let error = stream_state.error.lock().await.clone();

    let init_event = Event::default()
        .event("init")
        .data(serde_json::json!({"accumulated": accumulated}).to_string());

    if completed {
        let done_event = Event::default().event("done").data("{}");
        let stream = futures::stream::iter(vec![Ok(init_event), Ok(done_event)]).boxed();
        return Ok(Sse::new(stream));
    }

    if let Some(err) = error {
        let err_event = Event::default()
            .event("error")
            .data(serde_json::json!({"message": err}).to_string());
        let stream = futures::stream::iter(vec![Ok(init_event), Ok(err_event)]).boxed();
        return Ok(Sse::new(stream));
    }

    let mut rx = stream_state.tx.subscribe();
    let completed_arc = stream_state.completed.clone();
    let error_arc = stream_state.error.clone();
    let accumulated_arc = stream_state.accumulated.clone();

    let (sse_tx, sse_rx) = mpsc::unbounded::<Result<Event, Infallible>>();

    tokio::spawn(async move {
        let _ = sse_tx.unbounded_send(Ok(init_event));

        loop {
            match rx.recv().await {
                Ok(InsightStreamEvent::Chunk(chunk)) => {
                    let event = Event::default()
                        .event("chunk")
                        .data(serde_json::json!({"text": chunk}).to_string());
                    if sse_tx.unbounded_send(Ok(event)).is_err() {
                        break;
                    }
                }
                Ok(InsightStreamEvent::Reset) => {
                    let event = Event::default().event("reset").data("{}");
                    if sse_tx.unbounded_send(Ok(event)).is_err() {
                        break;
                    }
                    continue;
                }
                Ok(InsightStreamEvent::Done) => {
                    let event = Event::default().event("done").data("{}");
                    let _ = sse_tx.unbounded_send(Ok(event));
                    break;
                }
                Ok(InsightStreamEvent::Error(err)) => {
                    let event = Event::default()
                        .event("error")
                        .data(serde_json::json!({"message": err}).to_string());
                    let _ = sse_tx.unbounded_send(Ok(event));
                    break;
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let acc = accumulated_arc.lock().unwrap().clone();
                    let event = Event::default()
                        .event("init")
                        .data(serde_json::json!({"accumulated": acc}).to_string());
                    if sse_tx.unbounded_send(Ok(event)).is_err() {
                        break;
                    }
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }

            let completed = completed_arc.load(std::sync::atomic::Ordering::Relaxed);
            let error = error_arc.lock().await.clone();
            if completed {
                let event = Event::default().event("done").data("{}");
                let _ = sse_tx.unbounded_send(Ok(event));
                break;
            }
            if let Some(err) = error {
                let event = Event::default()
                    .event("error")
                    .data(serde_json::json!({"message": err}).to_string());
                let _ = sse_tx.unbounded_send(Ok(event));
                break;
            }
        }
    });

    Ok(Sse::new(sse_rx.boxed()))
}

async fn is_cancelled(state: &AppState, id: &str) -> bool {
    let cancels = state.insight_cancellations.read().await;
    cancels
        .get(id)
        .map(|(b, _)| b.load(Ordering::Relaxed))
        .unwrap_or(false)
}

async fn cleanup_cancelled_stream(state: &AppState, id: &str, stream_state: &InsightStreamState) {
    info!("[insight_analyze] Cancelling insight analysis for {}", id);
    *stream_state.error.lock().await = Some("Analysis cancelled".to_string());
    let _ = stream_state
        .tx
        .send(InsightStreamEvent::Error("Analysis cancelled".to_string()));
    {
        let mut streams = state.insight_streams.write().await;
        streams.remove(id);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_insight_analysis(
    state: Arc<AppState>,
    processor: Arc<Processor>,
    source: String,
    id: String,
    title: String,
    fallback_abstract: String,
    prompt: Option<String>,
    split_figures: Vec<String>,
    auto_review: Option<bool>,
    skip_validation: Option<bool>,
    keep_caption: Option<bool>,
    stream_state: InsightStreamState,
) {
    let fig_dir = crate::web::paper_figures_dir(&source, &id);
    let pdf_url = match state.db.get_paper(&id).await {
        Ok(Some(ref paper)) => match paper.source_type.as_str() {
            "arxiv" => format!(
                "https://arxiv.org/pdf/{}.pdf",
                paper.external_id.as_ref().unwrap_or(&paper.id)
            ),
            "openreview" => paper
                .external_id
                .as_ref()
                .map(|eid| format!("https://openreview.net/pdf?id={}", eid))
                .unwrap_or_else(|| paper.source_url.clone().unwrap_or_default()),
            _ => paper.source_url.clone().unwrap_or_default(),
        },
        _ => {
            format!("https://arxiv.org/pdf/{}.pdf", id)
        }
    };

    // Grab the cancellation flag early so we can pass it to LLM calls
    let cancel_flag = {
        let cancels = state.insight_cancellations.read().await;
        cancels.get(&id).map(|(f, _)| Arc::clone(f))
    };

    // Prepare MinerU figures and text (do once)
    // Read from editable_config so admin UI changes take effect without restart.
    let (mineru_api_key, mineru_base_url, mineru_no_cache) = {
        let editable = state.editable_config.read().await;
        (
            editable
                .mineru_api_key
                .as_ref()
                .filter(|s| !s.is_empty())
                .cloned(),
            editable.mineru_base_url.clone(),
            editable.mineru_no_cache,
        )
    };
    let (labeled_images, fallback_text) = if let Some(ref api_key) = mineru_api_key {
        let client = crate::mineru::MinerUClient::new(
            mineru_base_url.clone(),
            api_key.clone(),
            Some("figures".to_string()),
            mineru_no_cache,
        );
        let storage_id = format!("{}/{}", source, id);
        match client
            .extract_figures(&pdf_url, Some(&storage_id), &split_figures, true)
            .await
        {
            Ok(extracted) => {
                // Read image_names from cache meta so the prompt can mention actual filenames
                let image_names: Vec<String> = {
                    let meta_path = format!("{}/mineru.json", fig_dir);
                    if let Ok(meta_json) = tokio::fs::read_to_string(&meta_path).await {
                        serde_json::from_str::<crate::mineru::CacheMeta>(&meta_json)
                            .map(|m| m.image_names)
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    }
                };
                let mut labeled = Vec::new();
                for (i, (desc, bytes)) in extracted.images.iter().enumerate() {
                    let name = image_names
                        .get(i)
                        .expect("image_names must match extracted images");
                    let label =
                        format!("[Reference path: figure/{}] [Full Caption: {}]", name, desc);
                    let url = format!(
                        "data:image/jpeg;base64,{}",
                        base64::prelude::BASE64_STANDARD.encode(bytes)
                    );
                    labeled.push((name.clone(), label, url));
                }

                // Also include manual figures from manual_figures.json
                let manual_path = format!("{}/manual_figures.json", fig_dir);
                if let Ok(manual_json) = tokio::fs::read_to_string(&manual_path).await {
                    if let Ok(manual) =
                        serde_json::from_str::<crate::mineru::ManualFigures>(&manual_json)
                    {
                        for mf in manual {
                            let hires_png = format!("{}/hires/{}.png", fig_dir, mf.name);
                            if let Ok(bytes) = tokio::fs::read(&hires_png).await {
                                let label = format!(
                                    "[Reference path: figure/{}] [Full Caption: {}]",
                                    mf.name, mf.desc
                                );
                                let url = format!(
                                    "data:image/png;base64,{}",
                                    base64::prelude::BASE64_STANDARD.encode(&bytes)
                                );
                                labeled.push((mf.name.clone(), label, url));
                            } else {
                                warn!(
                                    "[insight_analyze] Manual figure {} hires PNG not found at {}, skipping",
                                    mf.name, hires_png
                                );
                            }
                        }
                    }
                }

                info!(
                    "[insight_analyze] MinerU extracted {} figures for {}",
                    labeled.len(),
                    id
                );

                // Validate extracted figures: every image/chart/table must have
                // a proper caption, not a placeholder.
                if !skip_validation.unwrap_or(false) {
                    if let Err(err_msg) = crate::mineru::validate_extracted_figures(
                        &extracted,
                        extracted.layout_doc.as_ref(),
                        Some(&id),
                    ) {
                        warn!(
                            "[insight_analyze] Figure validation failed for {}: {}",
                            id, err_msg
                        );
                        let db = &state.db;
                        let updates = DbPaperUpdate {
                            insight: Some(format!(
                                "Figure extraction validation failed: {}",
                                err_msg
                            )),
                            ..Default::default()
                        };
                        if let Err(e) = db.update_paper(&id, &updates).await {
                            warn!(
                                "[insight_analyze] Failed to save validation error for {}: {}",
                                id, e
                            );
                        }
                        *stream_state.error.lock().await =
                            Some(format!("Figure extraction validation failed: {}", err_msg));
                        let _ = stream_state.tx.send(InsightStreamEvent::Error(format!(
                            "Figure extraction validation failed: {}",
                            err_msg
                        )));
                        {
                            let mut streams = state.insight_streams.write().await;
                            streams.remove(&id);
                        }
                        return;
                    }
                }

                let text = if !extracted.markdown.is_empty() {
                    extracted.markdown
                } else {
                    fallback_abstract.clone()
                };
                (labeled, Some(text))
            }
            Err(e) => {
                warn!(
                    "[insight_analyze] MinerU failed for {}: {}, marking as failed",
                    id, e
                );
                let db = &state.db;
                let updates = DbPaperUpdate {
                    insight: Some(format!("Figure extraction failed: {}", e)),
                    ..Default::default()
                };
                if let Err(e) = db.update_paper(&id, &updates).await {
                    warn!("[insight_analyze] Failed to save error for {}: {}", id, e);
                }
                *stream_state.error.lock().await = Some(format!("Figure extraction failed: {}", e));
                let _ = stream_state.tx.send(InsightStreamEvent::Error(format!(
                    "Figure extraction failed: {}",
                    e
                )));
                {
                    let mut streams = state.insight_streams.write().await;
                    streams.remove(&id);
                }
                return;
            }
        }
    } else {
        warn!(
            "[insight_analyze] No MinerU API key configured, aborting analysis for {}",
            id
        );
        let db = &state.db;
        let updates = DbPaperUpdate {
            insight: Some("MinerU API not configured; cannot extract figures".to_string()),
            ..Default::default()
        };
        if let Err(e) = db.update_paper(&id, &updates).await {
            warn!("[insight_analyze] Failed to save error for {}: {}", id, e);
        }
        *stream_state.error.lock().await =
            Some("MinerU API not configured; cannot extract figures".to_string());
        let _ = stream_state.tx.send(InsightStreamEvent::Error(
            "MinerU API not configured; cannot extract figures".to_string(),
        ));
        {
            let mut streams = state.insight_streams.write().await;
            streams.remove(&id);
        }
        return;
    };

    // Check cancellation after MinerU extraction
    if is_cancelled(&state, &id).await {
        cleanup_cancelled_stream(&state, &id, &stream_state).await;
        return;
    }

    // Check if insight already exists (non-empty and not a temporary/error state)
    let existing_insight = {
        let db = &state.db;
        match db.get_paper(&id).await {
            Ok(Some(paper)) => {
                let insight = paper.insight;
                if crate::db::is_valid_insight(&insight) {
                    info!("[insight_analyze] Existing insight found for {}", id);
                    Some(insight)
                } else {
                    None
                }
            }
            _ => None,
        }
    };

    let hires_source = source.clone();
    let hires_id = id.clone();
    let hires_pdf_url = pdf_url.clone();
    let hires_keep_caption = keep_caption.unwrap_or(false);
    info!(
        "[insight_analyze] Starting hires screenshot task for {} (keep_caption={})",
        hires_id, hires_keep_caption
    );
    let hires_handle = tokio::spawn(async move {
        let result = crate::hires::generate_hires_images(
            &hires_source,
            &hires_id,
            &hires_pdf_url,
            600,
            hires_keep_caption,
        )
        .await;
        (hires_id, result)
    });

    // Determine whether to auto-review for this request.
    // Read fresh values from editable_config so admin UI changes take effect
    // immediately without a server restart.
    let (do_review, max_attempts) = {
        let cfg = state.editable_config.read().await;
        (
            auto_review.unwrap_or(cfg.auto_review_insight),
            cfg.insight_review_max_attempts,
        )
    };

    let mut insight_result = String::new();
    let mut review_result = String::new();
    let mut review_qualified = false;

    if do_review {
        // Retry loop: generate insight -> review -> if unqualified, retry (max N attempts)
        let mut attempt = 0;

        while attempt < max_attempts {
            attempt += 1;
            if is_cancelled(&state, &id).await {
                cleanup_cancelled_stream(&state, &id, &stream_state).await;
                return;
            }
            info!(
                "[insight_analyze] Insight generation attempt {}/{} for {}",
                attempt, max_attempts, id
            );

            let insight = if attempt == 1 && existing_insight.is_some() {
                existing_insight.clone().unwrap()
            } else {
                // Clear previous failed insight before regenerating
                if attempt > 1 {
                    info!("[insight_analyze] Clearing previous unqualified insight for {}, retrying...", id);
                    if let Ok(mut guard) = stream_state.accumulated.lock() {
                        guard.clear();
                    }
                    let _ = stream_state.tx.send(InsightStreamEvent::Reset);
                }
                let tx = stream_state.tx.clone();
                let acc = stream_state.accumulated.clone();
                let on_chunk = move |chunk: &str| {
                    if let Ok(mut guard) = acc.lock() {
                        guard.push_str(chunk);
                    }
                    let _ = tx.send(InsightStreamEvent::Chunk(chunk.to_string()));
                };
                match processor
                    .insight_analyze_with_callback(
                        &title,
                        &labeled_images,
                        fallback_text.as_deref(),
                        prompt.as_deref(),
                        cancel_flag.clone(),
                        on_chunk,
                    )
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        if e.to_string() == "Cancelled" {
                            cleanup_cancelled_stream(&state, &id, &stream_state).await;
                            return;
                        }
                        warn!("[insight_analyze] LLM analysis failed for {}: {}", id, e);
                        format!("Insight analysis failed: {}", e)
                    }
                }
            };

            // Screenshot full PDF pages + extract raw text for review comparison
            let (review_images, review_text) = match crate::hires::screenshot_pdf_pages(
                &source, &id, &pdf_url, 200,
            )
            .await
            {
                Ok((imgs, txt)) => {
                    info!(
                        "[insight_analyze] Loaded {} pages + {} chars text for review {}",
                        imgs.len(),
                        txt.len(),
                        id
                    );
                    (imgs, Some(txt))
                }
                Err(e) => {
                    warn!("[insight_analyze] Failed to screenshot PDF pages for review {}: {}, falling back to MinerU images", id, e);
                    let fallback: Vec<(String, String)> = labeled_images
                        .iter()
                        .map(|(_, l, u)| (l.clone(), u.clone()))
                        .collect();
                    (fallback, None)
                }
            };

            // Review the generated insight using full-page screenshots + extracted text
            match processor
                .review_insight(
                    &title,
                    &insight,
                    &review_images,
                    review_text.as_deref(),
                    None,
                    cancel_flag.clone(),
                )
                .await
            {
                Ok(review) => {
                    review_qualified = review.qualified;
                    review_result = review.content;
                    info!(
                        "[insight_analyze] Review attempt {}/{} for {}: qualified={}",
                        attempt, max_attempts, id, review_qualified
                    );
                    if review_qualified {
                        insight_result = insight;
                        break;
                    }
                    // Not qualified: store the current result as fallback and try again
                    insight_result = insight;
                }
                Err(e) => {
                    if e.to_string() == "Cancelled" {
                        cleanup_cancelled_stream(&state, &id, &stream_state).await;
                        return;
                    }
                    warn!("[insight_analyze] Insight review failed for {}: {}", id, e);
                    review_qualified = false;
                    review_result = format!("Review failed: {}", e);
                    insight_result = insight;
                }
            }
        }

        if !review_qualified && attempt >= max_attempts {
            warn!(
                "[insight_analyze] Insight for {} failed review after {} attempts, keeping last result",
                id, max_attempts
            );
        }
    } else {
        info!(
            "[insight_analyze] Auto-review disabled for {}, generating insight only",
            id
        );

        let insight = if let Some(existing) = existing_insight {
            existing
        } else {
            let tx = stream_state.tx.clone();
            let acc = stream_state.accumulated.clone();
            let on_chunk = move |chunk: &str| {
                if let Ok(mut guard) = acc.lock() {
                    guard.push_str(chunk);
                }
                let _ = tx.send(InsightStreamEvent::Chunk(chunk.to_string()));
            };
            match processor
                .insight_analyze_with_callback(
                    &title,
                    &labeled_images,
                    fallback_text.as_deref(),
                    prompt.as_deref(),
                    cancel_flag.clone(),
                    on_chunk,
                )
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    if e.to_string() == "Cancelled" {
                        cleanup_cancelled_stream(&state, &id, &stream_state).await;
                        return;
                    }
                    warn!("[insight_analyze] LLM analysis failed for {}: {}", id, e);
                    format!("Insight analysis failed: {}", e)
                }
            }
        };
        insight_result = insight;
        // Auto-review disabled: leave review fields empty.
        review_result = String::new();
    }

    // Check cancellation before saving results
    if is_cancelled(&state, &id).await {
        cleanup_cancelled_stream(&state, &id, &stream_state).await;
        return;
    }

    match hires_handle.await {
        Ok((hires_id, Ok(()))) => {
            info!(
                "[insight_analyze] Hires screenshot completed for {}",
                hires_id
            );
        }
        Ok((hires_id, Err(e))) => {
            warn!(
                "[insight_analyze] Hires screenshot failed for {}: {}",
                hires_id, e
            );
        }
        Err(e) => {
            warn!(
                "[insight_analyze] Hires screenshot task join failed for {}: {}",
                id, e
            );
        }
    }

    let db = &state.db;
    let now = Utc::now();
    let updates = DbPaperUpdate {
        insight: Some(insight_result),
        insight_processed_at: Some(now),
        insight_review: Some(review_result),
        insight_reviewed_at: None,
        ..Default::default()
    };
    if let Err(e) = db.update_paper(&id, &updates).await {
        warn!("[insight_analyze] Failed to save result for {}: {}", id, e);
        stream_state
            .completed
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = stream_state
            .tx
            .send(InsightStreamEvent::Error(format!("Failed to save: {}", e)));
    } else {
        if let Some(ref insight) = updates.insight {
            refresh_insight_html_cache(db, &source, &id, insight).await;
        }
        stream_state
            .completed
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = stream_state.tx.send(InsightStreamEvent::Done);
    }
    {
        let mut streams = state.insight_streams.write().await;
        streams.remove(&id);
    }
}

pub(crate) async fn clear_insight(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((source, id)): Path<(String, String)>,
) -> Result<Json<PaperResponse>, StatusCode> {
    if !check_auth(&headers, &state.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let db = &state.db;
    let paper = db
        .get_paper(&id)
        .await
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;
    if paper.checked_at.is_some() {
        return Err(StatusCode::FORBIDDEN);
    }

    // If the paper is currently being analyzed, signal cancellation
    let was_analyzing = paper.insight == "__ANALYZING__" || paper.insight_review == "__REVIEWING__";
    if was_analyzing {
        {
            let mut cancels = state.insight_cancellations.write().await;
            if let Some((flag, handle)) = cancels.remove(&id) {
                flag.store(true, Ordering::Relaxed);
                handle.abort();
                // Await in background to prevent zombie task accumulation.
                tokio::spawn(async move {
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), handle).await;
                });
                info!("[clear_insight] Aborted insight task for paper {}", id);
            }
        }
        // Also clear the stream state so clients get an error immediately
        {
            let mut streams = state.insight_streams.write().await;
            if let Some(stream_state) = streams.remove(&id) {
                *stream_state.error.lock().await = Some("Analysis cancelled".to_string());
                let _ = stream_state
                    .tx
                    .send(InsightStreamEvent::Error("Analysis cancelled".to_string()));
            }
        }
    }

    // Backup current insight before clearing
    if let Ok(Some(paper)) = db.get_paper(&id).await {
        if crate::db::is_valid_insight(&paper.insight) {
            let insight_processed_at_str = paper
                .insight_processed_at
                .as_ref()
                .map(|dt| dt.to_rfc3339());
            let insight_reviewed_at_str =
                paper.insight_reviewed_at.as_ref().map(|dt| dt.to_rfc3339());
            if let Err(e) = db
                .backup_insight(
                    &id,
                    &paper.insight,
                    insight_processed_at_str.as_deref(),
                    &paper.insight_review,
                    insight_reviewed_at_str.as_deref(),
                )
                .await
            {
                warn!("[clear_insight] Failed to backup insight for {}: {}", id, e);
            } else {
                info!("[clear_insight] Backed up insight for {}", id);
            }
        }
    }

    // Clear transparent image cache so re-generated insight gets fresh processing.
    let transparent_dir = format!("figures/{}/{}/transparent", source, id);
    if std::path::Path::new(&transparent_dir).exists() {
        if let Err(e) = tokio::fs::remove_dir_all(&transparent_dir).await {
            warn!(
                "[clear_insight] Failed to remove transparent cache for {}: {}",
                id, e
            );
        } else {
            info!("[clear_insight] Cleared transparent cache for {}", id);
        }
    }

    let updates = DbPaperUpdate {
        insight: Some(String::new()),
        insight_review: Some(String::new()),
        insight_reviewed_at: None,
        ..Default::default()
    };
    db.update_paper(&id, &updates).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    // Delete cached HTML (insight cleared)
    let _ = db.delete_insight_html_cache(&id).await;
    let paper = db
        .get_paper(&id)
        .await
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let (mark, tags_raw) = tokio::join!(db.get_mark(&id), db.get_paper_tags(&id));
    let mark = mark.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let tags_raw = tags_raw.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let tags: Vec<TagItem> = tags_raw
        .into_iter()
        .map(|(tag, weight)| TagItem { tag, weight })
        .collect();
    Ok(Json(PaperResponse::from_db(paper, mark, tags)))
}

pub(crate) async fn toggle_checked(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((_source, id)): Path<(String, String)>,
) -> Result<Json<PaperResponse>, StatusCode> {
    if !check_auth(&headers, &state.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let db = &state.db;
    let paper = db
        .get_paper(&id)
        .await
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;
    let now = Utc::now();
    let updates = DbPaperUpdate {
        checked_at: if paper.checked_at.is_some() {
            None
        } else {
            Some(now)
        },
        clear_checked_at: paper.checked_at.is_some(),
        ..Default::default()
    };
    db.update_paper(&id, &updates).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let paper = db
        .get_paper(&id)
        .await
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let (mark, tags_raw) = tokio::join!(db.get_mark(&id), db.get_paper_tags(&id));
    let mark = mark.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let tags_raw = tags_raw.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let tags: Vec<TagItem> = tags_raw
        .into_iter()
        .map(|(tag, weight)| TagItem { tag, weight })
        .collect();
    Ok(Json(PaperResponse::from_db(paper, mark, tags)))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_review_analysis(
    state: Arc<AppState>,
    processor: Arc<Processor>,
    source: String,
    id: String,
    title: String,
    insight: String,
    custom_prompt: Option<String>,
    cancel_flag: Arc<AtomicBool>,
) {
    let pdf_url = match state.db.get_paper(&id).await {
        Ok(Some(ref paper)) => match paper.source_type.as_str() {
            "arxiv" => format!(
                "https://arxiv.org/pdf/{}.pdf",
                paper.external_id.as_ref().unwrap_or(&paper.id)
            ),
            "openreview" => paper
                .external_id
                .as_ref()
                .map(|eid| format!("https://openreview.net/pdf?id={}", eid))
                .unwrap_or_else(|| paper.source_url.clone().unwrap_or_default()),
            _ => paper.source_url.clone().unwrap_or_default(),
        },
        _ => {
            format!("https://arxiv.org/pdf/{}.pdf", id)
        }
    };
    let cancel_flag = Some(cancel_flag);

    let (images, text) = match crate::hires::screenshot_pdf_pages(&source, &id, &pdf_url, 200).await
    {
        Ok((imgs, txt)) => {
            info!(
                "[run_review_analysis] Loaded {} pages + {} chars text for {}",
                imgs.len(),
                txt.len(),
                id
            );
            (imgs, Some(txt))
        }
        Err(e) => {
            warn!(
                "[run_review_analysis] Failed to screenshot PDF pages for {}: {}",
                id, e
            );
            (Vec::new(), None)
        }
    };

    if cancel_flag
        .as_ref()
        .map(|f| f.load(Ordering::Relaxed))
        .unwrap_or(false)
    {
        info!("[run_review_analysis] Review cancelled for {}", id);
        let updates = DbPaperUpdate {
            insight_review: Some(String::new()),
            insight_reviewed_at: None,
            ..Default::default()
        };
        if let Err(e) = state.db.update_paper(&id, &updates).await {
            warn!(
                "[run_review_analysis] Failed to clear cancelled review for {}: {}",
                id, e
            );
        }
        return;
    }

    let review = match processor
        .review_insight(
            &title,
            &insight,
            &images,
            text.as_deref(),
            custom_prompt.as_deref(),
            cancel_flag,
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            if e.to_string() == "Cancelled" {
                info!("[run_review_analysis] Review cancelled for {}", id);
                let updates = DbPaperUpdate {
                    insight_review: Some(String::new()),
                    insight_reviewed_at: None,
                    ..Default::default()
                };
                if let Err(e) = state.db.update_paper(&id, &updates).await {
                    warn!(
                        "[run_review_analysis] Failed to clear cancelled review for {}: {}",
                        id, e
                    );
                }
                return;
            }
            warn!("[run_review_analysis] Review failed for {}: {}", id, e);
            let updates = DbPaperUpdate {
                insight_review: Some(format!("Review failed: {}", e)),
                insight_reviewed_at: Some(Utc::now()),
                ..Default::default()
            };
            if let Err(e) = state.db.update_paper(&id, &updates).await {
                warn!(
                    "[run_review_analysis] Failed to save error for {}: {}",
                    id, e
                );
            }
            return;
        }
    };

    let now = Utc::now();
    let updates = DbPaperUpdate {
        insight_review: Some(review.content),
        insight_reviewed_at: Some(now),
        ..Default::default()
    };
    if let Err(e) = state.db.update_paper(&id, &updates).await {
        warn!(
            "[run_review_analysis] Failed to save result for {}: {}",
            id, e
        );
    }
}

pub(crate) async fn review_paper(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((source, id)): Path<(String, String)>,
    Json(req): Json<ReviewRequest>,
) -> Result<Json<PaperResponse>, StatusCode> {
    if !check_auth(&headers, &state.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let db = &state.db;
    let paper = db
        .get_paper(&id)
        .await
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    if !crate::db::is_valid_insight(&paper.insight) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if paper.checked_at.is_some() {
        let mark = db.get_mark(&id).await.map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        let tags_raw = db.get_paper_tags(&id).await.map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        let tags: Vec<TagItem> = tags_raw
            .into_iter()
            .map(|(tag, weight)| TagItem { tag, weight })
            .collect();
        return Ok(Json(PaperResponse::from_db(paper, mark, tags)));
    }

    // Prevent concurrent review for the same paper
    if paper.insight_review == "__REVIEWING__" {
        return Err(StatusCode::CONFLICT);
    }

    // Pick processor: use requested provider or fall back to first available
    let processor = if let Some(ref name) = req.provider {
        let processors = state.insight_processors.read().await;
        processors
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| Arc::clone(s))
            .ok_or(StatusCode::BAD_REQUEST)?
    } else {
        let insight_proc = state.comment_processor.read().await;
        insight_proc
            .clone()
            .ok_or(StatusCode::SERVICE_UNAVAILABLE)?
    };

    // Mark as reviewing immediately
    let mark_updates = DbPaperUpdate {
        insight_review: Some("__REVIEWING__".to_string()),
        insight_reviewed_at: None,
        ..Default::default()
    };
    db.update_paper(&id, &mark_updates).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Register cancellation token for review (mirrors insight_analyze)
    let cancel_flag = Arc::new(AtomicBool::new(false));

    // Spawn background task for actual review
    let state_clone = Arc::clone(&state);
    let state_for_cleanup = Arc::clone(&state);
    let processor_clone = Arc::clone(&processor);
    let id_bg = id.clone();
    let id_cleanup = id.clone();
    let title_bg = paper.title.clone();
    let insight_bg = paper.insight.clone();
    let custom_prompt = req.prompt;
    let source_bg = source.clone();
    let cancel_flag_bg = cancel_flag.clone();
    let handle = tokio::spawn(async move {
        run_review_analysis(
            state_clone,
            processor_clone,
            source_bg,
            id_bg,
            title_bg,
            insight_bg,
            custom_prompt,
            cancel_flag_bg,
        )
        .await;
        // Unregister cancellation when done
        {
            let mut cancels = state_for_cleanup.insight_cancellations.write().await;
            cancels.remove(&id_cleanup);
        }
    });
    {
        let mut cancels = state.insight_cancellations.write().await;
        cancels.insert(id.clone(), (cancel_flag, handle));
    }

    // Return current paper state with __REVIEWING__
    let updated = db
        .get_paper(&id)
        .await
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let (mark, tags_raw) = tokio::join!(db.get_mark(&id), db.get_paper_tags(&id));
    let mark = mark.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let tags_raw = tags_raw.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let tags: Vec<TagItem> = tags_raw
        .into_iter()
        .map(|(tag, weight)| TagItem { tag, weight })
        .collect();
    Ok(Json(PaperResponse::from_db(updated, mark, tags)))
}

pub(crate) async fn cancel_review(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((_source, id)): Path<(String, String)>,
) -> Result<Json<PaperResponse>, StatusCode> {
    if !check_auth(&headers, &state.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let db = &state.db;
    let paper = db
        .get_paper(&id)
        .await
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;
    if paper.checked_at.is_some() {
        return Err(StatusCode::FORBIDDEN);
    }
    if paper.insight_review == "__REVIEWING__" {
        // Signal cancellation and abort the background task
        let mut cancels = state.insight_cancellations.write().await;
        if let Some((flag, handle)) = cancels.remove(&id) {
            flag.store(true, Ordering::Relaxed);
            handle.abort();
        }
    }
    // Clear only review state, preserve insight content
    let updates = DbPaperUpdate {
        insight_review: Some(String::new()),
        insight_reviewed_at: None,
        ..Default::default()
    };
    db.update_paper(&id, &updates).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let paper = db
        .get_paper(&id)
        .await
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;
    let (mark, tags_raw) = tokio::join!(db.get_mark(&id), db.get_paper_tags(&id));
    let mark = mark.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let tags_raw = tags_raw.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let tags: Vec<TagItem> = tags_raw
        .into_iter()
        .map(|(tag, weight)| TagItem { tag, weight })
        .collect();
    info!("[cancel_review] Cancelled review for {}", id);
    Ok(Json(PaperResponse::from_db(paper, mark, tags)))
}

pub(crate) async fn list_paper_insight_backups(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((_source, id)): Path<(String, String)>,
) -> Result<Json<InsightBackupsResponse>, StatusCode> {
    if !check_auth(&headers, &state.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let db = &state.db;
    let rows = match db.list_paper_insight_backups(&id).await {
        Ok(r) => r,
        Err(e) => {
            warn!("[list_paper_insight_backups] Failed for {}: {}", id, e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    let backups: Vec<InsightBackupItem> = rows
        .into_iter()
        .map(
            |(
                id,
                insight,
                insight_processed_at,
                insight_review,
                insight_reviewed_at,
                created_at,
            )| {
                InsightBackupItem {
                    id,
                    insight,
                    insight_processed_at,
                    insight_review,
                    insight_reviewed_at,
                    created_at,
                }
            },
        )
        .collect();
    Ok(Json(InsightBackupsResponse { backups }))
}

pub(crate) async fn restore_insight_backup(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((source, id)): Path<(String, String)>,
    Json(req): Json<RestoreInsightRequest>,
) -> Result<Json<PaperResponse>, StatusCode> {
    if !check_auth(&headers, &state.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let db = &state.db;
    let paper = db
        .get_paper(&id)
        .await
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;
    if paper.checked_at.is_some() {
        return Err(StatusCode::FORBIDDEN);
    }

    let rows = match db.list_paper_insight_backups(&id).await {
        Ok(r) => r,
        Err(e) => {
            warn!(
                "[restore_insight_backup] Failed to list backups for {}: {}",
                id, e
            );
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let backup = rows
        .into_iter()
        .find(|(bid, _, _, _, _, _)| *bid == req.backup_id);
    let (_, insight, insight_processed_at, insight_review, insight_reviewed_at, _) = match backup {
        Some(b) => b,
        None => return Err(StatusCode::NOT_FOUND),
    };

    // Backup current content before restoring (skip if empty or still analyzing)
    if !paper.insight.is_empty() && paper.insight != "__ANALYZING__" {
        if let Err(e) = db
            .backup_insight(
                &id,
                &paper.insight,
                paper
                    .insight_processed_at
                    .as_ref()
                    .map(|d| d.to_rfc3339())
                    .as_deref(),
                &paper.insight_review,
                paper
                    .insight_reviewed_at
                    .as_ref()
                    .map(|d| d.to_rfc3339())
                    .as_deref(),
            )
            .await
        {
            warn!(
                "[restore_insight_backup] Failed to backup current insight for {}: {}",
                id, e
            );
        }
    }

    let updates = DbPaperUpdate {
        insight: Some(insight),
        insight_processed_at,
        insight_review: Some(insight_review),
        insight_reviewed_at,
        ..Default::default()
    };
    db.update_paper(&id, &updates).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    if let Some(ref insight) = updates.insight {
        refresh_insight_html_cache(db, &source, &id, insight).await;
    }

    let paper = db
        .get_paper(&id)
        .await
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let (mark, tags_raw) = tokio::join!(db.get_mark(&id), db.get_paper_tags(&id));
    let mark = mark.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let tags_raw = tags_raw.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let tags: Vec<TagItem> = tags_raw
        .into_iter()
        .map(|(tag, weight)| TagItem { tag, weight })
        .collect();
    Ok(Json(PaperResponse::from_db(paper, mark, tags)))
}

pub(crate) async fn delete_insight_backup(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((_source, id, backup_id)): Path<(String, String, i64)>,
) -> Result<StatusCode, StatusCode> {
    if !check_auth(&headers, &state.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    match state.db.delete_insight_backup(&id, backup_id).await {
        Ok(true) => Ok(StatusCode::NO_CONTENT),
        Ok(false) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            warn!(
                "[delete_insight_backup] Failed for {}/{}: {}",
                id, backup_id, e
            );
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

pub(crate) async fn format_insight(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((_source, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if !check_auth(&headers, &state.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let paper = match state.db.get_paper(&id).await {
        Ok(Some(p)) => p,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(e) => {
            warn!("[format_insight] Failed to fetch paper {}: {}", id, e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    if paper.checked_at.is_some() {
        return Err(StatusCode::FORBIDDEN);
    }

    let markdown = &paper.insight;
    if markdown.is_empty() || markdown == "__ANALYZING__" {
        return Ok(Json(serde_json::json!({
            "changed": false,
            "message": "No insight content to format"
        })));
    }

    let paper_id = format!("{}/{}", paper.source_type, paper.id);
    let zip_path = std::path::PathBuf::from(format!("figures/{}/mineru.zip", paper_id));
    let page_width_px = crate::mdfmt::page_width_from_layout(&zip_path, 600);
    let (new_md, _results) = crate::mdfmt::transform(&paper_id, markdown, false, page_width_px);

    let changed = new_md != *markdown;
    if !changed {
        return Ok(Json(serde_json::json!({
            "changed": false,
            "message": "No changes needed"
        })));
    }

    // Backup before overwriting
    let insight_processed_at_str = paper
        .insight_processed_at
        .as_ref()
        .map(|dt| dt.to_rfc3339());
    let insight_reviewed_at_str = paper.insight_reviewed_at.as_ref().map(|dt| dt.to_rfc3339());
    if let Err(e) = state
        .db
        .backup_insight(
            &id,
            &paper.insight,
            insight_processed_at_str.as_deref(),
            &paper.insight_review,
            insight_reviewed_at_str.as_deref(),
        )
        .await
    {
        warn!(
            "[format_insight] Failed to backup insight for {}: {}",
            id, e
        );
    }

    let updates = DbPaperUpdate {
        insight: Some(new_md.clone()),
        ..Default::default()
    };
    if let Err(e) = state.db.update_paper(&id, &updates).await {
        error!("[format_insight] Failed to save paper {}: {}", id, e);
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    // Refresh HTML cache with newly formatted insight content
    super::insight_html::refresh_insight_html_cache(&state.db, &paper.source_type, &id, &new_md)
        .await;

    Ok(Json(serde_json::json!({
        "changed": true,
        "message": "Insight formatted successfully"
    })))
}

pub(crate) async fn clear_all_insights(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if !check_auth(&headers, &state.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    match state.db.clear_all_insights().await {
        Ok(n) => {
            info!("[clear_all_insights] Cleared {} paper insights", n);
            Ok(Json(serde_json::json!({ "cleared": n })))
        }
        Err(e) => {
            warn!("[clear_all_insights] Failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
