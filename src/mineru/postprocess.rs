// SPDX-License-Identifier: MIT OR Apache-2.0

//! Post-processing: orphan rebind, merge, filter, caption propagation.
use crate::mineru::logging::pp_info;
use crate::mineru::types::{RawBlock, TitleHint};
use crate::mineru::utils::{
    bbox_contains, bbox_overlap_ratio, desc_snippet, extract_caption_number, has_sub_panel_label,
    is_caption_type_mismatch, looks_like_caption, same_block_type, union_bbox,
};
use tracing::info;

#[allow(clippy::used_underscore_binding)]
pub fn rebind_orphan_captions(
    candidates: &mut [RawBlock],
    orphans: &[(String, [f32; 4], i32)],
    split_figures: &[String],
) {
    // Compute an estimated page height for each page so we can cap the
    // rebind distance.  Body paragraphs that merely mention a figure
    // (e.g. "Figure 13 demonstrates...") can sit hundreds of points below
    // the actual figure and must not be rebound.
    let mut page_heights: std::collections::HashMap<i32, f32> = std::collections::HashMap::new();
    for c in candidates.iter() {
        let entry = page_heights.entry(c.page_idx).or_insert(0.0f32);
        *entry = entry.max(c.bbox[3]);
    }
    for h in page_heights.values_mut() {
        *h = (*h * 1.3).clamp(600.0, 1000.0);
    }

    for (cap_text, cap_bbox, cap_page) in orphans {
        // Guard against double-rebinding the same orphan in a second pass:
        // if a candidate on this page already carries this caption number,
        // the orphan has already been consumed and must not be re-bound again
        // (e.g. 1512.03385 page 8: Table 8 bound to a bare table in the
        // first pass would otherwise be re-bound to an image in the second
        // pass because the table is no longer "bare").
        //
        // BUT: a side-by-side sub-panel layout (e.g. 2604.21428 page 27
        // Figure 9) can produce a same-page orphan caption whose first-pass
        // bbox expansion was blocked by the engulfment guard (the wide
        // caption strip would have engulfed the other sub-panel before
        // propagate_captions merged it in).  Retry the expansion here: by
        // the second pass the sibling sub-panel has been merged into the
        // same group, so the engulfment check (which now skips same-caption
        // neighbors) passes naturally.
        if let Some(ref cap_num) = extract_caption_number(cap_text) {
            let already_bound_idx = candidates.iter().position(|c| {
                c.page_idx == *cap_page && c.caption_number.as_ref() == Some(cap_num)
            });
            if let Some(idx) = already_bound_idx {
                if !bbox_contains(candidates[idx].bbox, *cap_bbox) {
                    let prev_bbox = candidates[idx].bbox;
                    let new_bbox = union_bbox(prev_bbox, *cap_bbox);
                    let c_page = candidates[idx].page_idx;
                    let would_engulf = candidates.iter().enumerate().any(|(j, other)| {
                        j != idx
                            && other.page_idx == c_page
                            && other.caption_number.as_ref() != Some(cap_num)
                            && (bbox_contains(new_bbox, other.body_bbox)
                                || bbox_overlap_ratio(new_bbox, other.body_bbox) > 0.5)
                    });
                    if !would_engulf {
                        candidates[idx].bbox = new_bbox;
                        pp_info(&format!(
                            "[mineru] Second-pass bbox expansion: caption '{}' folded into bound candidate[{}]; bbox [{:.1},{:.1},{:.1},{:.1}] -> [{:.1},{:.1},{:.1},{:.1}]",
                            cap_text, idx,
                            prev_bbox[0], prev_bbox[1], prev_bbox[2], prev_bbox[3],
                            new_bbox[0], new_bbox[1], new_bbox[2], new_bbox[3]
                        ));
                    } else {
                        pp_info(&format!(
                            "[mineru] Second-pass bbox expansion skipped: '{}' union [{:.1},{:.1},{:.1},{:.1}] would still engulf a different-caption neighbor",
                            cap_text, new_bbox[0], new_bbox[1], new_bbox[2], new_bbox[3]
                        ));
                    }
                }
                pp_info(&format!(
                    "[mineru] orphan rebind skip: caption '{}' already bound to a candidate on page {}, ignoring",
                    cap_text, cap_page + 1
                ));
                continue;
            }
        }

        let cap_top = cap_bbox[1];
        let cap_bottom = cap_bbox[3];
        let cap_left = cap_bbox[0];
        let cap_right = cap_bbox[2];
        let page_h = page_heights.get(cap_page).copied().unwrap_or(800.0);

        let mut best_idx = None;
        let mut best_distance = f32::MAX;

        for (idx, c) in candidates.iter().enumerate() {
            if c.page_idx != *cap_page {
                continue;
            }
            // Orphan captions may come from multi-caption blocks (both image
            // and table).  Allow rebinding to any matching content type, not
            // just figures — a table orphan must be able to match a bare table.
            // Only rebind to images/tables that still have a placeholder desc.
            if !c.desc.starts_with("image on page")
                && !c.desc.starts_with("table on page")
                && !c.desc.starts_with("chart on page")
                && c.caption_number.is_some()
            {
                pp_info(&format!(
                    "[mineru] orphan rebind skip: candidate[{}] already has desc='{}' (cap={:?}), ignoring orphan '{}'",
                    idx, &c.desc[..c.desc.char_indices().nth(40).map(|(i, _)| i).unwrap_or(c.desc.len())], c.caption_number,
                    &cap_text[..cap_text.char_indices().nth(40).map(|(i, _)| i).unwrap_or(cap_text.len())]
                ));
                continue;
            }
            // Direction check.  Figure/chart captions are usually below the
            // body; table captions are usually above.  Allow either direction
            // for table candidates (caption can be above OR below the body)
            // but keep the strict caption-below-body rule for image/chart
            // candidates to match typical figure layouts.
            let img_top = c.bbox[1];
            let img_bottom = c.bbox[3];
            let cap_below_img = img_bottom < cap_top + 5.0;
            let cap_above_img = cap_bottom < img_top + 5.0;
            let direction_ok = if c.block_type == "table" {
                cap_below_img || cap_above_img
            } else {
                cap_below_img
            };
            if !direction_ok {
                pp_info(&format!(
                    "[mineru] orphan rebind skip: candidate[{}] type={} img=[{:.1},{:.1}] cap=[{:.1},{:.1}] (neither above nor below), ignoring orphan '{}'",
                    idx, c.block_type, img_top, img_bottom, cap_top, cap_bottom,
                    &cap_text[..cap_text.char_indices().nth(40).map(|(i, _)| i).unwrap_or(cap_text.len())]
                ));
                continue;
            }
            // Barrier check: a caption should not "hop over" an unrelated
            // figure/table body to reach this candidate.  If another candidate's
            // body sits geometrically between the caption and the target body
            // (with significant horizontal overlap), the caption almost certainly
            // belongs to a different figure or is merely an inline reference
            // inside body text (e.g. 2309.06180 page 12: a paragraph starting
            // with "Fig. 10." below Figure 17 must not bind to Figure 16).
            let barrier = candidates.iter().any(|other| {
                if other.page_idx != c.page_idx || std::ptr::eq(other, c) {
                    return false;
                }
                let [ox0, oy0, ox1, oy1] = other.body_bbox;
                let other_top = oy0.min(oy1);
                let other_bottom = oy0.max(oy1);
                let other_left = ox0.min(ox1);
                let other_right = ox0.max(ox1);
                let between = if cap_below_img {
                    img_bottom < other_top && other_bottom < cap_top
                } else {
                    cap_bottom < other_top && other_bottom < img_top
                };
                if !between {
                    return false;
                }
                let h_overlap = (cap_right.min(other_right) - cap_left.max(other_left)).max(0.0);
                let cap_w = (cap_right - cap_left).abs().max(1.0);
                h_overlap / cap_w > 0.25
            });
            if barrier {
                pp_info(&format!(
                    "[mineru] orphan rebind skip: candidate[{}] blocked by another figure body between it and caption '{}',",
                    idx,
                    &cap_text[..cap_text.char_indices().nth(40).map(|(i, _)| i).unwrap_or(cap_text.len())]
                ));
                continue;
            }
            // Type mismatch guard: a table caption should not bind to an image
            // candidate (and vice versa).  Only apply when a same-type bare
            // candidate exists on this page — if every bare candidate is the
            // "wrong" type we accept a cross-type rebind rather than losing
            // the caption entirely (e.g. 2407.08608 Figure 4 → bare table).
            if let Some(ref cap_num) = extract_caption_number(cap_text) {
                let caption_is_table = cap_num.starts_with("T:");
                let candidate_is_table = c.block_type == "table";
                let has_same_type_bare = candidates.iter().any(|oc| {
                    oc.page_idx == *cap_page
                        && (oc.desc.starts_with("image on page")
                            || oc.desc.starts_with("table on page")
                            || oc.desc.starts_with("chart on page"))
                        && (caption_is_table == (oc.block_type == "table"))
                });
                if has_same_type_bare {
                    if caption_is_table && !candidate_is_table {
                        pp_info(&format!(
                            "[mineru] orphan rebind skip: candidate[{}] type={} does not match table caption '{}', ignoring orphan",
                            idx, c.block_type,
                            &cap_text[..cap_text.char_indices().nth(40).map(|(i, _)| i).unwrap_or(cap_text.len())]
                        ));
                        continue;
                    }
                    if !caption_is_table && candidate_is_table {
                        pp_info(&format!(
                            "[mineru] orphan rebind skip: candidate[{}] type={} does not match figure caption '{}', ignoring orphan",
                            idx, c.block_type,
                            &cap_text[..cap_text.char_indices().nth(40).map(|(i, _)| i).unwrap_or(cap_text.len())]
                        ));
                        continue;
                    }
                }
            }
            // Require significant horizontal overlap — the caption should
            // span roughly the same width as the figure it describes.
            // Use cap_w (not min_w) as the denominator so tiny sub-panel
            // images cannot accept a full-width caption just because the
            // image itself is narrow (e.g. 2605.08074 para[2] 47-pt wide).
            // Exception: when an orphan caption is very close to the body
            // in the expected direction, trust the proximity signal over
            // strict horizontal overlap (e.g. composite figure sub-panels
            // in 2408.15998 page 19 Figure 6/7).
            let img_left = c.bbox[0];
            let img_right = c.bbox[2];
            let h_overlap = (img_right.min(cap_right) - img_left.max(cap_left)).max(0.0);
            let cap_w = (cap_right - cap_left).abs().max(1.0);
            let overlap_ratio = h_overlap / cap_w;
            // Pre-compute direction-gap so we can use it as a fallback signal
            let dir_gap = if cap_below_img {
                cap_top - img_bottom
            } else {
                img_top - cap_bottom
            };
            let bare_count = candidates
                .iter()
                .filter(|oc| {
                    oc.page_idx == *cap_page
                        && (oc.desc.starts_with("image on page")
                            || oc.desc.starts_with("table on page")
                            || oc.desc.starts_with("chart on page"))
                })
                .count();
            if overlap_ratio < 0.3 && dir_gap > 30.0 && bare_count > 1 {
                pp_info(&format!(
                    "[mineru] orphan rebind skip: candidate[{}] overlap_ratio={:.2} (h_overlap={:.1} / cap_w={:.1}) < 0.3, ignoring orphan '{}'",
                    idx, overlap_ratio, h_overlap, cap_w,
                    &cap_text[..cap_text.char_indices().nth(40).map(|(i, _)| i).unwrap_or(cap_text.len())]
                ));
                continue;
            }

            // Distance is the gap in the chosen direction.  Prefer below over
            // above for tables when both directions are possible (figure-like
            // layouts inside table candidates).
            let distance = if cap_below_img {
                cap_top - img_bottom
            } else {
                img_top - cap_bottom
            };
            // Reject captions that are too far from the image — body paragraphs
            // that merely mention a figure (e.g. "Figure 13 demonstrates...")
            // can sit hundreds of points below the actual figure.
            let max_distance = (page_h * 0.30).max(80.0);
            if distance > max_distance {
                pp_info(&format!(
                    "[mineru] orphan rebind skip: candidate[{}] distance={:.1} > max_distance={:.1}, ignoring orphan '{}'",
                    idx, distance, max_distance,
                    &cap_text[..cap_text.char_indices().nth(40).map(|(i, _)| i).unwrap_or(cap_text.len())]
                ));
                continue;
            }
            if distance < best_distance {
                best_distance = distance;
                best_idx = Some(idx);
            }
        }

        if let Some(idx) = best_idx {
            let cap_num = extract_caption_number(cap_text);
            // For split figures, keep only the figure label (text before the
            // first colon/period) so sub-panels do not inherit the full caption.
            let desc = if let Some(ref cn) = cap_num {
                if split_figures.contains(cn) {
                    if let Some(pos) = cap_text.find(':') {
                        cap_text[..pos].trim().to_string()
                    } else if let Some(pos) = cap_text.find('.') {
                        cap_text[..pos].trim().to_string()
                    } else {
                        cap_text.clone()
                    }
                } else {
                    cap_text.clone()
                }
            } else {
                cap_text.clone()
            };
            // Expand bbox to include the rebound caption strip so the hires
            // screenshot renders body + caption together.  Without this, an
            // orphan-caption block (e.g. Table 7 on page 8 of 1512.03385,
            // whose caption was originally placed inside the Table 8 para_block
            // by MinerU) would still render only the body region.
            //
            // BUT: when body and caption are far apart with another candidate
            // sandwiched between them (e.g. 1512.03385 page 8: Table 8 body
            // top, Table 7 caption + body in the middle, Table 8 caption at
            // bottom), the union bbox would engulf the intervening candidate's
            // body, leading to a hires crop containing two unrelated tables.
            // In that case, skip the expansion and keep only the body bbox —
            // visually losing the caption is preferable to engulfing a
            // neighbor.
            let prev_bbox = candidates[idx].bbox;
            let c_page = candidates[idx].page_idx;
            let new_bbox = union_bbox(prev_bbox, *cap_bbox);
            let would_engulf = candidates.iter().enumerate().any(|(j, other)| {
                j != idx
                    && other.page_idx == c_page
                    && !is_noise(other)
                    && (bbox_contains(new_bbox, other.body_bbox)
                        || bbox_overlap_ratio(new_bbox, other.body_bbox) > 0.5)
            });
            let c = &mut candidates[idx];
            c.desc = desc;
            if cap_num.is_some() {
                c.caption_number = cap_num;
            }
            if would_engulf {
                pp_info(&format!(
                    "[mineru] Orphan caption rebound (body-only bbox): '{}' attached to candidate[{}] but bbox NOT expanded to include caption [{:.1},{:.1},{:.1},{:.1}] — union [{:.1},{:.1},{:.1},{:.1}] would engulf same-page neighbor; keeping body bbox [{:.1},{:.1},{:.1},{:.1}]",
                    cap_text, idx,
                    cap_bbox[0], cap_bbox[1], cap_bbox[2], cap_bbox[3],
                    new_bbox[0], new_bbox[1], new_bbox[2], new_bbox[3],
                    prev_bbox[0], prev_bbox[1], prev_bbox[2], prev_bbox[3]
                ));
            } else {
                c.bbox = new_bbox;
                if has_sub_panel_label(cap_text) {
                    c.body_bbox = union_bbox(c.body_bbox, *cap_bbox);
                }
                pp_info(&format!(
                    "[mineru] Orphan caption rebound: '{}' -> image at [{:.1},{:.1},{:.1},{:.1}] (dist={:.1}); bbox expanded to [{:.1},{:.1},{:.1},{:.1}]",
                    cap_text, prev_bbox[0], prev_bbox[1], prev_bbox[2], prev_bbox[3], best_distance,
                    c.bbox[0], c.bbox[1], c.bbox[2], c.bbox[3]
                ));
            }
        } else {
            pp_info(&format!(
                "[mineru] Orphan caption on page {} not rebound: '{}'",
                cap_page + 1,
                cap_text
            ));
        }
    }
}

