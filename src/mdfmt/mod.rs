// SPDX-License-Identifier: MIT OR Apache-2.0

pub mod parser;

use crate::mdfmt::parser::{has_layout, parse_images, rebuild_image};
use std::path::{Path, PathBuf};

/// Try to locate the actual image file on disk for a given paper.
///
/// Supported URL forms:
/// - `figure/name`   → figures/{id}/hires/{name}.png  or  figures/{id}/{name}.jpg
/// - `table/name`    → same as figure
/// - `page://N`      → figures/{id}/pages/{N}.png
/// - `https://...`   → returns None (external, skip)
pub fn resolve_image_path(paper_id: &str, url: &str) -> Option<PathBuf> {
    let base = Path::new("figures").join(paper_id);

    if let Some(name) = url.strip_prefix("figure/") {
        let hires = base.join("hires").join(format!("{}.png", name));
        if hires.exists() {
            return Some(hires);
        }
        let fallback = base.join(format!("{}.jpg", name));
        if fallback.exists() {
            return Some(fallback);
        }
    } else if let Some(name) = url.strip_prefix("table/") {
        let hires = base.join("hires").join(format!("{}.png", name));
        if hires.exists() {
            return Some(hires);
        }
        let fallback = base.join(format!("{}.jpg", name));
        if fallback.exists() {
            return Some(fallback);
        }
    } else if let Some(page) = url.strip_prefix("page://") {
        let path = base.join("pages").join(format!("{}.png", page));
        if path.exists() {
            return Some(path);
        }
    }

    None
}

/// Get image dimensions (width, height) in pixels.
pub fn image_dimensions(path: &Path) -> Option<(u32, u32)> {
    image::image_dimensions(path).ok()
}

/// Layout decision for an image.
#[derive(Debug, Clone)]
pub struct Layout {
    /// Display width, e.g. "50%" or "480px".
    pub width: String,
    /// Horizontal alignment.
    pub align: &'static str,
}

/// Decide layout (size, align) for an image based on its dimensions.
///
/// Scaling is page-relative: every image is sized as a percentage of
/// `page_width_px` so that text appears at a consistent visual size
/// across all figures.
///
/// Rules:
/// - Very wide images (aspect > 3.0 for figures, > 5.2 for tables):
///   shrinking ruins horizontal readability → None (leave untouched).
/// - All other images: `display_pct = clamp(page_ratio * 120, 50, 95)`,
///   where `page_ratio = image_width / page_width`.  Images at or below
///   50% are right-floated; wider images are centered.
pub fn decide_layout(width: u32, height: u32, is_table: bool, page_width_px: f64) -> Option<Layout> {
    let aspect = width as f64 / height.max(1) as f64;

    // Very wide images: shrinking ruins horizontal readability.
    let aspect_threshold = if is_table { 5.2 } else { 3.0 };
    if aspect > aspect_threshold {
        return None;
    }

    // Scale based on the image's share of the page width.
    // This ensures consistent visual text size across all images
    // because every image is scaled by the same factor relative to the page.
    // The 120× multiplier means an image occupying ~79% of page width
    // already saturates to the 95% cap — intentional for full-width figures.
    let page_ratio = if page_width_px > 0.0 {
        (width as f64 / page_width_px).min(1.0)
    } else {
        // Corrupt or missing page image — fall back to a sensible ratio
        // that produces a mid-range display percentage.
        0.4
    };
    let display_pct = (page_ratio * 120.0).round() as u32;
    let display_pct = display_pct.clamp(50, 95);

    let align = if display_pct <= 50 { "right" } else { "center" };

    Some(Layout {
        width: format!("{}%", display_pct),
        align,
    })
}

/// Result of processing a single image.
#[derive(Debug, Clone)]
pub struct ImageResult {
    pub original: String,
    pub decision: Decision,
}

#[derive(Debug, Clone)]
pub enum Decision {
    /// Already has layout, skipped.
    SkippedLayout,
    /// External URL, skipped.
    SkippedExternal,
    /// File not found, skipped.
    SkippedMissing,
    /// Large image, left untouched.
    KeptLarge { width: u32, height: u32 },
    /// Image resized + aligned based on area/aspect rules.
    Resized {
        width: u32,
        height: u32,
        new_width: String,
        new_align: String,
    },
}

