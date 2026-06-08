// SPDX-License-Identifier: MIT OR Apache-2.0

//! Admin API: read and update `.env` configuration.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json},
};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{error, info};

use crate::env_editor::{
    delete_provider, env_path, read_providers, write_env_file, write_provider, ProviderConfig,
};
use crate::processor::{Processor, ProviderType};

use super::auth::check_auth;
use super::types::{
    AdminConfigResponse, AdminConfigUpdateRequest, AppState, ErrorResponse, FetchStatus,
    ProviderDetail, ProviderListResponse, ProviderUpdateRequest, ReorderProvidersRequest,
};

/// GET /api/admin/config — return current editable config.
pub(crate) async fn get_config(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<AdminConfigResponse>, (StatusCode, Json<ErrorResponse>)> {
    if !check_auth(&headers, &state.auth_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                code: 401,
                reason: "Unauthorized".into(),
            }),
        ));
    }

    let cfg = state.editable_config.read().await;
    Ok(Json(AdminConfigResponse {
        arxiv_fetch_cron: cfg.arxiv_fetch_cron.clone(),
        arxiv_query: cfg.arxiv_query.clone(),
        arxiv_max_results: cfg.arxiv_max_results,
        arxiv_page_size: cfg.arxiv_page_size,
        llm_max_workers: cfg.llm_max_workers,
        llm_max_retries: cfg.llm_max_retries,
        pdf_max_workers: cfg.pdf_max_workers,
        insight_max_workers: cfg.insight_max_workers,
        insight_review_max_attempts: cfg.insight_review_max_attempts,
        auto_review_insight: cfg.auto_review_insight,
        mineru_base_url: cfg.mineru_base_url.clone(),
        mineru_no_cache: cfg.mineru_no_cache,
        mineru_api_key: cfg.mineru_api_key.clone(),
    }))
}

/// POST /api/admin/config — update config, persist to `.env`, and update in-memory state.
pub(crate) async fn update_config(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<AdminConfigUpdateRequest>,
) -> Result<Json<AdminConfigResponse>, (StatusCode, Json<ErrorResponse>)> {
    if !check_auth(&headers, &state.auth_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                code: 401,
                reason: "Unauthorized".into(),
            }),
        ));
    }

    let env_file = env_path();
    let mut updates: HashMap<String, String> = HashMap::new();
    let mut needs_processor_rebuild = false;

    {
        let mut cfg = state.editable_config.write().await;

        if let Some(v) = req.arxiv_fetch_cron {
            updates.insert("ARXIV_FETCH_CRON".to_string(), v.clone());
            cfg.arxiv_fetch_cron = v;
        }
        if let Some(v) = req.arxiv_query {
            updates.insert("ARXIV_QUERY".to_string(), v.clone());
            cfg.arxiv_query = v;
        }
        if let Some(v) = req.arxiv_max_results {
            updates.insert("ARXIV_MAX_RESULTS".to_string(), v.to_string());
            cfg.arxiv_max_results = v;
        }
        if let Some(v) = req.arxiv_page_size {
            updates.insert("ARXIV_PAGE_SIZE".to_string(), v.to_string());
            cfg.arxiv_page_size = v;
        }
        if let Some(v) = req.llm_max_workers {
            if cfg.llm_max_workers != v {
                needs_processor_rebuild = true;
            }
            updates.insert("LLM_MAX_WORKERS".to_string(), v.to_string());
            cfg.llm_max_workers = v;
        }
        if let Some(v) = req.llm_max_retries {
            if cfg.llm_max_retries != v {
                needs_processor_rebuild = true;
            }
            updates.insert("LLM_MAX_RETRIES".to_string(), v.to_string());
            cfg.llm_max_retries = v;
        }
        if let Some(v) = req.pdf_max_workers {
            updates.insert("PDF_MAX_WORKERS".to_string(), v.to_string());
            cfg.pdf_max_workers = v;
        }
        if let Some(v) = req.insight_max_workers {
            if cfg.insight_max_workers != v {
                needs_processor_rebuild = true;
            }
            updates.insert("INSIGHT_MAX_WORKERS".to_string(), v.to_string());
            cfg.insight_max_workers = v;
        }
        if let Some(v) = req.insight_review_max_attempts {
            updates.insert("INSIGHT_REVIEW_MAX_ATTEMPTS".to_string(), v.to_string());
            cfg.insight_review_max_attempts = v;
        }
        if let Some(v) = req.auto_review_insight {
            updates.insert("AUTO_REVIEW_INSIGHT".to_string(), v.to_string());
            cfg.auto_review_insight = v;
        }
        if let Some(v) = req.mineru_base_url {
            updates.insert("MINERU_BASE_URL".to_string(), v.clone());
            cfg.mineru_base_url = v;
        }
        if let Some(v) = req.mineru_no_cache {
            updates.insert("MINERU_NO_CACHE".to_string(), v.to_string());
            cfg.mineru_no_cache = v;
        }
        if let Some(v) = req.mineru_api_key {
            updates.insert("MINERU_API_KEY".to_string(), v.clone());
            cfg.mineru_api_key = Some(v);
        }
    } // release write lock

    if !updates.is_empty() {
        if let Err(e) = write_env_file(&env_file, &updates) {
            error!("[admin] failed to write .env: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    code: 500,
                    reason: format!("Failed to save config: {}", e),
                }),
            ));
        }
        info!(
            "[admin] updated {} config items to {}",
            updates.len(),
            env_file
        );
    }

    if needs_processor_rebuild {
        if let Err(e) = rebuild_insight_processors(&state).await {
            error!("[admin] failed to rebuild processor: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    code: 500,
                    reason: format!("Config saved, but failed to rebuild processor: {}", e),
                }),
            ));
        }
    }

    let cfg = state.editable_config.read().await;
    Ok(Json(AdminConfigResponse {
        arxiv_fetch_cron: cfg.arxiv_fetch_cron.clone(),
        arxiv_query: cfg.arxiv_query.clone(),
        arxiv_max_results: cfg.arxiv_max_results,
        arxiv_page_size: cfg.arxiv_page_size,
        llm_max_workers: cfg.llm_max_workers,
        llm_max_retries: cfg.llm_max_retries,
        pdf_max_workers: cfg.pdf_max_workers,
        insight_max_workers: cfg.insight_max_workers,
        insight_review_max_attempts: cfg.insight_review_max_attempts,
        auto_review_insight: cfg.auto_review_insight,
        mineru_base_url: cfg.mineru_base_url.clone(),
        mineru_no_cache: cfg.mineru_no_cache,
        mineru_api_key: cfg.mineru_api_key.clone(),
    }))
}