/// After orphan-caption rebind, some captions may have landed on the wrong
/// type of candidate because MinerU mis-classified the original block
/// (e.g. a table block that actually contains a figure caption, or vice
/// versa).  When we find a type-mismatched pair on the same page — one
/// table with a figure caption and one image/chart with a table caption —
/// swap their captions so each goes to the correct type.
pub fn swap_mismatched_captions(candidates: &mut [RawBlock]) {
    let n = candidates.len();
    let mut swapped = vec![false; n];
    for i in 0..n {
        if swapped[i] {
            continue;
        }
        let a = &candidates[i];
        let a_is_table = a.block_type == "table";
        let a_cap = extract_caption_number(&a.desc);
        let a_mismatch = match a_cap {
            Some(ref cn) if a_is_table => cn.starts_with("F:"),
            Some(ref cn) if !a_is_table => cn.starts_with("T:"),
            _ => false,
        };
        if !a_mismatch {
            continue;
        }
        for j in (i + 1)..n {
            if swapped[j] {
                continue;
            }
            let b = &candidates[j];
            if a.page_idx != b.page_idx {
                continue;
            }
            let b_is_table = b.block_type == "table";
            let b_cap = extract_caption_number(&b.desc);
            let b_mismatch = match b_cap {
                Some(ref cn) if b_is_table => cn.starts_with("F:"),
                Some(ref cn) if !b_is_table => cn.starts_with("T:"),
                _ => false,
            };
            if !b_mismatch {
                continue;
            }
            // Verify that swapping would fix BOTH sides.
            let a_fixed = match b_cap {
                Some(ref cn) if a_is_table => cn.starts_with("T:"),
                Some(ref cn) if !a_is_table => cn.starts_with("F:"),
                _ => false,
            };
            let b_fixed = match a_cap {
                Some(ref cn) if b_is_table => cn.starts_with("T:"),
                Some(ref cn) if !b_is_table => cn.starts_with("F:"),
                _ => false,
            };
            if a_fixed && b_fixed {
                let a_desc = candidates[i].desc.clone();
                let a_cap = candidates[i].caption_number.clone();
                candidates[i].desc = std::mem::replace(&mut candidates[j].desc, a_desc);
                candidates[i].caption_number =
                    std::mem::replace(&mut candidates[j].caption_number, a_cap);
                for idx in [i, j] {
                    let bb = candidates[idx].body_bbox;
                    let w = (bb[2] - bb[0]).abs();
                    let h = (bb[3] - bb[1]).abs();
                    if w > 0.0 && h > 0.0 {
                        candidates[idx].bbox = bb;
                    }
                }
                pp_info(&format!(
                    "[mineru] Swapped mismatched captions on page {}: candidate[{}] ({}) <-> candidate[{}] ({})",
                    candidates[i].page_idx + 1,
                    i,
                    if a_is_table { "table w/ figure cap" } else { "image w/ table cap" },
                    j,
                    if b_is_table { "table w/ figure cap" } else { "image w/ table cap" },
                ));
                swapped[i] = true;
                swapped[j] = true;
                break;
            }
        }
    }
}

