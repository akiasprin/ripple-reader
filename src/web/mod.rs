// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::{Context, Result};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Html, Json},
    routing::{delete, get, post, put},
    Router,
};
use chrono::{DateTime, Utc};
use notify::Watcher;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceBuilder;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use tracing::{error, info, warn};

use crate::db::{Db, DbPaper, DbPaperUpdate};
use crate::processor::{parse_prompt_file, Processor};

mod types;
pub use types::EditableConfig;
pub(crate) use types::*;
mod auth;
use auth::check_auth;
mod admin;
mod comments;
pub mod figures;
mod frontend;
mod insight;
pub mod insight_html;

/// Static assets: any change should invalidate browser/CDN cache.
const VERSIONED_ASSETS: &[&str] = &[
    "static/app.js",
    "static/style.css",
    "static/pseudocode.min.js",
    "static/pseudocode.min.css",
    "static/vendor/katex.min.css",
    "static/vendor/katex.min.js",
    "static/vendor/auto-render.min.js",
    "static/vendor/prism-latte.css",
    "static/vendor/prism-macchiato.css",
    "static/vendor/prism-autoloader.min.js",
    "static/vendor/prettier-standalone.js",
];

/// Get the latest mtime from first-party assets as a cache-busting token (hex seconds).
/// Changes on every rebuild/deploy; O(1) stat calls, negligible at low traffic.
async fn asset_version() -> String {
    let mut newest = 0u64;
    for path in VERSIONED_ASSETS {
        if let Ok(meta) = tokio::fs::metadata(path).await {
            if let Ok(modified) = meta.modified() {
                if let Ok(dur) = modified.duration_since(std::time::UNIX_EPOCH) {
                    newest = newest.max(dur.as_secs());
                }
            }
        }
    }
    format!("{:x}", newest)
}

