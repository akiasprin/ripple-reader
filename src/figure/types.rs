// SPDX-License-Identifier: MIT OR Apache-2.0

//! Types for figure detection and processing.

use serde::Serialize;

/// A detected missed figure recommendation produced by smart_detect.
#[derive(Serialize)]
pub struct SmartDetectRecommendation {
    pub page_idx: i32,
    pub inferred_body_bbox: [f32; 4],
    pub caption_bbox: [f32; 4],
    pub caption_text: String,
    pub confidence: f32,
    pub reason: String,
}

/// Response wrapper for the smart_detect endpoint.
#[derive(Serialize)]
pub struct SmartDetectResponse {
    pub recommendations: Vec<SmartDetectRecommendation>,
}