/// Merge overlapping/adjacent blocks on the same page and drop noise.
/// `split_figures` contains caption numbers (e.g. "F:7") whose sub-figures
/// should NOT be merged into a single composite image.
pub fn post_process_blocks(
    blocks: Vec<RawBlock>,
    split_figures: &[String],
    page_text_edges: &std::collections::HashMap<i32, (f32, f32)>,
    page_columns: &std::collections::HashMap<i32, Vec<f32>>,
    paper_id: Option<&str>,
) -> Vec<RawBlock> {
    use std::collections::{HashMap, HashSet};

    // Pre-scan: count unique figure/table captions per page so that
    // page-singleton composite figures (e.g. 2605.31086 Figure 5 with
    // many tiny sub-panel icons) are not prematurely filtered by is_noise.
    let mut page_fig_caps: HashMap<i32, HashSet<String>> = HashMap::new();
    let mut page_tbl_caps: HashMap<i32, HashSet<String>> = HashMap::new();
    for b in &blocks {
        if let Some(ref cap) = b.caption_number {
            if is_caption_type_mismatch(cap, &b.block_type) {
                continue;
            }
            if cap.starts_with("F:") {
                page_fig_caps
                    .entry(b.page_idx)
                    .or_default()
                    .insert(cap.clone());
            } else if cap.starts_with("T:") {
                page_tbl_caps
                    .entry(b.page_idx)
                    .or_default()
                    .insert(cap.clone());
            }
        }
    }

    // Group by page, filtering noise but preserving image/chart blocks
    // on pages with exactly one figure caption (composite sub-panels).
    let mut by_page: HashMap<i32, Vec<RawBlock>> = HashMap::new();
    for b in blocks {
        let has_unique_fig = page_fig_caps
            .get(&b.page_idx)
            .map(|s| s.len() == 1)
            .unwrap_or(false);
        let has_unique_tbl = page_tbl_caps
            .get(&b.page_idx)
            .map(|s| s.len() == 1)
            .unwrap_or(false);
        let is_relevant_image =
            (b.block_type == "image" || b.block_type == "chart") && has_unique_fig;
        let is_relevant_table = b.block_type == "table" && has_unique_tbl;
        if is_noise(&b) && !is_relevant_image && !is_relevant_table {
            continue;
        }
        by_page.entry(b.page_idx).or_default().push(b);
    }

    let mut result = Vec::new();
    for (_page_idx, mut page_blocks) in by_page {
        // Estimate page dimensions from the largest bbox on this page
        let max_x = page_blocks.iter().map(|b| b.bbox[2]).fold(0.0f32, f32::max);
        let max_y = page_blocks.iter().map(|b| b.bbox[3]).fold(0.0f32, f32::max);
        // Heuristic: add ~30% margin for page edges; clamp to typical A4/Letter range
        let page_w = (max_x * 1.3).clamp(400.0, 700.0);
        let page_h = (max_y * 1.3).clamp(600.0, 1000.0);

        // Propagate caption numbers from neighbouring blocks that do have captions.
        // MinerU sometimes only attaches a caption to one sub-block of a multi-part
        // figure (e.g. only the right half of Figure 5 gets the caption, the left
        // half falls back to "image on page N").  Without this step the left half
        // remains unprotected and gets wrongly merged with an adjacent table.
        let cols_for_page = page_columns.get(&_page_idx).map(|v| v.as_slice());
        let propagated =
            propagate_captions(&mut page_blocks, page_w, page_h, cols_for_page, paper_id);
        if propagated > 0 {
            pp_info(&format!(
                "[mineru] page {}: propagated {} caption number(s)",
                _page_idx, propagated
            ));
        }

        // Page-singleton fallback: composite figures with sub-panels spread
        // across the page (e.g. paper 2605.04045 page 10 has 23 image sub-panels
        // for one Figure 4) leave gaps too wide for `propagate_captions`'s
        // geometric adjacency to span.  When a page has exactly ONE figure
        // caption among image/chart blocks, every uncaptioned image/chart
        // block on that page must belong to that same figure — there is no
        // other figure on the page for it to belong to.  Same logic applies
        // independently to tables.  The downstream merge still requires
        // geometric overlap, so a wrongly-tagged orphan from a different
        // figure cannot get fused into the wrong group.
        let page_singleton = propagate_page_unique_caption(&mut page_blocks);
        if page_singleton > 0 {
            pp_info(&format!(
                "[mineru] page {}: page-singleton propagated {} caption number(s)",
                _page_idx, page_singleton
            ));
        }

        // Targeted propagation for split figures: bare blocks that are aligned
        // with and horizontally adjacent to a split-caption block inherit that
        // caption.  This ensures all sub-blocks of a split figure carry the
        // caption so should_merge can keep them isolated from each other.
        if !split_figures.is_empty() {
            let n = page_blocks.len();
            let mut changed = true;
            while changed {
                changed = false;
                let mut to_propagate: Vec<(usize, String)> = Vec::new();
                for split_cap in split_figures {
                    for i in 0..n {
                        if page_blocks[i].caption_number.as_ref() != Some(split_cap) {
                            continue;
                        }
                        let cap_block = &page_blocks[i];
                        for (j, other) in page_blocks.iter().enumerate() {
                            if i == j || other.caption_number.is_some() {
                                continue;
                            }
                            if !same_block_type(&other.block_type, &cap_block.block_type) {
                                continue;
                            }
                            if !are_aligned(cap_block, other) {
                                continue;
                            }
                            let gap = if cap_block.bbox[2] < other.bbox[0] {
                                other.bbox[0] - cap_block.bbox[2]
                            } else if other.bbox[2] < cap_block.bbox[0] {
                                cap_block.bbox[0] - other.bbox[2]
                            } else {
                                0.0
                            };
                            if gap < 25.0 {
                                to_propagate.push((j, split_cap.clone()));
                            }
                        }
                    }
                }
                for (j, cap) in to_propagate {
                    page_blocks[j].caption_number = Some(cap.clone());
                    changed = true;
                    pp_info(&format!(
                        "[mineru] Split-figure propagation: '{}' -> block at [{:.1},{:.1},{:.1},{:.1}]",
                        cap, page_blocks[j].bbox[0], page_blocks[j].bbox[1],
                        page_blocks[j].bbox[2], page_blocks[j].bbox[3]
                    ));
                }
            }
        }

        // Sort by top edge, then left edge
        page_blocks.sort_by(|a, b| {
            let ay = a.bbox[1].min(a.bbox[3]);
            let by = b.bbox[1].min(b.bbox[3]);
            ay.partial_cmp(&by)
                .unwrap()
                .then_with(|| a.bbox[0].partial_cmp(&b.bbox[0]).unwrap())
        });

        // Keep a copy of the sorted original blocks for reference checks.
        let originals = page_blocks.clone();

        // Iterative merge until stable.
        // To avoid bbox-inflation side effects (where an already-merged large
        // bbox stops absorbing later sub-figures), each merged group keeps a
        // list of its original sub-block indices.  A new block joins a group
        // when it should_merge with *any* original sub-block in that group.
        pp_info(&format!(
            "[mineru] post_process page={}: {} blocks after sort",
            _page_idx + 1,
            page_blocks.len()
        ));
        for (i, b) in page_blocks.iter().enumerate() {
            pp_info(&format!(
                "[mineru]   block[{}]: type={} cap={:?} bbox={:?} body={:?} desc={}",
                i,
                b.block_type,
                b.caption_number,
                b.bbox,
                b.body_bbox,
                &b.desc[..b
                    .desc
                    .char_indices()
                    .nth(60)
                    .map(|(i, _)| i)
                    .unwrap_or(b.desc.len())]
            ));
        }

        let mut merged: Vec<(RawBlock, Vec<usize>)> = Vec::new();
        for (idx, block) in page_blocks.into_iter().enumerate() {
            let mut found = false;
            for (group, origins) in merged.iter_mut() {
                if group.caption_number.is_some()
                    && block.caption_number.is_some()
                    && group.caption_number != block.caption_number
                {
                    pp_info(&format!(
                        "[mineru] merge skip page={}: block[{}] cap={:?} vs group cap={:?}",
                        _page_idx + 1,
                        idx,
                        block.caption_number,
                        group.caption_number
                    ));
                    continue;
                }
                let can_merge = origins
                    .iter()
                    .any(|&oi| should_merge(&originals[oi], &block, page_w, page_h, split_figures));
                if can_merge {
                    let old_desc = group.desc.clone();
                    merge_into(group, &block);
                    pp_info(&format!(
                        "[mineru] merge page={}: block[{}] -> group origins={:?} | old={} new={}",
                        _page_idx + 1,
                        idx,
                        origins,
                        &old_desc[..old_desc
                            .char_indices()
                            .nth(40)
                            .map(|(i, _)| i)
                            .unwrap_or(old_desc.len())],
                        &group.desc[..group
                            .desc
                            .char_indices()
                            .nth(40)
                            .map(|(i, _)| i)
                            .unwrap_or(group.desc.len())]
                    ));
                    origins.push(idx);
                    found = true;
                    break;
                }
            }
            if !found {
                pp_info(&format!(
                    "[mineru] new group page={}: block[{}] cap={:?} desc={}",
                    _page_idx + 1,
                    idx,
                    block.caption_number,
                    &block.desc[..block
                        .desc
                        .char_indices()
                        .nth(40)
                        .map(|(i, _)| i)
                        .unwrap_or(block.desc.len())]
                ));
                merged.push((block, vec![idx]));
            }
        }

        // Because we added blocks in sorted order, a later group may still
        // absorb an earlier group if they should_merge.  Run additional passes
        // until stable.
        let mut pass = 0;
        let mut changed = true;
        while changed {
            changed = false;
            pass += 1;
            let mut new_merged: Vec<(RawBlock, Vec<usize>)> = Vec::new();
            for (block, origins) in merged.drain(..) {
                let mut found = false;
                for (existing, e_origins) in new_merged.iter_mut() {
                    if existing.caption_number.is_some()
                        && block.caption_number.is_some()
                        && existing.caption_number != block.caption_number
                    {
                        continue;
                    }
                    if e_origins.iter().any(|&ei| {
                        should_merge(&originals[ei], &block, page_w, page_h, split_figures)
                    }) {
                        let old_cap = existing.caption_number.clone();
                        merge_into(existing, &block);
                        pp_info(&format!(
                            "[mineru] re-merge page={} pass={}: group cap={:?} absorbs block origins={:?} -> new cap={:?} desc={}",
                            _page_idx + 1, pass, old_cap, origins, existing.caption_number,
                            &existing.desc[..existing.desc.char_indices().nth(40).map(|(i,_)| i).unwrap_or(existing.desc.len())]
                        ));
                        e_origins.extend_from_slice(&origins);
                        found = true;
                        changed = true;
                        break;
                    }
                }
                if !found {
                    new_merged.push((block, origins));
                }
            }
            merged = new_merged;
        }

        // Post-merge containment guard: a non-rectangular composite figure
        // (e.g. 5 sub-panels in a 2x3 grid with one cell missing) produces a
        // bounding rectangle that engulfs an adjacent unrelated figure sitting
        // in the "hole".  When a merged group's body_bbox fully contains
        // another figure's body_bbox on the same page, revert the merge and
        // keep the sub-panels as separate images — a correct partial crop is
        // better than a merged crop that swallows a foreign figure.
        let mut final_merged: Vec<(RawBlock, Vec<usize>)> = Vec::new();
        for (group, origins) in merged {
            if origins.len() <= 1 {
                final_merged.push((group, origins));
                continue;
            }
            let mut contained_other: Option<usize> = None;
            for (other_idx, other) in originals.iter().enumerate() {
                if origins.contains(&other_idx) {
                    continue;
                }
                if other.page_idx != group.page_idx {
                    continue;
                }
                if !same_block_type(&other.block_type, &group.block_type) {
                    continue;
                }
                // Ignore tiny decorations / icons
                let other_w = (other.body_bbox[2] - other.body_bbox[0]).abs();
                let other_h = (other.body_bbox[3] - other.body_bbox[1]).abs();
                if other_w < 30.0 || other_h < 30.0 {
                    continue;
                }
                if bbox_contains(group.body_bbox, other.body_bbox) {
                    // Guard against false positives in grid layouts: if the
                    // "contained" block is geometrically adjacent to any block
                    // already in the group, they are likely sub-panels of the
                    // same composite figure rather than an unrelated figure
                    // swallowed by the merge.
                    let adjacent_to_group = origins.iter().any(|&oi| {
                        should_merge_geometry_only(&originals[oi], other, page_w, page_h)
                    });
                    let same_figure = other.caption_number.is_none()
                        || other.caption_number == group.caption_number;
                    if adjacent_to_group && same_figure {
                        pp_info(&format!(
                            "[mineru] containment skip page={}: group cap={:?} body={:?} contains block[{}] body={:?}, but it is adjacent to a group member — keeping merged",
                            _page_idx + 1, group.caption_number, group.body_bbox, other_idx, originals[other_idx].body_bbox
                        ));
                        continue;
                    }
                    contained_other = Some(other_idx);
                    break;
                }
            }
            if let Some(other_idx) = contained_other {
                pp_info(&format!(
                    "[mineru] containment split page={}: group cap={:?} body={:?} contains block[{}] body={:?} | splitting into {} separate blocks",
                    _page_idx + 1, group.caption_number, group.body_bbox, other_idx, originals[other_idx].body_bbox, origins.len()
                ));
                for &idx in &origins {
                    final_merged.push((originals[idx].clone(), vec![idx]));
                }
            } else {
                pp_info(&format!(
                    "[mineru] final group page={}: {} blocks cap={:?} body={:?} desc={}",
                    _page_idx + 1,
                    origins.len(),
                    group.caption_number,
                    group.body_bbox,
                    &group.desc[..group
                        .desc
                        .char_indices()
                        .nth(60)
                        .map(|(i, _)| i)
                        .unwrap_or(group.desc.len())]
                ));
                final_merged.push((group, origins));
            }
        }
        merged = final_merged;

        let mut merged: Vec<RawBlock> = merged.into_iter().map(|(b, _)| b).collect();

        repair_leaked_caption_containers(&mut merged);

        // Snap figure/table bboxes to page body-column edges when the
        // figure's edge sits just inside the text column boundary (within
        // 15pt).  MinerU's image bbox often under-covers the outer frame
        // of a full-width figure by ~5-15pt, causing the right border to
        // be clipped in the hires crop.  This heuristic uses the page's
        // body text blocks as the ground-truth column width.
        if let Some(&(col_left, col_right)) = page_text_edges.get(&_page_idx) {
            const SNAP_THRESHOLD: f32 = 15.0;
            for b in &mut merged {
                let left = b.bbox[0].min(b.bbox[2]);
                let right = b.bbox[0].max(b.bbox[2]);
                let top = b.bbox[1].min(b.bbox[3]);
                let bottom = b.bbox[1].max(b.bbox[3]);
                let mut snapped = false;
                let mut new_left = left;
                let mut new_right = right;
                if col_right - right > 0.0 && col_right - right <= SNAP_THRESHOLD {
                    new_right = col_right;
                    snapped = true;
                }
                if left - col_left > 0.0 && left - col_left <= SNAP_THRESHOLD {
                    new_left = col_left;
                    snapped = true;
                }
                if snapped {
                    b.bbox = [new_left, top, new_right, bottom];
                    pp_info(&format!(
                        "[mineru] page {} snap: {} bbox expanded from [{:.1},{:.1},{:.1},{:.1}] to [{:.1},{:.1},{:.1},{:.1}]",
                        _page_idx + 1, b.block_type,
                        left, top, right, bottom,
                        new_left, top, new_right, bottom,
                    ));
                }
            }
        }

        result.extend(merged);
    }

    // Annotate sub-figures for split figures with (a)(b)(c) suffixes.
    if !split_figures.is_empty() {
        annotate_subfigures(&mut result, split_figures);
    }

    result
}

