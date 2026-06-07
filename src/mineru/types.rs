// SPDX-License-Identifier: MIT OR Apache-2.0

//! MinerU layout types and data structures.
use serde::{Deserialize, Serialize};

// ---- API response types ----
#[derive(Debug, Serialize)]
pub(crate) struct CreateTaskRequest {
    pub(crate) url: String,
    pub(crate) is_ocr: bool,
    pub(crate) language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model_version: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) no_cache: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateTaskResponse {
    pub(crate) code: i32,
    #[serde(default)]
    pub(crate) data: Option<TaskData>,
    #[serde(default)]
    pub(crate) msg: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TaskData {
    #[serde(default)]
    pub(crate) task_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct QueryTaskResponse {
    pub(crate) code: i32,
    #[serde(default)]
    pub(crate) data: Option<TaskResult>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TaskResult {
    #[serde(default)]
    pub(crate) state: Option<String>,
    #[serde(default)]
    pub(crate) full_zip_url: Option<String>,
    #[serde(default)]
    pub(crate) err_msg: Option<String>,
}

// ---- Caption node ----
#[derive(Debug, Deserialize)]
pub struct CaptionNode {
    #[serde(rename = "type")]
    pub node_type: String,
    pub content: String,
}

// ---- Layout JSON types ----
#[derive(Debug, Deserialize, Clone)]
pub struct LayoutDoc {
    pub pdf_info: Vec<LayoutPage>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LayoutPage {
    pub page_idx: i32,
    pub para_blocks: Vec<LayoutParaBlock>,
    #[serde(default)]
    pub preproc_blocks: Vec<LayoutParaBlock>,
    /// MinerU classifies "header" / "page_number" / etc. para_blocks as
    /// discarded.  Most of the time these are page artifacts that should
    /// be ignored, but occasionally MinerU mis-classifies actual figure
    /// content (e.g. the "Problem: ..." yellow box above Figure 1 in
    /// 2605.10889).  The top snap-up pass scans this list for substantive
    /// text contiguous with a figure body so the crop doesn't miss it.
    #[serde(default)]
    pub discarded_blocks: Vec<LayoutParaBlock>,
    /// `[width, height]` in PDF points. Optional because older fixtures
    /// may omit it; readers should fall back to a default.
    #[serde(default)]
    pub page_size: Vec<f32>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LayoutParaBlock {
    #[serde(rename = "type")]
    pub(crate) block_type: String,
    pub bbox: Vec<f32>,
    #[serde(default)]
    pub(crate) blocks: Vec<LayoutSubBlock>,
    /// Some MinerU outputs put `lines` directly on the para_block instead
    /// of inside `blocks` sub-blocks.  We treat them as a synthetic sub-block
    /// when `blocks` is empty.
    #[serde(default)]
    pub lines: Vec<LayoutLine>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LayoutSubBlock {
    #[serde(rename = "type")]
    pub sub_type: String,
    #[serde(default)]
    pub bbox: Vec<f32>,
    #[serde(default)]
    pub lines: Vec<LayoutLine>,
    #[serde(default)]
    pub image_path: Option<String>,
}

impl LayoutSubBlock {
    /// Returns true if this sub-block (or any of its spans) carries an
    /// image_path.  This is used to distinguish body (image/table/chart
    /// content) from caption text when MinerU outputs sub_type=N/A.
    pub(crate) fn has_image_path(&self) -> bool {
        if self.image_path.is_some() {
            return true;
        }
        for line in &self.lines {
            for span in &line.spans {
                if span.image_path.is_some() {
                    return true;
                }
            }
        }
        false
    }

    /// Returns true when this sub-block is a body envelope (image_body,
    /// table_body, chart_body, code_body).  MinerU's `image_body` sub-blocks
    /// often carry no `image_path` in their spans, so `has_image_path()`
    /// alone is insufficient to locate the body region.
    pub(crate) fn is_body(&self) -> bool {
        matches!(
            self.sub_type.as_str(),
            "image_body" | "table_body" | "chart_body" | "code_body"
        ) || self.has_image_path()
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct LayoutLine {
    #[serde(default)]
    pub bbox: Vec<f32>,
    pub spans: Vec<LayoutSpan>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LayoutSpan {
    #[serde(rename = "type")]
    pub span_type: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub image_path: Option<String>,
}

// ---- Extraction result types ----
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageBboxInfo {
    pub bbox: [f32; 4],
    pub page_idx: i32,
    pub content_type: String,
}

// ---- Cache and result types ----
#[derive(Debug, Serialize, Deserialize)]
pub struct CacheMeta {
    pub(crate) version: u32,
    pub image_descriptions: Vec<String>,
    #[serde(default)]
    pub image_names: Vec<String>,
    pub markdown: String,
    pub image_bboxes: Vec<ImageBboxInfo>,
    #[serde(default)]
    pub body_bboxes: Vec<ImageBboxInfo>,
    #[serde(default)]
    pub split_figures: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedFigures {
    pub images: Vec<(String, Vec<u8>)>, // (description, bytes)
    pub markdown: String,
    pub image_bboxes: Vec<ImageBboxInfo>,
    pub body_bboxes: Vec<ImageBboxInfo>,
    #[serde(skip)]
    pub layout_doc: Option<LayoutDoc>,
}

// ---- Processed block ----
#[derive(Debug, Clone)]
pub struct RawBlock {
    /// Full geometric extent used for hires cropping (body + captions +
    /// titles after absorption).  May be wider/taller than the original
    /// body envelope because captions and panel labels are included.
    pub bbox: [f32; 4],
    /// Original body envelope from MinerU's outer block.bbox, before
    /// caption expansion or title absorption.  Used by
    /// `absorb_titles_into_figures` so that a wide caption below the
    /// figure does not spuriously enlarge horizontal overlap with an
    /// unrelated section title above the figure.
    pub body_bbox: [f32; 4],
    pub block_type: String,
    pub img_path: String,
    pub desc: String,
    pub page_idx: i32,
    pub caption_number: Option<String>,
}

// ---- Title hint for caption absorption ----
pub(crate) struct TitleHint {
    pub bbox: [f32; 4],
    pub page_idx: i32,
}

// ---- Bbox adjustment for manual fine-tuning ----
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BboxAdjustment {
    #[serde(default)]
    pub left: f32,
    #[serde(default)]
    pub top: f32,
    #[serde(default)]
    pub right: f32,
    #[serde(default)]
    pub bottom: f32,
}

impl BboxAdjustment {
    pub fn apply(&self, bbox: [f32; 4]) -> [f32; 4] {
        [
            bbox[0] + self.left,
            bbox[1] + self.top,
            bbox[2] + self.right,
            bbox[3] + self.bottom,
        ]
    }
}

pub type BboxAdjustments = std::collections::HashMap<String, BboxAdjustment>;

/// A manually-created figure entry stored separately from MinerU output.
/// These are appended to the overlay and participate in hires generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualFigure {
    pub name: String,
    pub page_idx: i32,
    pub bbox: [f32; 4],
    pub body_bbox: [f32; 4],
    pub desc: String,
    pub content_type: String,
}

pub type ManualFigures = Vec<ManualFigure>;