// ---------------------------------------------------------------------------
// Provider CRUD
// ---------------------------------------------------------------------------

fn provider_to_detail((idx, p): (usize, ProviderConfig)) -> ProviderDetail {
    ProviderDetail {
        index: idx,
        name: p.name,
        provider_type: p.provider_type,
        base_url: p.base_url,
        api_keys: p.api_keys,
        model: p.model,
        max_tokens: p.max_tokens,
        reasoning_effort: p.reasoning_effort,
        is_digest: p.is_digest,
        is_comment: p.is_comment,
        enabled: p.enabled,
    }
}

/// GET /api/admin/providers
pub(crate) async fn list_providers(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<ProviderListResponse>, (StatusCode, Json<ErrorResponse>)> {
    if !check_auth(&headers, &state.auth_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                code: 401,
                reason: "Unauthorized".into(),
            }),
        ));
    }

    let env_file = env_path();
    match read_providers(&env_file) {
        Ok(list) => Ok(Json(ProviderListResponse {
            providers: list.into_iter().map(provider_to_detail).collect(),
        })),
        Err(e) => {
            error!("[admin] failed to read provider: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    code: 500,
                    reason: format!("Failed to read provider: {}", e),
                }),
            ))
        }
    }
}

/// POST /api/admin/providers — create
pub(crate) async fn create_provider(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ProviderUpdateRequest>,
) -> Result<Json<ProviderDetail>, (StatusCode, Json<ErrorResponse>)> {
    if !check_auth(&headers, &state.auth_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                code: 401,
                reason: "Unauthorized".into(),
            }),
        ));
    }

    let env_file = env_path();
    let pc = ProviderConfig {
        name: req.name,
        provider_type: req.provider_type,
        base_url: req.base_url,
        api_keys: req.api_keys,
        model: req.model,
        max_tokens: req.max_tokens,
        reasoning_effort: req.reasoning_effort,
        user_agent: String::new(),
        is_digest: req.is_digest,
        is_comment: req.is_comment,
        enabled: req.enabled,
    };

    match write_provider(&env_file, None, &pc) {
        Ok(idx) => {
            info!("[admin] created provider[{}]: {}", idx, pc.name);
            if let Err(e) = rebuild_insight_processors(&state).await {
                error!(
                    "[admin] failed to rebuild processor after creating provider: {}",
                    e
                );
            }
            Ok(Json(provider_to_detail((idx, pc))))
        }
        Err(e) => {
            error!("[admin] failed to add provider: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    code: 500,
                    reason: format!("Failed to add provider: {}", e),
                }),
            ))
        }
    }
}