fn repair_leaked_caption_containers(blocks: &mut Vec<RawBlock>) -> usize {
    let mut absorbed = vec![false; blocks.len()];
    let mut repairs = 0usize;

    for child_idx in 0..blocks.len() {
        if absorbed[child_idx] {
            continue;
        }
        let Some(child_cap) = blocks[child_idx]
            .caption_number
            .clone()
            .or_else(|| extract_caption_number(&blocks[child_idx].desc))
        else {
            continue;
        };
        if !looks_like_caption(&blocks[child_idx].desc) {
            continue;
        }

        let child = blocks[child_idx].clone();
        let child_top = child.bbox[1].min(child.bbox[3]);
        let child_bottom = child.bbox[1].max(child.bbox[3]);

        let mut best_parent: Option<usize> = None;
        let mut best_area = 0.0f32;
        for parent_idx in 0..blocks.len() {
            if parent_idx == child_idx || absorbed[parent_idx] {
                continue;
            }
            let parent = &blocks[parent_idx];
            if parent.page_idx != child.page_idx
                || !same_block_type(&parent.block_type, &child.block_type)
            {
                continue;
            }
            if !bbox_contains(parent.bbox, child.bbox) {
                continue;
            }

            let parent_top = parent.bbox[1].min(parent.bbox[3]);
            let parent_bottom = parent.bbox[1].max(parent.bbox[3]);
            let parent_body_bottom = parent.body_bbox[1].max(parent.body_bbox[3]);
            let leaked_below = parent_bottom - child_bottom;
            if parent_top >= child_top
                || parent_body_bottom > child_top + 5.0
                || leaked_below < 40.0
            {
                continue;
            }

            let parent_cap = parent
                .caption_number
                .clone()
                .or_else(|| extract_caption_number(&parent.desc));
            let parent_starts_with_caption = {
                let t = parent.desc.trim_start().to_ascii_lowercase();
                t.starts_with("figure")
                    || t.starts_with("fig.")
                    || t.starts_with("fig ")
                    || t.starts_with("table")
                    || t.starts_with("tbl")
            };
            if parent_starts_with_caption && parent_cap.as_ref() != Some(&child_cap) {
                continue;
            }

            let area =
                (parent.bbox[2] - parent.bbox[0]).abs() * (parent.bbox[3] - parent.bbox[1]).abs();
            if area > best_area {
                best_area = area;
                best_parent = Some(parent_idx);
            }
        }

        if let Some(parent_idx) = best_parent {
            let parent = &mut blocks[parent_idx];
            let prev_bbox = parent.bbox;
            let left = parent.bbox[0]
                .min(parent.bbox[2])
                .min(child.bbox[0].min(child.bbox[2]));
            let right = parent.bbox[0]
                .max(parent.bbox[2])
                .max(child.bbox[0].max(child.bbox[2]));
            let top = parent.bbox[1].min(parent.bbox[3]).min(child_top);
            parent.bbox = [left, top, right, child_bottom];
            parent.body_bbox = union_bbox(parent.body_bbox, child.body_bbox);
            parent.desc = child.desc.clone();
            parent.caption_number = Some(child_cap);
            absorbed[child_idx] = true;
            repairs += 1;
            pp_info(&format!(
                "[mineru] Repaired leaked figure container on page {}: bbox {:?} clipped/merged with caption {:?} -> {:?}",
                parent.page_idx + 1,
                prev_bbox,
                parent.caption_number,
                parent.bbox
            ));
        }
    }

    if repairs > 0 {
        let mut idx = 0usize;
        blocks.retain(|_| {
            let keep = !absorbed[idx];
            idx += 1;
            keep
        });
    }

    repairs
}

/// Annotate split-figure sub-blocks with (a)(b)(c) or (left)(right)
/// suffixes.  Blocks sharing the same split caption number are grouped
/// by page, sorted left-to-right by bbox, and their descriptions are
/// tagged with sequential labels.
#[allow(clippy::used_underscore_binding)]
fn annotate_subfigures(blocks: &mut [RawBlock], split_figures: &[String]) {
    use std::collections::HashMap;

    // Group blocks by (page_idx, caption_number) for split figures.
    let mut groups: HashMap<(i32, String), Vec<usize>> = HashMap::new();
    for (idx, b) in blocks.iter().enumerate() {
        if let Some(ref cap) = b.caption_number {
            if split_figures.contains(cap) {
                groups
                    .entry((b.page_idx, cap.clone()))
                    .or_default()
                    .push(idx);
            }
        }
    }

    for ((_page_idx, cap), mut indices) in groups {
        if indices.len() <= 1 {
            continue; // Nothing to annotate if only one sub-block.
        }
        // Sort row-major: top-to-bottom by y, then left-to-right within the
        // same visual row.  Pure x-sort mis-labels 2D-arranged panels (a 2×2
        // grid with two top panels and two bottom panels would get
        // (a)/(b)/(c)/(d) by column order instead of row order), and for
        // vertically-stacked panels with identical x (e.g. 2605.10380
        // Figure 3 sub-panels a/b/c all at x≈55) the stable-sort fallback
        // is non-deterministic.  Use a 20pt y-bucket so small jitter
        // between same-row panels (typically ≤5pt) keeps them grouped.
        const Y_BUCKET: f32 = 20.0;
        indices.sort_by_key(|&i| {
            let y_top = blocks[i].bbox[1].min(blocks[i].bbox[3]);
            let x_left = blocks[i].bbox[0].min(blocks[i].bbox[2]);
            let y_bucket = (y_top / Y_BUCKET).round() as i64;
            (y_bucket, (x_left * 1000.0) as i64)
        });

        let labels: Vec<String> = (0..indices.len())
            .map(|i| match i {
                0 => " (a)".to_string(),
                1 => " (b)".to_string(),
                2 => " (c)".to_string(),
                3 => " (d)".to_string(),
                4 => " (e)".to_string(),
                5 => " (f)".to_string(),
                _ => format!(" ({})", i + 1),
            })
            .collect();

        // Build a human-readable prefix from the caption number (e.g. "F:7" -> "Figure 7").
        let figure_prefix = cap.replace("F:", "Figure ").replace("T:", "Table ");

        for (i, &idx) in indices.iter().enumerate() {
            let b = &mut blocks[idx];
            // Always replace with a clean synthetic label like "Figure 7 (a)".
            // The full Figure-N caption text was previously appended only to
            // the panel that originally carried it (the others got the clean
            // label), producing an inconsistent listing in the insight analysis
            // sidebar.  When the user requests a split they want each panel
            // shown as its own independent thumbnail with just the index, not
            // a long sentence on (a) and short labels on (b)/(c)/(d).
            b.desc = format!("{} {}", figure_prefix, labels[i].trim());
            pp_info(&format!(
                "[mineru] Annotated sub-figure: '{}' -> page={}, idx={}",
                b.desc, b.page_idx, i
            ));
        }
    }
}

/// Detect column boundaries from text-block x-intervals on a page.
///
/// Approach: project all intervals onto the x-axis, merge overlaps, then
/// look for gaps wider than `MIN_GUTTER` between adjacent merged ranges.
/// A gap is treated as a column boundary if **both** sides carry
/// substantial coverage (≥ 20% of the total spread).  Returns the midpoint
/// of each qualifying gap.
///
/// Single-column pages return an empty vec.  Two-column papers return one
/// boundary; three-column layouts return two; etc.  Callers can then ask
/// "does this link cross any boundary?" instead of relying on a fixed
/// `h_gap >= 15pt` geometric heuristic that mis-fires on single-column
/// pages with widely-spaced sub-panels.
pub fn detect_column_boundaries(intervals: &[(f32, f32)]) -> Vec<f32> {
    const MIN_GUTTER: f32 = 15.0;
    if intervals.len() < 4 {
        return Vec::new();
    }
    let mut sorted: Vec<(f32, f32)> = intervals
        .iter()
        .map(|&(a, b)| (a.min(b), a.max(b)))
        .collect();
    sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    // Merge overlapping intervals.
    let mut merged: Vec<(f32, f32)> = Vec::new();
    for (s, e) in sorted {
        if let Some(last) = merged.last_mut() {
            if s <= last.1 {
                last.1 = last.1.max(e);
                continue;
            }
        }
        merged.push((s, e));
    }
    if merged.len() < 2 {
        return Vec::new();
    }
    let total_left = merged.first().map(|m| m.0).unwrap_or(0.0);
    let total_right = merged.last().map(|m| m.1).unwrap_or(0.0);
    let total_span = (total_right - total_left).max(1.0);
    let mut boundaries = Vec::new();
    for w in merged.windows(2) {
        let gap = w[1].0 - w[0].1;
        if gap < MIN_GUTTER {
            continue;
        }
        // Both sides must have substantial coverage: a tiny isolated
        // text snippet (e.g. a stray page number stub) doesn't define
        // a column.  Use 20% of the total spanned width as the floor.
        let left_span = w[0].1 - w[0].0;
        let right_span = w[1].1 - w[1].0;
        if left_span < 0.2 * total_span || right_span < 0.2 * total_span {
            continue;
        }
        boundaries.push((w[0].1 + w[1].0) / 2.0);
    }
    boundaries
}

