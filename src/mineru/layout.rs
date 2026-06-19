// SPDX-License-Identifier: MIT OR Apache-2.0

//! Layout block processing: caption detection, orphan identification.
use crate::mineru::logging::pp_info;
use crate::mineru::types::{CaptionNode, LayoutPage, LayoutParaBlock, LayoutSubBlock};
use crate::mineru::utils::{
    bbox_overlap_ratio, extract_caption_number, group_caption_lines, looks_like_caption,
};

impl LayoutParaBlock {
    /// Detect MinerU mis-labeling a figure as a table.
    ///
    /// Some papers (e.g. 2406.16860 page 18) place a 2x2 image grid below a
    /// real table.  MinerU groups the grid plus its "Figure N" caption into a
    /// single `table` para_block, with the real table's caption also nested
    /// above the grid.  When a `table` block carries a figure caption below
    /// its body, it should be treated as an image so the figure caption is
    /// selected and the grid is not swallowed by the table above.
    pub fn should_reclassify_table_as_image(&self) -> bool {
        if self.block_type != "table" {
            return false;
        }
        let min_body_top = self
            .all_subs()
            .iter()
            .filter(|b| b.is_body())
            .map(|b| b.bbox.get(1).copied().unwrap_or(f32::MAX))
            .fold(f32::MAX, f32::min);
        let max_body_bottom = self
            .all_subs()
            .iter()
            .filter(|b| b.is_body())
            .map(|b| b.bbox.get(3).copied().unwrap_or(0.0))
            .fold(0.0f32, f32::max);
        if min_body_top == f32::MAX || max_body_bottom == 0.0 {
            return false;
        }

        // Must contain a table caption above the body (the real table's caption)
        // AND a figure caption below the body (the mis-labeled figure's caption).
        let has_table_caption_above = self.all_subs().iter().any(|sub| {
            if sub.is_body() {
                return false;
            }
            let text: String = sub
                .lines
                .iter()
                .flat_map(|l| &l.spans)
                .filter_map(|s| s.content.as_ref())
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
            if text.is_empty() || !looks_like_caption(&text) {
                return false;
            }
            let Some(cn) = extract_caption_number(&text) else {
                return false;
            };
            if !cn.starts_with("T:") {
                return false;
            }
            let sub_bottom = sub.bbox.get(3).copied().unwrap_or(f32::MAX);
            sub_bottom <= min_body_top + 2.0
        });
        let has_figure_caption_below = self.all_subs().iter().any(|sub| {
            if sub.is_body() {
                return false;
            }
            let text: String = sub
                .lines
                .iter()
                .flat_map(|l| &l.spans)
                .filter_map(|s| s.content.as_ref())
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
            if text.is_empty() || !looks_like_caption(&text) {
                return false;
            }
            let Some(cn) = extract_caption_number(&text) else {
                return false;
            };
            if !cn.starts_with("F:") {
                return false;
            }
            let sub_top = sub.bbox.get(1).copied().unwrap_or(0.0);
            sub_top >= max_body_bottom - 2.0
        });

        has_table_caption_above && has_figure_caption_below
    }

    /// Return all sub-blocks, falling back to a synthetic sub-block built from
    /// the para-block's own `lines` when `blocks` is empty.  Some MinerU
    /// outputs (e.g. 2604.13627) put `lines` directly on the para_block
    /// instead of inside `blocks` sub-blocks.
    pub(crate) fn all_subs(&self) -> Vec<LayoutSubBlock> {
        if !self.blocks.is_empty() {
            self.blocks.clone()
        } else if !self.lines.is_empty() {
            vec![LayoutSubBlock {
                sub_type: String::new(),
                bbox: self.bbox.clone(),
                lines: self.lines.clone(),
                image_path: None,
            }]
        } else {
            vec![]
        }
    }

