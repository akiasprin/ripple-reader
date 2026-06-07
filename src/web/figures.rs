// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::{Context, Result};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Json,
};
use sha1::{Digest, Sha1};
use std::sync::Arc;
use tracing::{error, info, warn};

use super::types::{
    CaptionCandidate, CaptionCandidatesResponse, OverlayBbox, OverlayPage, OverlayResponse,
    SaveManualFigureRequest, SaveManualFigureResponse, SmartDetectResponse,
};
use crate::figure::detect::smart_detect_sync;
use crate::pdf::mutool::render_page_png;

async fn ensure_pdf(source: &str, id: &str, db: &crate::db::Db) -> Result<(), StatusCode> {
    let pdf_path = super::paper_pdf_path(source, id);
    if !std::path::Path::new(&pdf_path).exists() {
        let paper = db.get_paper(id).await.map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        let pdf_url = match paper.as_ref() {
            Some(p) => match p.source_type.as_str() {
                "arxiv" => format!(
                    "https://arxiv.org/pdf/{}.pdf",
                    p.external_id.as_ref().unwrap_or(&p.id)
                ),
                "openreview" => p
                    .external_id
                    .as_ref()
                    .map(|eid| format!("https://openreview.net/pdf?id={}", eid))
                    .unwrap_or_else(|| p.source_url.clone().unwrap_or_default()),
                _ => p.source_url.clone().unwrap_or_default(),
            },
            None => {
                format!("https://arxiv.org/pdf/{}.pdf", id)
            }
        };
        crate::pdf::download_pdf(source, id, &pdf_url)
            .await
            .map_err(|_| StatusCode::NOT_FOUND)?;
    }
    Ok(())
}

pub(crate) async fn get_paper_page(
    State(state): State<Arc<super::AppState>>,
    Path((source, id, page)): Path<(String, String, usize)>,
) -> Result<axum::response::Response, StatusCode> {
    ensure_pdf(&source, &id, &state.db).await?;
    let pdf_path = super::paper_pdf_path(&source, &id);

    let png_bytes = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
        render_page_png(&pdf_path, page, 150)
    })
    .await
    .map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .map_err(|_| StatusCode::NOT_FOUND)?;

    let response = axum::response::Response::builder()
        .header(axum::http::header::CONTENT_TYPE, "image/png")
        .body(axum::body::Body::from(png_bytes))
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(response)
}

// ---- LQIP thumb endpoints ----

pub(crate) async fn get_page_thumb(
    State(state): State<Arc<super::AppState>>,
    Path((source, id, page)): Path<(String, String, usize)>,
) -> Result<axum::response::Response, StatusCode> {
    ensure_pdf(&source, &id, &state.db).await?;
    let pdf_path = super::paper_pdf_path(&source, &id);

    let png_bytes = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
        render_page_png(&pdf_path, page, 25)
    })
    .await
    .map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .map_err(|_| StatusCode::NOT_FOUND)?;

    let response = axum::response::Response::builder()
        .header(axum::http::header::CONTENT_TYPE, "image/png")
        .header(axum::http::header::CACHE_CONTROL, "public, max-age=86400")
        .body(axum::body::Body::from(png_bytes))
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(response)
}

pub(crate) async fn get_figure_thumb(
    Path((source, id, name)): Path<(String, String, String)>,
) -> Result<axum::response::Response, StatusCode> {
    let (data, _) = read_figure_raw(&source, &id, &name).await?;
    let thumb = tokio::task::spawn_blocking(move || thumbnail_png(&data, 40))
        .await
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let response = axum::response::Response::builder()
        .header(axum::http::header::CONTENT_TYPE, "image/png")
        .header(axum::http::header::CACHE_CONTROL, "public, max-age=86400")
        .body(axum::body::Body::from(thumb))
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(response)
}

pub(crate) async fn get_table_thumb(
    Path((source, id, name)): Path<(String, String, String)>,
) -> Result<axum::response::Response, StatusCode> {
    get_figure_thumb(Path((source, id, name))).await
}

// ---- helpers ----

fn thumbnail_png(data: &[u8], width: u32) -> std::result::Result<Vec<u8>, anyhow::Error> {
    let img = image::load_from_memory(data)?;
    let h = (img.height() as f64 * width as f64 / img.width() as f64).round() as u32;
    let thumb = img.thumbnail(width, h);
    let mut buf = std::io::Cursor::new(Vec::new());
    thumb.write_to(&mut buf, image::ImageFormat::Png)?;
    Ok(buf.into_inner())
}