#[allow(clippy::used_underscore_binding)]
pub fn propagate_captions(
    blocks: &mut [RawBlock],
    page_w: f32,
    page_h: f32,
    column_boundaries: Option<&[f32]>,
    paper_id: Option<&str>,
) -> usize {
    let n = blocks.len();
    if n == 0 {
        return 0;
    }

    let page_idx = blocks.first().map(|b| b.page_idx + 1).unwrap_or(0);
    info!(
        "[propagate_captions] ===== START: paper={} page={} {} blocks, page_w={:.1}, page_h={:.1} =====",
        paper_id.unwrap_or("?"), page_idx, n, page_w, page_h
    );
    for (i, b) in blocks.iter().enumerate() {
        info!(
            "[propagate_captions] block[{}]: type={}, cap={:?}, bbox=[{:.1},{:.1},{:.1},{:.1}], size={:.1}x{:.1}",
            i,
            b.block_type,
            b.caption_number,
            b.bbox[0], b.bbox[1], b.bbox[2], b.bbox[3],
            (b.bbox[2] - b.bbox[0]).abs(),
            (b.bbox[3] - b.bbox[1]).abs()
        );
    }

    // Build weighted adjacency from geometry-only should_merge.
    // Weight = v_gap + h_gap, in units of 0.01 pt (scaled to i64 for Dijkstra).
    let mut adj: Vec<Vec<(usize, i64)>> = vec![vec![]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let merged = should_merge_geometry_only(&blocks[i], &blocks[j], page_w, page_h);
            if merged {
                let bi = &blocks[i];
                let bj = &blocks[j];
                let v_gap = if bi.bbox[3] < bj.bbox[1] {
                    bj.bbox[1] - bi.bbox[3]
                } else if bj.bbox[3] < bi.bbox[1] {
                    bi.bbox[1] - bj.bbox[3]
                } else {
                    0.0
                };
                let h_gap = if bi.bbox[2] < bj.bbox[0] {
                    bj.bbox[0] - bi.bbox[2]
                } else if bj.bbox[2] < bi.bbox[0] {
                    bi.bbox[0] - bj.bbox[2]
                } else {
                    0.0
                };
                let aligned = are_aligned(bi, bj);
                let h_overlap = (bi.bbox[2].min(bj.bbox[2]) - bi.bbox[0].max(bj.bbox[0])).max(0.0);
                // Base weight = total gap in 0.01pt.
                let mut weight = ((v_gap + h_gap).max(0.0) * 100.0).round() as i64;
                // Cross-column penalty: two blocks separated by a real
                // column boundary almost never belong to the same figure.
                // Detection mode:
                //   - column_boundaries = Some(detected): trust the detected
                //     boundaries; penalize only links that actually straddle
                //     a boundary.  Single-column pages pass Some(&[]) and
                //     get no penalty regardless of h_gap.
                //   - column_boundaries = None: fall back to the geometric
                //     heuristic "h_overlap=0 and h_gap≥15" so callers that
                //     don't (yet) compute column info still get protection.
                //
                // Reference case: 2605.10380 page 3, where Figure 3's (a)
                // sub-panel (left column) had a same-row cross-column link
                // to Figure 4 (right column) of weight 20 vs 23 for the
                // within-column path through (b).  The 10× penalty pushes
                // the cross-column distance to 200 so propagation stays
                // within the left column.
                const XCOL_PENALTY_MULTIPLIER: i64 = 10;
                let bi_left = bi.bbox[0].min(bi.bbox[2]);
                let bi_right = bi.bbox[0].max(bi.bbox[2]);
                let bj_left = bj.bbox[0].min(bj.bbox[2]);
                let bj_right = bj.bbox[0].max(bj.bbox[2]);
                let crosses_column = match column_boundaries {
                    Some(boundaries) => boundaries.iter().any(|&b| {
                        (bi_right <= b && bj_left >= b) || (bj_right <= b && bi_left >= b)
                    }),
                    None => h_overlap == 0.0 && h_gap >= 15.0,
                };
                if crosses_column {
                    weight = weight.saturating_mul(XCOL_PENALTY_MULTIPLIER);
                }
                info!(
                    "[propagate_captions] LINK block[{}]↔block[{}]: aligned={}, v_gap={:.1}, h_gap={:.1}, h_overlap={:.1}, w={:.2}{} | cap[{}]={:?}, cap[{}]={:?}",
                    i, j, aligned, v_gap, h_gap, h_overlap, weight as f32 / 100.0,
                    if h_overlap == 0.0 { " (xcol)" } else { "" },
                    i, bi.caption_number,
                    j, bj.caption_number
                );
                adj[i].push((j, weight));
                adj[j].push((i, weight));
            }
        }
    }

    // Find connected components via DFS.
    let mut visited = vec![false; n];
    let mut components: Vec<Vec<usize>> = Vec::new();
    for i in 0..n {
        if visited[i] {
            continue;
        }
        let mut stack = vec![i];
        let mut comp = Vec::new();
        visited[i] = true;
        while let Some(u) = stack.pop() {
            comp.push(u);
            for &(v, _) in &adj[u] {
                if !visited[v] {
                    visited[v] = true;
                    stack.push(v);
                }
            }
        }
        components.push(comp);
    }

    info!(
        "[propagate_captions] Found {} connected component(s)",
        components.len()
    );

    // Tie tolerance for Dijkstra distances in i64 (1 pt × 100 = 100).
    // Two paths within 1 pt are treated as equidistant.
    const TIE_EPS: i64 = 100;

    let mut total_propagated = 0usize;
    for (ci, comp) in components.iter().enumerate() {
        // Anchors: indices in `comp` whose caption is type-compatible with
        // their block_type.  Mis-typed captions (e.g. T:7 on an image block)
        // are not trusted as anchors and may be overwritten by propagation.
        let anchors: Vec<usize> = comp
            .iter()
            .copied()
            .filter(|&i| {
                blocks[i]
                    .caption_number
                    .as_ref()
                    .map(|c| !is_caption_type_mismatch(c, &blocks[i].block_type))
                    .unwrap_or(false)
            })
            .collect();

        let captions: std::collections::HashSet<String> = anchors
            .iter()
            .map(|&i| blocks[i].caption_number.clone().unwrap())
            .collect();
        let cap_list: Vec<String> = captions.iter().cloned().collect();
        info!(
            "[propagate_captions] Component[{}]: blocks={:?}, distinct_captions={:?} (count={})",
            ci,
            comp,
            cap_list,
            captions.len()
        );

        if captions.is_empty() {
            info!(
                "[propagate_captions] Component[{}]: SKIPPED — no caption available to propagate",
                ci
            );
            continue;
        }

        // Anchor lookup as a set for barrier check.
        let anchor_set: std::collections::HashSet<usize> = anchors.iter().copied().collect();

        // For each anchor A, run Dijkstra from A with the OTHER anchors as
        // barriers (we may reach them, but we never relax through them).
        // This yields the true geometric "reach distance" of each block to A
        // without crossing another figure's territory.
        let mut anchor_dists: Vec<std::collections::HashMap<usize, i64>> =
            Vec::with_capacity(anchors.len());
        for &a in &anchors {
            let dist = dijkstra_with_barriers(&adj, a, &anchor_set);
            anchor_dists.push(dist);
        }

        let mut propagated_in_comp = 0usize;
        let mut ambiguous_in_comp = 0usize;
        for &i in comp {
            // Don't overwrite blocks that already carry a valid caption.
            let cur_cap_valid = blocks[i]
                .caption_number
                .as_ref()
                .map(|c| !is_caption_type_mismatch(c, &blocks[i].block_type))
                .unwrap_or(false);
            if cur_cap_valid {
                continue;
            }

            // Collect (distance, caption) pairs for this block across anchors.
            let mut min_dist: Option<i64> = None;
            let mut nearest_caps: Vec<String> = Vec::new();
            for (ai, &a) in anchors.iter().enumerate() {
                let Some(&d) = anchor_dists[ai].get(&i) else {
                    continue;
                };
                let cap = blocks[a].caption_number.clone().unwrap();
                match min_dist {
                    None => {
                        min_dist = Some(d);
                        nearest_caps = vec![cap];
                    }
                    Some(md) => {
                        if d + TIE_EPS < md {
                            min_dist = Some(d);
                            nearest_caps = vec![cap];
                        } else if (d - md).abs() <= TIE_EPS && !nearest_caps.contains(&cap) {
                            nearest_caps.push(cap);
                        }
                    }
                }
            }

            match (min_dist, nearest_caps.len()) {
                (None, _) => {
                    info!(
                        "[propagate_captions]   → UNREACHABLE block[{}]: no anchor reaches this block",
                        i
                    );
                }
                (Some(d), 1) => {
                    let cap = nearest_caps.into_iter().next().unwrap();
                    let old = blocks[i].caption_number.clone();
                    blocks[i].caption_number = Some(cap.clone());
                    // Keep the original placeholder desc.  Copying the anchor's
                    // full caption text would fool `numbered_caption_barrier`
                    // into treating unrelated blocks (e.g. inline images far
                    // below a figure) as sub-panels of the same composite
                    // figure, causing the bbox to engulf content that should
                    // not be part of the crop.
                    total_propagated += 1;
                    propagated_in_comp += 1;
                    info!(
                        "[propagate_captions]   → PROPAGATE block[{}]: {:?} → {:?} (nearest, dist={:.2})",
                        i, old, cap, d as f32 / 100.0
                    );
                }
                (Some(d), _) => {
                    ambiguous_in_comp += 1;
                    info!(
                        "[propagate_captions]   → AMBIGUOUS block[{}]: tied between {:?} at dist={:.2} — leaving uncaptioned",
                        i, nearest_caps, d as f32 / 100.0
                    );
                }
            }
        }

        info!(
            "[propagate_captions] Component[{}]: propagated {} block(s), {} ambiguous",
            ci, propagated_in_comp, ambiguous_in_comp
        );
    }

    info!(
        "[propagate_captions] ===== END: paper={} page={} propagated {} caption number(s) =====",
        paper_id.unwrap_or("?"),
        page_idx,
        total_propagated
    );
    total_propagated
}

fn dijkstra_with_barriers(
    adj: &[Vec<(usize, i64)>],
    source: usize,
    barriers: &std::collections::HashSet<usize>,
) -> std::collections::HashMap<usize, i64> {
    use std::cmp::Reverse;
    use std::collections::{BinaryHeap, HashMap};

    let mut dist: HashMap<usize, i64> = HashMap::new();
    let mut heap: BinaryHeap<Reverse<(i64, usize)>> = BinaryHeap::new();
    dist.insert(source, 0);
    heap.push(Reverse((0, source)));

    while let Some(Reverse((d, u))) = heap.pop() {
        if let Some(&fd) = dist.get(&u) {
            if d > fd {
                continue;
            }
        }
        // We reached u; record it but don't relax further if u is a barrier
        // other than the source.
        if u != source && barriers.contains(&u) {
            continue;
        }
        for &(v, w) in &adj[u] {
            let nd = d + w;
            if dist.get(&v).is_none_or(|&fd| nd < fd) {
                dist.insert(v, nd);
                heap.push(Reverse((nd, v)));
            }
        }
    }
    dist
}

