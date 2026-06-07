// SPDX-License-Identifier: MIT OR Apache-2.0

//! SVG clipPath parsing for mutool-generated SVGs.

use crate::figure::geometry::{bbox_area, bbox_aspect_ratio};

/// Parse an SVG `matrix(a,b,c,d,e,f)` transform string.
pub fn parse_svg_matrix(transform: &str) -> Option<[f32; 6]> {
    let re = regex::Regex::new(r"matrix\(([^)]+)\)").ok()?;
    let cap = re.captures(transform)?;
    let vals: Vec<f32> = cap[1]
        .split(',')
        .filter_map(|s| s.trim().parse::<f32>().ok())
        .collect();
    if vals.len() == 6 {
        Some([vals[0], vals[1], vals[2], vals[3], vals[4], vals[5]])
    } else {
        None
    }
}

/// Extract all (x, y) coordinates from a simple SVG path `d` attribute.
/// Handles `M x y`, `H x`, `V y`, `L x y` commands.
pub fn parse_path_coords(d: &str) -> Vec<(f32, f32)> {
    let mut coords = Vec::new();
    // Tokenize: commands or numbers (including negatives, decimals, scientific)
    let tokens: Vec<String> = d
        .split_whitespace()
        .flat_map(|s: &str| {
            // Some paths have numbers directly adjacent to commands, e.g. "M100 200"
            // Split on command letters
            let mut parts = Vec::new();
            let mut current = String::new();
            for ch in s.chars() {
                if ch.is_alphabetic() {
                    if !current.is_empty() {
                        parts.push(current.clone());
                        current.clear();
                    }
                    parts.push(ch.to_string());
                } else {
                    current.push(ch);
                }
            }
            if !current.is_empty() {
                parts.push(current);
            }
            parts
        })
        .collect();

    let mut i = 0;
    let mut x = 0.0f32;
    let mut y = 0.0f32;
    let mut last_cmd = 'M';

    while i < tokens.len() {
        let t = &tokens[i];
        if let Some(cmd) = t.chars().next().filter(|c| c.is_alphabetic()) {
            last_cmd = cmd;
            i += 1;
            continue;
        }

        let parse_num = |s: &str| s.trim().parse::<f32>().ok();

        match last_cmd {
            'M' | 'm' | 'L' | 'l' => {
                if i + 1 < tokens.len() {
                    if let (Some(nx), Some(ny)) = (parse_num(t), parse_num(&tokens[i + 1])) {
                        if last_cmd.is_lowercase() {
                            x += nx;
                            y += ny;
                        } else {
                            x = nx;
                            y = ny;
                        }
                        coords.push((x, y));
                        i += 2;
                        // Subsequent coordinates are treated as implicit L
                        if last_cmd == 'M' || last_cmd == 'm' {
                            last_cmd = if last_cmd.is_lowercase() { 'l' } else { 'L' };
                        }
                        continue;
                    }
                }
            }
            'H' => {
                if let Some(nx) = parse_num(t) {
                    x = nx;
                    coords.push((x, y));
                    i += 1;
                    continue;
                }
            }
            'h' => {
                if let Some(dx) = parse_num(t) {
                    x += dx;
                    coords.push((x, y));
                    i += 1;
                    continue;
                }
            }
            'V' => {
                if let Some(ny) = parse_num(t) {
                    y = ny;
                    coords.push((x, y));
                    i += 1;
                    continue;
                }
            }
            'v' => {
                if let Some(dy) = parse_num(t) {
                    y += dy;
                    coords.push((x, y));
                    i += 1;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    coords
}

/// Compute the axis-aligned bbox of a path after applying an SVG transform matrix.
/// The matrix is [a, b, c, d, e, f] where:
///   x' = a*x + c*y + e
///   y' = b*x + d*y + f
pub fn transform_path_bbox(path_bbox: [f32; 4], m: [f32; 6]) -> [f32; 4] {
    let corners = [
        (path_bbox[0], path_bbox[1]),
        (path_bbox[2], path_bbox[1]),
        (path_bbox[0], path_bbox[3]),
        (path_bbox[2], path_bbox[3]),
    ];
    let mut xs = Vec::with_capacity(4);
    let mut ys = Vec::with_capacity(4);
    for (x, y) in corners {
        let tx = m[0] * x + m[2] * y + m[4];
        let ty = m[1] * x + m[3] * y + m[5];
        xs.push(tx);
        ys.push(ty);
    }
    [
        xs.iter().copied().fold(f32::INFINITY, f32::min),
        ys.iter().copied().fold(f32::INFINITY, f32::min),
        xs.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        ys.iter().copied().fold(f32::NEG_INFINITY, f32::max),
    ]
}

/// Extract all clipPath bounding boxes from a mutool-generated SVG.
/// Returns bboxes in the layout.json coordinate system (y=0 at top, downward).
pub fn extract_svg_clip_bboxes(svg: &str) -> Vec<[f32; 4]> {
    let mut bboxes = Vec::new();
    let _clip_count = svg.matches("<clipPath").count(); // Regex to find <clipPath id="..."> ... </clipPath>
    let clip_re =
        regex::Regex::new(r#"(?s)<clipPath\s+[^>]*id="([^"]+)"[^>]*>(.*?)</clipPath>"#).ok();
    let path_re =
        regex::Regex::new(r#"(?s)<path\s+([^>]*transform="([^"]+)"[^>]*)d="([^"]+)""#).ok();

    let clip_re = match clip_re {
        Some(re) => re,
        None => return bboxes,
    };
    let path_re = match path_re {
        Some(re) => re,
        None => return bboxes,
    };

    for cap in clip_re.captures_iter(svg) {
        let clip_content = &cap[2];
        for pcap in path_re.captures_iter(clip_content) {
            let transform = &pcap[2];
            let d = &pcap[3];

            let m = match parse_svg_matrix(transform) {
                Some(m) => m,
                None => continue,
            };

            let coords = parse_path_coords(d);
            if coords.len() < 2 {
                continue;
            }

            let min_x = coords.iter().map(|(x, _)| *x).fold(f32::INFINITY, f32::min);
            let min_y = coords.iter().map(|(_, y)| *y).fold(f32::INFINITY, f32::min);
            let max_x = coords
                .iter()
                .map(|(x, _)| *x)
                .fold(f32::NEG_INFINITY, f32::max);
            let max_y = coords
                .iter()
                .map(|(_, y)| *y)
                .fold(f32::NEG_INFINITY, f32::max);

            let local_bbox = [min_x, min_y, max_x, max_y];
            let bbox = transform_path_bbox(local_bbox, m);

            // Filter tiny clipPaths (decoration, borders, etc.)
            let area = bbox_area(bbox);
            let ratio = bbox_aspect_ratio(bbox);
            if area < 2000.0 || !(1.0 / 12.0..=12.0).contains(&ratio) {
                continue;
            }

            bboxes.push(bbox);
        }
    }

    bboxes
}