/// PUT /api/admin/providers/{index}
pub(crate) async fn update_provider(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(index): Path<usize>,
    Json(req): Json<ProviderUpdateRequest>,
) -> Result<Json<ProviderDetail>, (StatusCode, Json<ErrorResponse>)> {
    if !check_auth(&headers, &state.auth_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                code: 401,
                reason: "Unauthorized".into(),
            }),
        ));
    }

    let env_file = env_path();
    let pc = ProviderConfig {
        name: req.name,
        provider_type: req.provider_type,
        base_url: req.base_url,
        api_keys: req.api_keys,
        model: req.model,
        max_tokens: req.max_tokens,
        reasoning_effort: req.reasoning_effort,
        user_agent: String::new(),
        is_digest: req.is_digest,
        is_comment: req.is_comment,
        enabled: req.enabled,
    };

    match write_provider(&env_file, Some(index), &pc) {
        Ok(idx) => {
            info!("[admin] updated provider[{}]: {}", idx, pc.name);
            if let Err(e) = rebuild_insight_processors(&state).await {
                error!(
                    "[admin] failed to rebuild processor after updating provider: {}",
                    e
                );
            }
            Ok(Json(provider_to_detail((idx, pc))))
        }
        Err(e) => {
            error!("[admin] failed to update provider: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    code: 500,
                    reason: format!("Failed to update provider: {}", e),
                }),
            ))
        }
    }
}

/// DELETE /api/admin/providers/{index}
pub(crate) async fn delete_provider_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(index): Path<usize>,
) -> Result<Json<HashMap<String, String>>, (StatusCode, Json<ErrorResponse>)> {
    if !check_auth(&headers, &state.auth_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                code: 401,
                reason: "Unauthorized".into(),
            }),
        ));
    }

    let env_file = env_path();
    match delete_provider(&env_file, index) {
        Ok(_) => {
            info!("[admin] deleted provider[{}]", index);
            if let Err(e) = rebuild_insight_processors(&state).await {
                error!(
                    "[admin] failed to rebuild processor after deleting provider: {}",
                    e
                );
            }
            let mut res = HashMap::new();
            res.insert("status".to_string(), "deleted".to_string());
            Ok(Json(res))
        }
        Err(e) => {
            error!("[admin] failed to delete provider: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    code: 500,
                    reason: format!("Failed to delete provider: {}", e),
                }),
            ))
        }
    }
}

/// POST /api/admin/providers/reorder — reorder providers.
pub(crate) async fn reorder_providers(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ReorderProvidersRequest>,
) -> Result<Json<ProviderListResponse>, (StatusCode, Json<ErrorResponse>)> {
    if !check_auth(&headers, &state.auth_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                code: 401,
                reason: "Unauthorized".into(),
            }),
        ));
    }

    let env_file = env_path();
    match crate::env_editor::reorder_providers(&env_file, &req.order) {
        Ok(_) => {
            info!("[admin] Reordered providers: {:?}", req.order);
            if let Err(e) = rebuild_insight_processors(&state).await {
                error!(
                    "[admin] failed to rebuild processor after reordering providers: {}",
                    e
                );
            }
            // Return updated list
            match read_providers(&env_file) {
                Ok(list) => Ok(Json(ProviderListResponse {
                    providers: list.into_iter().map(provider_to_detail).collect(),
                })),
                Err(e) => {
                    error!("[admin] failed to read providers after reorder: {}", e);
                    Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            code: 500,
                            reason: format!("Reorder succeeded but read failed: {}", e),
                        }),
                    ))
                }
            }
        }
        Err(e) => {
            error!("[admin] failed to reorder providers: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    code: 500,
                    reason: format!("Reorder failed: {}", e),
                }),
            ))
        }
    }
}

