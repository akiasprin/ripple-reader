// SPDX-License-Identifier: MIT OR Apache-2.0

//! Pure geometry utilities for bounding-box operations.
//! Zero web/HTTP dependencies.

use crate::mineru::LayoutParaBlock;

/// Compute the horizontal overlap ratio between two bboxes.
/// Returns overlap width divided by the width of `a`.
pub fn horizontal_overlap(a: &[f32], b: &[f32]) -> f32 {
    if a.len() < 4 || b.len() < 4 {
        return 0.0;
    }
    let left = a[0].max(b[0]);
    let right = a[2].min(b[2]);
    let overlap = (right - left).max(0.0);
    let width = (a[2] - a[0]).max(1.0);
    overlap / width
}

/// Extract a 4-element bbox from a `LayoutParaBlock` if available.
pub fn block_bbox(block: &LayoutParaBlock) -> Option<[f32; 4]> {
    if block.bbox.len() >= 4 {
        Some([block.bbox[0], block.bbox[1], block.bbox[2], block.bbox[3]])
    } else {
        None
    }
}

/// Compute the area of a bbox.
pub fn bbox_area(bbox: [f32; 4]) -> f32 {
    (bbox[2] - bbox[0]).max(0.0) * (bbox[3] - bbox[1]).max(0.0)
}

/// Compute the aspect ratio (max(w,h) / min(w,h)) of a bbox.
pub fn bbox_aspect_ratio(bbox: [f32; 4]) -> f32 {
    let w = (bbox[2] - bbox[0]).max(1.0);
    let h = (bbox[3] - bbox[1]).max(1.0);
    w.max(h) / w.min(h)
}

/// Compute the Intersection over Union (IoU) of two bboxes.
pub fn bbox_iou(a: [f32; 4], b: [f32; 4]) -> f32 {
    let x1 = a[0].max(b[0]);
    let y1 = a[1].max(b[1]);
    let x2 = a[2].min(b[2]);
    let y2 = a[3].min(b[3]);
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let area_a = (a[2] - a[0]) * (a[3] - a[1]);
    let area_b = (b[2] - b[0]) * (b[3] - b[1]);
    let union = area_a + area_b - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}