    pub(crate) fn image_path(&self) -> Option<String> {
        // When sub_type is reliable (image_body/table_body/chart_body), prefer
        // those sub-blocks first.  When sub_type is N/A (common in newer MinerU
        // outputs), fall back to any sub-block that carries an image_path.
        for block in self.all_subs() {
            if block.sub_type == "image_body"
                || block.sub_type == "table_body"
                || block.sub_type == "chart_body"
            {
                for line in &block.lines {
                    for span in &line.spans {
                        if let Some(ref path) = span.image_path {
                            return Some(path.clone());
                        }
                    }
                }
                if let Some(ref path) = block.image_path {
                    return Some(path.clone());
                }
            }
        }
        for block in self.all_subs() {
            if !block.has_image_path() {
                continue;
            }
            for line in &block.lines {
                for span in &line.spans {
                    if let Some(ref path) = span.image_path {
                        return Some(path.clone());
                    }
                }
            }
            if let Some(ref path) = block.image_path {
                return Some(path.clone());
            }
        }
        None
    }

    /// Pick the caption sub-block(s) that semantically belong to this image/
    /// table/chart block.  Applies the same spatial filter as `caption_nodes()`,
    /// but returns the chosen sub-blocks so callers can derive both text and
    /// bbox from them without duplicating the selection logic.
    pub(crate) fn selected_captions(&self) -> Vec<LayoutSubBlock> {
        // Find the minimum top edge of all body sub-blocks (those carrying
        // an image_path).  A caption whose bottom edge is above this line is
        // spatially misplaced.
        let min_image_top = self
            .all_subs()
            .iter()
            .filter(|b| b.is_body())
            .map(|b| b.bbox.get(1).copied().unwrap_or(f32::MAX))
            .fold(f32::MAX, f32::min);
        let has_image_body = min_image_top != f32::MAX;
        let block_type_lower = self.block_type.to_lowercase();
        let is_image_block = block_type_lower == "image" || block_type_lower == "chart";

        // First pass: collect all caption candidates.
        let mut candidates: Vec<LayoutSubBlock> = Vec::new();
        for sub in self.all_subs() {
            if sub.is_body() {
                continue;
            }
            let text: String = sub
                .lines
                .iter()
                .flat_map(|l| &l.spans)
                .filter_map(|s| s.content.as_ref())
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
            if text.is_empty() || !looks_like_caption(&text) {
                continue;
            }
            // Spatial filter for image/chart blocks: caption must not sit
            // entirely above every body.
            if has_image_body && is_image_block {
                let cap_bottom = sub.bbox.get(3).copied().unwrap_or(0.0);
                if cap_bottom < min_image_top {
                    pp_info(&format!(
                        "[mineru] Spatial filter: caption above image (cap_bottom={:.1} < img_top={:.1}), skipping: {}",
                        cap_bottom, min_image_top, text.chars().take(60).collect::<String>()
                    ));
                    continue;
                }
            }
            candidates.push(sub);
        }

        // When a block has 2+ type-matched captions split above AND below the
        // body (e.g. 1512.03385 page 8: "Table 7" caption above Block [9]'s
        // body and "Table 8" caption below), we cannot tell from this block
        // alone which caption is the "true" one — neither candidate dominates
        // spatially.  Return empty so the block becomes a placeholder and the
        // rebind logic can assign each caption to the body it spatially
        // belongs to (see `orphan_captions` for the symmetric emission).
        let is_table_block = block_type_lower == "table";
        if has_image_body && (is_table_block || is_image_block) && candidates.len() > 1 {
            let max_body_bottom = self
                .all_subs()
                .iter()
                .filter(|b| b.is_body())
                .map(|b| b.bbox.get(3).copied().unwrap_or(0.0))
                .fold(0.0f32, f32::max);
            let has_above = candidates
                .iter()
                .any(|sub| sub.bbox.get(3).copied().unwrap_or(0.0) <= min_image_top);
            let has_below = candidates
                .iter()
                .any(|sub| sub.bbox.get(1).copied().unwrap_or(0.0) >= max_body_bottom);
            if has_above && has_below {
                // When all captions type-match and are split above+b below the
                // body, keep only the BELOW caption.  MinerU sometimes nests
                // the preceding block's caption above the current body (e.g.
                // 2112.10752 page 25: Table 14's caption appears above Table
                // 15's body in Table 15's para_block).  Keeping the below
                // caption lets this block keep its native caption while the
                // orphaned above caption is routed back to its own body by the
                // rebind logic.
                let below: Vec<_> = candidates
                    .iter()
                    .filter(|sub| sub.bbox.get(1).copied().unwrap_or(0.0) >= max_body_bottom)
                    .cloned()
                    .collect();
                if !below.is_empty() {
                    pp_info(&format!(
                        "[mineru] selected_captions: captions above AND below body — keeping {} below caption(s), dropping above so they become orphans (body_top={:.1} body_bottom={:.1})",
                        below.len(),
                        min_image_top, max_body_bottom
                    ));
                    return below;
                }
                pp_info(&format!(
                    "[mineru] selected_captions: block has captions above AND below body — returning empty so all become orphans for spatial rebind (body_top={:.1} body_bottom={:.1})",
                    min_image_top, max_body_bottom
                ));
                return Vec::new();
            }
        }

        // When MinerU mis-nests a foreign caption (e.g. "Figure 5" inside a
        // table block that actually contains Table 2), filter out the
        // type-mismatched caption so it becomes an orphan and can be rebound
        // to the correct block.  Only do this when there is a MIX of matched
        // and mismatched captions — if every caption type-matches (e.g. both
        // "Table 7" and "Table 8" inside one table block) the above
        // above-AND-below check has already cleared the candidates.
        let (type_matched, type_mismatched): (Vec<_>, Vec<_>) =
            candidates.iter().cloned().partition(|sub| {
                let text: String = sub
                    .lines
                    .iter()
                    .flat_map(|l| &l.spans)
                    .filter_map(|s| s.content.as_ref())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ");
                if let Some(ref cn) = extract_caption_number(&text) {
                    match block_type_lower.as_str() {
                        "table" => cn.starts_with("T:"),
                        "image" | "chart" => cn.starts_with("F:"),
                        _ => true,
                    }
                } else {
                    true
                }
            });

        if !type_matched.is_empty() && !type_mismatched.is_empty() {
            type_matched
        } else if type_matched.is_empty() && !type_mismatched.is_empty() {
            // All captions in this block are type-mismatched (e.g. 2106.09685
            // page 26 Block 5: a table body whose only nested caption is
            // "Figure 7: ...", which MinerU mis-attached from the adjacent
            // figure).  Returning empty here demotes the block to a
            // placeholder; `orphan_captions` then emits the mismatched
            // caption(s) so `rebind_orphan_captions` can route them to the
            // body they actually belong to, and the body's bbox is no longer
            // visually contaminated with foreign caption text.
            pp_info(&format!(
                "[mineru] selected_captions: all {} caption(s) are type-mismatched for {} block — returning empty so block becomes a placeholder",
                type_mismatched.len(), block_type_lower
            ));
            Vec::new()
        } else {
            candidates
        }
    }