/// Transform markdown: apply smart layout to images.
///
/// When `force` is true, existing layouts are overwritten.
/// Returns (new_markdown, list of per-image results).
pub fn transform(paper_id: &str, markdown: &str, force: bool) -> (String, Vec<ImageResult>) {
    let images = parse_images(markdown);
    let mut results = Vec::with_capacity(images.len());

    // If no images, return early.
    if images.is_empty() {
        return (markdown.to_string(), results);
    }

    // Estimate the page width in pixels.  Use a page:// image as the
    // ground-truth if available; otherwise fall back to the widest image
    // (which is typically close to full-page width in academic papers).
    let page_width_px = images
        .iter()
        .find(|img| img.url.starts_with("page://"))
        .and_then(|img| resolve_image_path(paper_id, &img.url))
        .and_then(|path| image_dimensions(&path))
        .map(|(w, _)| w as f64)
        .unwrap_or_else(|| {
            images
                .iter()
                .filter_map(|img| resolve_image_path(paper_id, &img.url))
                .filter_map(|path| image_dimensions(&path))
                .map(|(w, _)| w as f64)
                .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap_or(3000.0)
        });

    let mut output = String::with_capacity(markdown.len() + images.len() * 20);
    let mut last_end = 0usize;
    let mut prev_float = false;

    for img in images {
        // Copy text before this image reference.
        output.push_str(&markdown[last_end..img.offset]);

        let img_len = img.full.len();

        // When not forcing, skip images that already have layout —
        // do this BEFORE any I/O to avoid wasted filesystem reads.
        if !force && has_layout(&img) {
            output.push_str(&img.full);
            results.push(ImageResult {
                original: img.full,
                decision: Decision::SkippedLayout,
            });
            last_end = img.offset + img_len;
            continue;
        }

        let (decision, replacement) = if img.url.starts_with("http://")
            || img.url.starts_with("https://")
        {
            prev_float = false;
            (Decision::SkippedExternal, img.full.clone())
        } else {
            match resolve_image_path(paper_id, &img.url) {
                Some(path) => match image_dimensions(&path) {
                    Some((width, height)) => {
                        let url_lower = img.url.to_lowercase();
                        let alt_lower = img.alt.to_lowercase();
                        let is_table = img.url.starts_with("table/")
                            || url_lower.contains("/table")
                            || url_lower.contains("_table")
                            || alt_lower.contains("table")
                            || alt_lower.contains("表格");
                        match decide_layout(width, height, is_table, page_width_px) {
                            Some(mut layout) => {
                                // Avoid consecutive right-float images.
                                if layout.align == "right" && prev_float {
                                    layout.align = "center";
                                }
                                prev_float = layout.align == "right";
                                let decision = Decision::Resized {
                                    width,
                                    height,
                                    new_width: layout.width.clone(),
                                    new_align: layout.align.to_string(),
                                };
                                let repl = rebuild_image(&img, Some(&layout.width), Some(layout.align));
                                (decision, repl)
                            }
                            None => {
                                prev_float = false;
                                let decision = Decision::KeptLarge { width, height };
                                (decision, img.full.clone())
                            }
                        }
                    }
                    None => {
                        prev_float = false;
                        (Decision::SkippedMissing, img.full.clone())
                    }
                },
                None => {
                    prev_float = false;
                    (Decision::SkippedMissing, img.full.clone())
                }
            }
        };

        output.push_str(&replacement);
        last_end = img.offset + img_len;

        results.push(ImageResult {
            original: img.full,
            decision,
        });
    }

    // Copy remaining text after last image.
    output.push_str(&markdown[last_end..]);

    (output, results)
}

/// Pretty-print results.
pub fn print_results(results: &[ImageResult]) {
    for r in results {
        match &r.decision {
            Decision::SkippedLayout => {
                println!("  [SKIP] already has layout: {}", r.original)
            }
            Decision::SkippedExternal => {
                println!("  [SKIP] external URL: {}", r.original)
            }
            Decision::SkippedMissing => {
                println!("  [SKIP] file not found: {}", r.original)
            }
            Decision::KeptLarge { width, height } => {
                println!("  [KEEP] large ({}x{}px): {}", width, height, r.original)
            }
            Decision::Resized {
                width,
                height,
                new_width,
                new_align,
            } => {
                println!(
                    "  [RESIZE] ({}x{}px) → {} {}: {}",
                    width, height, new_width, new_align, r.original
                )
            }
        }
    }
}