fn propagate_page_unique_caption(blocks: &mut [RawBlock]) -> usize {
    if blocks.is_empty() {
        return 0;
    }

    let mut fig_caps: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut tbl_caps: std::collections::HashSet<String> = std::collections::HashSet::new();
    for b in blocks.iter() {
        if let Some(cap) = b.caption_number.as_ref() {
            // Skip wrong-type bindings so a misbound caption does not poison
            // the uniqueness count.
            if is_caption_type_mismatch(cap, &b.block_type) {
                continue;
            }
            if cap.starts_with("F:") {
                fig_caps.insert(cap.clone());
            } else if cap.starts_with("T:") {
                tbl_caps.insert(cap.clone());
            }
        }
    }

    let unique_fig = if fig_caps.len() == 1 {
        fig_caps.into_iter().next()
    } else {
        None
    };
    let unique_tbl = if tbl_caps.len() == 1 {
        tbl_caps.into_iter().next()
    } else {
        None
    };

    if unique_fig.is_none() && unique_tbl.is_none() {
        return 0;
    }

    let mut propagated = 0usize;
    for b in blocks.iter_mut() {
        // Already has a type-consistent caption — leave it alone.
        let has_valid_cap = b
            .caption_number
            .as_ref()
            .is_some_and(|c| !is_caption_type_mismatch(c, &b.block_type));
        if has_valid_cap {
            continue;
        }
        if b.caption_number.is_some() {
            continue;
        }
        let cap = match b.block_type.as_str() {
            "image" | "chart" => unique_fig.as_ref(),
            "table" => unique_tbl.as_ref(),
            _ => None,
        };
        if let Some(cap) = cap {
            let old = b.caption_number.clone();
            b.caption_number = Some(cap.clone());
            propagated += 1;
            info!(
                "[propagate_page_unique_caption] '{}' -> {} block at [{:.1},{:.1},{:.1},{:.1}] (was {:?})",
                cap, b.block_type, b.bbox[0], b.bbox[1], b.bbox[2], b.bbox[3], old
            );
        }
    }
    propagated
}

pub(crate) fn absorb_titles_into_figures(figures: &mut [RawBlock], titles: &[TitleHint]) {
    const X_OVERLAP_RATIO_MIN: f32 = 0.85;
    const ABOVE_GAP_MAX: f32 = 30.0;

    for title in titles {
        let [tx0, ty0, tx1, ty1] = title.bbox;
        let title_w = (tx1 - tx0).abs();
        if title_w < 1.0 {
            continue;
        }

        let mut best: Option<(usize, f32)> = None;
        for (i, fig) in figures.iter().enumerate() {
            if fig.page_idx != title.page_idx {
                continue;
            }
            if fig.caption_number.is_none() {
                continue;
            }
            // Use body_bbox (the original MinerU body envelope) for overlap
            // checking so that a wide caption below the figure does not
            // spuriously enlarge the horizontal span and pull in an
            // unrelated section title above the figure (e.g. "A Appendix"
            // above Figure 9 in 2604.18002).  The absorption itself still
            // expands the full bbox used for cropping.
            let [fx0, fy0, fx1, fy1] = fig.body_bbox;
            let x_overlap = (fx1.min(tx1) - fx0.max(tx0)).max(0.0);
            let x_overlap_ratio = x_overlap / title_w;
            if x_overlap_ratio < X_OVERLAP_RATIO_MIN {
                continue;
            }
            let inside = ty0 >= fy0 && ty1 <= fy1;
            let above_gap = fy0 - ty1;
            if !inside && !(0.0..=ABOVE_GAP_MAX).contains(&above_gap) {
                continue;
            }
            let score = if inside { 0.0 } else { above_gap };
            match best {
                Some((_, s)) if s <= score => {}
                _ => best = Some((i, score)),
            }
        }

        if let Some((idx, _)) = best {
            let fig = &mut figures[idx];
            let prev = fig.bbox;
            fig.bbox = union_bbox(fig.bbox, title.bbox);
            info!(
                "[absorb_titles] page {}: title [{:.1},{:.1},{:.1},{:.1}] absorbed into figure {:?}; bbox {:?} -> {:?}",
                title.page_idx + 1,
                tx0, ty0, tx1, ty1,
                fig.caption_number,
                prev,
                fig.bbox
            );
        }
    }
}

pub(crate) fn is_noise(b: &RawBlock) -> bool {
    let is_placeholder = b.desc.starts_with("image on page")
        || b.desc.starts_with("chart on page")
        || b.desc.starts_with("table on page");
    if !is_placeholder {
        return false;
    }

    let [x0, y0, x1, y1] = b.bbox;
    let w = (x1 - x0).abs();
    let h = (y1 - y0).abs();
    let area = w * h;

    // Too small to be a real figure/table (e.g. icons, page markers).
    // Only filter when BOTH dimensions are tiny — narrow/wide sub-panels
    // of composite figures (e.g. 2408.15998 page 19) are legitimate.
    if w < 40.0 && h < 40.0 {
        return true;
    }
    // Extreme aspect ratio = separator line or decoration stripe
    let ratio = w.max(h) / w.min(h).max(1.0);
    if ratio > 10.0 {
        return true;
    }
    // Wide but short banner / header stripe (e.g. author affiliation bar)
    if w > 300.0 && h < 55.0 && ratio > 6.0 {
        return true;
    }
    // Too small area (< ~0.25% of an A4 page)
    if area < 1200.0 {
        return true;
    }

    false
}

/// Whether a block's description is a bare placeholder generated by
/// MinerU (e.g. "image on page 23").  These blocks carry no semantic
/// content of their own and are likely sub-panels of a composite figure.
fn is_generic_placeholder(b: &RawBlock) -> bool {
    b.desc.starts_with("image on page")
        || b.desc.starts_with("chart on page")
        || b.desc.starts_with("table on page")
}

/// Returns the body_bbox when it has positive area, otherwise falls back
/// to bbox.  Using body_bbox prevents caption-expanded bboxes from giving
/// a figure artificial horizontal reach (e.g. 2306.00978 page 9: the
/// full-width "Figure 6" caption strip made block[0] span the entire page
/// width, letting it steal Figure 7's sub-images via propagate_captions).
fn get_geom_bbox(b: &RawBlock) -> [f32; 4] {
    let bb = b.body_bbox;
    let w = (bb[2] - bb[0]).abs();
    let h = (bb[3] - bb[1]).abs();
    if w > 0.0 && h > 0.0 {
        bb
    } else {
        b.bbox
    }
}

fn are_aligned(a: &RawBlock, b: &RawBlock) -> bool {
    let [ax0, ay0, ax1, ay1] = get_geom_bbox(a);
    let [bx0, by0, bx1, by1] = get_geom_bbox(b);
    let a_w = (ax1 - ax0).abs();
    let a_h = (ay1 - ay0).abs();
    let b_w = (bx1 - bx0).abs();
    let b_h = (by1 - by0).abs();

    let v_overlap = (ay1.min(by1) - ay0.max(by0)).max(0.0);
    let same_row = v_overlap > 0.0
        && (v_overlap / a_h.min(b_h)) > 0.5
        && ((a_h - b_h).abs() / a_h.max(b_h)) < 0.35;

    let h_overlap = (ax1.min(bx1) - ax0.max(bx0)).max(0.0);
    let v_gap = if ay1 < by0 {
        by0 - ay1
    } else if by1 < ay0 {
        ay0 - by1
    } else {
        0.0
    };
    let same_col = h_overlap > 0.0
        && (h_overlap / a_w.min(b_w)) > 0.5
        && ((a_w - b_w).abs() / a_w.max(b_w)) < 0.35
        && v_gap < a_h.min(b_h) * 0.30;

    same_row || same_col
}

