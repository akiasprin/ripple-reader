// SPDX-License-Identifier: MIT OR Apache-2.0

//! Caption detection, text processing, and geometry utilities.
use regex::Regex;
use sha1::{Digest, Sha1};

use crate::mineru::logging::pp_info;
use crate::mineru::types::LayoutLine;

const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

static NUM_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
static RE_CAPTION: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
static RE_NUM: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
static RE_PANEL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
static RE_SUB_PANEL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();

pub fn encode_base36_2(v: u16) -> String {
    let c1 = ALPHABET[(v / 36) as usize] as char;
    let c2 = ALPHABET[(v % 36) as usize] as char;
    format!("{}{}", c1, c2)
}

/// Generate a structured 6-char name: paper_id prefix (2) + index (2) + content hash (2).
pub(crate) fn structured_image_name(paper_id_prefix: &str, idx: usize, bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let hash_byte = digest[0] as u16;
    format!(
        "{}{:02}{}",
        paper_id_prefix,
        idx,
        encode_base36_2(hash_byte)
    )
}

pub(crate) fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut dp = vec![vec![0; b.len() + 1]; a.len() + 1];
    #[allow(clippy::needless_range_loop)]
    for i in 0..=a.len() {
        dp[i][0] = i;
    }
    #[allow(clippy::needless_range_loop)]
    for j in 0..=b.len() {
        dp[0][j] = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
        }
    }
    dp[a.len()][b.len()]
}

/// Heuristic: does the text look like a real figure/table caption?
/// Uses Levenshtein distance to tolerate single-character OCR errors
/// (e.g. "cigure" -> "figure", "tigure" -> "figure").
/// Fuzzy matches require a number after the first word (e.g. "cigure 5:")
/// to avoid matching unrelated words like "future".
pub(crate) fn looks_like_caption(text: &str) -> bool {
    let re_panel =
        RE_PANEL.get_or_init(|| regex::Regex::new(r"^\([a-zA-Z0-9]+\)(?:\s+|$)").unwrap());
    if re_panel.is_match(text) {
        return true;
    }

    // Normalize OCR artifacts: MinerU sometimes inserts '_' where a space
    // should be (e.g. "Fig._9" -> "Fig. 9").
    let normalized = text.replace('_', " ");
    let trimmed = normalized.trim_start();
    let first_end = trimmed
        .find(|c: char| c.is_whitespace() || c == ':')
        .unwrap_or(trimmed.len());
    // Strip trailing punctuation / digits so "Fig.1" and "table2" become
    // "Fig" and "table" respectively.
    let first = trimmed[..first_end]
        .trim_end_matches(|c: char| c == '.' || c == ',' || c.is_ascii_digit())
        .to_lowercase();

    let targets = [("figure", 1), ("fig", 1), ("table", 1), ("tbl", 1)];
    for (target, threshold) in targets {
        if first == target {
            return true;
        }
        let dist = levenshtein_distance(&first, target);
        if dist > 0 && dist <= threshold {
            // Fuzzy match must be followed by whitespace/colon and a digit
            // (e.g. "cigure 5:" or "tigure 3").
            let after = &trimmed[first_end..];
            let after_num = after.trim_start_matches(|c: char| c.is_whitespace() || c == ':');
            if after_num
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
            {
                return true;
            }
        }
    }
    false
}

