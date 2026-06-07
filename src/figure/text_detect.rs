// SPDX-License-Identifier: MIT OR Apache-2.0

//! Text-based figure body inference from orphan captions.
//!
//! Uses line-level clustering and block-based heuristics to infer the body
//! bbox of text-based figures (prompt templates, qualitative comparisons, etc.)
//! that MinerU fails to extract as standalone image blocks.

use crate::figure::geometry::{block_bbox, horizontal_overlap};
use crate::mineru::{extract_block_text, line_text, looks_like_caption_header, union_bbox};

const MAX_GAP: f32 = 40.0;
const MIN_WIDE_RATIO: f32 = 0.48;

static RE_DEMO: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
static RE_UPPER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();

/// Check if a text block contains structured labels typical of text-based figures
/// (prompt templates, qualitative comparisons, etc.).
pub fn has_structure_label(text: &str) -> bool {
    let lower = text.to_lowercase();
    // Substring matches (sufficiently unique prefixes/suffixes avoid false positives).
    let labels = [
        "[system]",
        "[user]",
        "[assistant]",
        "generation:",
        "prompt:",
        "output:",
        "output requirement:",
        "guidelines:",
        "rules:",
        "criteria for",
        "true (",
        "false (",
        "causal scaffold:",
        "edit target:",
        "important task setting:",
        "summarized demonstration",
    ];
    if labels.iter().any(|&label| lower.contains(label)) {
        return true;
    }
    // Standalone words that need whole-word matching.
    // SYSTEM/USER/ASSISTANT are matched case-sensitively (ALL CAPS) because
    // prompt templates use uppercase labels; this avoids false positives in
    // normal prose like "The user experience is important."
    // Demonstration(s) are less common in prose so case-insensitive is OK.
    let re_upper =
        RE_UPPER.get_or_init(|| regex::Regex::new(r"\b(SYSTEM|USER|ASSISTANT)\b").unwrap());
    let re_demo = RE_DEMO
        .get_or_init(|| regex::Regex::new(r"(?i)\b(DEMONSTRATION|DEMONSTRATIONS)\b").unwrap());
    re_upper.is_match(text) || re_demo.is_match(text)
}

/// Check if text starts with a numbered label like "1. Baseline (AdaLoRA):"
/// or "3. Output format".
pub fn has_numbered_label(text: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"^\d+\.\s+\w+").unwrap());
    re.is_match(text.trim())
}

/// Extract all lines from page blocks with their bboxes.
fn extract_page_lines(page_blocks: &[&crate::mineru::LayoutParaBlock]) -> Vec<(String, [f32; 4])> {
    let mut lines = Vec::new();
    for block in page_blocks {
        // Lines inside sub-blocks
        for sub in &block.blocks {
            for line in &sub.lines {
                if line.bbox.len() >= 4 {
                    let text = line_text(line);
                    if !text.is_empty() {
                        lines.push((
                            text,
                            [line.bbox[0], line.bbox[1], line.bbox[2], line.bbox[3]],
                        ));
                    }
                }
            }
        }
        // Lines directly on para_block
        for line in &block.lines {
            if line.bbox.len() >= 4 {
                let text = line_text(line);
                if !text.is_empty() {
                    lines.push((
                        text,
                        [line.bbox[0], line.bbox[1], line.bbox[2], line.bbox[3]],
                    ));
                }
            }
        }
    }
    lines
}

/// Cluster line bboxes by vertical proximity.  Small gaps → same cluster.
fn cluster_lines_by_gap(line_bboxes: &[[f32; 4]], max_gap: f32) -> Vec<Vec<[f32; 4]>> {
    let mut sorted = line_bboxes.to_vec();
    sorted.sort_by(|a, b| a[1].partial_cmp(&b[1]).unwrap_or(std::cmp::Ordering::Equal));

    let mut clusters: Vec<Vec<[f32; 4]>> = Vec::new();
    let mut current: Vec<[f32; 4]> = Vec::new();

    for bb in sorted {
        if current.is_empty() {
            current.push(bb);
        } else {
            let last = current.last().unwrap();
            let gap = bb[1] - last[3];
            if gap < max_gap {
                current.push(bb);
            } else {
                clusters.push(current);
                current = vec![bb];
            }
        }
    }
    if !current.is_empty() {
        clusters.push(current);
    }
    clusters
}

/// Find the cluster that vertically overlaps with the caption bbox.
fn find_caption_cluster(caption_bbox: [f32; 4], clusters: &[Vec<[f32; 4]>]) -> Option<usize> {
    for (i, cluster) in clusters.iter().enumerate() {
        for bb in cluster {
            let overlap_y = (bb[3].min(caption_bbox[3]) - bb[1].max(caption_bbox[1])).max(0.0);
            if overlap_y > 1.0 {
                return Some(i);
            }
        }
    }
    None
}