    pub(crate) fn caption_nodes(&self) -> Vec<CaptionNode> {
        let mut nodes = Vec::new();
        for sub in self.selected_captions() {
            for line in &sub.lines {
                for span in &line.spans {
                    if let Some(ref content) = span.content {
                        nodes.push(CaptionNode {
                            node_type: "text".to_string(),
                            content: content.clone(),
                        });
                    }
                }
            }
        }
        nodes
    }

    /// Returns the union bbox of the selected caption sub-block(s).  Used
    /// to expand a figure/table's screenshot region so the rendered hires
    /// image visually includes its caption strip in addition to the body.
    #[allow(dead_code)]
    pub fn caption_bbox(&self) -> Option<[f32; 4]> {
        let subs = self.selected_captions();
        if subs.is_empty() {
            return None;
        }
        let mut acc: Option<[f32; 4]> = None;
        for sub in subs {
            if sub.bbox.len() < 4 {
                continue;
            }
            let bb = [sub.bbox[0], sub.bbox[1], sub.bbox[2], sub.bbox[3]];
            acc = Some(match acc {
                None => bb,
                Some(prev) => [
                    prev[0].min(bb[0]),
                    prev[1].min(bb[1]),
                    prev[2].max(bb[2]),
                    prev[3].max(bb[3]),
                ],
            });
        }
        acc
    }

