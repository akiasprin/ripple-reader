// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::{Context, Result};
use base64::Engine;
use image::GenericImageView;
use pdfium_render::prelude::*;
use tracing::{info, warn};

/// Pad each side by 0.00t before cropping.  At 600dpi this is ~0px,
/// just enough breathing room to avoid clipping anti-aliased figure
/// edges while staying clear of adjacent body text on close-stacked
/// pages (1pt was too much and leaked text, e.g. 2304.10557 Figure 4).
const PRE_CROP_PAD_PT: f32 = 0.0;

/// Some figures (e.g. 2304.10557 Figure 1) have a LaTeX-declared
/// bbox that is much taller than the visible figure content, leaving
/// a large empty strip at the top.  Re-tighten the crop by scanning
/// for all-white border rows/columns and adding back a small fixed
/// padding after the trim.
const POST_TRIM_PAD_PX: u32 = 4;

/// Find the inner content bounds of an image by scanning for near-white
/// border rows/columns.  Returns `(left, top, right, bottom)` in pixels, where
/// `right` and `bottom` are exclusive (suitable for `crop(x, y, w, h)`).
///
/// `threshold`: a pixel counts as "white" when every RGB channel is `>= threshold`
/// (fully transparent pixels are also treated as white).
/// `row_white_ratio`: a row/column counts as empty when at least this fraction
/// of its pixels are white.
/// `max_trim_ratio`: per-side cap, so a wrong bbox or fully-white image can't
/// shrink the result to a single pixel.
/// `min_whitespace_ratio`: only trim when the total whitespace area exceeds this
/// fraction of the image; otherwise return the original bounds unchanged.
fn trim_whitespace_bounds(
    img: &image::DynamicImage,
    threshold: u8,
    row_white_ratio: f32,
    max_trim_ratio: f32,
    min_whitespace_ratio: f32,
) -> (u32, u32, u32, u32) {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return (0, 0, w, h);
    }
    let max_top = ((h as f32) * max_trim_ratio) as u32;
    let max_bottom = ((h as f32) * max_trim_ratio) as u32;
    let max_left = ((w as f32) * max_trim_ratio) as u32;
    let max_right = ((w as f32) * max_trim_ratio) as u32;

    let is_white = |px: image::Rgba<u8>| -> bool {
        if px[3] < 8 {
            return true;
        }
        px[0] >= threshold && px[1] >= threshold && px[2] >= threshold
    };
    let row_empty = |y: u32| -> bool {
        let mut whites: u32 = 0;
        for x in 0..w {
            if is_white(img.get_pixel(x, y)) {
                whites += 1;
            }
        }
        (whites as f32) / (w as f32) >= row_white_ratio
    };
    let col_empty = |x: u32| -> bool {
        let mut whites: u32 = 0;
        for y in 0..h {
            if is_white(img.get_pixel(x, y)) {
                whites += 1;
            }
        }
        (whites as f32) / (h as f32) >= row_white_ratio
    };

    let mut top: u32 = 0;
    while top < max_top && row_empty(top) {
        top += 1;
    }
    let mut bottom: u32 = h;
    while bottom > top + 1 && (h - bottom) < max_bottom && row_empty(bottom - 1) {
        bottom -= 1;
    }
    let mut left: u32 = 0;
    while left < max_left && col_empty(left) {
        left += 1;
    }
    let mut right: u32 = w;
    while right > left + 1 && (w - right) < max_right && col_empty(right - 1) {
        right -= 1;
    }

    // Use max edge trim ratio instead of total area ratio.
    // A wide image with a 100px bottom white strip (6% of total area)
    // would be rejected by area-ratio but is clearly visible.
    let top_ratio = if h > 0 { top as f32 / h as f32 } else { 0.0 };
    let bottom_ratio = if h > 0 {
        (h - bottom) as f32 / h as f32
    } else {
        0.0
    };
    let left_ratio = if w > 0 { left as f32 / w as f32 } else { 0.0 };
    let right_ratio = if w > 0 {
        (w - right) as f32 / w as f32
    } else {
        0.0
    };
    let max_edge_ratio = top_ratio.max(bottom_ratio).max(left_ratio).max(right_ratio);
    if max_edge_ratio < min_whitespace_ratio {
        return (0, 0, w, h);
    }

    (left, top, right, bottom)
}

