// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unified wrappers for mutool (MuPDF) command-line tools.

use anyhow::{Context, Result};

/// Render a single PDF page to PNG at the specified DPI using `mutool draw`.
pub fn render_page_png(pdf_path: &str, page: usize, dpi: u32) -> Result<Vec<u8>> {
    let out_dir = tempfile::tempdir()?;
    let out_pattern = out_dir.path().join("page-%d.png");

    let output = std::process::Command::new("mutool")
        .args([
            "draw",
            "-o",
            out_pattern.to_str().unwrap(),
            "-r",
            &dpi.to_string(),
            pdf_path,
            &page.to_string(),
        ])
        .output()?;

    if !output.status.success() {
        anyhow::bail!("mutool draw failed at {} DPI", dpi);
    }

    let page_file = out_dir.path().join(format!("page-{}.png", page));
    let data =
        std::fs::read(&page_file).or_else(|_| std::fs::read(out_dir.path().join("page.png")))?;
    Ok(data)
}

/// Run `mutool trace` on a single page and extract all `fill_path` bounding
/// boxes.  Returns bboxes in the layout.json coordinate system (y=0 at top,
/// downward).
pub fn run_mutool_trace_fill_paths(
    pdf_path: &str,
    page: i32,
    page_height: f32,
) -> Result<Vec<[f32; 4]>> {
    let output = std::process::Command::new("mutool")
        .args(["trace", pdf_path, &page.to_string()])
        .output()
        .context("Failed to run mutool trace")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("mutool trace failed: {}", stderr);
    }

    let text = String::from_utf8_lossy(&output.stdout);
    Ok(parse_mutool_trace_fill_paths(&text, page_height))
}

/// Parse `mutool trace` XML output, extracting `fill_path` bounding boxes.
/// Coordinates are converted from PDF space (y=0 at bottom) to layout space
/// (y=0 at top) using the page mediabox height.
pub fn parse_mutool_trace_fill_paths(text: &str, page_height: f32) -> Vec<[f32; 4]> {
    let mut bboxes = Vec::new();

    let mediabox_re = match regex::Regex::new(r#"<page[^>]*mediabox="([\d\s.]+)""#) {
        Ok(re) => re,
        Err(_) => return bboxes,
    };
    let ph = mediabox_re
        .captures(text)
        .and_then(|c| {
            let parts: Vec<f32> = c[1]
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
            parts.get(3).copied()
        })
        .unwrap_or(page_height);

    let fill_path_re =
        match regex::Regex::new(r#"(?s)<fill_path[^>]*transform="([^"]+)"[^>]*>(.*?)</fill_path>"#)
        {
            Ok(re) => re,
            Err(_) => return bboxes,
        };

    let x_re = match regex::Regex::new(r#"x="([^"]+)""#) {
        Ok(re) => re,
        Err(_) => return bboxes,
    };
    let y_re = match regex::Regex::new(r#"y="([^"]+)""#) {
        Ok(re) => re,
        Err(_) => return bboxes,
    };

    for cap in fill_path_re.captures_iter(text) {
        let transform = &cap[1];
        let content = &cap[2];

        let tvals: Vec<f32> = transform
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();
        if tvals.len() != 6 {
            continue;
        }
        let (a, b, c, d, e, f) = (tvals[0], tvals[1], tvals[2], tvals[3], tvals[4], tvals[5]);

        let mut xs = Vec::new();
        let mut ys = Vec::new();

        for line in content.lines() {
            let x_caps: Vec<f32> = x_re
                .captures_iter(line)
                .filter_map(|xc| xc[1].parse().ok())
                .collect();
            let y_caps: Vec<f32> = y_re
                .captures_iter(line)
                .filter_map(|yc| yc[1].parse().ok())
                .collect();

            if x_caps.len() == y_caps.len() && !x_caps.is_empty() {
                for (x, y) in x_caps.iter().zip(y_caps.iter()) {
                    let tx = a * x + c * y + e;
                    let ty = b * x + d * y + f;
                    xs.push(tx);
                    ys.push(ty);
                }
            }
        }

        if xs.is_empty() {
            continue;
        }

        let x0 = xs.iter().copied().fold(f32::INFINITY, f32::min);
        let x1 = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let y0_pdf = ys.iter().copied().fold(f32::INFINITY, f32::min);
        let y1_pdf = ys.iter().copied().fold(f32::NEG_INFINITY, f32::max);

        // Convert PDF coords (y=0 at bottom) to layout coords (y=0 at top)
        let y0_layout = ph - y1_pdf;
        let y1_layout = ph - y0_pdf;

        let w = x1 - x0;
        let h = y1_layout - y0_layout;
        let area = w * h;

        // Keep reasonably large rectangles (backgrounds / decoration bars)
        if area > 3000.0 && w > 100.0 && h > 10.0 {
            bboxes.push([x0, y0_layout, x1, y1_layout]);
        }
    }

    bboxes
}

/// Run mutool draw -F svg for a single page, returning the SVG string.
pub fn run_mutool_svg(pdf_path: &str, page: i32) -> Result<String> {
    let output = std::process::Command::new("mutool")
        .args(["draw", "-F", "svg", "-o", "-", pdf_path, &page.to_string()])
        .output()
        .context("Failed to run mutool draw -F svg")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("mutool draw svg failed: {}", stderr);
    }

    let svg = String::from_utf8_lossy(&output.stdout).to_string();
    if svg.len() < 100 {
        anyhow::bail!("mutool draw svg produced empty output");
    }
    Ok(svg)
}