    /// Union of every sub-block's bbox (image_body + image_caption +
    /// image_footnote + table_*) that semantically belongs to this block.
    /// MinerU's outer `block.bbox` covers only the body envelope and
    /// `caption_bbox()` only returns the SELECTED caption (the one closest
    /// to body centre).  Panel-label sub-captions like "(a) Encoder + LLM"
    /// sitting just above each sub-panel of a composite figure are valid
    /// `image_caption` children but get dropped by `selected_captions()`
    /// whenever a wider main caption (e.g. "Figure 7 ...") wins the
    /// spatial tiebreak.  Including the full union keeps those panel
    /// labels inside the cropped figure region.
    ///
    /// Caveat: MinerU sometimes mis-nests a foreign caption (e.g. "Table
    /// 7" caption ending up inside Table 8's para_block when both
    /// captions sit between the two table bodies on the same page).
    /// Compute a tighter bbox for a sub-block, capping outlier lines
    /// that end with a standalone digit (page numbers mis-merged by
    /// MinerU into caption text, e.g. 2502.03860 page 7 Figure 5
    /// caption line "...cell- 7").
    fn tight_sub_bbox(sub: &LayoutSubBlock) -> [f32; 4] {
        let mut bb = [sub.bbox[0], sub.bbox[1], sub.bbox[2], sub.bbox[3]];
        if !sub.sub_type.ends_with("_caption") || sub.lines.len() < 2 {
            return bb;
        }

        let mut x_maxes = Vec::new();
        for line in &sub.lines {
            if line.bbox.len() < 4 {
                continue;
            }
            let x_max = line.bbox[0].max(line.bbox[2]);
            x_maxes.push(x_max);
        }
        if x_maxes.len() < 2 {
            return bb;
        }
        x_maxes.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median_x_max = x_maxes[x_maxes.len() / 2];
        let max_x_max = *x_maxes.last().unwrap();

        if max_x_max <= median_x_max + 8.0 {
            return bb;
        }

        // Find the widest line and check if it ends with a standalone
        // digit (page number).
        for line in &sub.lines {
            if line.bbox.len() < 4 {
                continue;
            }
            let line_x_max = line.bbox[0].max(line.bbox[2]);
            if line_x_max < max_x_max - 0.5 {
                continue;
            }
            let text: String = line
                .spans
                .iter()
                .filter_map(|s| s.content.as_ref())
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
            let trimmed = text.trim();
            if let Some(last_word) = trimmed.split_whitespace().last() {
                if last_word.len() <= 2 && last_word.chars().all(|c| c.is_ascii_digit()) {
                    bb[2] = bb[2].min(median_x_max + 2.0);
                    break;
                }
            }
        }
        bb
    }

    /// To avoid bleeding the previous figure's caption into this crop,
    /// any caption whose own number (T:N / F:N) differs from this
    /// block's selected-caption number is excluded.  Captions with no
    /// number (panel labels like "(a) Encoder + LLM") are always
    /// included.
    pub(crate) fn full_sub_bbox(&self) -> Option<[f32; 4]> {
        // Determine this block's "primary" caption number from the
        // already-validated selected caption(s).  None means either no
        // caption survived selection, or the caption text has no
        // recognisable Figure/Table number.
        let selected = self.selected_captions();
        let selected_is_empty = selected.is_empty();
        let primary_cn: Option<String> = selected
            .iter()
            .filter_map(|sub| {
                let text: String = sub
                    .lines
                    .iter()
                    .flat_map(|l| &l.spans)
                    .filter_map(|s| s.content.as_ref())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ");
                extract_caption_number(&text)
            })
            .next();

        // Spatial filter: for image/chart blocks, exclude caption sub-blocks
        // that sit above every body (orphan captions mis-nested by MinerU).
        let block_type_lower = self.block_type.to_lowercase();
        let is_image_block = block_type_lower == "image" || block_type_lower == "chart";
        let min_image_top = self
            .all_subs()
            .iter()
            .filter(|b| b.is_body())
            .map(|b| b.bbox.get(1).copied().unwrap_or(f32::MAX))
            .fold(f32::MAX, f32::min);
        let has_image_body = min_image_top != f32::MAX;

        let mut acc: Option<[f32; 4]> = None;
        for sub in self.all_subs() {
            if sub.bbox.len() < 4 {
                continue;
            }
            if is_image_block && has_image_body && !sub.is_body() {
                let sub_bottom = sub.bbox.get(3).copied().unwrap_or(0.0);
                if sub_bottom < min_image_top {
                    continue;
                }
            }
            // Drop sub-blocks whose caption number contradicts ours
            // (foreign caption mis-nested by MinerU).  Sub-blocks with
            // no recognisable number — body envelopes and panel-label
            // captions — are always kept.
            let sub_text: String = sub
                .lines
                .iter()
                .flat_map(|l| &l.spans)
                .filter_map(|s| s.content.as_ref())
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
            let sub_cn = extract_caption_number(&sub_text);
            if let (Some(primary), Some(sub_n)) = (primary_cn.as_ref(), sub_cn.as_ref()) {
                if primary != sub_n {
                    continue;
                }
            }
            // Ambiguous multi-caption case: selected_captions returned empty
            // because the block has captions above AND below body and the
            // rebind logic will reassign them.  Drop every numbered caption
            // from the bbox so the placeholder candidate stays body-only.
            if selected_is_empty && sub_cn.is_some() {
                continue;
            }
            if !sub_text.trim().is_empty() && sub_cn.is_none() && !looks_like_caption(&sub_text) {
                continue;
            }
            let bb = Self::tight_sub_bbox(&sub);
            acc = Some(match acc {
                None => bb,
                Some(prev) => [
                    prev[0].min(bb[0]),
                    prev[1].min(bb[1]),
                    prev[2].max(bb[2]),
                    prev[3].max(bb[3]),
                ],
            });
        }
        acc
    }