/// Rebuild insight processors from the current `.env` provider list and
/// `editable_config` worker-count settings. New analysis tasks will pick up
/// the updated configuration; in-flight tasks keep their existing processors.
pub(crate) async fn rebuild_insight_processors(state: &AppState) -> anyhow::Result<()> {
    let env_file = env_path();
    let providers = read_providers(&env_file)?;
    let cfg = state.editable_config.read().await;

    // Read the latest prompt contents (the file watcher keeps these hot).
    let (insight_prompt, review_prompt) = {
        let map = state.insight_prompts.read().await;
        let insight = map.get("default").cloned().unwrap_or_default();
        drop(map);
        let review = tokio::fs::read_to_string("prompts/review.md")
            .await
            .unwrap_or_default();
        (insight, review)
    };

    // Find the provider marked for comment before consuming providers.
    let comment_name = providers
        .iter()
        .find(|(_, p)| p.is_comment == "true")
        .map(|(_, p)| p.name.clone());

    let mut new_processors = Vec::with_capacity(providers.len());
    for (_idx, p) in providers {
        let provider_type = ProviderType::parse(&p.provider_type);
        let reasoning_effort = if p.reasoning_effort.is_empty() {
            None
        } else {
            Some(p.reasoning_effort.clone())
        };
        let output_config_effort = reasoning_effort.clone();
        let user_agent = if p.user_agent.is_empty() {
            None
        } else {
            Some(p.user_agent.clone())
        };
        let max_tokens = p.max_tokens.parse().unwrap_or(0);

        let processor = Processor::new(
            provider_type,
            p.name.clone(),
            p.base_url.clone(),
            p.api_keys.clone(),
            p.model.clone(),
            max_tokens,
            reasoning_effort,
            output_config_effort,
            None,
            user_agent,
            cfg.llm_max_workers,
            cfg.insight_max_workers,
            cfg.llm_max_retries,
            state.summarize_prompt.clone(),
            state.translate_prompt.clone(),
            insight_prompt.clone(),
            review_prompt.clone(),
            None,
        );
        new_processors.push((p.name.clone(), Arc::new(processor)));
    }

    let comment_processor = comment_name
        .as_ref()
        .and_then(|name| new_processors.iter().find(|(n, _)| n == name))
        .map(|(_, proc)| Arc::clone(proc));

    let mut guard = state.insight_processors.write().await;
    *guard = new_processors;
    info!(
        "[admin] Rebuilt {} insight processor(s) with llm_max_workers={}, insight_max_workers={}, max_retries={}",
        guard.len(),
        cfg.llm_max_workers,
        cfg.insight_max_workers,
        cfg.llm_max_retries
    );
    drop(guard);

    // Update the dedicated insight processor in AppState
    let mut insight_guard = state.comment_processor.write().await;
    *insight_guard = comment_processor;
    info!(
        "[admin] Insight comment processor: {}",
        comment_name.unwrap_or_else(|| "(none)".to_string())
    );
    Ok(())
}

fn send_ev(tx: &tokio::sync::broadcast::Sender<String>, json: &str) {
    let _ = tx.send(json.to_string());
}