/// Stricter than `looks_like_caption`: returns true only when `text` *starts*
/// a caption header.  Used to distinguish caption lines embedded inside a
/// text/paragraph block from body sentences that merely mention a figure
/// (e.g. "Table 2 evaluates whether ..." should NOT match).
///
/// Required form: `<word> <number><suffix>` where
/// - `<word>` is `figure` / `fig` / `table` / `tbl` (exact or Levenshtein ≤ threshold)
/// - `<number>` is decimal digits or roman numerals
/// - `<suffix>` is one of:
///   - `:` immediately after the number
///   - `.` immediately after the number
///   - whitespace + an ASCII uppercase letter
pub fn looks_like_caption_header(text: &str) -> bool {
    let trimmed = text.trim_start();
    let first_end = trimmed
        .find(|c: char| c.is_whitespace() || c == ':' || c == '.')
        .unwrap_or(trimmed.len());
    let first = trimmed[..first_end].to_lowercase();

    let targets = [("figure", 1), ("fig", 1), ("table", 1), ("tbl", 1)];
    let mut word_match = false;
    for (target, threshold) in targets {
        if first == target {
            word_match = true;
            break;
        }
        let dist = levenshtein_distance(&first, target);
        if dist > 0 && dist <= threshold {
            word_match = true;
            break;
        }
    }
    if !word_match {
        return false;
    }

    // Skip whitespace, an optional trailing `.` after the word (e.g. "Fig. 5"),
    // then any leading `:` or whitespace before the number.
    let after_word = trimmed[first_end..].trim_start();
    let after_word = after_word.trim_start_matches('.').trim_start();
    let after_word = after_word.trim_start_matches(':').trim_start();

    let re = NUM_RE.get_or_init(|| regex::Regex::new(r"(?i)^([0-9]+|[ivxlcdm]+)").unwrap());
    let Some(m) = re.find(after_word) else {
        return false;
    };
    let after_num = &after_word[m.end()..];

    // Post-condition: one of three forms must hold.
    let mut chars = after_num.chars();
    match chars.next() {
        Some(':') | Some('.') => true,
        Some(c) if c.is_whitespace() => after_num
            .trim_start()
            .chars()
            .next()
            .map(|c| c.is_ascii_uppercase())
            .unwrap_or(false),
        _ => false,
    }
}