    pub(crate) fn caption(&self) -> Option<String> {
        self.caption_impl(false)
    }

    /// Return only the first text content of the caption (e.g. the first span
    /// of an image_caption).  Used for split figures so each sub-panel does not
    /// inherit the full multi-sentence caption.
    pub(crate) fn caption_first(&self) -> Option<String> {
        self.caption_impl(true)
    }

    pub(crate) fn caption_impl(&self, first_only: bool) -> Option<String> {
        let nodes = self.caption_nodes();
        let text: Vec<String> = nodes
            .iter()
            .filter(|n| n.node_type == "text")
            .map(|n| n.content.clone())
            .collect();
        if text.is_empty() {
            None
        } else if first_only {
            text.into_iter().next()
        } else {
            Some(text.join(" "))
        }
    }

    /// Returns true when this block contains a caption-like text whose bottom
    /// edge is above the top edge of every image body **and** the text looks
    /// like a real figure/table caption.
    pub(crate) fn has_caption_above_image(&self) -> bool {
        let min_image_top = self
            .all_subs()
            .iter()
            .filter(|b| b.is_body())
            .map(|b| b.bbox.get(1).copied().unwrap_or(f32::MAX))
            .fold(f32::MAX, f32::min);
        if min_image_top == f32::MAX {
            return false;
        }
        self.all_subs().iter().any(|b| {
            if b.is_body() {
                return false;
            }
            let cap_bottom = b.bbox.get(3).copied().unwrap_or(0.0);
            if cap_bottom >= min_image_top {
                return false;
            }
            let text: String = b
                .lines
                .iter()
                .flat_map(|l| &l.spans)
                .filter_map(|s| s.content.as_ref())
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
            if text.is_empty() {
                return false;
            }
            looks_like_caption(&text)
        })
    }

