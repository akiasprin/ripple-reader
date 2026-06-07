// SPDX-License-Identifier: MIT OR Apache-2.0

//! MinerU PDF extraction: layout parsing, caption detection, figure/table
//! extraction, post-processing (merge/filter/rebind/propagation), and validation.
//!
//! Split from the original single-file `mineru.rs` into focused modules.

mod client;
mod layout;
mod logging;
mod postprocess;
mod types;
mod utils;
mod validate;

// Re-export public API
pub use logging::{close_mineru_log, open_mineru_log};
pub use types::{
    BboxAdjustment, BboxAdjustments, CacheMeta, ExtractedFigures, ImageBboxInfo, LayoutDoc,
    LayoutLine, LayoutPage, LayoutParaBlock, LayoutSpan, ManualFigure, ManualFigures, RawBlock,
};
// LayoutParaBlock methods are in layout.rs (auto-imported when type is used)
pub use client::MinerUClient;
pub use layout::{collect_block_lines, extract_block_text};
pub use postprocess::{
    detect_column_boundaries, merge_into, post_process_blocks, propagate_captions,
    rebind_orphan_captions, should_merge, should_merge_geometry_only,
};
pub use utils::{
    bbox_contains, encode_base36_2, extract_caption_number, group_caption_lines, line_text,
    looks_like_caption_header, union_bbox,
};
pub use validate::validate_extracted_figures;