pub fn line_text(line: &LayoutLine) -> String {
    line.spans
        .iter()
        .filter_map(|s| s.content.as_ref())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Two lines are in the same visual cluster when their vertical gap is small
/// relative to the line height AND they share significant horizontal overlap.
/// Used to group line-level caption fragments scattered inside the same
/// paragraph block (e.g. MinerU mis-merging body and caption lines).
pub(crate) fn lines_in_same_cluster(a: &LayoutLine, b: &LayoutLine) -> bool {
    if a.bbox.len() < 4 || b.bbox.len() < 4 {
        return false;
    }
    let ax0 = a.bbox[0];
    let ay0 = a.bbox[1];
    let ax1 = a.bbox[2];
    let ay1 = a.bbox[3];
    let bx0 = b.bbox[0];
    let by0 = b.bbox[1];
    let bx1 = b.bbox[2];
    let by1 = b.bbox[3];

    let a_h = (ay1 - ay0).abs().max(1.0);
    let b_h = (by1 - by0).abs().max(1.0);
    let line_h = a_h.min(b_h);

    let v_gap = if ay1 < by0 {
        by0 - ay1
    } else if by1 < ay0 {
        ay0 - by1
    } else {
        0.0
    };
    if v_gap > line_h * 1.5 {
        return false;
    }

    let h_overlap = (ax1.min(bx1) - ax0.max(bx0)).max(0.0);
    let a_w = (ax1 - ax0).abs().max(1.0);
    let b_w = (bx1 - bx0).abs().max(1.0);
    let min_w = a_w.min(b_w);
    if h_overlap / min_w < 0.40 {
        return false;
    }
    true
}

/// Scan `lines` for caption header lines (per `looks_like_caption_header`) and
/// group each header with its visually adjacent continuation lines (same
/// column, near-zero vertical gap).  Returns one `(text, bbox)` per group with
/// the bbox computed as the union of the group's line bboxes.
///
/// Used when a text-type para_block accidentally merges caption lines with
/// unrelated body lines (e.g. 2605.08083 page 6 block[6] holds both
/// "Table 2 evaluates whether ..." and "Table 2: Generalization beyond ...").
pub fn group_caption_lines(lines: &[LayoutLine]) -> Vec<(String, [f32; 4])> {
    // Genuine figure/table captions are rarely more than a few lines tall.
    // Body paragraphs that happen to start with "Fig. 10. ..." can span many
    // lines and must not be swallowed as a single caption group.
    const MAX_CAPTION_HEIGHT_PT: f32 = 60.0;
    const MAX_CAPTION_LINES: usize = 4;

    let mut groups: Vec<(String, [f32; 4])> = Vec::new();
    if lines.iter().any(|l| l.bbox.len() < 4) {
        return groups; // need bboxes on every line to cluster meaningfully
    }
    let n = lines.len();
    let mut used = vec![false; n];

    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        let ya = lines[a].bbox[1];
        let yb = lines[b].bbox[1];
        ya.partial_cmp(&yb)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                let xa = lines[a].bbox[0];
                let xb = lines[b].bbox[0];
                xa.partial_cmp(&xb).unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    for &start in &order {
        if used[start] {
            continue;
        }
        let start_text = line_text(&lines[start]);
        if !looks_like_caption_header(&start_text) {
            continue;
        }

        let mut group: Vec<usize> = vec![start];
        used[start] = true;
        let start_top = lines[start].bbox[1].min(lines[start].bbox[3]);
        let start_bottom = lines[start].bbox[1].max(lines[start].bbox[3]);

        let mut changed = true;
        while changed {
            changed = false;
            for &j in &order {
                if used[j] {
                    continue;
                }
                let jt = line_text(&lines[j]);
                if looks_like_caption_header(&jt) {
                    continue; // another header starts a separate group
                }
                // Length/height guard: stop absorbing once the group looks
                // larger than a real caption strip.
                if group.len() >= MAX_CAPTION_LINES {
                    continue;
                }
                let group_bottom = group
                    .iter()
                    .map(|&g| lines[g].bbox[1].max(lines[g].bbox[3]))
                    .fold(start_bottom, f32::max);
                if group_bottom - start_top > MAX_CAPTION_HEIGHT_PT {
                    continue;
                }
                if group
                    .iter()
                    .any(|&g| lines_in_same_cluster(&lines[g], &lines[j]))
                {
                    group.push(j);
                    used[j] = true;
                    changed = true;
                }
            }
        }

        let mut x0 = f32::INFINITY;
        let mut y0 = f32::INFINITY;
        let mut x1 = f32::NEG_INFINITY;
        let mut y1 = f32::NEG_INFINITY;
        for &gi in &group {
            let bb = &lines[gi].bbox;
            x0 = x0.min(bb[0]);
            y0 = y0.min(bb[1]);
            x1 = x1.max(bb[2]);
            y1 = y1.max(bb[3]);
        }

        let mut ordered = group.clone();
        ordered.sort_by(|&a, &b| {
            let ya = lines[a].bbox[1];
            let yb = lines[b].bbox[1];
            ya.partial_cmp(&yb)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let xa = lines[a].bbox[0];
                    let xb = lines[b].bbox[0];
                    xa.partial_cmp(&xb).unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        let joined = ordered
            .iter()
            .map(|&i| line_text(&lines[i]))
            .collect::<Vec<_>>()
            .join(" ");
        groups.push((joined, [x0, y0, x1, y1]));
    }
    groups
}

pub(crate) fn has_sub_panel_label(text: &str) -> bool {
    let re = RE_SUB_PANEL.get_or_init(|| regex::Regex::new(r"^\([a-zA-Z0-9]+\)\s*").unwrap());
    re.is_match(text)
}

pub fn union_bbox(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

/// Returns the intersection area of two bboxes divided by the area of `inner`.
/// Used to detect near-engulfment: when an expanded bbox covers >50% of a
/// neighbour's body, the expansion is unsafe even if the neighbour is not
/// *fully* contained (e.g. off by 2 pt on one edge).
pub fn bbox_overlap_ratio(outer: [f32; 4], inner: [f32; 4]) -> f32 {
    let o_left = outer[0].min(outer[2]);
    let o_right = outer[0].max(outer[2]);
    let o_top = outer[1].min(outer[3]);
    let o_bottom = outer[1].max(outer[3]);
    let i_left = inner[0].min(inner[2]);
    let i_right = inner[0].max(inner[2]);
    let i_top = inner[1].min(inner[3]);
    let i_bottom = inner[1].max(inner[3]);

    let inter_w = (o_right.min(i_right) - o_left.max(i_left)).max(0.0);
    let inter_h = (o_bottom.min(i_bottom) - o_top.max(i_top)).max(0.0);
    let inter_area = inter_w * inter_h;

    let i_w = (i_right - i_left).abs();
    let i_h = (i_bottom - i_top).abs();
    let inner_area = i_w * i_h;

    if inner_area > 0.0 {
        inter_area / inner_area
    } else {
        0.0
    }
}

pub fn bbox_contains(outer: [f32; 4], inner: [f32; 4]) -> bool {
    let o_left = outer[0].min(outer[2]);
    let o_right = outer[0].max(outer[2]);
    let o_top = outer[1].min(outer[3]);
    let o_bottom = outer[1].max(outer[3]);
    let i_left = inner[0].min(inner[2]);
    let i_right = inner[0].max(inner[2]);
    let i_top = inner[1].min(inner[3]);
    let i_bottom = inner[1].max(inner[3]);

    o_left <= i_left && o_right >= i_right && o_top <= i_top && o_bottom >= i_bottom
}

pub(crate) fn same_block_type(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let is_fig = |t: &str| t == "image" || t == "chart" || t == "interline_equation";
    is_fig(a) && is_fig(b)
}

pub(crate) fn desc_snippet(desc: &str, max_chars: usize) -> &str {
    &desc[..desc
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(desc.len())]
}

pub(crate) fn is_caption_type_mismatch(caption: &str, block_type: &str) -> bool {
    if caption.starts_with("F:") && block_type == "table" {
        return true;
    }
    if caption.starts_with("T:") && (block_type == "image" || block_type == "chart") {
        return true;
    }
    false
}

/// Extract a normalised figure/table number from a caption string.
/// Returns `Some("F:5")` for "Figure 5 ...", `Some("T:2")` for "Table 2 ...", etc.
/// Sub-figure letters (a), (b) are ignored so that `Figure 5(a)` and `Figure 5(b)`
pub fn extract_caption_number(text: &str) -> Option<String> {
    // Normalize OCR artifacts: MinerU sometimes inserts '_' where a space
    // should be (e.g. "Fig._9" -> "Fig. 9").
    let normalized = text.replace('_', " ");

    // 1. Exact match for normal captions.  The number group accepts:
    //   - Letter-prefixed appendix figures: `B.1`, `C.2`, ... — 2404.19756
    //     appendix uses "Figure B.1 / B.2 / C.1 / D.3" style.
    //   - Chapter-numbered figures: `2.1`, `2.2`, ... — 2404.19756 body uses
    //     this style.  Without the optional decimal capture both styles
    //     collapse onto a single "F:N" key and lose identity for caption
    //     rebind and sub-panel merging.
    //   - Plain integers: `5`, `12`, ...
    //   - Roman numerals: `iv`, `XII`, ...
    // The letter-prefix branch comes first so "Figure B.1" doesn't get
    // partially matched by the roman-numeral branch.
    let re = RE_CAPTION.get_or_init(|| {
        Regex::new(r"(?i)\b(?:fig(?:ure)?|table|tbl)\.?\s*([A-Z]\.[0-9]+(?:\.[0-9]+)?|[0-9]+(?:\.[0-9]+)?|[ivxlcdm]+)\b")
            .unwrap()
    });
    if let Some(cap) = re.captures(&normalized) {
        let full = cap[0].to_lowercase();
        let kind = if full.starts_with("tab") || full.starts_with("tbl") {
            "T"
        } else {
            "F"
        };
        return Some(format!("{}:{}", kind, cap[1].to_ascii_uppercase()));
    }

    // 2. Fallback: tolerate single-character OCR errors (e.g. "cigure" -> "figure").
    let trimmed = normalized.trim_start();
    let first_end = trimmed
        .find(|c: char| c.is_whitespace() || c == ':')
        .unwrap_or(trimmed.len());
    let first = trimmed[..first_end].to_lowercase();
    let targets = [
        ("figure", "F", 1),
        ("fig", "F", 1),
        ("table", "T", 1),
        ("tbl", "T", 1),
    ];
    let re_num = RE_NUM.get_or_init(|| {
        Regex::new(r"(?i)^([A-Z]\.[0-9]+(?:\.[0-9]+)?|[0-9]+(?:\.[0-9]+)?|[ivxlcdm]+)\b").unwrap()
    });
    for (target, kind, threshold) in targets {
        let dist = levenshtein_distance(&first, target);
        if dist > 0 && dist <= threshold {
            let after = &trimmed[first_end..];
            let after_num = after.trim_start_matches(|c: char| c.is_whitespace() || c == ':');
            if let Some(cap) = re_num.captures(after_num) {
                let result = format!("{}:{}", kind, cap[1].to_ascii_uppercase());
                pp_info(&format!(
                    "[mineru] extract_caption_number fallback: '{}' ~{} dist={} -> {}",
                    text.chars().take(50).collect::<String>(),
                    target,
                    dist,
                    result
                ));
                return Some(result);
            }
        }
    }
    None
}