    /// Returns captions that belong to a different para_block (orphans).
    /// Two cases:
    ///   1. No image_path in this block: captions sitting above every body
    ///      and not horizontally overlapping any body are orphans.
    ///   2. Has image_path: captions whose sub-block bbox sits outside the
    ///      parent block's bbox are orphans (MinerU mis-grouping).
    pub(crate) fn orphan_captions(&self) -> Vec<(String, [f32; 4])> {
        let mut orphans = Vec::new();
        let min_image_top = self
            .all_subs()
            .iter()
            .filter(|b| b.is_body())
            .map(|b| b.bbox.get(1).copied().unwrap_or(f32::MAX))
            .fold(f32::MAX, f32::min);
        let max_image_bottom = self
            .all_subs()
            .iter()
            .filter(|b| b.is_body())
            .map(|b| b.bbox.get(3).copied().unwrap_or(0.0))
            .fold(0.0, f32::max);
        let has_body = min_image_top != f32::MAX;
        for sub in self.all_subs() {
            if sub.is_body() {
                continue;
            }
            let text: String = sub
                .lines
                .iter()
                .flat_map(|l| &l.spans)
                .filter_map(|s| s.content.as_ref())
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
            if text.is_empty() || !looks_like_caption(&text) {
                continue;
            }
            let sub_top = sub.bbox.get(1).copied().unwrap_or(0.0);
            let sub_bottom = sub.bbox.get(3).copied().unwrap_or(f32::MAX);
            let cap_left = sub.bbox.first().copied().unwrap_or(0.0);
            let cap_right = sub.bbox.get(2).copied().unwrap_or(0.0);

            // Case 1: no body in this block — every caption is an orphan.
            if !has_body {
                // Prefer line-level grouping when bboxes are available so we
                // can correctly extract caption lines embedded inside a text
                // block whose outer bbox covers only the body region (e.g.
                // 2605.08083 page 6 block[6]).  Only fall back to the whole
                // sub-block bbox when line-level bboxes are missing.
                let have_line_bboxes =
                    !sub.lines.is_empty() && sub.lines.iter().all(|l| l.bbox.len() >= 4);
                if have_line_bboxes {
                    let groups = group_caption_lines(&sub.lines);
                    for grp in groups {
                        orphans.push(grp);
                    }
                    continue;
                }
                orphans.push((text, [sub.bbox[0], sub.bbox[1], sub.bbox[2], sub.bbox[3]]));
                continue;
            }

            // Case 2: multiple captions in a single-body block.
            // MinerU sometimes groups a caption for an adjacent block here.
            // For image/chart: caption above body is the orphan.
            // For table: caption below body is the orphan (table captions
            // conventionally sit above the table).
            let caption_sub_count = self
                .all_subs()
                .iter()
                .filter(|b| {
                    !b.is_body() && {
                        let t: String = b
                            .lines
                            .iter()
                            .flat_map(|l| &l.spans)
                            .filter_map(|s| s.content.as_ref())
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(" ");
                        !t.is_empty() && looks_like_caption(&t)
                    }
                })
                .count();
            if caption_sub_count > 1 {
                let block_type_lower = self.block_type.to_lowercase();

                // When there is a MIX of type-matched and type-mismatched
                // captions in this block, the mismatched ones are orphans
                // (e.g. "Figure 5" inside a table block that actually
                // contains Table 2).  If every caption type-matches we must
                // fall back to spatial logic (e.g. "Table 7" above + "Table 8"
                // below the same body).
                let mut has_match = false;
                let mut has_mismatch = false;
                for other in self.all_subs() {
                    if other.is_body() {
                        continue;
                    }
                    let ot: String = other
                        .lines
                        .iter()
                        .flat_map(|l| &l.spans)
                        .filter_map(|s| s.content.as_ref())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" ");
                    if ot.is_empty() || !looks_like_caption(&ot) {
                        continue;
                    }
                    if let Some(ref cn) = extract_caption_number(&ot) {
                        let matched = match block_type_lower.as_str() {
                            "table" => cn.starts_with("T:"),
                            "image" | "chart" => cn.starts_with("F:"),
                            _ => true,
                        };
                        if matched {
                            has_match = true;
                        } else {
                            has_mismatch = true;
                        }
                    } else {
                        has_match = true;
                    }
                }
                let is_mixed = has_match && has_mismatch;

                if is_mixed {
                    let selected_cns: std::collections::HashSet<String> = self
                        .selected_captions()
                        .iter()
                        .filter_map(|s| {
                            let t: String = s
                                .lines
                                .iter()
                                .flat_map(|l| &l.spans)
                                .filter_map(|sp| sp.content.as_ref())
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(" ");
                            extract_caption_number(&t)
                        })
                        .collect();
                    let sub_cn = extract_caption_number(&text);
                    if let Some(ref cn) = sub_cn {
                        if selected_cns.contains(cn) {
                            continue; // Selected caption, not an orphan
                        }
                    }
                    // Type-mismatched or no number → orphan
                    orphans.push((text, [sub.bbox[0], sub.bbox[1], sub.bbox[2], sub.bbox[3]]));
                    continue;
                }

                // When all captions type-match but split above AND below
                // body, only emit the ABOVE caption(s) as orphans.
                // `selected_captions` keeps the below caption(s) so the
                // block retains its native caption; the foreign caption
                // above the body (mis-nested by MinerU from the preceding
                // block) is emitted here for spatial rebind.
                let has_above_cap = self.all_subs().iter().any(|b| {
                    if b.is_body() {
                        return false;
                    }
                    let bt: String = b
                        .lines
                        .iter()
                        .flat_map(|l| &l.spans)
                        .filter_map(|s| s.content.as_ref())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" ");
                    if bt.is_empty() || !looks_like_caption(&bt) {
                        return false;
                    }
                    b.bbox.get(3).copied().unwrap_or(0.0) <= min_image_top
                });
                let has_below_cap = self.all_subs().iter().any(|b| {
                    if b.is_body() {
                        return false;
                    }
                    let bt: String = b
                        .lines
                        .iter()
                        .flat_map(|l| &l.spans)
                        .filter_map(|s| s.content.as_ref())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" ");
                    if bt.is_empty() || !looks_like_caption(&bt) {
                        return false;
                    }
                    b.bbox.get(1).copied().unwrap_or(0.0) >= max_image_bottom
                });
                if has_above_cap && has_below_cap {
                    // Only emit the above caption(s) — the below one stays
                    // with this block via selected_captions.
                    if sub_bottom <= min_image_top {
                        orphans.push((text, [sub.bbox[0], sub.bbox[1], sub.bbox[2], sub.bbox[3]]));
                    }
                    continue;
                }