/// Generate full-page PNG screenshots from a PDF (no dependency on MinerU).
/// Renders each page at the given DPI and saves to figures/{paper_id}/pages/{page}.png.
/// Also extracts raw text from the PDF and caches it to figures/{paper_id}/text.txt.
/// Returns (list of (label, base64 data URL), extracted_text) for each page.
pub async fn screenshot_pdf_pages(
    source: &str,
    paper_id: &str,
    pdf_url: &str,
    dpi: u32,
) -> Result<(Vec<(String, String)>, String)> {
    let fig_dir = crate::web::paper_figures_dir(source, paper_id);
    let pages_dir = format!("{}/pages", fig_dir);
    let text_path = format!("{}/text.txt", fig_dir);
    let pages_path = std::path::Path::new(&pages_dir);
    let text_file = std::path::Path::new(&text_path);

    // Check if already cached
    let mut cached_images = Vec::new();
    let mut cached_text = String::new();
    let mut has_cache = false;

    if pages_path.exists() {
        let mut entries: Vec<_> = std::fs::read_dir(pages_path)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".png")
            })
            .collect();
        entries.sort_by_key(|e| e.file_name());
        if !entries.is_empty() {
            has_cache = true;
            info!(
                "[screenshot_pdf_pages] Using cached {} pages for {}",
                entries.len(),
                paper_id
            );
            for entry in entries {
                let bytes = tokio::fs::read(entry.path()).await?;
                let label = format!(
                    "Page {}",
                    entry.file_name().to_string_lossy().trim_end_matches(".png")
                );
                let url = format!(
                    "data:image/png;base64,{}",
                    base64::prelude::BASE64_STANDARD.encode(&bytes)
                );
                cached_images.push((label, url));
            }
        }
    }

    if text_file.exists() {
        cached_text = tokio::fs::read_to_string(&text_path)
            .await
            .unwrap_or_default();
        if !cached_text.is_empty() {
            has_cache = true;
            info!(
                "[screenshot_pdf_pages] Using cached text ({} chars) for {}",
                cached_text.len(),
                paper_id
            );
        }
    }

    if has_cache && !cached_images.is_empty() && !cached_text.is_empty() {
        return Ok((cached_images, cached_text));
    }

    // Download PDF (with local cache in pdf/{source}/)
    let pdf_bytes = crate::pdf::download_pdf(source, paper_id, pdf_url).await?;

    let pdf_size_mb = pdf_bytes.len() as f64 / 1024.0 / 1024.0;
    let paper_id = paper_id.to_string();

    let paper_id_clone = paper_id.to_string();
    let result = tokio::task::spawn_blocking(move || -> Result<(Vec<(String, String)>, String)> {
        let pdfium = Pdfium::default();
        let document = pdfium
            .load_pdf_from_reader(std::io::Cursor::new(pdf_bytes), None)
            .context("Failed to load PDF")?;

        std::fs::create_dir_all(&pages_dir)
            .with_context(|| format!("Failed to create pages dir: {}", pages_dir))?;

        let dpi = dpi; // Use caller-provided DPI
        let mut images = Vec::new();
        let page_count = document.pages().len() as u16;
        // Limit to first 20 pages to avoid overwhelming the LLM
        let max_pages = page_count.min(20);

        info!(
            "[screenshot_pdf_pages] ===== START: paper_id={}, pages={}/{}, dpi={}, pages_dir={}, pdf_size={:.2}MB =====",
            paper_id_clone, max_pages, page_count, dpi, pages_dir, pdf_size_mb
        );

        for page_idx in 0..max_pages {
            let page = match document.pages().get(page_idx) {
                Ok(p) => p,
                Err(e) => {
                    warn!("[screenshot_pdf_pages] Page {} not found for {}: {}", page_idx + 1, paper_id_clone, e);
                    continue;
                }
            };

            let page_width_pt = page.width().value;
            let scale = dpi as f32 / 72.0;
            let target_width = (page_width_pt * scale) as i32;
            let config = PdfRenderConfig::new()
                .set_target_width(target_width)
                .render_form_data(false);

            info!(
                "[screenshot_pdf_pages] Rendering page {}: size={:.1}x{:.1}pt, scale={:.2}, target_width={}",
                page_idx + 1, page_width_pt, page.height().value, scale, target_width
            );

            let bitmap = match page.render_with_config(&config) {
                Ok(b) => b,
                Err(e) => {
                    warn!("[screenshot_pdf_pages] Render failed for {} page {}: {}", paper_id_clone, page_idx + 1, e);
                    continue;
                }
            };

            let output_path = format!("{}/{}.png", pages_dir, page_idx + 1);
            match bitmap
                .as_image()
                .save_with_format(&output_path, image::ImageFormat::Png)
            {
                Ok(_) => {
                    let bytes = std::fs::read(&output_path)?;
                    let label = format!("Page {}", page_idx + 1);
                    let url = format!("data:image/png;base64,{}" , base64::prelude::BASE64_STANDARD.encode(&bytes));
                    images.push((label, url));
                    info!(
                        "[screenshot_pdf_pages] Saved page {} -> {} ({} bytes)",
                        page_idx + 1, output_path, bytes.len()
                    );
                }
                Err(e) => {
                    warn!("[screenshot_pdf_pages] Save failed for {} page {}: {}", paper_id_clone, page_idx + 1, e);
                }
            }
        }

        // Extract raw text from all pages
        let mut text = String::new();
        for page_idx in 0..document.pages().len() as u16 {
            if let Ok(page) = document.pages().get(page_idx) {
                if let Ok(page_text) = page.text() {
                    text.push_str(&page_text.all());
                    text.push('\n');
                }
            }
        }
        if let Err(e) = std::fs::write(&text_path, &text) {
            warn!("[screenshot_pdf_pages] Failed to save extracted text for {}: {}", paper_id_clone, e);
        } else {
            info!("[screenshot_pdf_pages] Extracted {} chars of text for {}", text.len(), paper_id_clone);
        }

        Ok((images, text))
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking failed: {}", e))??;

    info!(
        "[screenshot_pdf_pages] Generated {} page screenshots + {} chars text for {}",
        result.0.len(),
        result.1.len(),
        paper_id
    );
    Ok(result)
}

/// Render and save a single hires image from a PDF page using bbox info.
fn render_single_hires_image(
    document: &PdfDocument,
    bbox_info: &crate::mineru::ImageBboxInfo,
    name: &str,
    hires_dir: &str,
    dpi: u32,
) -> Result<()> {
    let page_idx = bbox_info.page_idx.max(0) as u16;
    let page = document
        .pages()
        .get(page_idx)
        .with_context(|| format!("Page {} not found", page_idx + 1))?;

    let page_height_pt = page.height().value;
    let page_width_pt = page.width().value;
    let [x0, y0, x1, y1] = bbox_info.bbox;

    // Screen pt to PDF pt (origin at bottom-left)
    let left_pt = x0.min(x1);
    let right_pt = x0.max(x1);
    let top_screen_pt = y0.min(y1);
    let bottom_screen_pt = y0.max(y1);

    let top_pdf = page_height_pt - top_screen_pt;
    let bottom_pdf = page_height_pt - bottom_screen_pt;

    let scale = dpi as f32 / 72.0;
    let target_width = (page_width_pt * scale) as i32;
    let config = PdfRenderConfig::new()
        .set_target_width(target_width)
        .render_form_data(false);

    info!(
        "[hires] >>> processing image {}: page_idx={}, page_size={:.1}x{:.1}pt, screen_bbox=[{:.1},{:.1},{:.1},{:.1}], \
        left_pt={:.1}, right_pt={:.1}, top_screen={:.1}, bottom_screen={:.1}, \
        top_pdf={:.1}, bottom_pdf={:.1}, scale={:.2}, target_width={}",
        name, page_idx, page_width_pt, page_height_pt, x0, y0, x1, y1,
        left_pt, right_pt, top_screen_pt, bottom_screen_pt,
        top_pdf, bottom_pdf, scale, target_width
    );

    let (clip_left, clip_top) = page
        .points_to_pixels(PdfPoints::new(left_pt), PdfPoints::new(top_pdf), &config)
        .with_context(|| format!("points_to_pixels failed for {}", name))?;

    let (clip_right, clip_bottom) = page
        .points_to_pixels(
            PdfPoints::new(right_pt),
            PdfPoints::new(bottom_pdf),
            &config,
        )
        .with_context(|| format!("points_to_pixels failed for {}", name))?;

    let clip_w = (clip_right - clip_left).max(1) as u32;
    let clip_h = (clip_bottom - clip_top).max(1) as u32;

    info!(
        "[hires] clip pixels: left={:.1}, top={:.1}, right={:.1}, bottom={:.1}, width={}, height={}",
        clip_left, clip_top, clip_right, clip_bottom, clip_w, clip_h
    );

    let bitmap = page
        .render_with_config(&config)
        .with_context(|| format!("render failed for {}", name))?;

    let output_path = format!("{}/{}.png", hires_dir, name);
    let mut img = bitmap.as_image();
    let img_w = img.width() as i32;
    let img_h = img.height() as i32;
    let pad_px = (PRE_CROP_PAD_PT * scale).round() as i32;
    let crop_left = (clip_left - pad_px).max(0) as u32;
    let crop_top = (clip_top - pad_px).max(0) as u32;
    let crop_right = (clip_right + pad_px).min(img_w);
    let crop_bottom = (clip_bottom + pad_px).min(img_h);
    let crop_w = (crop_right - crop_left as i32).max(1) as u32;
    let crop_h = (crop_bottom - crop_top as i32).max(1) as u32;
    let cropped = img.crop(crop_left, crop_top, crop_w, crop_h);
    let (cl, ct, cr, cb) = trim_whitespace_bounds(&cropped, 240, 0.99, 0.7, 0.05);
    let cropped_w = cropped.width();
    let cropped_h = cropped.height();
    let final_left = cl.saturating_sub(POST_TRIM_PAD_PX);
    let final_top = ct.saturating_sub(POST_TRIM_PAD_PX);
    let final_right = (cr + POST_TRIM_PAD_PX).min(cropped_w);
    let final_bottom = (cb + POST_TRIM_PAD_PX).min(cropped_h);
    let final_w = final_right.saturating_sub(final_left).max(1);
    let final_h = final_bottom.saturating_sub(final_top).max(1);
    let trimmed_top = ct;
    let trimmed_bottom = cropped_h - cb;
    let trimmed_left = cl;
    let trimmed_right = cropped_w - cr;
    if trimmed_top + trimmed_bottom + trimmed_left + trimmed_right > 0 {
        info!(
            "[hires] trim {}: pre={}x{}, trim T={} B={} L={} R={}, final={}x{}",
            name,
            cropped_w,
            cropped_h,
            trimmed_top,
            trimmed_bottom,
            trimmed_left,
            trimmed_right,
            final_w,
            final_h,
        );
    }
    let final_img = cropped.crop_imm(final_left, final_top, final_w, final_h);
    final_img
        .save_with_format(&output_path, image::ImageFormat::Png)
        .with_context(|| format!("save failed for {}", name))?;

    info!(
        "[hires] Generated image {}: {}x{} @ {}dpi -> {}",
        name, final_w, final_h, dpi, output_path
    );

    Ok(())
}

/// Generate a hires PNG for a manually-created figure from raw bbox info.
/// Downloads the PDF, renders the page, crops to bbox, and saves to hires dir.
pub async fn generate_manual_figure_hires(
    source: &str,
    paper_id: &str,
    pdf_url: &str,
    bbox_info: &crate::mineru::ImageBboxInfo,
    name: &str,
    dpi: u32,
) -> Result<()> {
    let fig_dir = crate::web::paper_figures_dir(source, paper_id);
    let hires_dir = format!("{}/hires", fig_dir);
    tokio::fs::create_dir_all(&hires_dir)
        .await
        .with_context(|| format!("Failed to create hires dir: {}", hires_dir))?;

    let pdf_bytes = crate::pdf::download_pdf(source, paper_id, pdf_url).await?;
    let name = name.to_string();
    let hires_dir = hires_dir.clone();
    let bbox_info = bbox_info.clone();

    tokio::task::spawn_blocking(move || -> Result<()> {
        let pdfium = Pdfium::default();
        let document = pdfium
            .load_pdf_from_reader(std::io::Cursor::new(pdf_bytes), None)
            .context("Failed to load PDF")?;
        render_single_hires_image(&document, &bbox_info, &name, &hires_dir, dpi)
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking failed: {}", e))??;

    Ok(())
}

/// Regenerate a single hires image by name, applying current bbox adjustments.
/// Does NOT touch `hires.done` or other existing images.
pub async fn regenerate_single_hires_image(
    source: &str,
    paper_id: &str,
    pdf_url: &str,
    image_name: &str,
    dpi: u32,
) -> Result<()> {
    let fig_dir = crate::web::paper_figures_dir(source, paper_id);
    let meta_path = format!("{}/mineru.json", fig_dir);
    if !std::path::Path::new(&meta_path).exists() {
        anyhow::bail!("No mineru.json found for {}", paper_id);
    }

    let meta_json = tokio::fs::read_to_string(&meta_path)
        .await
        .context("Failed to read mineru.json")?;
    let meta: crate::mineru::CacheMeta =
        serde_json::from_str(&meta_json).context("Failed to parse mineru.json")?;

    let (mut bbox_info, _is_manual) = if let Some(idx) =
        meta.image_names.iter().position(|n| n == image_name)
    {
        // MinerU-extracted figure
        let info = if !meta.body_bboxes.is_empty() {
            meta.body_bboxes
                .get(idx)
                .context("body_bboxes index out of range")?
                .clone()
        } else {
            meta.image_bboxes
                .get(idx)
                .context("image_bboxes index out of range")?
                .clone()
        };
        (info, false)
    } else {
        // Manual figure: look in manual_figures.json
        let manual_path = format!("{}/manual_figures.json", fig_dir);
        let manual_json = tokio::fs::read_to_string(&manual_path)
            .await
            .with_context(|| {
                format!(
                    "Image name {} not found in mineru.json or manual_figures.json",
                    image_name
                )
            })?;
        let manual: crate::mineru::ManualFigures =
            serde_json::from_str(&manual_json).context("Failed to parse manual_figures.json")?;
        let mf = manual
            .into_iter()
            .find(|m| m.name == image_name)
            .with_context(|| {
                format!(
                    "Image name {} not found in mineru.json or manual_figures.json",
                    image_name
                )
            })?;
        // Respect hires.done mode: no-caption → body_bbox, caption → full bbox
        let hires_done_path = format!("{}/hires/hires.done", fig_dir);
        let use_body_bbox = if let Ok(content) = tokio::fs::read_to_string(&hires_done_path).await {
            let is_no_caption = content.trim() == "no-caption";
            info!(
                "[hires] regenerate {}: hires.done='{}', use_body_bbox={}",
                image_name,
                content.trim(),
                is_no_caption
            );
            is_no_caption
        } else {
            info!(
                "[hires] regenerate {}: hires.done not found, defaulting to full bbox (caption)",
                image_name
            );
            false
        };
        let bbox_for_hires = if use_body_bbox { mf.body_bbox } else { mf.bbox };
        info!(
            "[hires] regenerate {}: manual figure bbox={:?}, body_bbox={:?}, chosen={:?}",
            image_name, mf.bbox, mf.body_bbox, bbox_for_hires
        );
        let info = crate::mineru::ImageBboxInfo {
            bbox: bbox_for_hires,
            page_idx: mf.page_idx,
            content_type: mf.content_type,
        };
        (info, true)
    };

    // Apply manual bbox adjustment for the target figure (covers both MinerU
    // and manual figures — the batch pass above only touches meta bboxes).
    let adj_path = format!("{}/bbox_adjustments.json", fig_dir);
    if let Ok(adj_json) = tokio::fs::read_to_string(&adj_path).await {
        if let Ok(adjs) = serde_json::from_str::<crate::mineru::BboxAdjustments>(&adj_json) {
            if let Some(adj) = adjs.get(image_name) {
                bbox_info.bbox = adj.apply(bbox_info.bbox);
            }
        }
    }

    let hires_dir = format!("{}/hires", fig_dir);
    tokio::fs::create_dir_all(&hires_dir)
        .await
        .with_context(|| format!("Failed to create hires dir: {}", hires_dir))?;

    let pdf_bytes = crate::pdf::download_pdf(source, paper_id, pdf_url).await?;
    let name = image_name.to_string();

    tokio::task::spawn_blocking(move || -> Result<()> {
        let pdfium = Pdfium::default();
        let document = pdfium
            .load_pdf_from_reader(std::io::Cursor::new(pdf_bytes), None)
            .context("Failed to load PDF")?;

        render_single_hires_image(&document, &bbox_info, &name, &hires_dir, dpi)
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking failed: {}", e))??;

    Ok(())
}

/// Generate high-resolution PNG screenshots from PDF using bbox info stored in mineru.json.
/// When `keep_caption` is false, crops using `body_bboxes` (without caption strip)
/// instead of the full `image_bboxes`.
pub async fn generate_hires_images(
    source: &str,
    paper_id: &str,
    pdf_url: &str,
    dpi: u32,
    keep_caption: bool,
) -> Result<()> {
    let fig_dir = crate::web::paper_figures_dir(source, paper_id);
    let meta_path = format!("{}/mineru.json", fig_dir);
    if !std::path::Path::new(&meta_path).exists() {
        anyhow::bail!("No mineru.json found for {}", paper_id);
    }

    let meta_json = tokio::fs::read_to_string(&meta_path)
        .await
        .context("Failed to read mineru.json")?;
    let mut meta: crate::mineru::CacheMeta =
        serde_json::from_str(&meta_json).context("Failed to parse mineru.json")?;

    // Load bbox adjustments once for both MinerU and manual figures
    let adj_path = format!("{}/bbox_adjustments.json", fig_dir);
    let adjustments: Option<crate::mineru::BboxAdjustments> =
        if let Ok(adj_json) = tokio::fs::read_to_string(&adj_path).await {
            serde_json::from_str(&adj_json).ok()
        } else {
            None
        };

    if let Some(ref adjs) = adjustments {
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

    if meta.image_bboxes.is_empty() {
        info!("[hires] No bbox info for {}, skipping", paper_id);
        return Ok(());
    }

    let hires_dir = format!("{}/hires", fig_dir);
    let hires_path = std::path::Path::new(&hires_dir);

    // Check if already generated using a done flag file that stores the keep_caption state
    let done_path = hires_path.join("hires.done");
    if done_path.exists() {
        let done_content = tokio::fs::read_to_string(&done_path)
            .await
            .unwrap_or_default();
        let expected = if keep_caption {
            "caption"
        } else {
            "no-caption"
        };
        if done_content.trim() == expected {
            info!(
                "[hires] Already generated for {} (done_content={}), skipping",
                paper_id, done_content
            );
            return Ok(());
        }
        info!(
            "[hires] Mode mismatch for {}: done_content='{}' but requested='{}'. Removing old hires dir and regenerating.",
            paper_id, done_content, expected
        );
        if let Err(e) = tokio::fs::remove_dir_all(&hires_path).await {
            warn!(
                "[hires] Failed to remove old hires dir for {}: {}",
                paper_id, e
            );
        }
    }

    let image_names = meta.image_names;

    // Download PDF (with local cache in pdf/{source}/)
    let pdf_bytes = crate::pdf::download_pdf(source, paper_id, pdf_url).await?;

    // Load manual figures for hires generation
    let manual_path = format!("{}/manual_figures.json", fig_dir);
    let mut manual_entries: Vec<(String, crate::mineru::ImageBboxInfo)> =
        if let Ok(manual_json) = tokio::fs::read_to_string(&manual_path).await {
            if let Ok(manual) = serde_json::from_str::<crate::mineru::ManualFigures>(&manual_json) {
                manual
                    .into_iter()
                    .map(|mf| {
                        let bbox = if keep_caption { mf.bbox } else { mf.body_bbox };
                        let info = crate::mineru::ImageBboxInfo {
                            bbox,
                            page_idx: mf.page_idx,
                            content_type: mf.content_type,
                        };
                        (mf.name, info)
                    })
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

    // Apply bbox adjustments to manual figures
    if let Some(ref adjs) = adjustments {
        for (name, info) in &mut manual_entries {
            if let Some(adj) = adjs.get(name) {
                info.bbox = adj.apply(info.bbox);
            }
        }
    }

    // Generate hires images in blocking thread (pdfium is not Send)
    let bboxes = if keep_caption {
        meta.image_bboxes
    } else if !meta.body_bboxes.is_empty() {
        meta.body_bboxes
    } else {
        warn!(
            "[hires] body_bboxes missing for {}, falling back to image_bboxes",
            paper_id
        );
        meta.image_bboxes
    };
    let paper_id = paper_id.to_string();
    let pdf_size_mb = pdf_bytes.len() as f64 / 1024.0 / 1024.0;

    info!(
        "[hires] ===== START: paper_id={}, pdf_url={}, dpi={}, bbox_count={}, hires_dir={}, pdf_size={:.2}MB =====",
        paper_id, pdf_url, dpi, bboxes.len(), hires_dir, pdf_size_mb
    );
    for (i, bbox_info) in bboxes.iter().enumerate() {
        info!(
            "[hires] bbox[{}]: type={}, page={}, bbox=[{:.1},{:.1},{:.1},{:.1}]",
            i + 1,
            bbox_info.content_type,
            bbox_info.page_idx + 1,
            bbox_info.bbox[0],
            bbox_info.bbox[1],
            bbox_info.bbox[2],
            bbox_info.bbox[3],
        );
    }

    tokio::task::spawn_blocking(move || -> Result<()> {
        let pdfium = Pdfium::default();
        let document = pdfium
            .load_pdf_from_reader(std::io::Cursor::new(pdf_bytes), None)
            .context("Failed to load PDF")?;

        std::fs::create_dir_all(&hires_dir)
            .with_context(|| format!("Failed to create hires dir: {}", hires_dir))?;

        for (i, bbox_info) in bboxes.iter().enumerate() {
            let name = image_names
                .get(i)
                .expect("image_names must match image_bboxes in cache");
            if let Err(e) = render_single_hires_image(&document, bbox_info, name, &hires_dir, dpi) {
                warn!("[hires] {}", e);
            }
        }

        // Also generate hires for manual figures
        for (name, bbox_info) in &manual_entries {
            if let Err(e) = render_single_hires_image(&document, bbox_info, name, &hires_dir, dpi) {
                warn!("[hires] manual figure {}: {}", name, e);
            }
        }

        // Write done flag to prevent re-generation
        let done_content = if keep_caption {
            "caption"
        } else {
            "no-caption"
        };
        if let Err(e) = std::fs::write(format!("{}/hires.done", hires_dir), done_content) {
            warn!("[hires] Failed to write hires.done for {}: {}", paper_id, e);
        }

        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking failed: {}", e))??;

    Ok(())
}
