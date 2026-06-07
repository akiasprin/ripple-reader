// SPDX-License-Identifier: MIT OR Apache-2.0

//! Validation of extracted figures against layout.json.
use crate::mineru::logging::{pp_info, pp_warn};
use crate::mineru::types::{ExtractedFigures, LayoutDoc};
use crate::mineru::utils::{
    bbox_contains, desc_snippet, extract_caption_number, has_sub_panel_label, looks_like_caption,
};

static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();

// LayoutParaBlock methods (caption, orphan_captions, etc.) are in layout.rs impl

pub fn validate_extracted_figures(
    extracted: &ExtractedFigures,
    doc: Option<&LayoutDoc>,
    paper_id: Option<&str>,
) -> Result<(), String> {
    let mut errors = Vec::new();
    let pid = paper_id.unwrap_or("?");

    // 1. Per-item check: each extraction must have a valid Figure/Table number, not a placeholder
    for (i, ((desc, _), bbox)) in extracted
        .images
        .iter()
        .zip(&extracted.image_bboxes)
        .enumerate()
    {
        let is_placeholder = (desc.starts_with("image on page")
            || desc.starts_with("chart on page")
            || desc.starts_with("table on page"))
            && !desc.contains("Figure")
            && !desc.contains("Table");

        if is_placeholder {
            // If this captionless block is fully contained within another
            // properly-captioned figure on the same page, it's a sub-panel
            // (e.g. table-format sub-figure of a composite figure) and does
            // not need its own caption.
            let contained_by_captioned = extracted
                .image_bboxes
                .iter()
                .zip(extracted.images.iter())
                .enumerate()
                .any(|(j, (other_bb, (other_desc, _)))| {
                    if j == i {
                        return false;
                    }
                    if other_bb.page_idx != bbox.page_idx {
                        return false;
                    }
                    let is_placeholder_other = (other_desc.starts_with("image on page")
                        || other_desc.starts_with("chart on page")
                        || other_desc.starts_with("table on page"))
                        && !other_desc.contains("Figure")
                        && !other_desc.contains("Table");
                    if is_placeholder_other {
                        return false;
                    }
                    bbox_contains(other_bb.bbox, bbox.bbox)
                });
            if contained_by_captioned {
                pp_info(&format!(
                    "[mineru] [{}] page={} type={}: captionless but contained by captioned figure, skipping",
                    i, bbox.page_idx + 1, bbox.content_type
                ));
                continue;
            }
            pp_warn(&format!(
                "[mineru] [{}] page={} type={}: Captionless desc='{}'",
                i,
                bbox.page_idx + 1,
                bbox.content_type,
                desc
            ));
            continue;
        }

        match bbox.content_type.as_str() {
            "image" | "chart" => {
                let has_fig = extract_caption_number(desc)
                    .map(|cn| cn.starts_with("F:"))
                    .unwrap_or(false);
                let has_tbl = extract_caption_number(desc)
                    .map(|cn| cn.starts_with("T:"))
                    .unwrap_or(false);
                if !has_fig && !has_tbl && !has_sub_panel_label(desc) {
                    pp_warn(&format!(
                        "[mineru] [{}] page={} type={}: missing figure number, desc='{}'",
                        i,
                        bbox.page_idx + 1,
                        bbox.content_type,
                        desc
                    ));
                    errors.push(format!(
                        "[{}] page={} type={}: missing figure number, desc='{}'",
                        i,
                        bbox.page_idx + 1,
                        bbox.content_type,
                        desc
                    ));
                }
            }
            "table" => {
                let has_tbl = extract_caption_number(desc)
                    .map(|cn| cn.starts_with("T:"))
                    .unwrap_or(false);
                let has_fig = extract_caption_number(desc)
                    .map(|cn| cn.starts_with("F:"))
                    .unwrap_or(false);
                if !has_tbl && !has_fig && !has_sub_panel_label(desc) {
                    pp_warn(&format!(
                        "[mineru] [{}] page={} type={}: missing table number, desc='{}'",
                        i,
                        bbox.page_idx + 1,
                        bbox.content_type,
                        desc
                    ));
                    errors.push(format!(
                        "[{}] page={} type={}: missing table number, desc='{}'",
                        i,
                        bbox.page_idx + 1,
                        bbox.content_type,
                        desc
                    ));
                }
            }
            _ => {}
        }
    }

    // 1b. Screenshot bbox spatial overlap check: pairwise comparison on same page.
    //     - One placeholder / one with "(a)/(b)" sub-panel label: info only (known dedup leak, non-blocking)
    //     - Both desc strictly start with "Figure N" / "Table N": hard error (caption mis-attached or bbox miscalculated)
    //     - Others: pp_warn (rare, ambiguous, non-blocking)
    let is_placeholder_desc = |d: &str| -> bool {
        (d.starts_with("image on page")
            || d.starts_with("chart on page")
            || d.starts_with("table on page"))
            && !d.contains("Figure")
            && !d.contains("Table")
    };
    let starts_with_strict_caption = |d: &str| -> bool {
        let re = RE.get_or_init(|| {
            regex::Regex::new(r"(?i)^\s*(?:fig(?:ure)?|table|tbl)\.?\s*[0-9]+").unwrap()
        });
        re.is_match(d)
    };
    for i in 0..extracted.image_bboxes.len() {
        for j in (i + 1)..extracted.image_bboxes.len() {
            let a = &extracted.image_bboxes[i];
            let b = &extracted.image_bboxes[j];
            if a.page_idx != b.page_idx {
                continue;
            }

            let ax1 = a.bbox[0].min(a.bbox[2]);
            let ax2 = a.bbox[0].max(a.bbox[2]);
            let ay1 = a.bbox[1].min(a.bbox[3]);
            let ay2 = a.bbox[1].max(a.bbox[3]);
            let bx1 = b.bbox[0].min(b.bbox[2]);
            let bx2 = b.bbox[0].max(b.bbox[2]);
            let by1 = b.bbox[1].min(b.bbox[3]);
            let by2 = b.bbox[1].max(b.bbox[3]);
            let ix1 = ax1.max(bx1);
            let iy1 = ay1.max(by1);
            let ix2 = ax2.min(bx2);
            let iy2 = ay2.min(by2);
            if ix2 <= ix1 || iy2 <= iy1 {
                continue;
            }

            let inter = (ix2 - ix1) * (iy2 - iy1);
            let area_a = (ax2 - ax1) * (ay2 - ay1);
            let area_b = (bx2 - bx1) * (by2 - by1);
            let min_area = area_a.min(area_b);
            if min_area <= 0.0 {
                continue;
            }
            let ratio = inter / min_area;

            let (desc_a, _) = &extracted.images[i];
            let (desc_b, _) = &extracted.images[j];
            let snippet_a = desc_snippet(desc_a, 60);
            let snippet_b = desc_snippet(desc_b, 60);
            let either_loose = is_placeholder_desc(desc_a)
                || is_placeholder_desc(desc_b)
                || has_sub_panel_label(desc_a)
                || has_sub_panel_label(desc_b);
            let both_strict_caption =
                starts_with_strict_caption(desc_a) && starts_with_strict_caption(desc_b);

            if either_loose {
                pp_info(&format!(
                    "[mineru] overlap (sub-panel/placeholder, not blocking): [{},{}] page={} ratio={:.2} '{}' vs '{}'",
                    i, j, a.page_idx + 1, ratio, snippet_a, snippet_b
                ));
            } else if both_strict_caption {
                pp_warn(&format!(
                    "[mineru] overlap: [{},{}] page={} ratio={:.2} '{}' vs '{}'",
                    i,
                    j,
                    a.page_idx + 1,
                    ratio,
                    snippet_a,
                    snippet_b
                ));
                errors.push(format!(
                    "[{},{}] page={}: screenshot bbox spatial overlap (ratio={:.2}), '{}' vs '{}'",
                    i,
                    j,
                    a.page_idx + 1,
                    ratio,
                    snippet_a,
                    snippet_b
                ));
            } else {
                pp_warn(&format!(
                    "[mineru] overlap (ambiguous caption): [{},{}] page={} ratio={:.2} '{}' vs '{}'",
                    i, j, a.page_idx + 1, ratio, snippet_a, snippet_b
                ));
            }
        }
    }

    // 2. Layout-level completeness check (requires LayoutDoc)
    if let Some(doc) = doc {
        // 2a. Collect caption numbers from layout
        let mut layout_figs: std::collections::HashSet<u32> = std::collections::HashSet::new();
        let mut layout_tbls: std::collections::HashSet<u32> = std::collections::HashSet::new();
        // Detect figure captions that sit below a code block (code-above-caption
        // layout). These typically describe code examples, not real figures.
        let mut fig_may_be_code: std::collections::HashSet<u32> = std::collections::HashSet::new();
        // Pre-collect large interline_equation bboxes per page.  MinerU
        // sometimes classifies figure images as equations (e.g. diagrams
        // with \framebox in 2304.10557 Figure 4).  Large blocks (h > 50 pt
        // or area > 5000) are likely real figures.
        let interline_bboxes: std::collections::HashMap<i32, Vec<[f32; 4]>> = doc
            .pdf_info
            .iter()
            .map(|p| {
                let bboxes: Vec<[f32; 4]> = p
                    .preproc_blocks
                    .iter()
                    .filter(|b| b.block_type.to_lowercase() == "interline_equation")
                    .filter_map(|b| {
                        if b.bbox.len() >= 4 {
                            let w = (b.bbox[2] - b.bbox[0]).abs();
                            let h = (b.bbox[3] - b.bbox[1]).abs();
                            if h > 50.0 || w * h > 5000.0 {
                                Some([b.bbox[0], b.bbox[1], b.bbox[2], b.bbox[3]])
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                    .collect();
                (p.page_idx, bboxes)
            })
            .collect();
        let mut fig_from_interline: std::collections::HashSet<u32> =
            std::collections::HashSet::new();

        for page in &doc.pdf_info {
            // Pre-collect code block bboxes for spatial checks on this page
            let code_bboxes: Vec<[f32; 4]> = page
                .para_blocks
                .iter()
                .filter(|b| b.block_type.to_lowercase() == "code")
                .filter_map(|b| {
                    (b.bbox.len() >= 4).then_some([b.bbox[0], b.bbox[1], b.bbox[2], b.bbox[3]])
                })
                .collect();

            for block in &page.para_blocks {
                // Skip code blocks — their code_caption sub-blocks often contain
                // "Figure X" labels that describe code listings, not real figures.
                if block.block_type.to_lowercase() == "code" {
                    pp_info(&format!(
                        "[mineru] validate: skipping code block on page {}",
                        page.page_idx + 1
                    ));
                    continue;
                }

                let is_body_block = {
                    let bt = block.block_type.to_lowercase();
                    bt == "image" || bt == "table" || bt == "chart"
                };

                let mut check_caption = |cap_text: &str| {
                    if let Some(ref cap_num) = extract_caption_number(cap_text) {
                        if let Ok(n) = cap_num[2..].parse::<u32>() {
                            // Non-body block with a code block sitting above it
                            // likely describes a code example, not a real figure.
                            if !is_body_block
                                && cap_num.starts_with("F:")
                                && block.bbox.len() >= 4
                                && !code_bboxes.is_empty()
                            {
                                let block_top = block.bbox[1].min(block.bbox[3]);
                                let block_left = block.bbox[0].min(block.bbox[2]);
                                let block_right = block.bbox[0].max(block.bbox[2]);
                                let block_width = (block_right - block_left).max(1.0);

                                let has_code_above = code_bboxes.iter().any(|cb| {
                                    let cb_bottom = cb[1].max(cb[3]);
                                    let cb_left = cb[0].min(cb[2]);
                                    let cb_right = cb[0].max(cb[2]);
                                    let h_overlap = (block_right.min(cb_right)
                                        - block_left.max(cb_left))
                                    .max(0.0);
                                    let overlap_ratio = h_overlap / block_width;
                                    cb_bottom < block_top && overlap_ratio > 0.3
                                });

                                if has_code_above {
                                    pp_info(&format!(
                                        "[mineru] validate: detected code-above-caption layout for Figure {} on page {}, excluding from missing check",
                                        n, page.page_idx + 1
                                    ));
                                    fig_may_be_code.insert(n);
                                }
                            }

                            // Captions from non-body blocks on pages with large
                            // interline_equation blocks nearby: MinerU likely
                            // classified the figure image as an equation (e.g.
                            // 2304.10557 Figure 4).
                            if !is_body_block && cap_num.starts_with("F:") && block.bbox.len() >= 4
                            {
                                let cap_left = block.bbox[0].min(block.bbox[2]);
                                let cap_right = block.bbox[0].max(block.bbox[2]);
                                let cap_top = block.bbox[1].min(block.bbox[3]);
                                let cap_bottom = block.bbox[1].max(block.bbox[3]);
                                if let Some(iboxes) = interline_bboxes.get(&page.page_idx) {
                                    let nearby = iboxes.iter().any(|ib| {
                                        let h_gap =
                                            (cap_left - ib[2]).max(ib[0] - cap_right).max(0.0);
                                        let v_gap =
                                            (cap_top - ib[3]).max(ib[1] - cap_bottom).max(0.0);
                                        h_gap + v_gap < 100.0
                                    });
                                    if nearby {
                                        pp_info(&format!(
                                            "[mineru] validate: Figure {} on page {} near interline_equation, excluding from missing check",
                                            n, page.page_idx + 1
                                        ));
                                        fig_from_interline.insert(n);
                                    }
                                }
                            }

                            if cap_num.starts_with("F:") {
                                pp_info(&format!(
                                    "[mineru] validate: collected Figure {} from {} block on page {}",
                                    n, block.block_type, page.page_idx + 1
                                ));
                                layout_figs.insert(n);
                            } else if cap_num.starts_with("T:") {
                                pp_info(&format!(
                                    "[mineru] validate: collected Table {} from {} block on page {}",
                                    n, block.block_type, page.page_idx + 1
                                ));
                                layout_tbls.insert(n);
                            }
                        }
                    }
                };

                if let Some(ref cap_text) = block.caption() {
                    check_caption(cap_text);
                }
                for (text, _) in block.orphan_captions() {
                    check_caption(&text);
                }
            }
        }

        // 2b. Collect numbers from extracted results
        let mut extracted_figs: std::collections::HashSet<u32> = std::collections::HashSet::new();
        let mut extracted_tbls: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for (desc, _) in &extracted.images {
            if let Some(ref cap_num) = extract_caption_number(desc) {
                if let Ok(n) = cap_num[2..].parse::<u32>() {
                    if cap_num.starts_with("F:") {
                        extracted_figs.insert(n);
                    } else if cap_num.starts_with("T:") {
                        extracted_tbls.insert(n);
                    }
                }
            }
        }

        // 2c. Figure 1~N continuity check
        // Only report gaps for figures the layout actually knows about.
        // If MinerU never detected a figure at all (no caption anywhere),
        // it's an upstream limitation, not an extraction bug.
        let fig_gaps: Vec<u32> = if extracted_figs.is_empty() {
            vec![]
        } else {
            let max_fig = *extracted_figs.iter().max().unwrap();
            let all_gaps: Vec<u32> = (1..=max_fig)
                .filter(|n| !extracted_figs.contains(n))
                .collect();
            let layout_unknown: Vec<u32> = all_gaps
                .iter()
                .copied()
                .filter(|n| !layout_figs.contains(n))
                .collect();
            if !layout_unknown.is_empty() {
                pp_info(&format!(
                    "[mineru] validate: Figure(s) {:?} not in layout, skipping gap report (upstream limitation)",
                    layout_unknown
                ));
            }
            all_gaps
                .into_iter()
                .filter(|n| layout_figs.contains(n))
                .filter(|n| !fig_may_be_code.contains(n) && !fig_from_interline.contains(n))
                .collect()
        };
        if !fig_gaps.is_empty() {
            pp_warn(&format!(
                "[mineru] validate: Figure gaps detected: {:?}",
                fig_gaps
            ));
            errors.push(format!(
                "Figure numbers not continuous: missing {:?}",
                fig_gaps
            ));
        }

        // 2d. Table 1~N continuity check
        let tbl_gaps: Vec<u32> = if extracted_tbls.is_empty() {
            vec![]
        } else {
            let max_tbl = *extracted_tbls.iter().max().unwrap();
            let all_gaps: Vec<u32> = (1..=max_tbl)
                .filter(|n| !extracted_tbls.contains(n))
                .collect();
            let layout_unknown: Vec<u32> = all_gaps
                .iter()
                .copied()
                .filter(|n| !layout_tbls.contains(n))
                .collect();
            if !layout_unknown.is_empty() {
                pp_info(&format!(
                    "[mineru] validate: Table(s) {:?} not in layout, skipping gap report (upstream limitation)",
                    layout_unknown
                ));
            }
            all_gaps
                .into_iter()
                .filter(|n| layout_tbls.contains(n))
                .collect()
        };
        if !tbl_gaps.is_empty() {
            pp_warn(&format!(
                "[mineru] validate: Table gaps detected: {:?}",
                tbl_gaps
            ));
            errors.push(format!(
                "Table numbers not continuous: missing {:?}",
                tbl_gaps
            ));
        }

        // 2e. Captions present in layout but missing from extracted results
        // Filter out figures that likely describe code examples (code block
        // sitting above the caption block).
        let missing_figs: Vec<u32> = layout_figs
            .difference(&extracted_figs)
            .copied()
            .filter(|n| !fig_may_be_code.contains(n) && !fig_from_interline.contains(n))
            .collect();
        let missing_tbls: Vec<u32> = layout_tbls.difference(&extracted_tbls).copied().collect();
        if !missing_figs.is_empty() {
            pp_warn(&format!(
                "[mineru] validate: layout had Figure(s) {:?} but not in extracted results",
                missing_figs
            ));
            errors.push(format!(
                "layout had figure {:?} but not found in extracted results",
                missing_figs
            ));
        }
        if !missing_tbls.is_empty() {
            pp_warn(&format!(
                "[mineru] validate: layout had Table(s) {:?} but not in extracted results",
                missing_tbls
            ));
            errors.push(format!(
                "layout had table {:?} but not found in extracted results",
                missing_tbls
            ));
        }

        // 2f. body_bbox covers text block detection
        // Principle: use original block bbox from layout as "figure area ground truth".
        // If a text/list/title/interline_equation block is contained within an extracted
        // figure body_bbox, but itself lies inside an original image/chart/table block's bbox,
        // it is figure-internal text (MinerU misclassification), not a bug.
        // Only text blocks outside all original figure blocks being swallowed is a real bug.
        let mut text_blocks: Vec<(String, [f32; 4], i32, String)> = Vec::new();
        let mut figure_zones: Vec<([f32; 4], i32)> = Vec::new();
        for page in &doc.pdf_info {
            for block in &page.para_blocks {
                let bt = block.block_type.to_lowercase();
                if block.bbox.len() >= 4 {
                    let bb = [block.bbox[0], block.bbox[1], block.bbox[2], block.bbox[3]];
                    if bt == "image" || bt == "chart" || bt == "table" {
                        figure_zones.push((bb, page.page_idx));
                    }
                }
                if bt != "text" && bt != "list" && bt != "title" && bt != "interline_equation" {
                    continue;
                }
                if block.bbox.len() < 4 {
                    continue;
                }
                let block_text: String = block
                    .blocks
                    .iter()
                    .flat_map(|sub| sub.lines.iter())
                    .flat_map(|line| line.spans.iter())
                    .filter_map(|span| span.content.as_ref())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ");
                let block_text_direct: String = block
                    .lines
                    .iter()
                    .flat_map(|line| line.spans.iter())
                    .filter_map(|span| span.content.as_ref())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ");
                let full_text = if block_text.is_empty() {
                    block_text_direct
                } else {
                    block_text
                };
                // Exclude blocks that look like captions
                if extract_caption_number(&full_text).is_some()
                    || looks_like_caption(&full_text)
                    || has_sub_panel_label(&full_text)
                {
                    continue;
                }
                let bb = [block.bbox[0], block.bbox[1], block.bbox[2], block.bbox[3]];
                text_blocks.push((
                    bt,
                    bb,
                    page.page_idx,
                    desc_snippet(&full_text, 40).to_string(),
                ));
            }
        }

        // Helper: whether a text block lies inside any original figure zone
        let in_any_figure_zone = |tb: &[f32; 4], page: i32| -> bool {
            for (zone, zpage) in &figure_zones {
                if *zpage != page {
                    continue;
                }
                // Require significant intersection (not just edge contact)
                let zx1 = zone[0].min(zone[2]);
                let zx2 = zone[0].max(zone[2]);
                let zy1 = zone[1].min(zone[3]);
                let zy2 = zone[1].max(zone[3]);
                let tx1 = tb[0].min(tb[2]);
                let tx2 = tb[0].max(tb[2]);
                let ty1 = tb[1].min(tb[3]);
                let ty2 = tb[1].max(tb[3]);
                let ix1 = zx1.max(tx1);
                let iy1 = zy1.max(ty1);
                let ix2 = zx2.min(tx2);
                let iy2 = zy2.min(ty2);
                if ix2 <= ix1 || iy2 <= iy1 {
                    continue;
                }
                let inter = (ix2 - ix1) * (iy2 - iy1);
                let text_area = (tx2 - tx1).max(0.0) * (ty2 - ty1).max(0.0);
                if text_area > 0.0 && inter / text_area >= 0.5 {
                    return true;
                }
            }
            false
        };

        for (i, (_, _)) in extracted.images.iter().enumerate() {
            let fig_body = &extracted.body_bboxes[i];
            let fig_type = extracted.image_bboxes[i].content_type.to_lowercase();
            // interline_equation type extractions come from equation blocks;
            // covering their own source block is not a bug.
            if fig_type == "interline_equation" {
                continue;
            }
            let fx1 = fig_body.bbox[0].min(fig_body.bbox[2]);
            let fx2 = fig_body.bbox[0].max(fig_body.bbox[2]);
            let fy1 = fig_body.bbox[1].min(fig_body.bbox[3]);
            let fy2 = fig_body.bbox[1].max(fig_body.bbox[3]);

            for (tb_type, tb, t_page, t_snippet) in &text_blocks {
                if *t_page != fig_body.page_idx {
                    continue;
                }
                // If this text block lies inside an original figure zone,
                // it is figure-internal text; skip.
                if in_any_figure_zone(tb, *t_page) {
                    continue;
                }
                let tx1 = tb[0].min(tb[2]);
                let tx2 = tb[0].max(tb[2]);
                let ty1 = tb[1].min(tb[3]);
                let ty2 = tb[1].max(tb[3]);
                let ix1 = fx1.max(tx1);
                let iy1 = fy1.max(ty1);
                let ix2 = fx2.min(tx2);
                let iy2 = fy2.min(ty2);
                if ix2 <= ix1 || iy2 <= iy1 {
                    continue;
                }
                let inter = (ix2 - ix1) * (iy2 - iy1);
                let text_area = (tx2 - tx1).max(0.0) * (ty2 - ty1).max(0.0);
                if text_area <= 0.0 {
                    continue;
                }
                let ratio = inter / text_area;

                let is_equation = tb_type == "interline_equation";
                if bbox_contains(fig_body.bbox, *tb) {
                    if is_equation {
                        // Equation swallowed by a non-equation figure is a clear bug
                        pp_warn(&format!(
                            "[mineru] validate: [{}] page={} body_bbox fully contains equation '{}'",
                            i, fig_body.page_idx + 1, t_snippet
                        ));
                        errors.push(format!(
                            "[{}] page={} body_bbox fully covers equation '{}'",
                            i,
                            fig_body.page_idx + 1,
                            t_snippet
                        ));
                    } else {
                        // text / list / title being covered may be figure-internal text
                        // misclassified by MinerU, or a side-effect of composite figure bbox.
                        // Warning only, not errors, to avoid false positives blocking the flow.
                        pp_warn(&format!(
                            "[mineru] validate: [{}] page={} body_bbox fully contains text block '{}' (area={:.0}, type={})",
                            i, fig_body.page_idx + 1, t_snippet, text_area, tb_type
                        ));
                    }
                } else if is_equation && ratio >= 0.30 {
                    pp_warn(&format!(
                        "[mineru] validate: [{}] page={} body_bbox overlaps equation '{}' by {:.1}%",
                        i, fig_body.page_idx + 1, t_snippet, ratio * 100.0
                    ));
                    errors.push(format!(
                        "[{}] page={} body_bbox overlaps equation '{}' ({:.1}%)",
                        i,
                        fig_body.page_idx + 1,
                        t_snippet,
                        ratio * 100.0
                    ));
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "[{}] figure extraction validation failed ({} issues):\n{}",
            pid,
            errors.len(),
            errors.join("\n")
        ))
    }
}