async fn index() -> axum::response::Response {
    use axum::response::IntoResponse;
    match tokio::fs::read_to_string("static/index.html").await {
        Ok(html) => {
            let html = html.replace("__ASSET_VER__", &asset_version().await);
            (
                [(
                    axum::http::header::CACHE_CONTROL,
                    HeaderValue::from_static("no-cache"),
                )],
                Html(html),
            )
                .into_response()
        }
        Err(e) => {
            error!("read index.html failed: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "index.html missing").into_response()
        }
    }
}

async fn admin_page() -> axum::response::Response {
    use axum::response::IntoResponse;
    match tokio::fs::read_to_string("static/admin.html").await {
        Ok(html) => {
            let html = html.replace("__ASSET_VER__", &asset_version().await);
            (
                [(
                    axum::http::header::CACHE_CONTROL,
                    HeaderValue::from_static("no-cache"),
                )],
                Html(html),
            )
                .into_response()
        }
        Err(e) => {
            error!("read admin.html failed: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "admin.html missing").into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// Paper storage path helpers: files are organized under source-specific dirs
// ---------------------------------------------------------------------------
pub fn paper_pdf_path(source: &str, id: &str) -> String {
    format!("pdf/{}/{}.pdf", source, id)
}

pub fn paper_figures_dir(source: &str, id: &str) -> String {
    format!("figures/{}/{}", source, id)
}

#[derive(Deserialize)]
struct PromptsQuery {
    provider: Option<String>,
}

#[derive(Serialize)]
struct PromptsResponse {
    insight: String,
    review: String,
    applied_variant: Option<String>,
    applied_variant_mtime: Option<String>,
}

/// Extract the [SYSTEM] section from a prompt file for API responses.
fn extract_system_prompt(content: &str) -> String {
    parse_prompt_file(content).system
}

async fn get_prompts(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PromptsQuery>,
) -> Json<PromptsResponse> {
    let insight_prompts = state.insight_prompts.read().await;
    let (insight, applied_variant) = if let Some(ref provider) = query.provider {
        // Match provider name against variant keys (case-insensitive substring match)
        let matched = insight_prompts
            .iter()
            .find(|(key, _)| provider.to_lowercase().contains(&key.to_lowercase()));
        match matched {
            Some((variant, prompt)) => (extract_system_prompt(prompt), Some(variant.clone())),
            None => {
                let default = insight_prompts
                    .get("default")
                    .map(|s| s.as_str())
                    .unwrap_or("");
                (extract_system_prompt(default), None)
            }
        }
    } else {
        let default = insight_prompts
            .get("default")
            .map(|s| s.as_str())
            .unwrap_or("");
        (extract_system_prompt(default), None)
    };
    drop(insight_prompts);

    let applied_variant_mtime = {
        let mtime_map = state.insight_prompts_mtime.read().await;
        let key = applied_variant.as_deref().unwrap_or("default");
        mtime_map
            .get(key)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
    };

    // Review prompt: read from file directly so it always reflects the latest
    let review_prompt = tokio::fs::read_to_string("prompts/review.md")
        .await
        .unwrap_or_default();

    Json(PromptsResponse {
        insight,
        review: extract_system_prompt(&review_prompt),
        applied_variant,
        applied_variant_mtime,
    })
}

async fn list_papers(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListQuery>,
) -> Result<Json<ListResponse>, StatusCode> {
    let page = params.page.unwrap_or(1).max(1);
    let page_size = params.page_size.unwrap_or(10).clamp(1, 200);
    let offset = (page - 1) * page_size;

    let db = &state.db;

    let mark_filter = params.mark.filter(|m| !m.is_empty());
    let insight_filter = params.insight;
    let tag_filter = params.tag.filter(|t| !t.is_empty());
    let checked_filter = params.checked.filter(|v| !v.is_empty());
    let source_type_filter = params.source.filter(|v| !v.is_empty());

    // Run count and list queries in parallel to reduce latency.
    let (total, papers) = tokio::join!(
        db.count_papers(
            params.search.clone(),
            mark_filter.clone(),
            insight_filter.clone(),
            tag_filter.clone(),
            checked_filter.clone(),
            source_type_filter.clone(),
        ),
        db.list_papers_light(
            page_size,
            offset,
            params.search.clone(),
            mark_filter.clone(),
            insight_filter.clone(),
            tag_filter.clone(),
            checked_filter.clone(),
            params.sort.clone(),
            params.order.clone(),
            source_type_filter.clone(),
        ),
    );
    let total = total.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let papers = papers.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let ids: Vec<String> = papers.iter().map(|p| p.id.clone()).collect();

    // Run marks and tags queries in parallel.
    let (marks, all_tags) = tokio::join!(db.list_marks(ids.clone()), db.list_papers_tags(&ids),);
    let marks = marks.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let all_tags = all_tags.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(ListResponse {
        total,
        page,
        page_size,
        papers: papers
            .into_iter()
            .map(|p| {
                let mark = marks.get(&p.id).cloned();
                let tags: Vec<TagItem> = all_tags
                    .get(&p.id)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(tag, weight)| TagItem { tag, weight })
                    .collect();
                PaperListItemResponse::from_db_list_item(p, mark, tags)
            })
            .collect(),
    }))
}

async fn get_paper(
    State(state): State<Arc<AppState>>,
    Path((_source, id)): Path<(String, String)>,
) -> Result<Json<PaperResponse>, StatusCode> {
    let db = &state.db;
    let paper = db
        .get_paper(&id)
        .await
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    // Run mark and tags queries in parallel to reduce latency.
    let (mark, tags_raw) = tokio::join!(db.get_mark(&id), db.get_paper_tags(&id),);
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

async fn update_paper(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((source, id)): Path<(String, String)>,
    Json(req): Json<UpdateRequest>,
) -> Result<Json<PaperResponse>, (StatusCode, Json<ErrorResponse>)> {
    if !check_auth(&headers, &state.auth_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                code: 401,
                reason: "Unauthorized".into(),
            }),
        ));
    }
    let db = &state.db;
    let paper = db
        .get_paper(&id)
        .await
        .map_err(|e| {
            error!("{}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    code: 500,
                    reason: "Database query failed".into(),
                }),
            )
        })?
        .ok_or((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                code: 404,
                reason: "Paper not found".into(),
            }),
        ))?;

    let updates = DbPaperUpdate {
        title: req.title.as_ref().map(|s| crate::db::normalize_text(s)),
        score: req.score,
        paper_type: req
            .paper_type
            .as_ref()
            .map(|s| crate::db::normalize_text(s)),
        summary: req
            .summary
            .as_ref()
            .map(|s| crate::db::normalize_text_convert_quotes(s)),
        abstract_zh: req
            .abstract_zh
            .as_ref()
            .map(|s| crate::db::normalize_text_convert_quotes(s)),
        abstract_en: req
            .abstract_en
            .as_ref()
            .map(|s| crate::db::normalize_text_convert_quotes(s)),
        insight: req.insight.as_ref().map(|s| crate::db::normalize_text(s)),
        insight_processed_at: None,
        insight_review: req
            .insight_review
            .as_ref()
            .map(|s| crate::db::normalize_text(s)),
        insight_reviewed_at: None,
        checked_at: req.checked_at,
        clear_checked_at: false,
    };

    // Prevent overwriting insight content for already-checked papers
    let is_content_update = updates.insight.is_some() || updates.insight_review.is_some();
    if is_content_update && paper.checked_at.is_some() {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                code: 403,
                reason: "Cannot edit insight on a confirmed paper; unconfirm first".into(),
            }),
        ));
    }

    let updated = db.update_paper(&id, &updates).await.map_err(|e| {
        error!("{}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                code: 500,
                reason: "Save failed".into(),
            }),
        )
    })?;
    if !updated {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                code: 404,
                reason: "Paper not found".into(),
            }),
        ));
    }

    // Refresh insight HTML cache
    if let Some(ref insight) = updates.insight {
        insight_html::refresh_insight_html_cache(&state.db, &source, &id, insight).await;
    }

    let paper = db
        .get_paper(&id)
        .await
        .map_err(|e| {
            error!("{}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    code: 500,
                    reason: "Database query failed".into(),
                }),
            )
        })?
        .ok_or((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                code: 404,
                reason: "Paper not found".into(),
            }),
        ))?;

    let (mark, tags_raw) = tokio::join!(db.get_mark(&id), db.get_paper_tags(&id));
    let mark = mark.map_err(|e| {
        error!("{}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                code: 500,
                reason: "Database query failed".into(),
            }),
        )
    })?;
    let tags_raw = tags_raw.map_err(|e| {
        error!("{}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                code: 500,
                reason: "Database query failed".into(),
            }),
        )
    })?;
    let tags: Vec<TagItem> = tags_raw
        .into_iter()
        .map(|(tag, weight)| TagItem { tag, weight })
        .collect();
    Ok(Json(PaperResponse::from_db(paper, mark, tags)))
}