/// Infer the body bbox of a text-based figure from an orphan caption.
///
/// Uses a two-strategy approach:
/// 1. Line-level clustering (preferred) — clusters lines by vertical gap,
///    finds the cluster containing the caption, and uses its union bbox
///    as the body.  Compensates for MinerU's inaccurate block bboxes.
/// 2. Block-based fallback — the original heuristic for wide blocks.
///
/// `fill_bboxes` contains PDF drawing bboxes (from `mutool trace`) that
/// represent background fills and decoration bars.  When available they
/// are used to guide virtual expansion so the body covers the full
/// visual area including decorative borders.
pub fn infer_text_figure_body(
    caption_bbox: [f32; 4],
    caption_text: &str,
    page_blocks: &[&crate::mineru::LayoutParaBlock],
    page_width: f32,
    fill_bboxes: &[[f32; 4]],
) -> Option<([f32; 4], f32, String)> {
    // ---- Strategy 1: line-level clustering ----
    let lines = extract_page_lines(page_blocks);
    let line_bboxes: Vec<[f32; 4]> = lines.iter().map(|(_, bb)| *bb).collect();
    let clusters = cluster_lines_by_gap(&line_bboxes, 50.0);

    if let Some(cluster_idx) = find_caption_cluster(caption_bbox, &clusters) {
        let cluster = &clusters[cluster_idx];

        // Strip caption lines from the cluster so the body does not
        // swallow the caption gap area.
        let content_lines: Vec<[f32; 4]> = cluster
            .iter()
            .filter(|bb| {
                let overlap_y = (bb[3].min(caption_bbox[3]) - bb[1].max(caption_bbox[1])).max(0.0);
                overlap_y < 2.0
            })
            .copied()
            .collect();
        let effective_cluster = if content_lines.len() >= 3 {
            // The remaining lines may span multiple figures (e.g. two
            // qualitative-comparison galleries on the same page with a
            // small gap between them).  Split by the largest vertical gap
            // and keep only the group closest to the caption.
            let mut sorted = content_lines.clone();
            sorted.sort_by(|a, b| a[1].partial_cmp(&b[1]).unwrap_or(std::cmp::Ordering::Equal));

            let mut max_gap = 0.0f32;
            let mut split_idx = 0;
            for i in 1..sorted.len() {
                let gap = sorted[i][1] - sorted[i - 1][3];
                if gap > max_gap {
                    max_gap = gap;
                    split_idx = i;
                }
            }

            if max_gap > 25.0 {
                let above = &sorted[..split_idx];
                let below = &sorted[split_idx..];

                let dist = |group: &[[f32; 4]]| -> f32 {
                    if group.is_empty() {
                        return f32::INFINITY;
                    }
                    // Use the closest line in the group (not the group bbox),
                    // because the group may span both sides of the caption
                    // (e.g. figure-8 content + figure-9 content).
                    let mut min_dist = f32::INFINITY;
                    for bb in group {
                        if bb[3] <= caption_bbox[1] {
                            min_dist = min_dist.min(caption_bbox[1] - bb[3]);
                        } else if bb[1] >= caption_bbox[3] {
                            min_dist = min_dist.min(bb[1] - caption_bbox[3]);
                        }
                    }
                    min_dist
                };

                let above_dist = dist(above);
                let below_dist = dist(below);

                if above_dist <= below_dist {
                    above.to_vec()
                } else {
                    below.to_vec()
                }
            } else {
                sorted
            }
        } else if content_lines.len() >= 2 {
            content_lines
        } else {
            cluster.clone()
        };

        let mut body = effective_cluster[0];
        for &bb in &effective_cluster[1..] {
            body = union_bbox(body, bb);
        }

        // Virtual expansion: if the cluster is narrower than the actual
        // background area (common for code/prompt blocks where MinerU
        // squeezes bboxes), expand to capture the full colored background
        // and any decorative bars beside it.
        //
        // When expansion triggers, the original cluster center is NOT
        // trustworthy (MinerU text bboxes are often left-shifted for
        // prompt labels like "SYSTEM").  Anchor on the page center
        // instead so the body aligns with the actual background area.
        let body_w = body[2] - body[0];
        let mut target_w = page_width * 0.70;

        // If mutool trace fill_paths are available, use them to determine
        // the actual background + decoration width.
        if !fill_bboxes.is_empty() {
            let mut min_x = f32::INFINITY;
            let mut max_x = f32::NEG_INFINITY;
            for fb in fill_bboxes {
                let overlap_y = (fb[3].min(body[3]) - fb[1].max(body[1])).max(0.0);
                if overlap_y > 5.0 {
                    min_x = min_x.min(fb[0]);
                    max_x = max_x.max(fb[2]);
                }
            }
            if max_x > min_x {
                let fill_w = max_x - min_x;
                if fill_w > body_w {
                    target_w = fill_w;
                }
            }
        }

        if body_w < target_w {
            let margin = page_width * 0.02;
            // Anchor on the actual fill bbox edges so the body precisely
            // covers the background instead of being a slightly off-center
            // symmetric expansion.
            let mut min_x = f32::INFINITY;
            let mut max_x = f32::NEG_INFINITY;
            for fb in fill_bboxes {
                let overlap_y = (fb[3].min(body[3]) - fb[1].max(body[1])).max(0.0);
                if overlap_y > 5.0 {
                    min_x = min_x.min(fb[0]);
                    max_x = max_x.max(fb[2]);
                }
            }
            if max_x > min_x {
                body[0] = min_x.max(margin);
                body[2] = max_x.min(page_width - margin);
            } else {
                let half_w = target_w / 2.0;
                let page_center = page_width / 2.0;
                body[0] = (page_center - half_w).max(margin);
                body[2] = (page_center + half_w).min(page_width - margin);
            }
        }

        // Do NOT expand body height using fill_bboxes — this avoids
        // including large padding areas (e.g. tcolorbox top/bottom margins)
        // that contain no text content.  The text cluster's union bbox already
        // captures the actual content height accurately.

        // Crop at caption top
        body[3] = body[3].min(caption_bbox[1] - 2.0);

        let h = body[3] - body[1];
        let w = body[2] - body[0];
        if h > 30.0 && w > page_width * 0.25 {
            return Some((body, 0.65, "text_figure_cluster".to_string()));
        }
    }

    // ---- Strategy 2: block-based fallback (for wide-body figures) ----
    let mut blocks_above: Vec<_> = page_blocks
        .iter()
        .filter(|b| {
            let Some(bb) = block_bbox(b) else {
                return false;
            };
            bb[3] < caption_bbox[1] + 5.0
        })
        .copied()
        .collect();
    blocks_above.sort_by(|a, b| {
        let ay = block_bbox(a).map(|bb| bb[3]).unwrap_or(0.0);
        let by = block_bbox(b).map(|bb| bb[3]).unwrap_or(0.0);
        by.partial_cmp(&ay).unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut collected: Vec<[f32; 4]> = Vec::new();
    let mut structure_label_count = 0;
    let mut wide_block_count = 0;
    let mut last_bottom = caption_bbox[1];

    for block in &blocks_above {
        let Some(bb) = block_bbox(block) else {
            continue;
        };
        let text = extract_block_text(block);

        if looks_like_caption_header(&text) {
            break;
        }
        if matches!(block.block_type.as_str(), "image" | "chart" | "table") {
            break;
        }

        let gap = last_bottom - bb[3];
        if gap > MAX_GAP && !collected.is_empty() {
            break;
        }

        let h_overlap = horizontal_overlap(&bb, &caption_bbox);
        let is_structured = has_structure_label(&text) || has_numbered_label(&text);
        let is_wide = (bb[2] - bb[0]) > page_width * MIN_WIDE_RATIO;

        if h_overlap < 0.15 && !is_structured {
            let w = bb[2] - bb[0];
            let is_narrow = w < page_width * 0.45;
            if is_narrow && !is_structured && text.len() > 80 {
                if collected.len() >= 2 {
                    break;
                }
                continue;
            }
            if collected.is_empty() {
                continue;
            }
        }

        if is_structured {
            structure_label_count += 1;
        }
        if is_wide {
            wide_block_count += 1;
        }

        collected.push(bb);
        last_bottom = bb[1];
    }

    if !collected.is_empty() {
        let (confidence, reason) = if structure_label_count >= 2 {
            (0.75, "text_figure_structure_labels")
        } else if structure_label_count >= 1 && wide_block_count >= 1 {
            (0.60, "text_figure_mixed_signals")
        } else if wide_block_count >= 2 {
            (0.55, "text_figure_wide_blocks")
        } else {
            return None;
        };

        let mut body = collected[0];
        for &bb in &collected[1..] {
            body = union_bbox(body, bb);
        }
        return Some((body, confidence, reason.to_string()));
    }

    // ---- Strategy 3: caption nested inside a multi-line block ----
    for block in page_blocks {
        let block_text = extract_block_text(block);
        if block_text.contains(caption_text) {
            if let Some(bb) = block_bbox(block) {
                let block_h = bb[3] - bb[1];
                let caption_h = caption_bbox[3] - caption_bbox[1];
                if block_h > caption_h * 1.5 {
                    let mut body = [bb[0], bb[1], bb[2], caption_bbox[1] - 2.0];
                    // Expand narrow bodies
                    let body_w = body[2] - body[0];
                    if body_w < page_width * 0.30 {
                        let margin = page_width * 0.06;
                        body[0] = margin;
                        body[2] = page_width - margin;
                    }
                    return Some((body, 0.50, "text_figure_same_block".to_string()));
                }
            }
        }
    }

    None
}