// ---------------------------------------------------------------------------
// Caption candidates: given a body_bbox on a specific page, return nearby
// text blocks that look like captions, scored by proximity and keyword match.
// Reads from `layout.json` inside the cached `mineru.zip`.
// ---------------------------------------------------------------------------

pub(crate) async fn get_caption_candidates(
    Path((source, id, page)): Path<(String, String, i32)>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<CaptionCandidatesResponse>, (StatusCode, Json<super::types::ErrorResponse>)> {
    use super::types::ErrorResponse;

    let err = |code: u16, reason: String| -> (StatusCode, Json<ErrorResponse>) {
        (
            StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(ErrorResponse { code, reason }),
        )
    };

    let body_bbox: [f32; 4] = match params.get("body_bbox") {
        Some(s) => {
            let parts: Vec<f32> = s.split(',').filter_map(|p| p.parse().ok()).collect();
            if parts.len() == 4 {
                [parts[0], parts[1], parts[2], parts[3]]
            } else {
                return Err(err(
                    400,
                    "body_bbox must be 4 comma-separated floats".into(),
                ));
            }
        }
        None => return Err(err(400, "missing body_bbox param".into())),
    };

    let zip_path = format!("{}/mineru.zip", super::paper_figures_dir(&source, &id));
    if !std::path::Path::new(&zip_path).exists() {
        return Err(err(404, "mineru.zip not found".into()));
    }

    let candidates = tokio::task::spawn_blocking(move || -> Result<Vec<CaptionCandidate>> {
        let file = std::fs::File::open(&zip_path).context("open mineru.zip")?;
        let mut archive = zip::ZipArchive::new(file).context("read mineru.zip")?;
        let mut buf = String::new();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            if entry.name().ends_with("layout.json") {
                use std::io::Read;
                entry.read_to_string(&mut buf)?;
                break;
            }
        }
        if buf.is_empty() {
            anyhow::bail!("layout.json not found in mineru.zip");
        }
        let doc: crate::mineru::LayoutDoc =
            serde_json::from_str(&buf).context("parse layout.json")?;

        let page_data = doc.pdf_info.into_iter().find(|p| p.page_idx == page - 1);
        let page = match page_data {
            Some(p) => p,
            None => return Ok(Vec::new()),
        };

        let [bx0, by0, bx1, by1] = body_bbox;
        let body_height = by1 - by0;

        let mut candidates: Vec<CaptionCandidate> = Vec::new();

        let all_blocks: Vec<&crate::mineru::LayoutParaBlock> = page
            .para_blocks
            .iter()
            .chain(page.preproc_blocks.iter())
            .chain(page.discarded_blocks.iter())
            .collect();

        for block in all_blocks {
            let bb = if block.bbox.len() >= 4 {
                [block.bbox[0], block.bbox[1], block.bbox[2], block.bbox[3]]
            } else {
                continue;
            };

            // Extract text from all lines/spans in this block
            let mut text_parts: Vec<String> = Vec::new();
            for sub in &block.blocks {
                for line in &sub.lines {
                    for span in &line.spans {
                        if let Some(ref content) = span.content {
                            if !content.trim().is_empty() {
                                text_parts.push(content.trim().to_string());
                            }
                        }
                    }
                }
            }
            // Fallback: lines directly on para_block
            for line in &block.lines {
                for span in &line.spans {
                    if let Some(ref content) = span.content {
                        if !content.trim().is_empty() {
                            text_parts.push(content.trim().to_string());
                        }
                    }
                }
            }

            if text_parts.is_empty() {
                continue;
            }

            let text = text_parts.join(" ");
            let [tx0, ty0, tx1, ty1] = bb;

            // Spatial filter: must be near the body bbox.
            // Prefer below the body, but allow adjacent (within body_height margin).
            let vertical_gap = if ty0 >= by1 {
                ty0 - by1 // below
            } else if ty1 <= by0 {
                by0 - ty1 // above
            } else {
                0.0 // overlapping vertically
            };

            // Horizontal overlap ratio
            let overlap_left = bx0.max(tx0);
            let overlap_right = bx1.min(tx1);
            let h_overlap = (overlap_right - overlap_left).max(0.0);
            let body_width = (bx1 - bx0).max(1.0);
            let h_overlap_ratio = h_overlap / body_width;

            // Maximum allowed vertical distance: 2x body height or 200 points
            let max_v_dist = (body_height * 2.0).max(200.0);
            if vertical_gap > max_v_dist || h_overlap_ratio < 0.1 {
                continue;
            }

            // Text matching score
            let lower = text.to_lowercase();
            let keyword_score = if lower.contains("figure") || lower.contains("fig.") {
                1.0
            } else if lower.contains("table") {
                0.9
            } else if lower.contains("chart") {
                0.8
            } else if lower.contains("algorithm") {
                0.7
            } else {
                0.3
            };

            // Proximity score: closer is better (exponential decay)
            let proximity_score = (-vertical_gap / 50.0).exp();

            // Combined score
            let score = keyword_score * 0.6 + proximity_score * 0.3 + h_overlap_ratio * 0.1;

            candidates.push(CaptionCandidate {
                text,
                bbox: bb,
                score: score.min(1.0),
            });
        }

        // Sort by score descending
        candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        candidates.truncate(10);
        Ok(candidates)
    })
    .await
    .map_err(|e| err(500, format!("spawn blocking failed: {}", e)))?
    .map_err(|e| err(500, format!("{}", e)))?;

    Ok(Json(CaptionCandidatesResponse { candidates }))
}