/// Geometry-only version of should_merge, used for caption propagation.
/// Omits the caption-number guard so that blocks without captions can still
/// be linked to their spatial neighbours.
/// "chart" and "image" are both figure types — treat them as equivalent
/// so sub-panels classified differently by MinerU can still merge into the
/// same composite figure (e.g. 1312.5602 page 7).
#[allow(clippy::used_underscore_binding, unused_variables)]
pub fn should_merge_geometry_only(a: &RawBlock, b: &RawBlock, page_w: f32, _page_h: f32) -> bool {
    if !same_block_type(&a.block_type, &b.block_type)
        && (a.block_type == "table" || b.block_type == "table")
    {
        let [ax0, ay0, ax1, ay1] = get_geom_bbox(a);
        let [bx0, by0, bx1, by1] = get_geom_bbox(b);
        let a_h = (ay1 - ay0).abs();
        let b_h = (by1 - by0).abs();
        let v_overlap = (ay1.min(by1) - ay0.max(by0)).max(0.0);
        let min_h = a_h.min(b_h).max(1.0);
        let same_row = v_overlap > 0.0 && (v_overlap / min_h) > 0.5;
        let height_similar = a_h.max(b_h) / min_h < 2.5;
        let h_gap = if ax1 < bx0 {
            bx0 - ax1
        } else if bx1 < ax0 {
            ax0 - bx1
        } else {
            0.0
        };
        if !(same_row && height_similar && h_gap < 25.0) {
            return false;
        }
    }
    // Interline_equation must not link with figures/tables.
    // Real equations (e.g. 2604.20556 page 5 LRS formula) can sit
    // geometrically close to a composite figure and would otherwise
    // inherit its caption via propagate_captions and get merged in.
    // Misclassified figures (standalone large interline_equation blocks
    // like 2404.11614 Figure 3) are handled via orphan rebind, not
    // propagation linking.
    let a_is_interline = a.block_type == "interline_equation";
    let b_is_interline = b.block_type == "interline_equation";
    if a_is_interline != b_is_interline {
        return false;
    }

    let [ax0, ay0, ax1, ay1] = get_geom_bbox(a);
    let [bx0, by0, bx1, by1] = get_geom_bbox(b);

    let a_w = (ax1 - ax0).abs();
    let a_h = (ay1 - ay0).abs();
    let b_w = (bx1 - bx0).abs();
    let b_h = (by1 - by0).abs();
    let min_w = a_w.min(b_w);
    let min_h = a_h.min(b_h);

    // Vertically-stacked blocks with different confirmed captions are almost
    // certainly separate figures (e.g. Figure 33 and Figure 34 on 2605.22821
    // page 23).  Reject the link so propagate_captions keeps them in separate
    // components.  Side-by-side blocks are exempt — they may be sub-panels of
    // the same composite figure arranged horizontally.
    let v_overlap = (ay1.min(by1) - ay0.max(by0)).max(0.0);
    let same_row = v_overlap > 0.0 && (v_overlap / min_h) > 0.5;
    if !same_row
        && a.caption_number.is_some()
        && b.caption_number.is_some()
        && a.caption_number != b.caption_number
    {
        return false;
    }

    // Relative-gap guard: only apply the strict hard-reject when BOTH blocks
    // lack a caption.  If at least one side already carries a caption we let
    // the looser IoU / gap thresholds below decide, so that multi-row
    // sub-figures of the same figure (e.g. Figure 5 in 1312.6114) can still
    // be linked for caption propagation.  The caption-unique guard in
    // propagate_captions prevents cross-contamination.
    let aligned = are_aligned(a, b);
    if !aligned {
        let v_gap = if ay1 < by0 {
            by0 - ay1
        } else if by1 < ay0 {
            ay0 - by1
        } else {
            0.0
        };
        if a.caption_number.is_none() && b.caption_number.is_none() && v_gap > min_h * 0.40 {
            info!(
                "[should_merge_geometry_only] false: page={} gap={:.1} > {:.1} (both no cap, not aligned) | '{}' vs '{}'",
                a.page_idx + 1, v_gap, min_h * 0.40,
                desc_snippet(&a.desc, 30), desc_snippet(&b.desc, 30)
            );
            return false;
        }
    }

    // IoU threshold
    let left = ax0.max(bx0);
    let right = ax1.min(bx1);
    let top = ay0.max(by0);
    let bottom = ay1.min(by1);

    if right > left && bottom > top {
        let inter = (right - left) * (bottom - top);
        let a_area = a_w * a_h;
        let b_area = b_w * b_h;
        let union = a_area + b_area - inter;
        if union > 0.0 && inter / union > 0.05 {
            info!(
                "[should_merge_geometry_only] true: page={} IoU={:.2} | '{}' vs '{}'",
                a.page_idx + 1,
                inter / union,
                desc_snippet(&a.desc, 30),
                desc_snippet(&b.desc, 30)
            );
            return true;
        }
    }

    // Use size-relative thresholds for geometry-only linking.
    let is_table_pair = a.block_type == "table" && b.block_type == "table";
    let h_overlap = (ax1.min(bx1) - ax0.max(bx0)).max(0.0);
    let h_gap = if ax1 < bx0 {
        bx0 - ax1
    } else if bx1 < ax0 {
        ax0 - bx1
    } else {
        0.0
    };
    let v_overlap = (ay1.min(by1) - ay0.max(by0)).max(0.0);
    // Cross-column guard: blocks with zero horizontal overlap and a
    // significant horizontal gap sit in different columns of a two-column
    // paper (e.g. 2502.03860 page 7).  Linking them would let
    // propagate_captions "steal" sub-panels from the left-column figure
    // into the right-column figure.
    // Exception: side-by-side sub-panel labels (e.g. "(a) Gemma 5B" and
    // "(b) Gemma 9B" in 2604.21428 page 27) are part of the same composite
    // figure even when the horizontal gap exceeds the column threshold.
    let same_row =
        v_overlap > 0.0 && (v_overlap / min_h) > 0.5 && ((a_h - b_h).abs() / a_h.max(b_h)) < 0.35;
    let a_has_cap = extract_caption_number(&a.desc).is_some();
    let b_has_cap = extract_caption_number(&b.desc).is_some();
    if h_overlap == 0.0 && h_gap > (page_w * 0.02).max(10.0) {
        // For same-row blocks, allow the link if at least one side carries a
        // real caption (e.g. "Figure 5").  Sub-panel labels like "(a) Gemma 5B"
        // do NOT count — they belong to one figure and must not authorise a
        // cross-column link to a block from a different figure (e.g. 2502.03860
        // page 6: Fig.5 sub-panel "(b) Llama-3.1-8B" on the left must not link
        // to Fig.6's bare block on the right).
        // This also lets a bare sub-panel (e.g. the left half of Figure 5 in
        // 2304.10557 page 6) link to its captioned sibling so caption
        // propagation can assign the correct figure number instead of stealing
        // a distant figure's caption.
        if !same_row || !(a_has_cap || b_has_cap) {
            if !same_row {
                info!(
                    "[should_merge_geometry_only] false: page={} not same_row h_gap={:.1} | '{}' vs '{}'",
                    a.page_idx + 1, h_gap,
                    desc_snippet(&a.desc, 30), desc_snippet(&b.desc, 30)
                );
                return false;
            }
            // same_row is true but neither side has a caption.
            // If both blocks are wide (> 25 % page width), they are likely in
            // different columns of a two-column paper (e.g. 2502.03860 page 6:
            // Fig.5 sub-panels on the left vs Fig.6 on the right).  Reject.
            // Narrow blocks (< 25 %) are usually bare sub-panels of a single-
            // column composite figure and are allowed with the original threshold.
            let a_wide = a_w > page_w * 0.25;
            let b_wide = b_w > page_w * 0.25;
            if a_wide && b_wide {
                info!(
                    "[should_merge_geometry_only] false: page={} two wide blocks in different columns (h_gap={:.1}) | '{}' vs '{}'",
                    a.page_idx + 1, h_gap,
                    desc_snippet(&a.desc, 30), desc_snippet(&b.desc, 30)
                );
                return false;
            }
            if h_gap > min_w * 1.5 {
                info!(
                    "[should_merge_geometry_only] false: page={} cross-column h_gap={:.1} > threshold={:.1} | '{}' vs '{}'",
                    a.page_idx + 1, h_gap, (page_w * 0.02).max(10.0),
                    desc_snippet(&a.desc, 30), desc_snippet(&b.desc, 30)
                );
                return false;
            }
        }
    }
    let h_overlap_ratio = if a_w.min(b_w) > 0.0 {
        h_overlap / a_w.min(b_w)
    } else {
        0.0
    };
    let (h_gap_threshold, v_gap_threshold) = if is_table_pair {
        (min_w * 0.15, min_h * 0.15)
    } else {
        // Vertically stacked images (multi-row composite figures) can have
        // large gaps between rows.  Use an absolute cap so sub-panels above
        // the caption still link to the main body (e.g. 2604.13030 page 10:
        // 176 pt gap between sub-panels and main figure).
        //
        // Distinguish same-caption pairs (legitimate composite figure
        // sub-panels) from no-cap / different-cap pairs (independent figures
        // stacked on the same page).  The latter get a much tighter cap so
        // unrelated charts like those on 2605.22821 page 23 (124 pt gap)
        // do not get linked.
        let a_is_generic = is_generic_placeholder(a);
        let b_is_generic = is_generic_placeholder(b);
        let same_confirmed_cap = a.caption_number.is_some()
            && b.caption_number.is_some()
            && a.caption_number == b.caption_number;
        let v_thresh = if h_overlap_ratio > 0.5 {
            if same_confirmed_cap || a_is_generic || b_is_generic {
                (min_h * 0.30).max(250.0)
            } else {
                (min_h * 0.30).max(100.0)
            }
        } else {
            min_h * 0.30
        };
        // Side-by-side sub-images of the same composite figure can have
        // significant horizontal gaps between them (e.g. 2306.00978 page 9
        // Figure 7's four sub-images separated by descriptive text).
        let h_thresh = if same_row {
            (min_w * 1.5).max(100.0)
        } else {
            min_w * 0.30
        };
        (h_thresh, v_thresh)
    };
    let v_gap = if ay1 < by0 {
        by0 - ay1
    } else if by1 < ay0 {
        ay0 - by1
    } else {
        0.0
    };
    let v_overlap_ratio = if a_h.min(b_h) > 0.0 {
        v_overlap / a_h.min(b_h)
    } else {
        0.0
    };
    if h_overlap_ratio > 0.3 && v_gap < v_gap_threshold {
        info!(
            "[should_merge_geometry_only] true: page={} h_ovr={:.2} v_gap={:.1} < {:.1} | '{}' vs '{}'",
            a.page_idx + 1, h_overlap_ratio, v_gap, v_gap_threshold,
            desc_snippet(&a.desc, 30), desc_snippet(&b.desc, 30)
        );
        return true;
    }

    if v_overlap_ratio > 0.3 && h_gap < h_gap_threshold {
        info!(
            "[should_merge_geometry_only] true: page={} v_ovr={:.2} h_gap={:.1} < {:.1} | '{}' vs '{}'",
            a.page_idx + 1, v_overlap_ratio, h_gap, h_gap_threshold,
            desc_snippet(&a.desc, 30), desc_snippet(&b.desc, 30)
        );
        return true;
    }

    info!(
        "[should_merge_geometry_only] false: page={} aligned={} h_ovr={:.2} v_gap={:.1} v_ovr={:.2} h_gap={:.1} | '{}' vs '{}'",
        a.page_idx + 1, aligned, h_overlap_ratio, v_gap, v_overlap_ratio, h_gap,
        desc_snippet(&a.desc, 30), desc_snippet(&b.desc, 30)
    );
    false
}

pub fn should_merge(
    a: &RawBlock,
    b: &RawBlock,
    page_w: f32,
    page_h: f32,
    split_figures: &[String],
) -> bool {
    // Split-figure guard
    if let Some(ref an) = a.caption_number {
        if split_figures.contains(an) {
            return false;
        }
    }
    if let Some(ref bn) = b.caption_number {
        if split_figures.contains(bn) {
            return false;
        }
    }

    match (&a.caption_number, &b.caption_number) {
        (Some(an), Some(bn)) if an != bn => {
            info!(
                "[should_merge] false: cap mismatch {} vs {} | '{}' vs '{}'",
                an,
                bn,
                desc_snippet(&a.desc, 25),
                desc_snippet(&b.desc, 25)
            );
            return false;
        }
        _ => {}
    }

    // Interline_equation must not merge with figures/tables.
    let a_is_interline = a.block_type == "interline_equation";
    let b_is_interline = b.block_type == "interline_equation";
    if a_is_interline != b_is_interline {
        info!(
            "[should_merge] false: interline_equation cross-type | '{}' vs '{}'",
            desc_snippet(&a.desc, 25),
            desc_snippet(&b.desc, 25)
        );
        return false;
    }

    // "chart" and "image" are both figure types — treat as equivalent
    // so sub-panels of the same figure can merge (e.g. 1312.5602 page 7).
    if !same_block_type(&a.block_type, &b.block_type)
        && (a.caption_number.is_some() || b.caption_number.is_some())
    {
        let same_caption = a.caption_number.is_some()
            && b.caption_number.is_some()
            && a.caption_number == b.caption_number;
        if !same_caption {
            info!(
                "[should_merge] false: cross-type {} vs {} | '{}' vs '{}'",
                a.block_type,
                b.block_type,
                desc_snippet(&a.desc, 25),
                desc_snippet(&b.desc, 25)
            );
            return false;
        }
    }

    if numbered_caption_barrier(a, b) || numbered_caption_barrier(b, a) {
        info!(
            "[should_merge] false: caption barrier | '{}' vs '{}'",
            desc_snippet(&a.desc, 25),
            desc_snippet(&b.desc, 25)
        );
        return false;
    }

    let [ax0, ay0, ax1, ay1] = a.bbox;
    let [bx0, by0, bx1, by1] = b.bbox;

    let a_w = (ax1 - ax0).abs();
    let a_h = (ay1 - ay0).abs();
    let b_w = (bx1 - bx0).abs();
    let b_h = (by1 - by0).abs();

    let same_confirmed_cap = a.caption_number.is_some()
        && b.caption_number.is_some()
        && a.caption_number == b.caption_number;
    let aligned = are_aligned(a, b);
    if !aligned && !same_confirmed_cap {
        let v_gap = if ay1 < by0 {
            by0 - ay1
        } else if by1 < ay0 {
            ay0 - by1
        } else {
            0.0
        };
        let min_h = a_h.min(b_h);
        if v_gap > min_h * 0.40 {
            info!(
                "[should_merge] false: v_gap={:.1} > {:.1} (not aligned, no shared cap) | '{}' vs '{}'",
                v_gap, min_h * 0.40, desc_snippet(&a.desc, 25), desc_snippet(&b.desc, 25)
            );
            return false;
        }
    }

    let left = ax0.max(bx0);
    let right = ax1.min(bx1);
    let top = ay0.max(by0);
    let bottom = ay1.min(by1);

    if right > left && bottom > top {
        let inter = (right - left) * (bottom - top);
        let a_area = a_w * a_h;
        let b_area = b_w * b_h;
        let union = a_area + b_area - inter;
        if union > 0.0 && inter / union > 0.05 {
            info!(
                "[should_merge] true: IoU={:.2} | '{}' vs '{}'",
                inter / union,
                desc_snippet(&a.desc, 25),
                desc_snippet(&b.desc, 25)
            );
            return true;
        }
    }

    let is_table_pair = a.block_type == "table" && b.block_type == "table";
    let both_no_cap = a.caption_number.is_none() && b.caption_number.is_none();
    let aligned2 = are_aligned(a, b);

    let (h_gap_threshold, v_gap_threshold) = if is_table_pair {
        if both_no_cap && !aligned2 {
            ((page_w * 0.04).max(10.0), (page_h * 0.02).max(5.0))
        } else {
            ((page_w * 0.08).max(20.0), (page_h * 0.04).max(10.0))
        }
    } else {
        if both_no_cap && !aligned2 {
            ((page_w * 0.10).max(20.0), (page_h * 0.08).max(20.0))
        } else {
            // For blocks that already share a confirmed caption (e.g. sub-panels
            // of a multi-row figure linked by propagate_captions), allow larger
            // vertical gaps so the whole composite merges into one image.
            // Generic placeholders ("image on page N") also get the larger
            // threshold because they are likely bare sub-panels of a composite
            // figure (e.g. 2604.13030 page 10).
            let a_is_generic = is_generic_placeholder(a);
            let b_is_generic = is_generic_placeholder(b);
            let v_thresh = if same_confirmed_cap || a_is_generic || b_is_generic {
                (page_h * 0.22).max(60.0)
            } else {
                (page_h * 0.12).max(40.0)
            };
            ((page_w * 0.25).max(40.0), v_thresh)
        }
    };

    let h_overlap = (ax1.min(bx1) - ax0.max(bx0)).max(0.0);
    let h_overlap_ratio = if a_w.min(b_w) > 0.0 {
        h_overlap / a_w.min(b_w)
    } else {
        0.0
    };
    let v_gap = if ay1 < by0 {
        by0 - ay1
    } else if by1 < ay0 {
        ay0 - by1
    } else {
        0.0
    };
    if h_overlap_ratio > 0.3 && v_gap < v_gap_threshold {
        info!(
            "[should_merge] true: h_ovr={:.2} v_gap={:.1} < {:.1} | '{}' vs '{}'",
            h_overlap_ratio,
            v_gap,
            v_gap_threshold,
            desc_snippet(&a.desc, 25),
            desc_snippet(&b.desc, 25)
        );
        return true;
    }

    let v_overlap = (ay1.min(by1) - ay0.max(by0)).max(0.0);
    let v_overlap_ratio = if a_h.min(b_h) > 0.0 {
        v_overlap / a_h.min(b_h)
    } else {
        0.0
    };
    let h_gap = if ax1 < bx0 {
        bx0 - ax1
    } else if bx1 < ax0 {
        ax0 - bx1
    } else {
        0.0
    };
    if v_overlap_ratio > 0.3 && h_gap < h_gap_threshold {
        info!(
            "[should_merge] true: v_ovr={:.2} h_gap={:.1} < {:.1} | '{}' vs '{}'",
            v_overlap_ratio,
            h_gap,
            h_gap_threshold,
            desc_snippet(&a.desc, 25),
            desc_snippet(&b.desc, 25)
        );
        return true;
    }

    info!(
        "[should_merge] false: page={} aligned={} same_cap={} h_ovr={:.2} v_gap={:.1} v_ovr={:.2} h_gap={:.1} | '{}' vs '{}'",
        a.page_idx + 1, aligned2, same_confirmed_cap, h_overlap_ratio, v_gap, v_overlap_ratio, h_gap,
        desc_snippet(&a.desc, 25), desc_snippet(&b.desc, 25)
    );
    false
}