                // Fallback to spatial logic when not mixed.
                let is_table = block_type_lower == "table";
                let suspect = if is_table {
                    sub_top > max_image_bottom
                } else {
                    sub_bottom < min_image_top
                };
                if suspect {
                    orphans.push((text, [sub.bbox[0], sub.bbox[1], sub.bbox[2], sub.bbox[3]]));
                    continue;
                }
            }
            //
            // Additional case: even if the single caption visually overlaps
            // the body, when its caption number (F:N / T:N) is type-mismatched
            // against this block's type, MinerU has mis-attached a foreign
            // caption (e.g. 2106.09685 page 26: a "Figure 7" caption nested
            // inside Table 18's table block).  Emit it as an orphan so the
            // rebind logic can route it to the body it actually belongs to.
            // `selected_captions` has already dropped this caption from the
            // block's `desc`, so the body's bbox must also drop the caption
            // strip — which only happens if the caption becomes an orphan.
            if caption_sub_count == 1 {
                if let Some(ref cn) = extract_caption_number(&text) {
                    let block_type_lower = self.block_type.to_lowercase();
                    let is_table = block_type_lower == "table";
                    let is_image_or_chart = matches!(block_type_lower.as_str(), "image" | "chart");
                    let mismatch = (is_table && cn.starts_with("F:"))
                        || (is_image_or_chart && cn.starts_with("T:"));
                    if mismatch {
                        pp_info(&format!(
                            "[mineru] orphan: single type-mismatched caption '{}' in {} block — emitting as orphan",
                            cn, self.block_type
                        ));
                        orphans.push((text, [sub.bbox[0], sub.bbox[1], sub.bbox[2], sub.bbox[3]]));
                        continue;
                    }
                }
            }
            // Case 3: single caption inside block.  Only orphan it if it sits
            // above every body and does NOT horizontally overlap any body.
            // Exception: for image/chart blocks, captions belong below the
            // body, so one sitting above is always an orphan regardless of
            // horizontal overlap (selected_captions will drop it otherwise).
            let block_type_lower = self.block_type.to_lowercase();
            let is_image_or_chart = block_type_lower == "image" || block_type_lower == "chart";
            if sub_bottom >= min_image_top {
                // Caption sits below the body.  For image/chart this is the
                // normal position, so keep it.  For other block types, if it
                // does NOT horizontally overlap any body sub-block, it likely
                // belongs to a different block on the same page (e.g. MinerU
                // mis-grouping a figure caption inside a code block).  Emit as
                // orphan so rebind can route it to the correct body.
                if !is_image_or_chart {
                    let overlaps_any_body =
                        self.all_subs().iter().filter(|b| b.is_body()).any(|b| {
                            let img_left = b.bbox.first().copied().unwrap_or(0.0);
                            let img_right = b.bbox.get(2).copied().unwrap_or(0.0);
                            let h_overlap =
                                (img_right.min(cap_right) - img_left.max(cap_left)).max(0.0);
                            let img_w = (img_right - img_left).abs().max(1.0);
                            h_overlap / img_w > 0.10
                        });
                    if !overlaps_any_body {
                        pp_info(&format!(
                            "[mineru] orphan: caption below body with no h-overlap in {} block — emitting as orphan",
                            self.block_type
                        ));
                        orphans.push((text, [sub.bbox[0], sub.bbox[1], sub.bbox[2], sub.bbox[3]]));
                    }
                }
                continue;
            }
            if !is_image_or_chart {
                let overlaps_body = self.all_subs().iter().filter(|b| b.is_body()).any(|b| {
                    let img_left = b.bbox.first().copied().unwrap_or(0.0);
                    let img_right = b.bbox.get(2).copied().unwrap_or(0.0);
                    let h_overlap = (img_right.min(cap_right) - img_left.max(cap_left)).max(0.0);
                    let img_w = (img_right - img_left).abs().max(1.0);
                    h_overlap / img_w > 0.30
                });
                if overlaps_body {
                    continue;
                }
            }
            orphans.push((text, [sub.bbox[0], sub.bbox[1], sub.bbox[2], sub.bbox[3]]));
        }
        orphans
    }
}