pub(crate) async fn run_fetch(state: Arc<AppState>) -> anyhow::Result<()> {
    use crate::config::Config;
    use crate::db::Db;
    use crate::pipeline::process_paper_with_cache;
    use crate::processor::{Processor, ProviderType};
    use crate::query::build_query;
    use crate::source::arxiv::fetch_papers;
    use tokio::sync::Semaphore;

    let cfg = Config::from_env()?;
    let db = Db::new(&cfg.database_url).await?;

    let summarizer = cfg
        .summarizer
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No summarizer configured"))?;
    let processor = Processor::new(
        ProviderType::parse(&summarizer.provider_type),
        summarizer.name.clone(),
        summarizer.base_url.clone(),
        summarizer.api_keys.clone(),
        summarizer.model.clone(),
        summarizer.max_tokens,
        summarizer.reasoning_effort.clone(),
        summarizer.output_config_effort.clone(),
        None,
        summarizer.user_agent.clone(),
        cfg.llm_max_workers,
        cfg.insight_max_workers,
        cfg.llm_max_retries,
        cfg.summarize_prompt.clone(),
        cfg.translate_prompt.clone(),
        cfg.insight_prompt.clone(),
        cfg.review_prompt.clone(),
        None,
    );
    let arc_proc = Arc::new(processor);

    let started = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let tx = state.fetch_tx.clone();

    {
        let mut s = state.fetch_status.write().await;
        *s = FetchStatus {
            running: true,
            total: 0,
            completed: 0,
            failed: 0,
            message: "Building query...".into(),
            started_at: Some(started),
            phase: "fetching".into(),
        };
    }

    let query = build_query(&cfg.arxiv_query, &cfg.arxiv_categories, &cfg.arxiv_keywords);

    {
        let mut s = state.fetch_status.write().await;
        s.message = format!("Fetching arXiv... (max_results={})", cfg.arxiv_max_results);
    }

    let papers = fetch_papers(&query, cfg.arxiv_max_results, cfg.arxiv_page_size).await?;
    let total = papers.len();

    {
        let mut s = state.fetch_status.write().await;
        s.total = total;
        s.message = format!("Fetched {} papers, starting processing...", total);
        s.phase = "processing".into();
    }
    send_ev(
        &tx,
        &format!(
            "{{\"type\":\"progress\",\"total\":{},\"completed\":0,\"failed\":0}}",
            total
        ),
    );

    let pdf_sem = Arc::new(Semaphore::new(cfg.pdf_max_workers));
    let db_sem = Arc::new(Semaphore::new(cfg.llm_max_workers));
    let mut handles = Vec::with_capacity(total);

    let _batch_size = (total / 20).max(1);
    for paper in papers.iter() {
        let p = paper.clone();
        let proc = Arc::clone(&arc_proc);
        let db_c = db.clone();
        let pdf_sem_c = Arc::clone(&pdf_sem);
        let state_c = Arc::clone(&state);
        let db_sem_c = Arc::clone(&db_sem);
        let tx_c = tx.clone();

        handles.push(tokio::spawn(async move {
            if state_c
                .fetch_cancel
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return;
            }
            let _permit = db_sem_c.acquire().await;
            match process_paper_with_cache(&proc, &db_c, &p, &pdf_sem_c, false).await {
                Ok((_summary, _text, _cached)) => {
                    let mut s = state_c.fetch_status.write().await;
                    s.completed += 1;
                    let c = s.completed;
                    let f = s.failed;
                    let t = s.total;
                    drop(s);
                    send_ev(
                        &tx_c,
                        &format!(
                            "{{\"type\":\"progress\",\"total\":{},\"completed\":{},\"failed\":{}}}",
                            t, c, f
                        ),
                    );
                    send_ev(
                        &tx_c,
                        &format!("{{\"type\":\"log\",\"text\":\"  [OK] {} (ok)\"}}", p.id),
                    );
                }
                Err(_e) => {
                    let mut s = state_c.fetch_status.write().await;
                    s.failed += 1;
                    let c = s.completed;
                    let f = s.failed;
                    let t = s.total;
                    drop(s);
                    send_ev(
                        &tx_c,
                        &format!(
                            "{{\"type\":\"progress\",\"total\":{},\"completed\":{},\"failed\":{}}}",
                            t, c, f
                        ),
                    );
                    send_ev(
                        &tx_c,
                        &format!("{{\"type\":\"log\",\"text\":\"  [FAIL] {} (fail)\"}}", p.id),
                    );
                }
            }
        }));
    }
    for h in handles {
        let _ = h.await;
    }

    {
        let mut s = state.fetch_status.write().await;
        s.running = false;
        s.message = format!(
            "Done! {} total, {} succeeded, {} failed",
            s.total, s.completed, s.failed
        );
        s.phase = "done".into();
        let msg = s.message.clone();
        let tf = s.total;
        let cf = s.completed;
        let ff = s.failed;
        drop(s);
        send_ev(
            &tx,
            &format!(
                "{{\"type\":\"progress\",\"total\":{},\"completed\":{},\"failed\":{}}}",
                tf, cf, ff
            ),
        );
        send_ev(
            &tx,
            &format!("{{\"type\":\"done\",\"message\":\"{}\"}}", msg),
        );
        info!(
            "[fetch] Task complete: total={}, completed={}, failed={}",
            tf, cf, ff
        );
    }
    Ok(())
}
/// GET /api/admin/logs — return latest log content (last 300 lines).
pub(crate) async fn get_logs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    use super::types::ErrorResponse;
    if !check_auth(&headers, &state.auth_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                code: 401,
                reason: "Unauthorized".into(),
            }),
        ));
    }

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let lp = format!("logs/run.log.{}", today);
    let content = match tokio::fs::read_to_string(&lp).await {
        Ok(c) => c,
        Err(_) => {
            // Fallback: find the most recent log file
            let mut best: Option<(std::time::SystemTime, String)> = None;
            let mut entries = match tokio::fs::read_dir("logs").await {
                Ok(e) => e,
                Err(_) => {
                    return Ok(Json(
                        serde_json::json!({ "logs": Vec::<String>::new(), "path": lp }),
                    ))
                }
            };
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("run.log.") || name.starts_with("run-") {
                    if let Ok(meta) = entry.metadata().await {
                        if let Ok(modified) = meta.modified() {
                            let is_newer = best.as_ref().is_none_or(|b| modified > b.0);
                            if is_newer {
                                best = Some((modified, name));
                            }
                        }
                    }
                }
            }
            match best {
                Some((_, name)) => tokio::fs::read_to_string(format!("logs/{}", name))
                    .await
                    .unwrap_or_default(),
                None => String::new(),
            }
        }
    };

    let lines: Vec<&str> = content.lines().collect();
    let tail: Vec<String> = lines
        .iter()
        .rev()
        .take(300)
        .map(|s| s.to_string())
        .collect();
    let tail: Vec<String> = tail.into_iter().rev().collect();

    Ok(Json(serde_json::json!({ "logs": tail, "path": lp })))
}