fn numbered_caption_barrier(captioned: &RawBlock, other: &RawBlock) -> bool {
    let Some(cap) = captioned.caption_number.as_ref() else {
        return false;
    };
    if extract_caption_number(&captioned.desc).as_ref() != Some(cap) {
        return false;
    }
    if other.caption_number.as_ref() == Some(cap) {
        return false;
    }
    if extract_caption_number(&other.desc).as_ref() == Some(cap) {
        return false;
    }
    // A block without its own caption number might be a sub-panel (e.g. "(b) ..."
    // directly below a table).  Only treat it as a barrier when its width is
    // clearly smaller than the captioned block — a narrow panel belonging to a
    // different figure rather than a full-width continuation of the same block.
    if other.caption_number.is_none() {
        let cap_w = (captioned.body_bbox[2] - captioned.body_bbox[0])
            .abs()
            .max(1.0);
        let other_w = (other.body_bbox[2] - other.body_bbox[0]).abs();
        if other_w / cap_w >= 0.65 {
            return false;
        }
    }

    let captioned_bottom = captioned.bbox[1].max(captioned.bbox[3]);
    let other_top = other.bbox[1].min(other.bbox[3]);
    other_top >= captioned_bottom - 2.0
}

pub fn merge_into(target: &mut RawBlock, other: &RawBlock) {
    let before = target.clone();
    let leak_repair = leaked_container_repair(&before, other);

    // Expand to bounding rectangle
    target.bbox[0] = target.bbox[0].min(other.bbox[0]);
    target.bbox[1] = target.bbox[1].min(other.bbox[1]);
    target.bbox[2] = target.bbox[2].max(other.bbox[2]);
    target.bbox[3] = target.bbox[3].max(other.bbox[3]);
    // Body envelope merges independently.
    target.body_bbox[0] = target.body_bbox[0].min(other.body_bbox[0]);
    target.body_bbox[1] = target.body_bbox[1].min(other.body_bbox[1]);
    target.body_bbox[2] = target.body_bbox[2].max(other.body_bbox[2]);
    target.body_bbox[3] = target.body_bbox[3].max(other.body_bbox[3]);

    // Merge descriptions: avoid fusing placeholder texts like "image on page N"
    // into real captions.  Keep the richer caption when one side is only a
    // fallback placeholder.
    let is_placeholder = |d: &str| {
        (d.starts_with("image on page")
            || d.starts_with("chart on page")
            || d.starts_with("table on page"))
            && !d.contains("Figure")
            && !d.contains("Table")
    };
    // Sub-string containment check: a short panel label like "(a)" can appear
    // inside a long caption body (e.g. "In (a), we observe ...").  Only treat
    // it as true containment when the shorter text is a prefix/suffix or is
    // itself substantial (>=20 chars).
    fn is_contained(shorter: &str, longer: &str) -> bool {
        if longer.starts_with(shorter) || longer.ends_with(shorter) {
            return true;
        }
        if shorter.len() >= 20 {
            return longer.contains(shorter);
        }
        false
    }
    let (shorter, longer) = if target.desc.len() < other.desc.len() {
        (&target.desc[..], &other.desc[..])
    } else {
        (&other.desc[..], &target.desc[..])
    };
    let contained = target.desc == other.desc || is_contained(shorter, longer);
    // When the shorter desc is a prefix of the longer one but the longer
    // carries a caption number that the shorter lacks (e.g. sub-panel label
    // "(b) Dataset: …" prefixed to "(b) Dataset: … Figure 11: …"), prefer
    // the richer desc instead of treating the shorter as "containing" it.
    if contained
        && target.desc.len() < other.desc.len()
        && other.desc.starts_with(&target.desc)
        && extract_caption_number(&target.desc).is_none()
        && extract_caption_number(&other.desc).is_some()
    {
        target.desc = other.desc.clone();
    } else if !contained {
        if is_placeholder(&target.desc) && !is_placeholder(&other.desc) {
            target.desc = other.desc.clone();
        } else if !is_placeholder(&target.desc) && is_placeholder(&other.desc) {
            // keep target.desc
        } else if is_placeholder(&target.desc) && is_placeholder(&other.desc) {
            target.desc = other.desc.clone();
        } else {
            target.desc = format!("{}; {}", target.desc, other.desc);
        }
    }

    // Keep the image from the larger sub-block for LLM analysis
    let target_area =
        (target.bbox[2] - target.bbox[0]).abs() * (target.bbox[3] - target.bbox[1]).abs();
    let other_area = (other.bbox[2] - other.bbox[0]).abs() * (other.bbox[3] - other.bbox[1]).abs();
    if other_area > target_area {
        target.img_path = other.img_path.clone();
    }

    // Prefer "image" over "chart"/"table" for merged composite figures
    if other.block_type == "image" {
        target.block_type = "image".to_string();
    }

    // Inherit caption number: if target lacks one but other has it,
    // propagate it so that later blocks with a *different* number are
    // correctly rejected by the caption-number guard in should_merge.
    // Also replace a mismatched caption (e.g. T:7 on an image block) with a
    // valid one from the other block.
    let target_valid = target
        .caption_number
        .as_ref()
        .is_some_and(|c| !is_caption_type_mismatch(c, &target.block_type));
    let other_valid = other
        .caption_number
        .as_ref()
        .is_some_and(|c| !is_caption_type_mismatch(c, &other.block_type));
    if (!target_valid && other_valid)
        || (target.caption_number.is_none() && other.caption_number.is_some())
    {
        target.caption_number = other.caption_number.clone();
    }

    if let Some((bbox, body_bbox, desc, cap)) = leak_repair {
        target.bbox = bbox;
        target.body_bbox = body_bbox;
        target.desc = desc;
        target.caption_number = cap;
    }
}

type LeakRepair = Option<([f32; 4], [f32; 4], String, Option<String>)>;

fn leaked_container_repair(a: &RawBlock, b: &RawBlock) -> LeakRepair {
    fn cap_of(b: &RawBlock) -> Option<String> {
        b.caption_number
            .clone()
            .or_else(|| extract_caption_number(&b.desc))
    }

    fn try_pair(parent: &RawBlock, child: &RawBlock) -> LeakRepair {
        if parent.page_idx != child.page_idx
            || !same_block_type(&parent.block_type, &child.block_type)
        {
            return None;
        }
        if !bbox_contains(parent.bbox, child.bbox) {
            return None;
        }
        if !looks_like_caption(&child.desc) {
            return None;
        }

        let child_cap = cap_of(child)?;
        let parent_cap = cap_of(parent);
        let parent_starts_with_caption = {
            let t = parent.desc.trim_start().to_ascii_lowercase();
            t.starts_with("figure")
                || t.starts_with("fig.")
                || t.starts_with("fig ")
                || t.starts_with("table")
                || t.starts_with("tbl")
        };
        if parent_starts_with_caption && parent_cap.as_ref() != Some(&child_cap) {
            return None;
        }

        let parent_top = parent.bbox[1].min(parent.bbox[3]);
        let parent_bottom = parent.bbox[1].max(parent.bbox[3]);
        let parent_body_bottom = parent.body_bbox[1].max(parent.body_bbox[3]);
        let child_top = child.bbox[1].min(child.bbox[3]);
        let child_bottom = child.bbox[1].max(child.bbox[3]);
        let leaked_below = parent_bottom - child_bottom;
        if parent_top >= child_top || parent_body_bottom > child_top + 5.0 || leaked_below < 40.0 {
            return None;
        }

        let left = parent.bbox[0]
            .min(parent.bbox[2])
            .min(child.bbox[0].min(child.bbox[2]));
        let right = parent.bbox[0]
            .max(parent.bbox[2])
            .max(child.bbox[0].max(child.bbox[2]));
        let top = parent.bbox[1]
            .min(parent.bbox[3])
            .min(child.bbox[1].min(child.bbox[3]));
        Some((
            [left, top, right, child_bottom],
            union_bbox(parent.body_bbox, child.body_bbox),
            child.desc.clone(),
            Some(child_cap),
        ))
    }

    try_pair(a, b).or_else(|| try_pair(b, a))
}
