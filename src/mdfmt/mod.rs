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

/// Decide layout (size, align) for an image based on its dimensions.
///
/// Rules — width only, aspect-ratio guard:
/// - Huge images (width >= 2500): leave untouched (None)
/// - Very wide images (aspect > 3.0): lots of horizontal detail, shrinking makes them unreadable → None
/// - Tall/skinny images (aspect < 0.6, e.g. single-page screenshots): 80% center
/// - Everything else by width:
///   width < 600      → 40% right
///   600 – 1000       → 50% right
///   1000 – 1500      → 60% right
///   1500 – 2000      → 70% right
///   >= 2000          → leave untouched
pub fn decide_layout(
    width: u32,
    height: u32,
    is_table: bool,
) -> Option<(&'static str, &'static str)> {
    let aspect = width as f64 / height.max(1) as f64;

    // Huge width: leave untouched
    if width >= 2500 {
        return None;
    }

    // Very wide images: shrinking ruins horizontal readability.
    // Tables get a more lenient threshold since they often have simpler content.
    let aspect_threshold = if is_table { 5.2 } else { 3.0 };
    if aspect > aspect_threshold {
        return None;
    }

    // Everything else: 50% right
    Some(("50%", "right"))
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
        new_size: String,
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

    let mut output = String::with_capacity(markdown.len() + images.len() * 20);
    let mut last_end = 0usize;

    for img in images {
        // Copy text before this image reference.
        output.push_str(&markdown[last_end..img.offset]);

        let (decision, replacement) = if img.url.starts_with("http://")
            || img.url.starts_with("https://")
        {
            (Decision::SkippedExternal, img.full.clone())
        } else {
            match resolve_image_path(paper_id, &img.url) {
                Some(path) => match image_dimensions(&path) {
                    Some((width, height)) => {
                        let is_table = img.url.starts_with("table/") || img.alt.contains("Table");
                        match decide_layout(width, height, is_table) {
                            Some((new_size, new_align)) => {
                                let decision = Decision::Resized {
                                    width,
                                    height,
                                    new_size: new_size.to_string(),
                                    new_align: new_align.to_string(),
                                };
                                let repl = rebuild_image(&img, Some(new_size), Some(new_align));
                                (decision, repl)
                            }
                            None => {
                                let decision = Decision::KeptLarge { width, height };
                                (decision, img.full.clone())
                            }
                        }
                    }
                    None => (Decision::SkippedMissing, img.full.clone()),
                },
                None => (Decision::SkippedMissing, img.full.clone()),
            }
        };

        let img_len = img.full.len();

        // When not forcing, skip images that already have layout.
        if !force && has_layout(&img) {
            output.push_str(&img.full);
            results.push(ImageResult {
                original: img.full,
                decision: Decision::SkippedLayout,
            });
            last_end = img.offset + img_len;
            continue;
        }

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
                new_size,
                new_align,
            } => {
                println!(
                    "  [RESIZE] ({}x{}px) → {} {}: {}",
                    width, height, new_size, new_align, r.original
                )
            }
        }
    }
}