async fn set_mark(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((_source, id)): Path<(String, String)>,
    Json(req): Json<MarkRequest>,
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

    db.set_mark(&id, req.mark.as_deref()).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Sync top-2 tags into interest preferences when marked as critical
    if req.mark.as_deref() == Some("critical") {
        let _ = db.sync_interest_from_critical_paper(&id).await;
    }

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

async fn delete_paper(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((source, id)): Path<(String, String)>,
) -> Result<StatusCode, StatusCode> {
    if !check_auth(&headers, &state.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    state.db.delete_paper(&id).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    // Clean up associated files
    let figure_dir = paper_figures_dir(&source, &id);
    let _ = tokio::fs::remove_dir_all(&figure_dir).await;
    let pdf_path = paper_pdf_path(&source, &id);
    let _ = tokio::fs::remove_file(&pdf_path).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn restore_paper(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((_source, id)): Path<(String, String)>,
) -> Result<StatusCode, StatusCode> {
    if !check_auth(&headers, &state.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    state.db.restore_paper(&id).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_paper_tokens(
    State(state): State<Arc<AppState>>,
    Path((_source, id)): Path<(String, String)>,
) -> Result<Json<TokenCountResponse>, StatusCode> {
    let paper = state
        .db
        .get_paper(&id)
        .await
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;
    let bpe = tiktoken_rs::o200k_base().map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let insight_tokens = bpe.encode_with_special_tokens(&paper.insight).len();
    let review_tokens = if paper.insight_review.is_empty() {
        0
    } else {
        bpe.encode_with_special_tokens(&paper.insight_review).len()
    };
    Ok(Json(TokenCountResponse {
        insight_tokens,
        review_tokens,
    }))
}

async fn delete_tag_papers(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(tag): Path<String>,
) -> Result<Json<DeleteTagResponse>, StatusCode> {
    if !check_auth(&headers, &state.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let ids = state.db.get_paper_ids_by_tag(&tag).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let papers = state.db.get_papers_by_ids(&ids).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let mut count = 0;
    for paper in papers {
        let mark = state.db.get_mark(&paper.id).await.map_err(|e| {
            error!(
                "[delete_tag_papers] Failed to get mark for paper {}: {}",
                paper.id, e
            );
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        if mark.as_deref() == Some("critical") {
            warn!(
                "[delete_tag_papers] Skipping critical paper {} under tag '{}'",
                paper.id, tag
            );
            continue;
        }
        if state.db.delete_paper(&paper.id).await.map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })? {
            let figure_dir = paper_figures_dir(&paper.source_type, &paper.id);
            let _ = tokio::fs::remove_dir_all(&figure_dir).await;
            let pdf_path = paper_pdf_path(&paper.source_type, &paper.id);
            let _ = tokio::fs::remove_file(&pdf_path).await;
            count += 1;
        }
    }
    info!(
        "[delete_tag_papers] Soft-deleted {} papers under tag '{}' (skipped critical)",
        count, tag
    );
    Ok(Json(DeleteTagResponse { deleted: count }))
}

async fn related_papers(
    State(state): State<Arc<AppState>>,
    Path((_source, id)): Path<(String, String)>,
) -> Result<Json<Vec<RelatedPaper>>, StatusCode> {
    let db = &state.db;
    let related = db.find_related_papers(&id, 5).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    if related.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let ids: Vec<String> = related.iter().map(|(id, _)| id.clone()).collect();
    let papers = db.get_papers_by_ids(&ids).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let paper_map: std::collections::HashMap<String, DbPaper> =
        papers.into_iter().map(|p| (p.id.clone(), p)).collect();
    let result: Vec<RelatedPaper> = related
        .into_iter()
        .filter_map(|(rid, sim)| {
            paper_map.get(&rid).map(|p| RelatedPaper {
                id: p.id.clone(),
                title: p.title.clone(),
                score: p.score,
                similarity: sim,
                source_type: p.source_type.clone(),
            })
        })
        .collect();
    Ok(Json(result))
}

async fn insight_neighbors(
    State(state): State<Arc<AppState>>,
    Path((_source, id)): Path<(String, String)>,
) -> Result<Json<InsightNeighborsResponse>, StatusCode> {
    let (prev, next) = state.db.get_insight_neighbors(&id).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(InsightNeighborsResponse {
        prev: prev.map(|(id, title, source_type)| InsightNeighbor {
            id,
            title,
            source_type,
        }),
        next: next.map(|(id, title, source_type)| InsightNeighbor {
            id,
            title,
            source_type,
        }),
    }))
}

async fn list_tags(State(state): State<Arc<AppState>>) -> Result<Json<Vec<TagInfo>>, StatusCode> {
    let db = &state.db;
    let tags = db.list_tags_with_counts().await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let result: Vec<TagInfo> = tags
        .into_iter()
        .map(|(tag, count)| TagInfo { tag, count })
        .collect();
    Ok(Json(result))
}

async fn tags_dashboard(
    State(state): State<Arc<AppState>>,
) -> Result<Json<TagsDashboardResponse>, StatusCode> {
    let db = &state.db;
    let dash = db.get_tags_dashboard().await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(TagsDashboardResponse {
        tags: dash
            .tags
            .into_iter()
            .map(|(tag, count)| TagInfo { tag, count })
            .collect(),
        orphan_count: dash.orphan_count,
        uninteresting_count: dash.uninteresting_count,
        disinterest_tags: dash
            .disinterest_tags
            .into_iter()
            .map(|(tag, count)| TagPreferenceInfo { tag, count })
            .collect(),
        interest_tags: dash.interest_tags,
    }))
}

async fn list_orphan_tag_papers(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<PaperResponse>>, StatusCode> {
    let db = &state.db;
    let papers = db.list_orphan_tag_papers().await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let ids: Vec<String> = papers.iter().map(|p| p.id.clone()).collect();
    let marks = db.list_marks(ids.clone()).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let all_tags = db.list_papers_tags(&ids).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let result: Vec<PaperResponse> = papers
        .into_iter()
        .map(|p| {
            let mark = marks.get(&p.id).cloned();
            let tags: Vec<TagItem> = all_tags
                .get(&p.id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|(tag, weight)| TagItem { tag, weight })
                .collect();
            PaperResponse::from_db(p, mark, tags)
        })
        .collect();
    Ok(Json(result))
}

async fn list_uninteresting_papers(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<PaperResponse>>, StatusCode> {
    let db = &state.db;
    let papers = db.list_uninteresting_papers().await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let ids: Vec<String> = papers.iter().map(|p| p.id.clone()).collect();
    let marks = db.list_marks(ids.clone()).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let all_tags = db.list_papers_tags(&ids).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let result: Vec<PaperResponse> = papers
        .into_iter()
        .map(|p| {
            let mark = marks.get(&p.id).cloned();
            let tags: Vec<TagItem> = all_tags
                .get(&p.id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|(tag, weight)| TagItem { tag, weight })
                .collect();
            PaperResponse::from_db(p, mark, tags)
        })
        .collect();
    Ok(Json(result))
}

async fn list_disinterest_tags(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<TagPreferenceInfo>>, StatusCode> {
    let db = &state.db;
    let tags = db.list_disinterest_tags_with_counts().await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let result: Vec<TagPreferenceInfo> = tags
        .into_iter()
        .map(|(tag, count)| TagPreferenceInfo { tag, count })
        .collect();
    Ok(Json(result))
}

async fn list_interest_tags(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<String>>, StatusCode> {
    let db = &state.db;
    let tags = db.list_tag_preferences("interest").await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(tags))
}

async fn delete_uninteresting_papers(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<DeleteUninterestingRequest>,
) -> Result<Json<UninterestingDeleteResponse>, StatusCode> {
    if !check_auth(&headers, &state.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let db = &state.db;
    let papers = db
        .list_uninteresting_papers_by_tags(&req.tags)
        .await
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let mut deleted = 0;
    let mut skipped = 0;
    for paper in &papers {
        let mark = db.get_mark(&paper.id).await.ok().flatten();
        if mark.as_deref() == Some("critical") {
            warn!(
                "[delete_uninteresting] Skipping critical paper {}",
                paper.id
            );
            skipped += 1;
            continue;
        }
        if db.delete_paper(&paper.id).await.map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })? {
            let figure_dir = paper_figures_dir(&paper.source_type, &paper.id);
            let _ = tokio::fs::remove_dir_all(&figure_dir).await;
            let pdf_path = paper_pdf_path(&paper.source_type, &paper.id);
            let _ = tokio::fs::remove_file(&pdf_path).await;
            deleted += 1;
        }
    }
    info!(
        "[delete_uninteresting] Deleted {} papers, skipped {} critical",
        deleted, skipped
    );
    Ok(Json(UninterestingDeleteResponse { deleted, skipped }))
}

async fn list_paper_dates(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<DateTreeResponse>>, StatusCode> {
    let tree = state.db.list_paper_dates_tree().await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let response: Vec<DateTreeResponse> = tree
        .into_iter()
        .map(|y| DateTreeResponse {
            year: y.label,
            count: y.count,
            months: y
                .children
                .into_iter()
                .map(|m| MonthNode {
                    month: m.label,
                    count: m.count,
                    days: m
                        .children
                        .into_iter()
                        .map(|d| DayNode {
                            day: d.label,
                            count: d.count,
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect();
    Ok(Json(response))
}

async fn list_papers_by_date(
    State(state): State<Arc<AppState>>,
    Path(date): Path<String>,
) -> Result<Json<Vec<PaperByDateResponse>>, StatusCode> {
    let papers = state.db.list_papers_by_date(&date).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let response: Vec<PaperByDateResponse> = papers
        .into_iter()
        .map(|p| PaperByDateResponse {
            id: p.id,
            title: p.title,
            authors: p.authors,
            abstract_zh: p.abstract_zh,
            score: p.score,
            mark: p.mark,
            source_type: p.source_type,
            tags: p
                .tags
                .into_iter()
                .map(|(tag, weight)| TagItem { tag, weight })
                .collect(),
        })
        .collect();
    Ok(Json(response))
}

#[allow(clippy::too_many_arguments)]
pub async fn run_server(
    db_path: String,
    password: Option<String>,
    port: u16,
    insight_processors: Vec<(String, Arc<Processor>)>,
    comment_processor: Option<Arc<Processor>>,
    summarize_prompt: String,
    translate_prompt: String,
    insight_prompts: Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
    insight_prompts_mtime: Arc<RwLock<std::collections::HashMap<String, DateTime<Utc>>>>,
    editable_config: crate::web::types::EditableConfig,
) -> Result<()> {
    let insight_streams: Arc<RwLock<BTreeMap<String, InsightStreamState>>> =
        Arc::new(RwLock::new(BTreeMap::new()));
    let db = Db::new(&db_path)
        .await
        .context("Failed to initialize database")?;

    // Hash password with bcrypt. Token will be loaded from or stored to DB.
    let password_hash = match password {
        Some(ref plain) => {
            let hash =
                crate::web::auth::hash_password(plain).context("Failed to hash web password")?;
            info!("[auth] Password configured; bcrypt hash generated");
            Some(hash)
        }
        None => {
            info!("[auth] No password configured; running in open mode");
            None
        }
    };

    // Ensure default user exists in DB with the current password hash.
    // Auth token is now persisted in the database, not regenerated on restart.
    let auth_token = if let Err(e) = db.ensure_default_user(password_hash.clone()).await {
        warn!("[init] Failed to ensure default user: {}", e);
        None
    } else {
        let stored_hash = db.get_default_password_hash().await.ok().flatten();
        let existing_token = db.get_default_auth_token().await.ok().flatten();

        // Reuse existing token only if password hash hasn't changed
        match (existing_token, stored_hash) {
            (Some(token), Some(ref h)) if Some(h.clone()) == password_hash => {
                info!("[auth] Reusing existing auth token from database");
                Some(token)
            }
            _ if password_hash.is_some() => {
                let token = crate::web::auth::generate_auth_token();
                if let Err(e) = db.store_auth_token(&token).await {
                    warn!("[auth] Failed to store auth token: {}", e);
                } else {
                    info!("[auth] Generated new persistent auth token");
                }
                Some(token)
            }
            _ => None,
        }
    };

    // Migrate insight headings: replace leading "# " with "> "
    match db.migrate_insight_headings().await {
        Ok(count) => {
            if count > 0 {
                info!(
                    "[init] Migrated {} insight heading(s) from '# ' to '> '",
                    count
                );
            }
        }
        Err(e) => warn!("[init] Failed to migrate insight headings: {}", e),
    }

    // Build author stats from existing papers if table is empty
    match db.get_authors_without_external_stats(1).await {
        Ok(v) if v.is_empty() => {
            info!("[init] Building author stats from existing papers...");
            match db.build_author_stats_from_papers().await {
                Ok(count) => info!("[init] Built {} author stats from existing papers", count),
                Err(e) => warn!("[init] Failed to build author stats: {}", e),
            }
        }
        _ => {}
    }

    // Initialize tag preferences: promote critical paper tags to interest
    if let Err(e) = db.ensure_tag_preferences().await {
        warn!("[init] Failed to ensure tag preferences: {}", e);
    }

    let insight_cancellations: Arc<crate::web::types::InsightCancellationMap> =
        Arc::new(tokio::sync::RwLock::new(BTreeMap::new()));

    // Share the processor list between the web state and the prompt watcher so
    // admin UI rebuilds are visible to the watcher without a restart.
    let insight_processors_arc: crate::web::types::InsightProcessors =
        Arc::new(tokio::sync::RwLock::new(insight_processors));

    // Watch prompts/ directory for changes to prompt files
    let insight_prompts_watch = Arc::clone(&insight_prompts);
    let insight_prompts_mtime_watch = Arc::clone(&insight_prompts_mtime);
    let processors_watch = Arc::clone(&insight_processors_arc);
    tokio::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut watcher = match notify::RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                let _ = tx.send(res);
            },
            notify::Config::default(),
        ) {
            Ok(w) => w,
            Err(e) => {
                warn!("[prompts] Failed to create file watcher: {}", e);
                return;
            }
        };
        if let Err(e) = watcher.watch(
            std::path::Path::new("prompts"),
            notify::RecursiveMode::NonRecursive,
        ) {
            warn!("[prompts] Failed to watch prompts/ directory: {}", e);
            return;
        }
        info!("[prompts] Watching prompts/ directory for changes");
        while let Some(res) = rx.recv().await {
            match res {
                Ok(event) => {
                    if event.kind.is_modify() || event.kind.is_create() || event.kind.is_remove() {
                        let has_prompt = event.paths.iter().any(|p| {
                            p.file_name()
                                .and_then(|n| n.to_str())
                                .map(|n| {
                                    n == "insight.md"
                                        || n == "review.md"
                                        || n == "revise.md"
                                        || (n.starts_with("insight_") && n.ends_with(".md"))
                                })
                                .unwrap_or(false)
                        });
                        if has_prompt {
                            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

                            // Reload review and revise templates into all processors
                            let review_content =
                                std::fs::read_to_string("prompts/review.md").unwrap_or_default();
                            let revise_content =
                                std::fs::read_to_string("prompts/revise.md").unwrap_or_default();
                            {
                                let procs = processors_watch.read().await;
                                for (_, proc) in procs.iter() {
                                    // Read the current default insight prompt content for this processor
                                    let insight_content = {
                                        let map = insight_prompts_watch.read().await;
                                        map.get("default").cloned().unwrap_or_default()
                                    };
                                    proc.reload_templates(
                                        &insight_content,
                                        &review_content,
                                        &revise_content,
                                    );
                                }
                            }

                            let mut map = insight_prompts_watch.write().await;
                            let mut mtime_map = insight_prompts_mtime_watch.write().await;

                            // Save old state for diff
                            let old_map: std::collections::HashMap<String, String> =
                                map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

                            map.clear();
                            mtime_map.clear();
                            let mut new_map = std::collections::HashMap::new();
                            if let Ok(entries) = std::fs::read_dir("prompts") {
                                for entry in entries.filter_map(|e| e.ok()) {
                                    let name = entry.file_name().to_string_lossy().to_string();
                                    let (variant, path) = if name == "insight.md" {
                                        ("default".to_string(), entry.path())
                                    } else if name.starts_with("insight_") && name.ends_with(".md")
                                    {
                                        let v = name
                                            .trim_start_matches("insight_")
                                            .trim_end_matches(".md")
                                            .to_string();
                                        (v, entry.path())
                                    } else {
                                        continue;
                                    };
                                    if let Ok(content) = std::fs::read_to_string(&path) {
                                        if !content.is_empty() {
                                            new_map.insert(variant.clone(), content.clone());
                                            map.insert(variant.clone(), content);
                                            if let Ok(meta) = std::fs::metadata(&path) {
                                                if let Ok(modified) = meta.modified() {
                                                    if let Ok(dur) = modified
                                                        .duration_since(std::time::UNIX_EPOCH)
                                                    {
                                                        if let Some(dt) =
                                                            chrono::DateTime::from_timestamp(
                                                                dur.as_secs() as i64,
                                                                0,
                                                            )
                                                        {
                                                            mtime_map.insert(variant, dt);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            // Skip if nothing actually changed (e.g. file was touched but content identical)
                            if old_map == new_map {
                                continue;
                            }

                            // Compute and print diff
                            let old_keys: std::collections::HashSet<_> = old_map.keys().collect();
                            let new_keys: std::collections::HashSet<_> = new_map.keys().collect();

                            for key in old_keys.union(&new_keys) {
                                let key_str = key.as_str();
                                match (old_map.get(*key), new_map.get(*key)) {
                                    (Some(old), Some(new)) if old != new => {
                                        info!("[prompts] Modified insight prompt: {}", key_str);
                                        // Line-by-line diff
                                        let old_lines: Vec<&str> = old.lines().collect();
                                        let new_lines: Vec<&str> = new.lines().collect();
                                        let mut old_idx = 0usize;
                                        let mut new_idx = 0usize;
                                        while old_idx < old_lines.len() || new_idx < new_lines.len()
                                        {
                                            match (old_lines.get(old_idx), new_lines.get(new_idx)) {
                                                (Some(a), Some(b)) if a == b => {
                                                    old_idx += 1;
                                                    new_idx += 1;
                                                }
                                                _ => {
                                                    // Try to find next matching point
                                                    let mut found = false;
                                                    for look_ahead in 1..=3 {
                                                        if let Some(next_old) =
                                                            old_lines.get(old_idx + look_ahead)
                                                        {
                                                            if new_lines.get(new_idx)
                                                                == Some(next_old)
                                                            {
                                                                for i in 0..look_ahead {
                                                                    info!(
                                                                        "[prompts] - {}: {}",
                                                                        key_str,
                                                                        old_lines[old_idx + i]
                                                                    );
                                                                }
                                                                old_idx += look_ahead;
                                                                found = true;
                                                                break;
                                                            }
                                                        }
                                                        if let Some(next_new) =
                                                            new_lines.get(new_idx + look_ahead)
                                                        {
                                                            if old_lines.get(old_idx)
                                                                == Some(next_new)
                                                            {
                                                                for i in 0..look_ahead {
                                                                    info!(
                                                                        "[prompts] + {}: {}",
                                                                        key_str,
                                                                        new_lines[new_idx + i]
                                                                    );
                                                                }
                                                                new_idx += look_ahead;
                                                                found = true;
                                                                break;
                                                            }
                                                        }
                                                    }
                                                    if !found {
                                                        if old_idx < old_lines.len() {
                                                            info!(
                                                                "[prompts] - {}: {}",
                                                                key_str, old_lines[old_idx]
                                                            );
                                                            old_idx += 1;
                                                        }
                                                        if new_idx < new_lines.len() {
                                                            info!(
                                                                "[prompts] + {}: {}",
                                                                key_str, new_lines[new_idx]
                                                            );
                                                            new_idx += 1;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    (None, Some(_)) => {
                                        info!("[prompts] Added insight prompt: {}", key_str);
                                    }
                                    (Some(_), None) => {
                                        info!("[prompts] Removed insight prompt: {}", key_str);
                                    }
                                    _ => {}
                                }
                            }

                            info!(
                                "[prompts] Reloaded insight prompts: {:?}",
                                map.keys().collect::<Vec<_>>()
                            );
                            drop(map);
                            drop(mtime_map);
                        }
                    }
                }
                Err(e) => warn!("[prompts] Watch error: {}", e),
            }
        }
    });

    let state = Arc::new(AppState {
        db,
        password_hash,
        auth_token,
        insight_processors: Arc::clone(&insight_processors_arc),
        comment_processor: Arc::new(tokio::sync::RwLock::new(comment_processor)),
        summarize_prompt,
        translate_prompt,
        insight_prompts,
        insight_prompts_mtime,
        insight_streams,
        insight_cancellations,
        editable_config: Arc::new(tokio::sync::RwLock::new(editable_config)),
        fetch_status: Arc::new(tokio::sync::RwLock::new(Default::default())),
        fetch_tx: tokio::sync::broadcast::channel(65536).0,
        fetch_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });

    // Start cron scheduler for arXiv fetch using the `cron` crate.
    // Computes the exact time until the next scheduled execution and sleeps
    // precisely to that moment, avoiding wasteful minute-by-minute polling.
    {
        let state_cron = Arc::clone(&state);
        tokio::spawn(async move {
            use chrono::{FixedOffset, Utc};
            use cron::Schedule;
            use std::str::FromStr;
            use tokio::time::{sleep, Duration};

            // Force UTC+8 regardless of server system timezone
            let tz = FixedOffset::east_opt(8 * 3600).expect("UTC+8 is valid");
            let normalize_cron_expr = |expr: &str| -> String {
                let trimmed = expr.trim();
                if trimmed.split_whitespace().count() == 5 {
                    format!("0 {}", trimmed)
                } else {
                    trimmed.to_string()
                }
            };

            loop {
                let cfg = state_cron.editable_config.read().await;
                let raw_expr = cfg.arxiv_fetch_cron.trim().to_string();
                drop(cfg);

                if raw_expr.is_empty() || raw_expr.eq_ignore_ascii_case("disabled") {
                    sleep(Duration::from_secs(60)).await;
                    continue;
                }

                let expr = normalize_cron_expr(&raw_expr);
                if expr != raw_expr {
                    tracing::info!(
                        "[cron] Normalized legacy cron expression '{}' to '{}'",
                        raw_expr,
                        expr
                    );
                }

                let schedule = match Schedule::from_str(&expr) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!("[cron] Invalid cron expression '{}': {}", expr, e);
                        sleep(Duration::from_secs(60)).await;
                        continue;
                    }
                };

                let now = Utc::now().with_timezone(&tz);
                let next = match schedule.upcoming(tz).next() {
                    Some(t) => t,
                    None => {
                        tracing::warn!("[cron] No upcoming executions for '{}'", expr);
                        sleep(Duration::from_secs(3600)).await;
                        continue;
                    }
                };

                let dur = (next - now).to_std().unwrap_or(Duration::from_secs(0));
                tracing::info!("[cron] Next arXiv fetch at {} CST (in {:?})", next, dur);
                sleep(dur).await;

                // Re-read config after waking to verify the cron expression
                // hasn't changed while we were sleeping.
                let cfg = state_cron.editable_config.read().await;
                let current_raw_expr = cfg.arxiv_fetch_cron.trim().to_string();
                drop(cfg);
                if normalize_cron_expr(&current_raw_expr) != expr {
                    tracing::info!("[cron] Cron expression changed while sleeping, recalculating");
                    continue;
                }

                let status = state_cron.fetch_status.read().await;
                let already_running = status.running;
                drop(status);
                if already_running {
                    tracing::warn!("[cron] Fetch already running, skipping scheduled run");
                    continue;
                }

                let s = state_cron.fetch_status.clone();
                let s2 = Arc::clone(&state_cron);
                tokio::spawn(async move {
                    let mut st = s.write().await;
                    *st = FetchStatus {
                        running: true,
                        total: 0,
                        completed: 0,
                        failed: 0,
                        message: "Scheduled fetch...".into(),
                        started_at: Some(
                            chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                        ),
                        phase: "fetching".into(),
                    };
                    drop(st);
                    if let Err(e) = admin::run_fetch(s2).await {
                        tracing::error!("[cron] Fetch failed: {}", e);
                    }
                });
            }
        });
    }

    // Choose recovery processor: prefer the dedicated comment processor, fallback to first.
    let recovery_processor = {
        let comment_proc = state.comment_processor.read().await;
        if let Some(ref p) = *comment_proc {
            Some(Arc::clone(p))
        } else {
            let procs = state.insight_processors.read().await;
            procs.first().map(|(_, s)| Arc::clone(s))
        }
    };

    // Recover any interrupted insight analysis and review tasks
    if let Some(processor) = recovery_processor {
        let state_clone = Arc::clone(&state);
        let processor_clone = Arc::clone(&processor);
        tokio::spawn(async move {
            let analyzing_papers = {
                let db = &state_clone.db;
                match db.list_analyzing_papers().await {
                    Ok(papers) => papers,
                    Err(e) => {
                        warn!("[recovery] Failed to list analyzing papers: {}", e);
                        return;
                    }
                }
            };
            if !analyzing_papers.is_empty() {
                info!(
                    "[recovery] Found {} paper(s) stuck in ANALYZING state, restarting tasks",
                    analyzing_papers.len()
                );
                for paper in analyzing_papers {
                    info!(
                        "[recovery] Restarting insight analysis for {}: {}",
                        paper.id, paper.title
                    );
                    let stream_state = crate::web::types::InsightStreamState::new();
                    {
                        let mut streams = state_clone.insight_streams.write().await;
                        streams.insert(paper.id.clone(), stream_state.clone());
                    }
                    let cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let state_for_task = Arc::clone(&state_clone);
                    let processor_for_task = Arc::clone(&processor_clone);
                    let state_for_cleanup = Arc::clone(&state_clone);
                    let id = paper.id.clone();
                    let title = paper.title.clone();
                    let abstract_text = paper.abstract_zh.clone();
                    let stream_state_for_task = stream_state.clone();
                    let source = paper.source_type.clone();
                    let handle = tokio::spawn(async move {
                        insight::run_insight_analysis(
                            state_for_task,
                            processor_for_task,
                            source,
                            id.clone(),
                            title,
                            abstract_text,
                            None,
                            vec![],
                            None,
                            None,
                            None,
                            stream_state_for_task,
                        )
                        .await;
                        let mut cancels = state_for_cleanup.insight_cancellations.write().await;
                        cancels.remove(&id);
                    });
                    {
                        let mut cancels = state_clone.insight_cancellations.write().await;
                        cancels.insert(paper.id.clone(), (cancel_flag, handle));
                    }
                }
            }

            let reviewing_papers = {
                let db = &state_clone.db;
                match db.list_reviewing_papers().await {
                    Ok(papers) => papers,
                    Err(e) => {
                        warn!("[recovery] Failed to list reviewing papers: {}", e);
                        return;
                    }
                }
            };
            if !reviewing_papers.is_empty() {
                info!(
                    "[recovery] Found {} paper(s) stuck in REVIEWING state, restarting tasks",
                    reviewing_papers.len()
                );
                for paper in reviewing_papers {
                    info!(
                        "[recovery] Restarting review for {}: {}",
                        paper.id, paper.title
                    );
                    let cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let cancel_flag_task = Arc::clone(&cancel_flag);
                    let state_for_task = Arc::clone(&state_clone);
                    let processor_for_task = Arc::clone(&processor_clone);
                    let state_for_cleanup = Arc::clone(&state_clone);
                    let id = paper.id.clone();
                    let title = paper.title.clone();
                    let insight = paper.insight.clone();
                    let source = paper.source_type.clone();
                    let handle = tokio::spawn(async move {
                        insight::run_review_analysis(
                            state_for_task,
                            processor_for_task,
                            source,
                            id.clone(),
                            title,
                            insight,
                            None,
                            cancel_flag_task,
                        )
                        .await;
                        let mut cancels = state_for_cleanup.insight_cancellations.write().await;
                        cancels.remove(&id);
                    });
                    {
                        let mut cancels = state_clone.insight_cancellations.write().await;
                        cancels.insert(paper.id.clone(), (cancel_flag, handle));
                    }
                }
            }
        });
    }

    let static_svc = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::if_not_present(
            axum::http::header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache, no-store, must-revalidate"),
        ))
        .service(ServeDir::new("static"));

    let app = Router::new()
        .nest_service("/static", static_svc)
        .route("/", get(index))
        .route("/tags", get(index))
        .route("/dates", get(index))
        .route("/insight/{*rest}", get(index))
        .route("/admin", get(admin_page))
        .route("/api/auth", get(auth::auth_status).post(auth::auth_login))
        .route(
            "/api/admin/config",
            get(admin::get_config).post(admin::update_config),
        )
        .route(
            "/api/admin/providers",
            get(admin::list_providers).post(admin::create_provider),
        )
        .route(
            "/api/admin/providers/{index}",
            put(admin::update_provider).delete(admin::delete_provider_handler),
        )
        .route(
            "/api/admin/providers/reorder",
            post(admin::reorder_providers),
        )
        .route("/api/admin/logs", get(admin::get_logs))
        .route(
            "/api/admin/fetch",
            get(admin::get_fetch_status).post(admin::start_fetch),
        )
        .route("/api/admin/fetch/stream", get(admin::fetch_stream))
        .route("/api/admin/fetch/cancel", post(admin::cancel_fetch))
        .route("/api/papers", get(list_papers))
        .route("/api/papers/dates", get(list_paper_dates))
        .route("/api/papers/dates/{date}", get(list_papers_by_date))
        .route(
            "/api/papers/{source}/{id}",
            get(get_paper).put(update_paper).delete(delete_paper),
        )
        .route("/api/papers/{source}/{id}/restore", post(restore_paper))
        .route("/api/papers/{source}/{id}/tokens", get(get_paper_tokens))
        .route(
            "/api/papers/{source}/{id}/mark",
            get(get_paper).put(set_mark),
        )
        .route(
            "/api/papers/{source}/{id}/insight",
            post(insight::insight_analyze).delete(insight::clear_insight),
        )
        .route(
            "/api/papers/{source}/{id}/insight-html",
            get(insight_html::insight_html),
        )
        .route(
            "/api/papers/{source}/{id}/insight-stream",
            get(insight::insight_stream),
        )
        .route(
            "/api/papers/{source}/{id}/checked",
            post(insight::toggle_checked),
        )
        .route(
            "/api/papers/{source}/{id}/review",
            post(insight::review_paper).delete(insight::cancel_review),
        )
        .route(
            "/api/papers/{source}/{id}/insight-backups",
            get(insight::list_paper_insight_backups).post(insight::restore_insight_backup),
        )
        .route(
            "/api/papers/{source}/{id}/insight-backups/{backup_id}",
            delete(insight::delete_insight_backup),
        )
        .route(
            "/api/papers/{source}/{id}/format-insight",
            post(insight::format_insight),
        )
        .route("/api/prompts", get(get_prompts))
        .route(
            "/api/insight-providers",
            get(insight::list_insight_providers),
        )
        .route("/api/insight-progress", get(insight::insight_progress))
        .route("/api/insights", delete(insight::clear_all_insights))
        .route("/api/papers/{source}/{id}/related", get(related_papers))
        .route(
            "/api/papers/{source}/{id}/insight-neighbors",
            get(insight_neighbors),
        )
        .route("/api/tags", get(list_tags))
        .route("/api/tags/dashboard", get(tags_dashboard))
        .route("/api/tags/{tag}/papers", delete(delete_tag_papers))
        .route("/api/papers/orphan-tags", get(list_orphan_tag_papers))
        .route(
            "/api/papers/uninteresting",
            get(list_uninteresting_papers).delete(delete_uninteresting_papers),
        )
        .route(
            "/api/tag-preferences/disinterest",
            get(list_disinterest_tags),
        )
        .route("/api/tag-preferences/interest", get(list_interest_tags))
        .route(
            "/api/papers/{source}/{id}/comments",
            get(comments::list_comments).post(comments::add_comment),
        )
        .route(
            "/api/papers/{source}/{id}/ai-comments",
            post(comments::add_ai_comment),
        )
        .route("/api/comments/{id}/apply", post(comments::apply_ai_comment))
        .route(
            "/api/comments/{id}/reject",
            post(comments::reject_ai_comment),
        )
        .route(
            "/api/comments/{id}/regenerate",
            post(comments::regenerate_ai_comment),
        )
        .route("/api/comments/{id}", delete(comments::delete_comment))
        .route(
            "/api/papers/{source}/{id}/page/{n}",
            get(figures::get_paper_page),
        )
        .route(
            "/api/papers/{source}/{id}/page/{n}/thumb",
            get(figures::get_page_thumb),
        )
        .route(
            "/api/papers/{source}/{id}/figure/{name}",
            get(figures::get_paper_figure),
        )
        .route(
            "/api/papers/{source}/{id}/figure/{name}/thumb",
            get(figures::get_figure_thumb),
        )
        .route(
            "/api/papers/{source}/{id}/table/{name}",
            get(figures::get_paper_table),
        )
        .route(
            "/api/papers/{source}/{id}/table/{name}/thumb",
            get(figures::get_table_thumb),
        )
        .route(
            "/api/papers/{source}/{id}/overlay",
            get(figures::get_paper_overlay),
        )
        .route(
            "/api/papers/{source}/{id}/page/{page}/caption-candidates",
            get(figures::get_caption_candidates),
        )
        .route(
            "/api/papers/{source}/{id}/manual-figures",
            post(figures::save_manual_figure),
        )
        .route(
            "/api/papers/{source}/{id}/manual-figures/{name}",
            delete(figures::delete_manual_figure),
        )
        .route(
            "/api/papers/{source}/{id}/smart-detect",
            post(figures::smart_detect),
        )
        .route(
            "/api/papers/{source}/{id}/bbox-adjustments",
            get(figures::get_bbox_adjustments).post(figures::save_bbox_adjustment),
        )
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive());

    // Resume any AI comments that were left in "pending" state after a restart
    comments::resume_pending_ai_comments(&state);

    let app = app.with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    info!("[web] Starting server on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