/// POST /api/admin/fetch — start arXiv fetch task.
pub(crate) async fn start_fetch(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    if !check_auth(&headers, &state.auth_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                code: 401,
                reason: "Unauthorized".into(),
            }),
        ));
    }

    let status = state.fetch_status.read().await;
    if status.running {
        return Err((
            StatusCode::CONFLICT,
            Json(ErrorResponse {
                code: 409,
                reason: "A fetch task is already running".into(),
            }),
        ));
    }
    drop(status);

    {
        let mut s = state.fetch_status.write().await;
        *s = FetchStatus {
            running: true,
            total: 0,
            completed: 0,
            failed: 0,
            message: "Starting...".into(),
            started_at: Some(chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()),
            phase: "fetching".into(),
        };
    }
    let state_clone = Arc::clone(&state);
    tokio::spawn(async move {
        if let Err(e) = run_fetch(state_clone).await {
            tracing::error!("[fetch] Fetch task failed: {}", e);
        }
    });

    Ok(Json(serde_json::json!({ "status": "started" })))
}

/// GET /api/admin/fetch/status — get fetch task status.
pub(crate) async fn get_fetch_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<FetchStatus>, (StatusCode, Json<ErrorResponse>)> {
    if !check_auth(&headers, &state.auth_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                code: 401,
                reason: "Unauthorized".into(),
            }),
        ));
    }
    let status = state.fetch_status.read().await;
    Ok(Json(status.clone()))
}

/// GET /api/admin/fetch/stream — SSE stream of fetch progress.
/// POST /api/admin/fetch/cancel — stop current fetch task.
pub(crate) async fn cancel_fetch(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    if !check_auth(&headers, &state.auth_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                code: 401,
                reason: "Unauthorized".into(),
            }),
        ));
    }
    state
        .fetch_cancel
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let mut s = state.fetch_status.write().await;
    s.running = false;
    s.message = "Cancelled".into();
    s.phase = "idle".into();
    Ok(Json(serde_json::json!({ "status": "cancelled" })))
}

/// GET /api/admin/fetch/stream - SSE stream of fetch progress.
pub(crate) async fn fetch_stream(State(state): State<Arc<AppState>>) -> axum::response::Response {
    use futures::stream::StreamExt;
    use std::convert::Infallible;
    use tokio_stream::wrappers::BroadcastStream;

    let rx = state.fetch_tx.subscribe();
    let stream = BroadcastStream::new(rx).map(|result| match result {
        Ok(msg) => Ok::<_, Infallible>(format!("data: {}\n\n", msg)),
        Err(_) => Ok("data: keepalive\n\n".to_string()),
    });
    (
        StatusCode::OK,
        [
            ("Content-Type", "text/event-stream"),
            ("Cache-Control", "no-cache"),
        ],
        axum::body::Body::from_stream(stream),
    )
        .into_response()
}
