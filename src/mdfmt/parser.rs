// SPDX-License-Identifier: MIT OR Apache-2.0

use regex::Regex;
use std::sync::LazyLock;

/// Regex for image references with optional sizing and alignment.
/// Groups: 1=alt, 2=url, 3=size, 4=height, 5=align
static IMAGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"!\[([^\]]*)\]\(([^)\s]+)(?:\s*=\s*(\d+(?:\.\d+)?(?:%|px)?)(?:x(\d+(?:\.\d+)?(?:%|px)?))?(?:\s+(left|right|inline|center))?)?\)"
    )
    .unwrap()
});

/// A single image reference found in markdown.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageRef {
    /// Full match text, e.g. `![desc](figure/foo.png =80% center)`
    pub full: String,
    /// Alt text inside `![...]`
    pub alt: String,
    /// URL / path inside `(...)` excluding any sizing/alignment suffix
    pub url: String,
    /// Optional sizing suffix, e.g. `=80%`, `=400px`, `=100%x200px`
    pub size: Option<String>,
    /// Optional alignment: left, right, center, inline
    pub align: Option<String>,
    /// Byte offset in the original markdown where this reference starts
    pub offset: usize,
}

/// Parse all image references from markdown text.
pub fn parse_images(text: &str) -> Vec<ImageRef> {
    let mut refs = Vec::new();
    for m in IMAGE_RE.find_iter(text) {
        let caps = IMAGE_RE.captures(m.as_str()).unwrap();
        let alt = caps
            .get(1)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        let url = caps
            .get(2)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        let size = caps.get(3).map(|m| m.as_str().to_string());
        let align = caps.get(5).map(|m| m.as_str().to_string());
        refs.push(ImageRef {
            full: m.as_str().to_string(),
            alt,
            url,
            size,
            align,
            offset: m.start(),
        });
    }
    refs
}

/// Check whether an image already has layout (size or alignment) applied.
pub fn has_layout(img: &ImageRef) -> bool {
    img.size.is_some() || img.align.is_some()
}

/// Build the replacement string for an image reference.
/// If size/align are already present they are overwritten.
pub fn rebuild_image(img: &ImageRef, new_size: Option<&str>, new_align: Option<&str>) -> String {
    let size_part = new_size.map(|s| format!(" ={}", s)).unwrap_or_default();
    let align_part = new_align.map(|a| format!(" {}", a)).unwrap_or_default();
    format!("![{}]({}{}{})", img.alt, img.url, size_part, align_part)
}