// ---------------------------------------------------------------------------
// Bbox overlay endpoint: returns every extracted figure's bbox grouped by page
// so the standalone `static/overlay.html` viewer can draw an SVG overlay on top
// of the rendered page PNG.  Reads from `figures/{id}/mineru.json` for bboxes
// and from the cached `mineru.zip`'s `layout.json` for page sizes.
// ---------------------------------------------------------------------------

pub(crate) async fn get_paper_overlay(
    Path((source, id)): Path<(String, String)>,
) -> Result<Json<OverlayResponse>, StatusCode> {
    let fig_dir = super::paper_figures_dir(&source, &id);
    let meta_path = format!("{}/mineru.json", fig_dir);
    let zip_path = format!("{}/mineru.zip", fig_dir);

    if !std::path::Path::new(&meta_path).exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    let fig_dir_for_spawn = fig_dir.clone();
    let (meta, page_sizes) = tokio::task::spawn_blocking(move || -> Result<(crate::mineru::CacheMeta, std::collections::HashMap<i32, [f32; 2]>)> {
        let meta_json = std::fs::read_to_string(&meta_path)
            .context("read mineru.json")?;
        let mut meta: crate::mineru::CacheMeta = serde_json::from_str(&meta_json)
            .context("parse mineru.json")?;

        // Apply manual bbox adjustments if present
        let adj_path = format!("{}/bbox_adjustments.json", fig_dir_for_spawn);
        if let Ok(adj_json) = std::fs::read_to_string(&adj_path) {
            if let Ok(adjs) = serde_json::from_str::<crate::mineru::BboxAdjustments>(&adj_json) {
                for (i, name) in meta.image_names.iter().enumerate() {
                    if let Some(adj) = adjs.get(name) {
                        if let Some(b) = meta.image_bboxes.get_mut(i) {
                            b.bbox = adj.apply(b.bbox);
                        }
                        if let Some(b) = meta.body_bboxes.get_mut(i) {
                            b.bbox = adj.apply(b.bbox);
                        }
                    }
                }
            }
        }

        let mut page_sizes: std::collections::HashMap<i32, [f32; 2]> = Default::default();
        if std::path::Path::new(&zip_path).exists() {
            let file = std::fs::File::open(&zip_path)?;
            let mut archive = zip::ZipArchive::new(file)?;
            let mut buf = String::new();
            for i in 0..archive.len() {
                let mut entry = archive.by_index(i)?;
                if entry.name().ends_with("layout.json") {
                    use std::io::Read;
                    entry.read_to_string(&mut buf)?;
                    break;
                }
            }
            if !buf.is_empty() {
                let doc: crate::mineru::LayoutDoc = serde_json::from_str(&buf)
                    .context("parse layout.json")?;
                for page in &doc.pdf_info {
                    if page.page_size.len() >= 2 {
                        page_sizes.insert(page.page_idx, [page.page_size[0], page.page_size[1]]);
                    }
                }
            }
        }
        Ok((meta, page_sizes))
    })
    .await
    .map_err(|e| { error!("{}", e); StatusCode::INTERNAL_SERVER_ERROR })?
    .map_err(|e| {
        warn!("[overlay] {}/{}: {}", source, id, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let default_size: [f32; 2] = [612.0, 792.0];

    let mut by_page: std::collections::BTreeMap<i32, Vec<OverlayBbox>> = Default::default();
    let n = meta.image_bboxes.len().min(meta.image_descriptions.len());
    for i in 0..n {
        let img = &meta.image_bboxes[i];
        let body = meta.body_bboxes.get(i).map(|b| b.bbox).unwrap_or(img.bbox);
        let desc = meta.image_descriptions[i].clone();
        let name = meta.image_names.get(i).cloned().unwrap_or_default();
        let caption_number = crate::mineru::extract_caption_number(&desc);
        by_page.entry(img.page_idx).or_default().push(OverlayBbox {
            bbox: img.bbox,
            body_bbox: body,
            page_idx: img.page_idx,
            content_type: img.content_type.clone(),
            desc,
            name,
            caption_number,
        });
    }

    // Merge manual figures
    let manual_path = format!("{}/manual_figures.json", fig_dir);
    if let Ok(manual_json) = tokio::fs::read_to_string(&manual_path).await {
        if let Ok(manual) = serde_json::from_str::<crate::mineru::ManualFigures>(&manual_json) {
            // Also apply bbox adjustments to manual figures
            let adj_path = format!("{}/bbox_adjustments.json", fig_dir);
            let adjustments: Option<crate::mineru::BboxAdjustments> =
                if let Ok(adj_json) = tokio::fs::read_to_string(&adj_path).await {
                    serde_json::from_str(&adj_json).ok()
                } else {
                    None
                };
            for mf in manual {
                let mut bbox = mf.bbox;
                let mut body_bbox = mf.body_bbox;
                if let Some(ref adjs) = adjustments {
                    if let Some(adj) = adjs.get(&mf.name) {
                        bbox = adj.apply(bbox);
                        body_bbox = adj.apply(body_bbox);
                    }
                }
                let caption_number = crate::mineru::extract_caption_number(&mf.desc);
                by_page.entry(mf.page_idx).or_default().push(OverlayBbox {
                    bbox,
                    body_bbox,
                    page_idx: mf.page_idx,
                    content_type: mf.content_type,
                    desc: mf.desc,
                    name: mf.name,
                    caption_number,
                });
            }
        }
    }

    // Total page count: prefer layout.json (every page has page_size); fall back
    // to max page_idx seen in bboxes when zip is missing.
    let max_layout_page = page_sizes.keys().max().copied();
    let max_bbox_page = by_page.keys().max().copied();
    let total_pages = max_layout_page
        .or(max_bbox_page)
        .map(|m| m + 1)
        .unwrap_or(0);

    // Emit one OverlayPage per page index in [0, total_pages), so the viewer
    // can browse pages without extracted figures and spot missed ones.
    let mut pages: Vec<OverlayPage> = Vec::with_capacity(total_pages as usize);
    for page_idx in 0..total_pages {
        let size = page_sizes.get(&page_idx).copied().unwrap_or(default_size);
        let bboxes = by_page.remove(&page_idx).unwrap_or_default();
        pages.push(OverlayPage {
            page_idx,
            page_size: size,
            bboxes,
        });
    }

    Ok(Json(OverlayResponse {
        paper_id: id,
        total_pages,
        pages,
    }))
}

async fn read_figure_raw(
    source: &str,
    id: &str,
    name: &str,
) -> Result<(Vec<u8>, &'static str), StatusCode> {
    let fig_dir = super::paper_figures_dir(source, id);
    // Try hires PNG first
    let hires_path = format!("{}/hires/{}.png", fig_dir, name);
    if std::path::Path::new(&hires_path).exists() {
        let data = tokio::fs::read(&hires_path).await.map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        return Ok((data, "image/png"));
    }
    warn!("Figure {} for paper {}/{} not found in hires PNG. Falling back to MinerU extracted figure.", name, source, id);
    // Fallback to MinerU extracted figure
    let mineru_path = format!("{}/{}.jpg", fig_dir, name);
    if std::path::Path::new(&mineru_path).exists() {
        let data = tokio::fs::read(&mineru_path).await.map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        return Ok((data, "image/jpeg"));
    }
    warn!(
        "Figure {} for paper {}/{} not found in either hires PNG or MinerU extracted figure.",
        name, source, id
    );
    Err(StatusCode::NOT_FOUND)
}

pub(crate) async fn get_paper_figure(
    Path((source, id, name)): Path<(String, String, String)>,
) -> Result<axum::response::Response, StatusCode> {
    let (data, content_type) = read_figure_raw(&source, &id, &name).await?;
    let response = axum::response::Response::builder()
        .header(axum::http::header::CONTENT_TYPE, content_type)
        .body(axum::body::Body::from(data))
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(response)
}

pub(crate) async fn get_paper_table(
    Path((source, id, name)): Path<(String, String, String)>,
) -> Result<axum::response::Response, StatusCode> {
    let (data, content_type) = read_figure_raw(&source, &id, &name).await?;
    let response = axum::response::Response::builder()
        .header(axum::http::header::CONTENT_TYPE, content_type)
        .body(axum::body::Body::from(data))
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(response)
}

pub(crate) async fn get_bbox_adjustments(
    State(state): State<Arc<super::AppState>>,
    headers: HeaderMap,
    Path((source, id)): Path<(String, String)>,
) -> Result<Json<super::types::BboxAdjustmentsResponse>, StatusCode> {
    if !super::auth::check_auth(&headers, &state.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let adj_path = format!(
        "{}/bbox_adjustments.json",
        super::paper_figures_dir(&source, &id)
    );
    let adjustments = if std::path::Path::new(&adj_path).exists() {
        let adj_json = tokio::fs::read_to_string(&adj_path).await.map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        serde_json::from_str(&adj_json).unwrap_or_default()
    } else {
        std::collections::HashMap::new()
    };
    Ok(Json(super::types::BboxAdjustmentsResponse { adjustments }))
}

pub(crate) async fn save_bbox_adjustment(
    State(state): State<Arc<super::AppState>>,
    headers: HeaderMap,
    Path((source, id)): Path<(String, String)>,
    Json(req): Json<super::types::SaveBboxAdjustmentRequest>,
) -> Result<StatusCode, (StatusCode, Json<super::types::ErrorResponse>)> {
    use super::types::ErrorResponse;

    let err = |code: u16, reason: String| -> (StatusCode, Json<ErrorResponse>) {
        (
            StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(ErrorResponse { code, reason }),
        )
    };

    if !super::auth::check_auth(&headers, &state.auth_token) {
        return Err(err(401, "Unauthorized".into()));
    }

    let adj_path = format!(
        "{}/bbox_adjustments.json",
        super::paper_figures_dir(&source, &id)
    );
    let mut adjustments: crate::mineru::BboxAdjustments =
        if std::path::Path::new(&adj_path).exists() {
            let adj_json = tokio::fs::read_to_string(&adj_path)
                .await
                .map_err(|e| err(500, format!("Failed to read bbox_adjustments.json: {}", e)))?;
            serde_json::from_str(&adj_json).unwrap_or_default()
        } else {
            std::collections::HashMap::new()
        };

    let image_name = req.name.clone();
    adjustments.insert(
        req.name,
        crate::mineru::BboxAdjustment {
            left: req.left,
            top: req.top,
            right: req.right,
            bottom: req.bottom,
        },
    );

    let adj_json = serde_json::to_string_pretty(&adjustments)
        .map_err(|e| err(500, format!("Failed to serialize adjustment: {}", e)))?;
    tokio::fs::write(&adj_path, adj_json)
        .await
        .map_err(|e| err(500, format!("Failed to write bbox_adjustments.json: {}", e)))?;

    // Regenerate only the modified image instead of all hires images
    let paper = state
        .db
        .get_paper(&id)
        .await
        .map_err(|e| err(500, format!("Failed to read paper: {}", e)))?;
    let pdf_url = match paper.as_ref() {
        Some(p) => match p.source_type.as_str() {
            "arxiv" => format!(
                "https://arxiv.org/pdf/{}.pdf",
                p.external_id.as_ref().unwrap_or(&p.id)
            ),
            "openreview" => p
                .external_id
                .as_ref()
                .map(|eid| format!("https://openreview.net/pdf?id={}", eid))
                .unwrap_or_else(|| p.source_url.clone().unwrap_or_default()),
            _ => p.source_url.clone().unwrap_or_default(),
        },
        None => {
            format!("https://arxiv.org/pdf/{}.pdf", id)
        }
    };
    crate::hires::regenerate_single_hires_image(&source, &id, &pdf_url, &image_name, 600)
        .await
        .map_err(|e| err(500, format!("Failed to regenerate hires PNG: {}", e)))?;

    Ok(StatusCode::OK)
}

pub(crate) async fn save_manual_figure(
    State(state): State<Arc<super::AppState>>,
    headers: HeaderMap,
    Path((source, id)): Path<(String, String)>,
    Json(req): Json<SaveManualFigureRequest>,
) -> Result<Json<SaveManualFigureResponse>, (StatusCode, Json<super::types::ErrorResponse>)> {
    use super::types::ErrorResponse;

    let err = |code: u16, reason: String| -> (StatusCode, Json<ErrorResponse>) {
        (
            StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(ErrorResponse { code, reason }),
        )
    };

    if !super::auth::check_auth(&headers, &state.auth_token) {
        return Err(err(401, "Unauthorized".into()));
    }

    let fig_dir = super::paper_figures_dir(&source, &id);
    let manual_path = format!("{}/manual_figures.json", fig_dir);

    let mut manual: crate::mineru::ManualFigures = if std::path::Path::new(&manual_path).exists() {
        let json = tokio::fs::read_to_string(&manual_path)
            .await
            .map_err(|e| err(500, format!("Failed to read manual_figures.json: {}", e)))?;
        serde_json::from_str(&json).unwrap_or_default()
    } else {
        Vec::new()
    };

    // Generate name in MinerU style with 99 prefix: 99{idx:02}{hash_2char}
    let idx = manual.len();
    let seed = format!(
        "{}-{}-{:.1}-{:.1}-{:.1}-{:.1}-{}-{}",
        id,
        idx,
        req.body_bbox[0],
        req.body_bbox[1],
        req.body_bbox[2],
        req.body_bbox[3],
        req.caption_text,
        req.content_type
    );
    let mut hasher = Sha1::new();
    hasher.update(seed.as_bytes());
    let digest = hasher.finalize();
    let hash_byte = digest[0] as u16;
    let name = format!(
        "99{:02}{}",
        idx + 1,
        crate::mineru::encode_base36_2(hash_byte)
    );

    // Compute full bbox = union(body_bbox, caption_bbox)
    let bbox = match req.caption_bbox {
        Some(cb) => {
            let [bx0, by0, bx1, by1] = req.body_bbox;
            let [cx0, cy0, cx1, cy1] = cb;
            [bx0.min(cx0), by0.min(cy0), bx1.max(cx1), by1.max(cy1)]
        }
        None => req.body_bbox,
    };

    let mf = crate::mineru::ManualFigure {
        name: name.clone(),
        page_idx: req.page_idx,
        bbox,
        body_bbox: req.body_bbox,
        desc: req.caption_text,
        content_type: req.content_type.clone(),
    };
    manual.push(mf);

    let json = serde_json::to_string_pretty(&manual).map_err(|e| {
        err(
            500,
            format!("Failed to serialize manual_figures.json: {}", e),
        )
    })?;
    tokio::fs::write(&manual_path, json)
        .await
        .map_err(|e| err(500, format!("Failed to write manual_figures.json: {}", e)))?;

    // Generate hires PNG for the new manual figure.
    // Respect hires.done mode: no-caption → body_bbox, caption → full bbox.
    let hires_done_path = format!("{}/hires/hires.done", fig_dir);
    let use_body_bbox = if let Ok(done_content) = tokio::fs::read_to_string(&hires_done_path).await
    {
        done_content.trim() == "no-caption"
    } else {
        false
    };
    let bbox_for_hires = if use_body_bbox { req.body_bbox } else { bbox };
    let bbox_info = crate::mineru::ImageBboxInfo {
        bbox: bbox_for_hires,
        page_idx: req.page_idx,
        content_type: req.content_type.clone(),
    };
    let hires_dir = format!("{}/hires", fig_dir);
    tokio::fs::create_dir_all(&hires_dir)
        .await
        .map_err(|e| err(500, format!("Failed to create hires directory: {}", e)))?;

    let paper = state
        .db
        .get_paper(&id)
        .await
        .map_err(|e| err(500, format!("Failed to read paper: {}", e)))?;
    let pdf_url = match paper.as_ref() {
        Some(p) => match p.source_type.as_str() {
            "arxiv" => format!(
                "https://arxiv.org/pdf/{}.pdf",
                p.external_id.as_ref().unwrap_or(&p.id)
            ),
            "openreview" => p
                .external_id
                .as_ref()
                .map(|eid| format!("https://openreview.net/pdf?id={}", eid))
                .unwrap_or_else(|| p.source_url.clone().unwrap_or_default()),
            _ => p.source_url.clone().unwrap_or_default(),
        },
        None => {
            format!("https://arxiv.org/pdf/{}.pdf", id)
        }
    };

    crate::hires::generate_manual_figure_hires(&source, &id, &pdf_url, &bbox_info, &name, 600)
        .await
        .map_err(|e| err(500, format!("Failed to generate hires PNG: {}", e)))?;

    Ok(Json(SaveManualFigureResponse { name }))
}

pub(crate) async fn delete_manual_figure(
    State(state): State<Arc<super::AppState>>,
    headers: HeaderMap,
    Path((source, id, name)): Path<(String, String, String)>,
) -> Result<StatusCode, (StatusCode, Json<super::types::ErrorResponse>)> {
    use super::types::ErrorResponse;

    let err = |code: u16, reason: String| -> (StatusCode, Json<ErrorResponse>) {
        (
            StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(ErrorResponse { code, reason }),
        )
    };

    if !super::auth::check_auth(&headers, &state.auth_token) {
        return Err(err(401, "Unauthorized".into()));
    }

    let fig_dir = super::paper_figures_dir(&source, &id);
    let manual_path = format!("{}/manual_figures.json", fig_dir);

    if !std::path::Path::new(&manual_path).exists() {
        return Err(err(404, "manual_figures.json does not exist".into()));
    }

    let json = tokio::fs::read_to_string(&manual_path)
        .await
        .map_err(|e| err(500, format!("Failed to read manual_figures.json: {}", e)))?;
    let mut manual: crate::mineru::ManualFigures = serde_json::from_str(&json).unwrap_or_default();

    let original_len = manual.len();
    manual.retain(|mf| mf.name != name);

    if manual.len() == original_len {
        return Err(err(404, format!("Figure {} not found", name)));
    }

    let json = serde_json::to_string_pretty(&manual).map_err(|e| {
        err(
            500,
            format!("Failed to serialize manual_figures.json: {}", e),
        )
    })?;
    tokio::fs::write(&manual_path, json)
        .await
        .map_err(|e| err(500, format!("Failed to write manual_figures.json: {}", e)))?;

    // Delete hires PNG if exists
    let hires_path = format!("{}/hires/{}.png", fig_dir, name);
    if std::path::Path::new(&hires_path).exists() {
        match tokio::fs::remove_file(&hires_path).await {
            Ok(_) => info!("[manual-figure] Deleted hires PNG: {}", hires_path),
            Err(e) => warn!("[manual-figure] Failed to delete hires PNG: {}", e),
        }
    }

    info!("[manual-figure] Deleted {} for {}/{}", name, source, id);

    Ok(StatusCode::OK)
}

// ---- Smart detect: find missed figures by orphan captions -----------------

pub(crate) async fn smart_detect(
    Path((source, id)): Path<(String, String)>,
) -> Result<Json<SmartDetectResponse>, StatusCode> {
    let fig_dir = super::paper_figures_dir(&source, &id);
    let meta_path = format!("{}/mineru.json", fig_dir);
    let zip_path = format!("{}/mineru.zip", fig_dir);

    if !std::path::Path::new(&meta_path).exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    let source_for_spawn = source.clone();
    let id_for_spawn = id.clone();
    let recommendations = tokio::task::spawn_blocking(move || {
        let pdf_path = super::paper_pdf_path(&source_for_spawn, &id_for_spawn);
        smart_detect_sync(
            std::path::Path::new(&meta_path),
            std::path::Path::new(&zip_path),
            std::path::Path::new(&fig_dir),
            std::path::Path::new(&pdf_path),
            &source_for_spawn,
            &id_for_spawn,
        )
    })
    .await
    .map_err(|e| {
        error!("smart_detect spawn_blocking failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .map_err(|e| {
        warn!("[smart-detect] {}/{}: {}", source, id, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(SmartDetectResponse { recommendations }))
}
