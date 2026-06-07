// SPDX-License-Identifier: MIT OR Apache-2.0

//! Smart figure detection: find missed figures by orphan captions.

use crate::figure::geometry::{
    bbox_area, bbox_aspect_ratio, bbox_iou, block_bbox, horizontal_overlap,
};
use crate::figure::svg::extract_svg_clip_bboxes;
use crate::figure::text_detect::infer_text_figure_body;
use crate::figure::types::SmartDetectRecommendation;
use crate::mineru::{
    collect_block_lines, extract_block_text, extract_caption_number, group_caption_lines,
    looks_like_caption_header, union_bbox, LayoutDoc, LayoutLine,
};
use crate::pdf::mutool::{run_mutool_svg, run_mutool_trace_fill_paths};
use anyhow::{Context, Result};
use tracing::warn;

/// Check if a candidate bbox overlaps with any known bbox above a threshold.
pub fn overlaps_known(candidate: [f32; 4], known: &[[f32; 4]], threshold: f32) -> bool {
    known.iter().any(|k| bbox_iou(candidate, *k) > threshold)
}

/// Find missed figures by scanning for orphan captions and inferring their bodies.
///
/// This is the synchronous core of `smart_detect`; the web handler wraps it in
/// `spawn_blocking` to avoid blocking the async runtime.
pub fn smart_detect_sync(
    meta_path: &std::path::Path,
    zip_path: &std::path::Path,
    fig_dir: &std::path::Path,
    pdf_path: &std::path::Path,
    source: &str,
    id: &str,
) -> Result<Vec<SmartDetectRecommendation>> {
    let meta_json = std::fs::read_to_string(meta_path).context("read mineru.json")?;
    let meta: crate::mineru::CacheMeta =
        serde_json::from_str(&meta_json).context("parse mineru.json")?;

    // Collect known captions
    let mut known_captions: std::collections::HashSet<String> = std::collections::HashSet::new();
    for desc in &meta.image_descriptions {
        if let Some(cap_num) = extract_caption_number(desc) {
            known_captions.insert(cap_num);
        }
    }

    // Collect known bboxes for deduplication (MinerU + manual figures)
    let mut known_bboxes: Vec<[f32; 4]> = Vec::new();
    for img in &meta.image_bboxes {
        known_bboxes.push(img.bbox);
    }
    for body in &meta.body_bboxes {
        known_bboxes.push(body.bbox);
    }
    let manual_path = fig_dir.join("manual_figures.json");
    if let Ok(manual_json) = std::fs::read_to_string(&manual_path) {
        if let Ok(manual) = serde_json::from_str::<crate::mineru::ManualFigures>(&manual_json) {
            for mf in manual {
                known_bboxes.push(mf.body_bbox);
                // Also register manual figure captions so re-scanning
                // does not surface the same orphan again.
                if let Some(cap_num) = extract_caption_number(&mf.desc) {
                    known_captions.insert(cap_num);
                }
            }
        }
    }

    // Read layout.json
    let mut doc: Option<LayoutDoc> = None;
    if zip_path.exists() {
        let file = std::fs::File::open(zip_path)?;
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
            doc = Some(serde_json::from_str(&buf).context("parse layout.json")?);
        }
    }

    let doc = match doc {
        Some(d) => d,
        None => return Ok(Vec::new()),
    };

    // Group orphan captions by page (block-level + line-level detection)
    let mut orphan_by_page: std::collections::BTreeMap<i32, Vec<([f32; 4], String)>> =
        std::collections::BTreeMap::new();
    // Also track page sizes for text-figure inference
    let mut page_sizes: std::collections::HashMap<i32, [f32; 2]> = std::collections::HashMap::new();

    for page in &doc.pdf_info {
        if page.page_size.len() >= 2 {
            page_sizes.insert(page.page_idx, [page.page_size[0], page.page_size[1]]);
        }

        let all_blocks: Vec<&crate::mineru::LayoutParaBlock> = page
            .para_blocks
            .iter()
            .chain(page.preproc_blocks.iter())
            .chain(page.discarded_blocks.iter())
            .collect();

        for block in &all_blocks {
            let text = extract_block_text(block);
            if !text.is_empty() {
                // Block-level: caption starts with "Figure" / "Table"
                if looks_like_caption_header(&text) {
                    if let Some(cap_num) = extract_caption_number(&text) {
                        if !known_captions.contains(&cap_num) {
                            if let Some(cb) = block_bbox(block) {
                                orphan_by_page
                                    .entry(page.page_idx)
                                    .or_default()
                                    .push((cb, text));
                                continue; // done with this block
                            }
                        }
                    }
                }
            }

            // Line-level: MinerU sometimes merges caption lines into
            // adjacent blocks (e.g. Figure 12 caption inside code block).
            let lines: Vec<LayoutLine> = collect_block_lines(block).into_iter().cloned().collect();
            if lines.len() >= 2 && lines.iter().all(|l| l.bbox.len() >= 4) {
                let groups = group_caption_lines(&lines);
                for (caption_text, caption_bbox) in groups {
                    if let Some(cap_num) = extract_caption_number(&caption_text) {
                        if !known_captions.contains(&cap_num) {
                            orphan_by_page
                                .entry(page.page_idx)
                                .or_default()
                                .push((caption_bbox, caption_text));
                        }
                    }
                }
            }
        }
    }

    if orphan_by_page.is_empty() {
        return Ok(Vec::new());
    }

    let mut recommendations: Vec<SmartDetectRecommendation> = Vec::new();

    for (page_idx, captions) in orphan_by_page {
        // ---- Step 1: try mutool SVG approach ----
        let svg_candidates = if pdf_path.exists() {
            match run_mutool_svg(pdf_path.to_str().unwrap_or(""), page_idx + 1) {
                Ok(svg) => {
                    let bboxes = extract_svg_clip_bboxes(&svg);
                    let filtered: Vec<_> = bboxes
                        .into_iter()
                        .filter(|b| !overlaps_known(*b, &known_bboxes, 0.3))
                        .collect();
                    filtered
                }
                Err(e) => {
                    warn!(
                        "[smart-detect] mutool SVG failed for page {} of {}/{}: {}",
                        page_idx, source, id, e
                    );
                    Vec::new()
                }
            }
        } else {
            warn!("[smart-detect] PDF not found for {}/{}", source, id);
            Vec::new()
        };

        // Rebuild all_blocks for this page (needed for text-figure inference)
        let page = doc.pdf_info.iter().find(|p| p.page_idx == page_idx);
        let all_blocks: Vec<&crate::mineru::LayoutParaBlock> = match page {
            Some(p) => p
                .para_blocks
                .iter()
                .chain(p.preproc_blocks.iter())
                .chain(p.discarded_blocks.iter())
                .collect(),
            None => Vec::new(),
        };
        let page_width = page_sizes.get(&page_idx).map(|s| s[0]).unwrap_or(595.0);
        let page_height = page_sizes.get(&page_idx).map(|s| s[1]).unwrap_or(842.0);

        // Extract PDF fill paths for this page (backgrounds / decoration bars)
        let fill_bboxes = if pdf_path.exists() {
            match run_mutool_trace_fill_paths(
                pdf_path.to_str().unwrap_or(""),
                page_idx + 1,
                page_height,
            ) {
                Ok(bboxes) => bboxes,
                Err(e) => {
                    warn!(
                        "[smart-detect] mutool trace fill_paths failed for page {} of {}/{}: {}",
                        page_idx, source, id, e
                    );
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        for (caption_bbox, caption_text) in captions {
            let mut best_body: Option<[f32; 4]> = None;
            let mut best_area: f32 = 0.0;
            let mut best_confidence: f32 = 0.0;
            let mut best_reason: String = String::new();

            // Match orphan caption against SVG clipPath candidates
            for clip_bbox in &svg_candidates {
                let overlap = horizontal_overlap(clip_bbox, &caption_bbox);
                if clip_bbox[3] > caption_bbox[1] + 20.0 {
                    continue;
                }
                if overlap < 0.1 {
                    continue;
                }
                let area = bbox_area(*clip_bbox);
                if area > best_area {
                    best_area = area;
                    best_body = Some(*clip_bbox);
                    best_confidence = 0.92;
                    best_reason = "mutool_svg_clip_path".to_string();
                }
            }

            // ---- Step 2: text-figure inference (SVG missed) ----
            if best_body.is_none() {
                if let Some((body, conf, reason)) = infer_text_figure_body(
                    caption_bbox,
                    &caption_text,
                    &all_blocks,
                    page_width,
                    &fill_bboxes,
                ) {
                    best_body = Some(body);
                    best_confidence = conf;
                    best_reason = reason;
                }
            }

            if let Some(body) = best_body {
                recommendations.push(SmartDetectRecommendation {
                    page_idx,
                    inferred_body_bbox: body,
                    caption_bbox,
                    caption_text,
                    confidence: best_confidence,
                    reason: best_reason,
                });
            }
        }
    }

    // Deduplicate by body bbox IoU
    let mut deduped: Vec<SmartDetectRecommendation> = Vec::new();
    for rec in recommendations {
        let mut merged = false;
        for d in &mut deduped {
            if d.page_idx != rec.page_idx {
                continue;
            }
            if bbox_iou(d.inferred_body_bbox, rec.inferred_body_bbox) > 0.5 {
                d.inferred_body_bbox = union_bbox(d.inferred_body_bbox, rec.inferred_body_bbox);
                d.caption_bbox = union_bbox(d.caption_bbox, rec.caption_bbox);
                if rec.confidence > d.confidence {
                    d.confidence = rec.confidence;
                    d.reason = rec.reason.clone();
                }
                if !d.caption_text.contains(&rec.caption_text) {
                    d.caption_text.push_str(" | ");
                    d.caption_text.push_str(&rec.caption_text);
                }
                merged = true;
                break;
            }
        }
        if !merged {
            deduped.push(rec);
        }
    }

    // Final filter
    let mut filtered: Vec<SmartDetectRecommendation> = Vec::new();
    for mut rec in deduped {
        let area = bbox_area(rec.inferred_body_bbox);
        let ratio = bbox_aspect_ratio(rec.inferred_body_bbox);
        if area < 800.0 || !(1.0 / 15.0..=15.0).contains(&ratio) {
            continue;
        }
        // Ensure body is above caption (layout coords)
        if rec.inferred_body_bbox[3] > rec.caption_bbox[1] {
            rec.inferred_body_bbox = [
                rec.inferred_body_bbox[0],
                rec.inferred_body_bbox[1].min(rec.caption_bbox[1] - 5.0),
                rec.inferred_body_bbox[2],
                rec.caption_bbox[1] - 2.0,
            ];
        }
        filtered.push(rec);
    }

    filtered.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap());
    Ok(filtered)
}