impl LayoutPage {
    /// Recover the `image_path` for an image/table/chart `para_block` whose
    /// own sub-blocks have been stripped — MinerU sets `lines_deleted: true`
    /// and drops the `image_path` from `para_blocks` for tables it failed to
    /// transcribe inline (typically math-heavy tables, e.g. 2605.14037
    /// Tables 6 & 7 in the scaling-study appendix).  The underlying body
    /// still appears in `preproc_blocks` with `image_path` intact, so we
    /// match by (block_type, near-identical bbox) to recover the crop.
    /// Without this fallback the candidate-collection loop silently skips
    /// the table and downstream `Table N` orphan captions can't rebind.
    pub(crate) fn resolve_image_path(&self, block: &LayoutParaBlock) -> Option<String> {
        if let Some(p) = block.image_path() {
            return Some(p);
        }
        if block.bbox.len() < 4 {
            return None;
        }
        let target = [block.bbox[0], block.bbox[1], block.bbox[2], block.bbox[3]];
        for pre in &self.preproc_blocks {
            if pre.block_type != block.block_type {
                continue;
            }
            if pre.bbox.len() < 4 {
                continue;
            }
            let cand = [pre.bbox[0], pre.bbox[1], pre.bbox[2], pre.bbox[3]];
            if bbox_overlap_ratio(cand, target) >= 0.8 && bbox_overlap_ratio(target, cand) >= 0.8 {
                if let Some(p) = pre.image_path() {
                    return Some(p);
                }
            }
        }
        None
    }
}

/// Extract all text from a `LayoutParaBlock`, including both direct lines
/// and lines inside sub-blocks.
pub fn extract_block_text(block: &LayoutParaBlock) -> String {
    let mut parts = Vec::new();
    for sub in &block.blocks {
        for line in &sub.lines {
            for span in &line.spans {
                if let Some(ref content) = span.content {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        parts.push(trimmed.to_string());
                    }
                }
            }
        }
    }
    for line in &block.lines {
        for span in &line.spans {
            if let Some(ref content) = span.content {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    parts.push(trimmed.to_string());
                }
            }
        }
    }
    parts.join(" ")
}

/// Collect all lines from a `LayoutParaBlock` (both direct lines and sub-block lines).
pub fn collect_block_lines(block: &LayoutParaBlock) -> Vec<&crate::mineru::LayoutLine> {
    let mut lines: Vec<&crate::mineru::LayoutLine> = Vec::new();
    for sub in &block.blocks {
        for line in &sub.lines {
            lines.push(line);
        }
    }
    for line in &block.lines {
        lines.push(line);
    }
    lines
}
