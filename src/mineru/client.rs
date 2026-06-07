// SPDX-License-Identifier: MIT OR Apache-2.0

//! MinerU API client: download, process, cache.
use anyhow::{Context, Result};

use crate::mineru::logging::{pp_debug, pp_info, pp_warn, with_paper_id};
use crate::mineru::postprocess::{
    absorb_titles_into_figures, post_process_blocks, rebind_orphan_captions,
    swap_mismatched_captions,
};
use crate::mineru::types::{
    CacheMeta, CreateTaskRequest, CreateTaskResponse, ExtractedFigures, ImageBboxInfo, LayoutDoc,
    QueryTaskResponse, RawBlock, TitleHint,
};
use crate::mineru::utils::{
    extract_caption_number, has_sub_panel_label, looks_like_caption, looks_like_caption_header,
    structured_image_name, union_bbox,
};
use crate::mineru::validate::validate_extracted_figures;

const MAX_DISCARDED_STEP_GAP: f32 = 15.0;
const MAX_EXPANSION: f32 = 10.0;
const MIN_DISCARDED_TEXT_CHARS: usize = 10;
const MIN_FREE_FOR_EXPANSION: f32 = 50.0;
const MIN_TOP_FROM_PAGE: f32 = 50.0;

pub struct MinerUClient {
    pub(crate) client: reqwest::Client,
    base_url: String,
    api_key: String,
    cache_dir: Option<String>,
    no_cache: bool,
}

impl MinerUClient {
    pub fn new(
        base_url: String,
        api_key: String,
        cache_dir: Option<String>,
        no_cache: bool,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            api_key,
            cache_dir,
            no_cache,
        }
    }

    /// Re-process from cached zip only. Never calls the API.
    /// Returns error if zip is missing or corrupted.
    /// `split_figures` contains caption numbers (e.g. "F:7", "F:8") whose
    /// sub-figures should NOT be merged into a single composite image.
    pub async fn reprocess_from_zip(
        &self,
        cache_key: &str,
        split_figures: &[String],
        skip_validation: bool,
        skip_images: bool,
    ) -> Result<ExtractedFigures> {
        with_paper_id(
            cache_key,
            self.reprocess_from_zip_inner(cache_key, split_figures, skip_validation, skip_images),
        )
        .await
    }

    async fn reprocess_from_zip_inner(
        &self,
        cache_key: &str,
        split_figures: &[String],
        skip_validation: bool,
        skip_images: bool,
    ) -> Result<ExtractedFigures> {
        let cache_path =
            std::path::Path::new(self.cache_dir.as_ref().context("cache_dir not set")?)
                .join(cache_key);

        let zip_path = cache_path.join("mineru.zip");
        anyhow::ensure!(
            zip_path.exists(),
            "mineru.zip not found at {}",
            zip_path.display()
        );

        pp_info(&format!(
            "[mineru] Re-processing from cached zip for {}",
            cache_key
        ));
        let zip_bytes = tokio::fs::read(&zip_path)
            .await
            .with_context(|| format!("Failed to read cached zip: {}", zip_path.display()))?;

        let result = self
            .process_zip(&zip_bytes, split_figures, Some(cache_key))
            .await
            .context("Failed to process cached zip")?;

        if !skip_validation {
            if let Err(e) =
                validate_extracted_figures(&result, result.layout_doc.as_ref(), Some(cache_key))
            {
                return Err(anyhow::anyhow!(e));
            }
        }

        pp_info(&format!(
            "[mineru] Re-processed {} figures from cached zip for {}",
            result.images.len(),
            cache_key
        ));

        if let Err(e) = self
            .save_to_cache(&cache_path, &result, split_figures, skip_images)
            .await
        {
            pp_warn(&format!(
                "[mineru] Cache save failed after zip reprocess for {}: {}",
                cache_key, e
            ));
        }

        Ok(result)
    }

    /// Extract figures from a PDF via MinerU API.
    /// `split_figures` contains caption numbers (e.g. "F:7") whose sub-figures
    /// should be kept as separate images instead of being merged.
    /// When `split_figures` is non-empty, cache is skipped to avoid overwriting
    /// the default merged result.
    pub async fn extract_figures(
        &self,
        pdf_url: &str,
        cache_key: Option<&str>,
        split_figures: &[String],
        skip_validation: bool,
    ) -> Result<ExtractedFigures> {
        if let Some(key) = cache_key {
            with_paper_id(
                key,
                self.extract_figures_inner(pdf_url, cache_key, split_figures, skip_validation),
            )
            .await
        } else {
            self.extract_figures_inner(pdf_url, cache_key, split_figures, skip_validation)
                .await
        }
    }

    async fn extract_figures_inner(
        &self,
        pdf_url: &str,
        cache_key: Option<&str>,
        split_figures: &[String],
        skip_validation: bool,
    ) -> Result<ExtractedFigures> {
        if let (Some(ref dir), Some(key)) = (&self.cache_dir, cache_key) {
            let cache_path = std::path::Path::new(dir).join(key);
            let meta_path = cache_path.join("mineru.json");

            // Check cached split config before deciding whether to reuse images.
            let cached_split = if meta_path.exists() {
                match tokio::fs::read_to_string(&meta_path).await {
                    Ok(json) => serde_json::from_str::<CacheMeta>(&json)
                        .ok()
                        .map(|m| m.split_figures),
                    Err(_) => None,
                }
            } else {
                None
            };
            let split_match = cached_split
                .as_ref()
                .map(|s| s == split_figures)
                .unwrap_or(false);

            if split_match {
                match self.load_from_cache(&cache_path).await {
                    Ok(result) => {
                        pp_info(&format!(
                            "[mineru] Cache hit for {} (split config matches)",
                            key
                        ));
                        return Ok(result);
                    }
                    Err(e) => {
                        pp_warn(&format!(
                            "[mineru] Cache load failed for {}: {}, trying zip fallback",
                            key, e
                        ));
                    }
                }
            } else if cached_split.is_some() {
                pp_info(&format!(
                    "[mineru] Cached split config mismatch for {}: cached={:?}, requested={:?}, re-processing from zip",
                    key, cached_split.unwrap(), split_figures
                ));
            }

            // Try re-processing from cached zip (always with the requested split config).
            let zip_path = cache_path.join("mineru.zip");
            if zip_path.exists() {
                pp_info(&format!(
                    "[mineru] Trying to re-process from cached zip for {}",
                    key
                ));
                match tokio::fs::read(&zip_path).await {
                    Ok(zip_bytes) => {
                        match self.process_zip(&zip_bytes, split_figures, Some(key)).await {
                            Ok(result) => {
                                pp_info(&format!(
                                    "[mineru] Re-processed {} figures from cached zip for {}",
                                    result.images.len(),
                                    key
                                ));
                                if !skip_validation {
                                    if let Err(e) = validate_extracted_figures(
                                        &result,
                                        result.layout_doc.as_ref(),
                                        Some(key),
                                    ) {
                                        return Err(anyhow::anyhow!(e));
                                    }
                                }
                                if let Err(e) = self
                                    .save_to_cache(&cache_path, &result, split_figures, false)
                                    .await
                                {
                                    pp_warn(&format!(
                                        "[mineru] Cache save failed after zip reprocess for {}: {}",
                                        key, e
                                    ));
                                }
                                return Ok(result);
                            }
                            Err(e) => {
                                pp_warn(&format!(
                                    "[mineru] Zip re-process failed for {}: {}",
                                    key, e
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        pp_warn(&format!(
                            "[mineru] Failed to read cached zip for {}: {}",
                            key, e
                        ));
                    }
                }
            }
        }

        // Download zip first, save it to disk, *then* validate.
        // This guarantees the raw zip is cached even when downstream
        // processing fails, so engineers can inspect it later.
        let zip_bytes = self.download_mineru_zip(pdf_url).await?;

        let cache_path = if let (Some(ref dir), Some(key)) = (&self.cache_dir, cache_key) {
            let path = std::path::Path::new(dir).join(key);
            if let Err(e) = tokio::fs::create_dir_all(&path).await {
                pp_warn(&format!(
                    "[mineru] Failed to create cache dir for {}: {}",
                    key, e
                ));
            }
            if let Err(e) = tokio::fs::write(path.join("mineru.zip"), &zip_bytes).await {
                pp_warn(&format!(
                    "[mineru] Cache save (zip) failed for {}: {}",
                    key, e
                ));
            }
            Some(path)
        } else {
            None
        };

        let result = match self.process_zip(&zip_bytes, split_figures, cache_key).await {
            Ok(r) => r,
            Err(e) => {
                pp_warn(&format!(
                    "[mineru] process_zip failed for {} (zip already saved to cache): {}",
                    cache_key.unwrap_or("?"),
                    e
                ));
                return Err(e);
            }
        };

        if !skip_validation {
            if let Err(e) =
                validate_extracted_figures(&result, result.layout_doc.as_ref(), cache_key)
            {
                return Err(anyhow::anyhow!(e));
            }
        }

        if let Some(path) = cache_path {
            if let Err(e) = self
                .save_to_cache(&path, &result, split_figures, false)
                .await
            {
                pp_warn(&format!(
                    "[mineru] Cache save (json+images) failed for {}: {}",
                    cache_key.unwrap_or("?"),
                    e
                ));
            }
        }

        Ok(result)
    }

    /// Download a MinerU result zip for `pdf_url` and return the raw bytes.
    /// Does **not** run `process_zip` or validation.
    pub async fn download_mineru_zip(&self, pdf_url: &str) -> Result<Vec<u8>> {
        let task_id = self.create_task(pdf_url).await?;
        pp_info(&format!("[mineru] Task created: {}", task_id));

        let zip_url = self.poll_task(&task_id).await?;
        pp_info(&format!("[mineru] Task done, zip URL: {}", zip_url));

        let zip_bytes = self.download_zip(&zip_url).await?;
        pp_info(&format!("[mineru] Downloaded {} bytes", zip_bytes.len()));

        Ok(zip_bytes)
    }

    async fn load_from_cache(&self, cache_path: &std::path::Path) -> Result<ExtractedFigures> {
        let meta_path = cache_path.join("mineru.json");
        let meta_json = tokio::fs::read_to_string(&meta_path)
            .await
            .context("Failed to read cache meta")?;
        let meta: CacheMeta =
            serde_json::from_str(&meta_json).context("Failed to parse cache meta")?;

        let mut images = Vec::new();
        for (i, desc) in meta.image_descriptions.iter().enumerate() {
            let name = meta
                .image_names
                .get(i)
                .expect("image_names must match image_descriptions in cache");
            let img_path = cache_path.join(format!("{}.jpg", name));
            let bytes = tokio::fs::read(&img_path)
                .await
                .with_context(|| format!("Failed to read cached image: {}", img_path.display()))?;
            let desc_short = &desc[..desc
                .char_indices()
                .nth(80)
                .map(|(j, _)| j)
                .unwrap_or(desc.len())];
            match meta.image_bboxes.get(i) {
                Some(bb) => pp_info(&format!(
                    "[mineru] (cached) name={} idx={} page={} type={} bbox=[{:.1},{:.1},{:.1},{:.1}] desc={}",
                    name, i, bb.page_idx + 1, bb.content_type,
                    bb.bbox[0], bb.bbox[1], bb.bbox[2], bb.bbox[3], desc_short
                )),
                None => pp_info(&format!(
                    "[mineru] (cached) name={} idx={} (no bbox info) desc={}",
                    name, i, desc_short
                )),
            }
            images.push((desc.clone(), bytes));
        }

        Ok(ExtractedFigures {
            images,
            markdown: meta.markdown,
            image_bboxes: meta.image_bboxes,
            body_bboxes: meta.body_bboxes,
            layout_doc: None,
        })
    }

    async fn save_to_cache(
        &self,
        cache_path: &std::path::Path,
        result: &ExtractedFigures,
        split_figures: &[String],
        skip_images: bool,
    ) -> Result<()> {
        tokio::fs::create_dir_all(cache_path)
            .await
            .context("Failed to create cache dir")?;

        let paper_id_prefix = cache_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("xx")
            .chars()
            .take(2)
            .collect::<String>();
        let image_names: Vec<String> = result
            .images
            .iter()
            .enumerate()
            .map(|(i, (_, bytes))| structured_image_name(&paper_id_prefix, i, bytes))
            .collect();

        for (i, name) in image_names.iter().enumerate() {
            let (desc, _) = &result.images[i];
            let desc_short = &desc[..desc
                .char_indices()
                .nth(80)
                .map(|(j, _)| j)
                .unwrap_or(desc.len())];
            match result.image_bboxes.get(i) {
                Some(bb) => pp_info(&format!(
                    "[mineru] name={} idx={} page={} type={} bbox=[{:.1},{:.1},{:.1},{:.1}] desc={}",
                    name, i, bb.page_idx + 1, bb.content_type,
                    bb.bbox[0], bb.bbox[1], bb.bbox[2], bb.bbox[3], desc_short
                )),
                None => pp_info(&format!(
                    "[mineru] name={} idx={} (no bbox info) desc={}",
                    name, i, desc_short
                )),
            }
        }

        let meta = CacheMeta {
            version: 1,
            image_descriptions: result.images.iter().map(|(desc, _)| desc.clone()).collect(),
            image_names: image_names.clone(),
            markdown: result.markdown.clone(),
            image_bboxes: result.image_bboxes.clone(),
            body_bboxes: result.body_bboxes.clone(),
            split_figures: split_figures.to_vec(),
        };

        let meta_json =
            serde_json::to_string_pretty(&meta).context("Failed to serialize cache meta")?;
        tokio::fs::write(cache_path.join("mineru.json"), meta_json)
            .await
            .context("Failed to write cache meta")?;

        if !skip_images {
            for ((_, bytes), name) in result.images.iter().zip(&image_names) {
                let img_path = cache_path.join(format!("{}.jpg", name));
                tokio::fs::write(&img_path, bytes).await.with_context(|| {
                    format!("Failed to write cached image: {}", img_path.display())
                })?;
            }
        }

        Ok(())
    }

    async fn create_task(&self, pdf_url: &str) -> Result<String> {
        let url = format!("{}/api/v4/extract/task", self.base_url);
        let req_body = CreateTaskRequest {
            url: pdf_url.to_string(),
            is_ocr: true,
            language: "el".to_string(),
            model_version: Some("vlm".to_string()),
            no_cache: self.no_cache,
        };
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&req_body)
            .send()
            .await
            .context("Failed to create MinerU task")?;

        let status = resp.status();
        let body_text = resp
            .text()
            .await
            .context("Failed to read MinerU create task response")?;
        pp_info(&format!(
            "[mineru] POST {} status={} body={}",
            url, status, body_text
        ));

        let body: CreateTaskResponse = serde_json::from_str(&body_text).with_context(|| {
            format!("Failed to parse MinerU create task response: {}", body_text)
        })?;

        if body.code != 0 {
            anyhow::bail!(
                "MinerU API error ({}): {}",
                body.code,
                body.msg.unwrap_or_default()
            );
        }

        let task_id = body
            .data
            .and_then(|d| d.task_id)
            .context("MinerU response missing task_id")?;

        Ok(task_id)
    }

    async fn poll_task(&self, task_id: &str) -> Result<String> {
        let url = format!("{}/api/v4/extract/task/{}", self.base_url, task_id);
        let max_attempts = 600; // 600 * 3s = 30 minutes max
        let interval = std::time::Duration::from_secs(3);

        for attempt in 1..=max_attempts {
            let resp = self
                .client
                .get(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .send()
                .await
                .context("Failed to query MinerU task")?;

            let status = resp.status();
            let body_text = resp
                .text()
                .await
                .context("Failed to read MinerU query response")?;

            let body: QueryTaskResponse = serde_json::from_str(&body_text)
                .with_context(|| format!("Failed to parse MinerU query response: {}", body_text))?;

            if body.code != 0 {
                pp_warn(&format!(
                    "[mineru] GET {} status={} body={}",
                    url, status, body_text
                ));
                anyhow::bail!("MinerU API error: code={}", body.code);
            }

            if let Some(data) = body.data {
                match data.state.as_deref() {
                    Some("done") => {
                        pp_info(&format!(
                            "[mineru] GET {} status={} body={}",
                            url, status, body_text
                        ));
                        return data.full_zip_url.context("MinerU done but no full_zip_url");
                    }
                    Some("failed") => {
                        pp_warn(&format!(
                            "[mineru] GET {} status={} body={}",
                            url, status, body_text
                        ));
                        anyhow::bail!("MinerU task failed: {}", data.err_msg.unwrap_or_default());
                    }
                    Some(state) => {
                        if attempt % 10 == 0 {
                            pp_info(&format!(
                                "[mineru] GET {} status={} body={}",
                                url, status, body_text
                            ));
                            pp_info(&format!(
                                "[mineru] Task {} state: {} (attempt {})",
                                task_id, state, attempt
                            ));
                        }
                    }
                    None => {}
                }
            }

            tokio::time::sleep(interval).await;
        }

        anyhow::bail!(
            "MinerU task polling timed out after {} attempts",
            max_attempts
        )
    }

    async fn download_zip(&self, zip_url: &str) -> Result<Vec<u8>> {
        pp_info(&format!("[mineru] Downloading ZIP from {}", zip_url));
        let resp = self
            .client
            .get(zip_url)
            .send()
            .await
            .context("Failed to download MinerU result ZIP")?;

        let status = resp.status();
        let bytes = resp.bytes().await.context("Failed to read ZIP bytes")?;
        pp_info(&format!(
            "[mineru] Downloaded {} bytes (HTTP {})",
            bytes.len(),
            status
        ));

        Ok(bytes.to_vec())
    }

    pub async fn process_zip(
        &self,
        zip_bytes: &[u8],
        split_figures: &[String],
        paper_id: Option<&str>,
    ) -> Result<ExtractedFigures> {
        self.process_zip_inner(zip_bytes, split_figures, paper_id)
            .await
    }

    async fn process_zip_inner(
        &self,
        zip_bytes: &[u8],
        split_figures: &[String],
        paper_id: Option<&str>,
    ) -> Result<ExtractedFigures> {
        let out_dir = tempfile::tempdir().context("Failed to create temp dir")?;
        let zip_path = out_dir.path().join("result.zip");

        tokio::fs::write(&zip_path, zip_bytes)
            .await
            .context("Failed to write ZIP file")?;

        // Extract ZIP synchronously
        let extract_dir = out_dir.path().join("extracted");
        std::fs::create_dir_all(&extract_dir).context("Failed to create extract dir")?;

        let file = std::fs::File::open(&zip_path).context("Failed to open ZIP")?;
        let mut archive = zip::ZipArchive::new(file).context("Failed to read ZIP archive")?;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i).context("Failed to read ZIP entry")?;
            // Prevent zip-slip: reject entries with '..' or absolute paths
            let name = file.name();
            if name.contains("..") || std::path::Path::new(name).is_absolute() {
                anyhow::bail!("Refusing to extract ZIP entry with unsafe path: {}", name);
            }
            let outpath = extract_dir.join(name);

            if file.name().ends_with('/') {
                std::fs::create_dir_all(&outpath)?;
            } else {
                if let Some(parent) = outpath.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut outfile = std::fs::File::create(&outpath)?;
                std::io::copy(&mut file, &mut outfile)?;
            }
        }

        pp_info(&format!(
            "[mineru] ZIP extracted to {}",
            extract_dir.display()
        ));

        // Find layout.json and markdown files
        let mut layout_path = None;
        let mut markdown_path = None;
        let mut images_dir = None;

        for entry in walkdir::WalkDir::new(&extract_dir).max_depth(2) {
            let entry = entry?;
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            if name == "layout.json" || name.ends_with("_layout.json") {
                layout_path = Some(path.to_path_buf());
            } else if name == "full.md" || name.ends_with(".md") {
                if markdown_path.is_none() || name == "full.md" {
                    markdown_path = Some(path.to_path_buf());
                }
            } else if name == "images" && path.is_dir() {
                images_dir = Some(path.to_path_buf());
            }
        }

        pp_info(&format!(
            "[mineru] Found files: layout={:?}, md={:?}, images={:?}",
            layout_path.as_ref().map(|p| p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()),
            markdown_path.as_ref().map(|p| p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()),
            images_dir.as_ref().map(|p| p.display().to_string()),
        ));

        // Read markdown
        let markdown = if let Some(path) = markdown_path {
            tokio::fs::read_to_string(&path).await.unwrap_or_default()
        } else {
            String::new()
        };

        // Try layout.json (has bbox info)
        let mut images = Vec::new();
        let mut bboxes = Vec::new();
        let mut body_bboxes = Vec::new();
        let mut layout_doc: Option<LayoutDoc> = None;

        if let Some(layout_path) = layout_path {
            let layout_json = tokio::fs::read_to_string(&layout_path).await?;
            match serde_json::from_str::<LayoutDoc>(&layout_json) {
                Ok(doc) => {
                    pp_info(&format!(
                        "[mineru] Parsed layout.json: {} pages",
                        doc.pdf_info.len()
                    ));

                    // Stage 1: collect raw candidates from layout blocks
                    let mut candidates: Vec<RawBlock> = Vec::new();
                    let mut all_orphan_caps: Vec<(String, [f32; 4], i32)> = Vec::new();
                    let mut orphan_caps: Vec<(String, [f32; 4], i32)> = Vec::new();
                    // Title hints: bbox + page_idx for `title`-typed para_blocks.
                    // Captioned figures absorb spatially-adjacent titles after the
                    // merge step so panel labels (e.g. "Audio-Visual Comprehension"
                    // sitting above the topmost sub-panel of a composite figure)
                    // appear inside the cropped image.
                    let mut title_hints: Vec<TitleHint> = Vec::new();
                    let mut page_text_edges: std::collections::HashMap<i32, (f32, f32)> =
                        std::collections::HashMap::new();
                    // Per-page text-block x-intervals.  Used to detect
                    // column boundaries: when the merged coverage of these
                    // intervals leaves a gap ≥15pt with substantial spans
                    // on both sides, that gap's midpoint defines a column
                    // boundary.  Driven by *text* blocks (not figures/
                    // tables) because text obeys column constraints while
                    // figures often span across columns.
                    let mut page_text_intervals: std::collections::HashMap<i32, Vec<(f32, f32)>> =
                        std::collections::HashMap::new();
                    for page in &doc.pdf_info {
                        for block in &page.para_blocks {
                            let block_type = &block.block_type;
                            let resolved_path = page.resolve_image_path(block);
                            let has_path = resolved_path.is_some();
                            let recovered_from_preproc =
                                resolved_path.is_some() && block.image_path().is_none();
                            let bbox_str = if block.bbox.len() >= 4 {
                                format!(
                                    "[{:.1},{:.1},{:.1},{:.1}]",
                                    block.bbox[0], block.bbox[1], block.bbox[2], block.bbox[3]
                                )
                            } else {
                                "[]".to_string()
                            };
                            pp_info(&format!(
                                "[mineru] layout block: page={}, type={}, has_img_path={}, bbox={}",
                                page.page_idx, block_type, has_path, bbox_str
                            ));
                            if recovered_from_preproc {
                                pp_info(&format!(
                                    "[mineru] image_path recovered from preproc_blocks: page={} type={} bbox={} (para_block had lines_deleted)",
                                    page.page_idx, block_type, bbox_str
                                ));
                            }
                            let is_interline_figure = block.block_type == "interline_equation"
                                && resolved_path.is_some()
                                && {
                                    let has_nearby_caption = page.para_blocks.iter().any(|other| {
                                        if other.block_type != "text"
                                            && other.block_type != "title"
                                            && other.block_type != "interline_equation"
                                        {
                                            return false;
                                        }
                                        if other.bbox.len() < 4 || block.bbox.len() < 4 {
                                            return false;
                                        }
                                        let inter_bottom = block.bbox[1].max(block.bbox[3]);
                                        let other_top = other.bbox[1].min(other.bbox[3]);
                                        if other_top <= inter_bottom {
                                            return false;
                                        }
                                        let gap = other_top - inter_bottom;
                                        if gap > 25.0 {
                                            return false;
                                        }
                                        let h_overlap = (block.bbox[0]
                                            .max(block.bbox[2])
                                            .min(other.bbox[0].max(other.bbox[2]))
                                            - block.bbox[0]
                                                .min(block.bbox[2])
                                                .max(other.bbox[0].min(other.bbox[2])))
                                        .max(0.0);
                                        if h_overlap < 20.0 {
                                            return false;
                                        }
                                        let mut other_spans = Vec::new();
                                        for sub in &other.blocks {
                                            for line in &sub.lines {
                                                other_spans.extend(line.spans.iter());
                                            }
                                        }
                                        for line in &other.lines {
                                            other_spans.extend(line.spans.iter());
                                        }
                                        let other_text: String = other_spans
                                            .into_iter()
                                            .filter_map(|s| s.content.as_ref())
                                            .cloned()
                                            .collect::<Vec<_>>()
                                            .join(" ");
                                        looks_like_caption_header(&other_text)
                                    });
                                    has_nearby_caption
                                };
                            if block.block_type == "image"
                                || block.block_type == "table"
                                || block.block_type == "chart"
                                || is_interline_figure
                            {
                                if let Some(img_path) = resolved_path {
                                    let mut body_bbox = if block.bbox.len() >= 4 {
                                        [block.bbox[0], block.bbox[1], block.bbox[2], block.bbox[3]]
                                    } else {
                                        [0.0, 0.0, 0.0, 0.0]
                                    };
                                    // Ensure body_bbox covers every body
                                    // sub-block.  MinerU's outer block.bbox is
                                    // sometimes narrower than the union of its
                                    // image_body / table_body children (e.g.
                                    // 2005.11401 Figure 2 where a left-side
                                    // text chart sits in its own body sub-block
                                    // outside block.bbox).
                                    pp_debug(&format!(
                                        "[mineru] body_bbox start: page={} type={} bbox=[{:.1},{:.1},{:.1},{:.1}]",
                                        page.page_idx, block.block_type,
                                        body_bbox[0], body_bbox[1], body_bbox[2], body_bbox[3]
                                    ));
                                    for sub in block.all_subs() {
                                        if sub.bbox.len() < 4 {
                                            pp_debug(&format!(
                                                "[mineru] body loop skip (bbox too short): page={} type={} sub_type={} bbox_len={}",
                                                page.page_idx, block.block_type, sub.sub_type, sub.bbox.len()
                                            ));
                                            continue;
                                        }
                                        if !sub.is_body() {
                                            pp_debug(&format!(
                                                "[mineru] body loop skip (not body): page={} type={} sub_type={} has_img_path={}",
                                                page.page_idx, block.block_type, sub.sub_type, sub.has_image_path()
                                            ));
                                            continue;
                                        }
                                        pp_debug(&format!(
                                            "[mineru] body loop union: page={} type={} sub_type={} bbox=[{:.1},{:.1},{:.1},{:.1}]",
                                            page.page_idx, block.block_type, sub.sub_type,
                                            sub.bbox[0], sub.bbox[1], sub.bbox[2], sub.bbox[3]
                                        ));
                                        body_bbox = union_bbox(
                                            body_bbox,
                                            [sub.bbox[0], sub.bbox[1], sub.bbox[2], sub.bbox[3]],
                                        );
                                    }
                                    // Fallback: some sub-blocks carry visual
                                    // content but are not recognised as body
                                    // by is_body() (empty sub_type and no
                                    // image_path, e.g. 2605.28691 Table 2
                                    // right sub-panel b).  Union them if
                                    // they have no text content.
                                    for sub in block.all_subs() {
                                        if sub.bbox.len() < 4 {
                                            pp_debug(&format!(
                                                "[mineru] fallback skip (bbox too short): page={} type={} sub_type={} bbox_len={}",
                                                page.page_idx, block.block_type, sub.sub_type, sub.bbox.len()
                                            ));
                                            continue;
                                        }
                                        if sub.is_body() {
                                            pp_debug(&format!(
                                                "[mineru] fallback skip (already body): page={} type={} sub_type={}",
                                                page.page_idx, block.block_type, sub.sub_type
                                            ));
                                            continue;
                                        }
                                        let sub_text: String = sub
                                            .lines
                                            .iter()
                                            .flat_map(|l| &l.spans)
                                            .filter_map(|s| s.content.as_ref())
                                            .cloned()
                                            .collect::<Vec<_>>()
                                            .join(" ");
                                        if !sub_text.trim().is_empty() {
                                            pp_debug(&format!(
                                                "[mineru] fallback skip (has text): page={} type={} sub_type={} text_len={} text_preview={:?}",
                                                page.page_idx, block.block_type, sub.sub_type,
                                                sub_text.len(),
                                                &sub_text.chars().take(40).collect::<String>()
                                            ));
                                            continue;
                                        }
                                        pp_debug(&format!(
                                            "[mineru] fallback union: page={} type={} sub_type={} bbox=[{:.1},{:.1},{:.1},{:.1}] text_len=0",
                                            page.page_idx, block.block_type, sub.sub_type,
                                            sub.bbox[0], sub.bbox[1], sub.bbox[2], sub.bbox[3]
                                        ));
                                        body_bbox = union_bbox(
                                            body_bbox,
                                            [sub.bbox[0], sub.bbox[1], sub.bbox[2], sub.bbox[3]],
                                        );
                                    }
                                    pp_debug(&format!(
                                        "[mineru] body_bbox final: page={} type={} bbox=[{:.1},{:.1},{:.1},{:.1}]",
                                        page.page_idx, block.block_type,
                                        body_bbox[0], body_bbox[1], body_bbox[2], body_bbox[3]
                                    ));
                                    // Snapshot the body envelope before any
                                    // sub-block unioning so the "above body" /
                                    // horizontal-overlap checks below operate
                                    // on the unchanged body geometry rather
                                    // than the growing body_bbox.
                                    let initial_body_top = body_bbox[1].min(body_bbox[3]);
                                    let initial_body_left = body_bbox[0].min(body_bbox[2]);
                                    let initial_body_right = body_bbox[0].max(body_bbox[2]);
                                    let initial_body_width =
                                        (initial_body_right - initial_body_left).abs().max(1.0);
                                    // Pull `*_footnote` sub-blocks that carry a
                                    // sub-panel label (e.g. "(a) Iso-step
                                    // scavenging performance") into body_bbox.
                                    // MinerU often groups such labels under
                                    // `table_footnote` / `image_footnote` and they
                                    // sit outside the parent para_block.bbox, so
                                    // `keep_caption=false` hires crops would
                                    // otherwise strip them.  They are semantically
                                    // part of the body, not the main "Figure X" /
                                    // "Table X" caption strip that keep_caption=
                                    // false is meant to remove.  Filtering by
                                    // `has_sub_panel_label` avoids pulling in
                                    // unrelated side annotations that MinerU also
                                    // tags `*_footnote` (e.g. "User: ..." Q&A
                                    // panels in 2408.15998).
                                    for sub in block.all_subs() {
                                        if sub.bbox.len() < 4 {
                                            continue;
                                        }
                                        if !sub.sub_type.ends_with("_footnote") {
                                            continue;
                                        }
                                        let sub_text: String = sub
                                            .lines
                                            .iter()
                                            .flat_map(|l| &l.spans)
                                            .filter_map(|s| s.content.as_ref())
                                            .cloned()
                                            .collect::<Vec<_>>()
                                            .join(" ");
                                        if !has_sub_panel_label(&sub_text) {
                                            continue;
                                        }
                                        body_bbox = union_bbox(
                                            body_bbox,
                                            [sub.bbox[0], sub.bbox[1], sub.bbox[2], sub.bbox[3]],
                                        );
                                    }
                                    // Pull `*_caption` sub-blocks that sit above
                                    // the body and represent panel-level labels
                                    // (e.g. "trace:" or "(a) graph w/o
                                    // superposition" in 2604.20587 Figure 4).
                                    // `full_sub_bbox()` filters these out via
                                    // the spatial check (cap_bottom <
                                    // min_image_top), and `selected_captions()`
                                    // applies the same filter, so without this
                                    // pull `keep_caption=false` crops silently
                                    // drop the labels.  We require:
                                    //   - the sub-block is a `*_caption`
                                    //   - it sits above the body's top
                                    //   - `extract_caption_number` is None
                                    //     (skip the main "Figure X"/"Table X"
                                    //     caption when MinerU mis-places it
                                    //     above body, e.g. 2408.15998 page 19
                                    //     block 24)
                                    //   - horizontal overlap with body is
                                    //     significant (excludes side-panel
                                    //     captions positioned to the right of
                                    //     the body, e.g. 2408.15998 page 19
                                    //     block 29 Q&A annotations)
                                    //   - for image/chart blocks: the caption
                                    //     must not sit far above the body
                                    //     (foreign panel labels mis-nested by
                                    //     MinerU, e.g. 1608.05343 page 14
                                    //     Figure 12 containing Figure 11's
                                    //     "(b)" label ~75 pt above the body).
                                    //     A 30 pt threshold keeps genuine
                                    //     nearby panel labels (2604.20587
                                    //     Figure 4, gap <= 18 pt) while
                                    //     rejecting distant foreign ones.
                                    let block_type_lower = block.block_type.to_lowercase();
                                    let is_image_block =
                                        block_type_lower == "image" || block_type_lower == "chart";
                                    for sub in block.all_subs() {
                                        if sub.bbox.len() < 4 {
                                            continue;
                                        }
                                        if !sub.sub_type.ends_with("_caption") {
                                            continue;
                                        }
                                        let sub_bottom = sub.bbox[1].max(sub.bbox[3]);
                                        if is_image_block && sub_bottom < initial_body_top - 30.0 {
                                            continue;
                                        }
                                        if sub_bottom > initial_body_top + 1.0 {
                                            continue;
                                        }
                                        let sub_text: String = sub
                                            .lines
                                            .iter()
                                            .flat_map(|l| &l.spans)
                                            .filter_map(|s| s.content.as_ref())
                                            .cloned()
                                            .collect::<Vec<_>>()
                                            .join(" ");
                                        if extract_caption_number(&sub_text).is_some() {
                                            continue;
                                        }
                                        let sub_left = sub.bbox[0].min(sub.bbox[2]);
                                        let sub_right = sub.bbox[0].max(sub.bbox[2]);
                                        let sub_width = (sub_right - sub_left).abs().max(1.0);
                                        let overlap = (sub_right.min(initial_body_right)
                                            - sub_left.max(initial_body_left))
                                        .max(0.0);
                                        let overlap_ratio =
                                            overlap / sub_width.min(initial_body_width);
                                        if overlap_ratio < 0.3 {
                                            continue;
                                        }
                                        // Guard against section titles
                                        // mis-classified as `image_caption` by
                                        // MinerU (e.g. 2605.18753 page 24
                                        // "F.4 Cost-Effectiveness Analysis").
                                        // Genuine panel labels either look
                                        // like captions, carry a panel label,
                                        // or are very short, OR overlap the
                                        // body heavily (panel titles like
                                        // "Scaled Dot-Product Attention").
                                        if !looks_like_caption(&sub_text)
                                            && !has_sub_panel_label(&sub_text)
                                            && sub_text.len() > 25
                                            && overlap_ratio < 0.5
                                        {
                                            continue;
                                        }
                                        body_bbox = union_bbox(
                                            body_bbox,
                                            [sub.bbox[0], sub.bbox[1], sub.bbox[2], sub.bbox[3]],
                                        );
                                    }
                                    // Pull `*_caption` sub-blocks that sit BELOW
                                    // the body and represent panel-level labels
                                    // (e.g. "(a) Learned Frey Face manifold" below
                                    // each panel in 1312.6114 Figure 4). Same
                                    // criteria as the above-body loop, but checking
                                    // distance below the body instead of above.
                                    let initial_body_bottom = body_bbox[1].max(body_bbox[3]);
                                    for sub in block.all_subs() {
                                        if sub.bbox.len() < 4 {
                                            continue;
                                        }
                                        if !sub.sub_type.ends_with("_caption") {
                                            continue;
                                        }
                                        let sub_top = sub.bbox[1].min(sub.bbox[3]);
                                        if sub_top < initial_body_bottom - 1.0 {
                                            continue;
                                        }
                                        if is_image_block && sub_top > initial_body_bottom + 30.0 {
                                            continue;
                                        }
                                        let sub_text: String = sub
                                            .lines
                                            .iter()
                                            .flat_map(|l| &l.spans)
                                            .filter_map(|s| s.content.as_ref())
                                            .cloned()
                                            .collect::<Vec<_>>()
                                            .join(" ");
                                        if extract_caption_number(&sub_text).is_some() {
                                            continue;
                                        }
                                        let sub_left = sub.bbox[0].min(sub.bbox[2]);
                                        let sub_right = sub.bbox[0].max(sub.bbox[2]);
                                        let sub_width = (sub_right - sub_left).abs().max(1.0);
                                        let overlap = (sub_right.min(initial_body_right)
                                            - sub_left.max(initial_body_left))
                                        .max(0.0);
                                        let overlap_ratio =
                                            overlap / sub_width.min(initial_body_width);
                                        if overlap_ratio < 0.3 {
                                            continue;
                                        }
                                        // Guard against section titles
                                        // mis-classified as `image_caption` by
                                        // MinerU (e.g. 2605.18753 page 24
                                        // "F.4 Cost-Effectiveness Analysis").
                                        // Genuine panel labels either look
                                        // like captions, carry a panel label,
                                        // or are very short, OR overlap the
                                        // body heavily (panel titles like
                                        // "Scaled Dot-Product Attention").
                                        if !looks_like_caption(&sub_text)
                                            && !has_sub_panel_label(&sub_text)
                                            && sub_text.len() > 25
                                            && overlap_ratio < 0.5
                                        {
                                            continue;
                                        }
                                        body_bbox = union_bbox(
                                            body_bbox,
                                            [sub.bbox[0], sub.bbox[1], sub.bbox[2], sub.bbox[3]],
                                        );
                                    }
                                    // Expand bbox to cover the body envelope plus
                                    // every sub-block's bbox (image_body +
                                    // image_caption + image_footnote + table_*).
                                    // Layout.json reports the parent block.bbox as
                                    // the body envelope only, and `caption_bbox()`
                                    // only returns the SELECTED caption (the one
                                    // closest to body centre).  Panel-label
                                    // sub-captions like "(a) Encoder + LLM" above
                                    // each panel of a composite figure are valid
                                    // `image_caption` children that lose the
                                    // tiebreak when a wider main caption (e.g.
                                    // "Figure 7 ...") is also present, so they
                                    // never reach `caption_bbox()`.  Unioning every
                                    // sub-block keeps those labels inside the crop.
                                    let full_bb = block.full_sub_bbox();
                                    let bbox = match full_bb {
                                        Some(full) => union_bbox(body_bbox, full),
                                        None => body_bbox,
                                    };
                                    pp_info(&format!(
                                        "[mineru] full_sub_bbox: page={} type={} body_bbox={:?} full_sub={:?} final_bbox={:?}",
                                        page.page_idx, block.block_type, body_bbox, full_bb, bbox
                                    ));
                                    // Use layout.json caption (spatially validated).
                                    let layout_caption = block.caption();
                                    let _spatially_invalid = block.has_caption_above_image();
                                    let desc = layout_caption.unwrap_or_default();
                                    let mut desc = if desc.is_empty() {
                                        format!(
                                            "{} on page {}",
                                            block.block_type,
                                            page.page_idx + 1
                                        )
                                    } else {
                                        desc
                                    };
                                    let cap_num = extract_caption_number(&desc);

                                    // For split figures, use layout.json's first caption span.
                                    if let Some(ref cn) = cap_num {
                                        if split_figures.contains(cn) {
                                            if let Some(first) = block.caption_first() {
                                                desc = first;
                                            }
                                        }
                                    }
                                    candidates.push(RawBlock {
                                        bbox,
                                        body_bbox,
                                        block_type: block.block_type.clone(),
                                        img_path,
                                        desc: desc.clone(),
                                        page_idx: page.page_idx,
                                        caption_number: cap_num.clone(),
                                    });
                                    pp_info(&format!(
                                        "[mineru] candidate[{}]: page={} type={} cap={:?} bbox={:?} body={:?} desc={}",
                                        candidates.len() - 1,
                                        page.page_idx,
                                        block.block_type,
                                        cap_num,
                                        bbox,
                                        body_bbox,
                                        &desc[..desc.char_indices().nth(80).map(|(i, _)| i).unwrap_or(desc.len())]
                                    ));
                                } else {
                                    pp_warn(&format!(
                                        "[mineru] Skipping {} block on page {}: no image_path in layout.json",
                                        block.block_type, page.page_idx
                                    ));
                                }
                            } else if block.bbox.len() >= 4 {
                                // Track body-text column edges for snap-to-edge.
                                let entry = page_text_edges
                                    .entry(page.page_idx)
                                    .or_insert((f32::MAX, 0.0));
                                entry.0 = entry.0.min(block.bbox[0]);
                                entry.1 = entry.1.max(block.bbox[2]);
                                // Track per-block x-intervals (used for
                                // column-boundary detection later).  Skip
                                // very wide blocks that visually span the
                                // whole page — those are most likely
                                // section headings or full-width figures
                                // mis-typed as `text`, and they would mask
                                // the gutter between columns.
                                let bw = (block.bbox[2] - block.bbox[0]).abs();
                                let is_columnar = matches!(
                                    block.block_type.as_str(),
                                    "text" | "title" | "list" | "code"
                                );
                                if is_columnar && bw < 400.0 {
                                    page_text_intervals.entry(page.page_idx).or_default().push((
                                        block.bbox[0].min(block.bbox[2]),
                                        block.bbox[0].max(block.bbox[2]),
                                    ));
                                }
                            }
                            // Collect orphan captions for later rebinding.
                            for (text, bbox) in block.orphan_captions() {
                                orphan_caps.push((text.clone(), bbox, page.page_idx));
                                all_orphan_caps.push((text, bbox, page.page_idx));
                            }
                            // Collect title bboxes so captioned figures can
                            // absorb panel labels that sit above them.
                            if block.block_type == "title" && block.bbox.len() >= 4 {
                                title_hints.push(TitleHint {
                                    bbox: [
                                        block.bbox[0],
                                        block.bbox[1],
                                        block.bbox[2],
                                        block.bbox[3],
                                    ],
                                    page_idx: page.page_idx,
                                });
                            }
                        }

                        // Fallback: MinerU sometimes filters OCR-mangled captions
                        // (e.g. "cigure" instead of "figure") out of para_blocks,
                        // leaving an empty text block.  Scan preproc_blocks for any
                        // orphan caption that did not survive into para_blocks.
                        let mut para_orphan_keys = std::collections::HashSet::new();
                        for (text, bbox, _page) in &orphan_caps {
                            let key = format!(
                                "{}:{:.1}:{:.1}:{:.1}:{:.1}",
                                text, bbox[0], bbox[1], bbox[2], bbox[3]
                            );
                            para_orphan_keys.insert(key);
                        }
                        for block in &page.preproc_blocks {
                            for (text, bbox) in block.orphan_captions() {
                                let key = format!(
                                    "{}:{:.1}:{:.1}:{:.1}:{:.1}",
                                    text, bbox[0], bbox[1], bbox[2], bbox[3]
                                );
                                if !para_orphan_keys.contains(&key) {
                                    pp_info(&format!("[mineru] preproc orphan caption: page={} bbox={:?} text={}", page.page_idx, bbox, &text[..text.char_indices().nth(60).map(|(i, _)| i).unwrap_or(text.len())]));
                                    orphan_caps.push((text.clone(), bbox, page.page_idx));
                                    all_orphan_caps.push((text, bbox, page.page_idx));
                                    para_orphan_keys.insert(key);
                                }
                            }
                        }

                        // Re-bind orphan captions to the nearest bare image above them.
                        if !orphan_caps.is_empty() {
                            rebind_orphan_captions(&mut candidates, &orphan_caps, split_figures);
                        }

                        // Summary: count image/table blocks on this page for diagnosis
                        let img_blocks = page
                            .preproc_blocks
                            .iter()
                            .filter(|b| b.block_type == "image" || b.block_type == "chart")
                            .count();
                        let tbl_blocks = page
                            .preproc_blocks
                            .iter()
                            .filter(|b| b.block_type == "table")
                            .count();
                        let candidates_on_page = candidates
                            .iter()
                            .filter(|c| c.page_idx == page.page_idx)
                            .count();
                        pp_info(&format!(
                            "[mineru] page {} summary: {} candidates, image_blocks={}, table_blocks={}, orphan_captions={}",
                            page.page_idx + 1, candidates_on_page, img_blocks, tbl_blocks, orphan_caps.len()
                        ));
                    }

                    let raw_count = candidates.len();
                    // Per-page column boundaries derived from text intervals.
                    // Empty Vec = single-column page (geometric heuristic in
                    // propagate_captions will then NOT penalize cross-x links
                    // because Some(&[]) is treated as "single column, no
                    // boundary").
                    let page_columns: std::collections::HashMap<i32, Vec<f32>> =
                        page_text_intervals
                            .iter()
                            .map(|(p, intervals)| {
                                let boundaries =
                                    crate::mineru::postprocess::detect_column_boundaries(intervals);
                                if !boundaries.is_empty() {
                                    pp_info(&format!(
                                        "[mineru] page {} column boundaries: {:?}",
                                        p + 1,
                                        boundaries
                                    ));
                                }
                                (*p, boundaries)
                            })
                            .collect();
                    // Stage 2: merge adjacent/overlapping bboxes and filter noise
                    let mut processed = post_process_blocks(
                        candidates,
                        split_figures,
                        &page_text_edges,
                        &page_columns,
                        paper_id,
                    );
                    // Absorb spatially-adjacent title-typed para_blocks into
                    // each captioned figure's bbox so that panel labels (e.g.
                    // "Audio-Visual Comprehension" above the topmost sub-panel
                    // of Figure 4 in 2605.04045) appear inside the cropped
                    // hires image.
                    if !title_hints.is_empty() {
                        absorb_titles_into_figures(&mut processed, &title_hints);
                    }
                    // Absorb text/list blocks that sit directly above a figure
                    // body and contain panel labels (e.g. "(a) standard prompting"
                    // above Figure 5 in 2201.11903) or stack vertically as a
                    // figure legend (e.g. three short text lines "Standard
                    // prompting" / "Chain-of-thought prompting" / "- - Prior
                    // supervised best" above Figure 4 in 2201.11903 — MinerU
                    // emits one block per line when icons interrupt the inline
                    // flow).  MinerU emits these as separate para_blocks rather
                    // than sub-blocks of the figure.
                    //
                    // We walk bottom-up from each captioned image/chart's
                    // body_top with a `top_cursor`, accepting blocks under two
                    // rules:
                    //   - Panel label / list legend (gap ≤ 5pt): the original
                    //     behavior used by existing fixtures (Figure 5 here,
                    //     plus 2604.20587, 2605.04045).
                    //   - Legend-line stack (gap ≤ 12pt per step, short
                    //     height ≤ 12pt, narrow width ≤ 0.7 × body_width,
                    //     fully inside body's x-range, no caption number):
                    //     captures stacked single-line text legends.  The
                    //     cursor advances after each absorption so subsequent
                    //     stacked lines remain reachable.
                    for page in &doc.pdf_info {
                        for c in processed.iter_mut().filter(|c| {
                            c.caption_number.is_some()
                                && c.page_idx == page.page_idx
                                && (c.block_type == "image" || c.block_type == "chart")
                        }) {
                            let body_top = c.body_bbox[1].min(c.body_bbox[3]);
                            let body_left = c.body_bbox[0].min(c.body_bbox[2]);
                            let body_right = c.body_bbox[0].max(c.body_bbox[2]);
                            let body_width = (body_right - body_left).abs().max(1.0);

                            // Collect candidate text/list blocks above the body,
                            // sorted by bottom-Y descending (closest first) so
                            // the cursor walks contiguously upward.
                            let mut above: Vec<&crate::mineru::types::LayoutParaBlock> = page
                                .para_blocks
                                .iter()
                                .filter(|b| {
                                    (b.block_type == "text" || b.block_type == "list")
                                        && b.bbox.len() >= 4
                                        && b.bbox[1].max(b.bbox[3]) < body_top
                                })
                                .collect();
                            above.sort_by(|a, b| {
                                let ba = a.bbox[1].max(a.bbox[3]);
                                let bb = b.bbox[1].max(b.bbox[3]);
                                bb.partial_cmp(&ba).unwrap_or(std::cmp::Ordering::Equal)
                            });

                            let mut top_cursor = body_top;
                            for block in above {
                                let block_top = block.bbox[1].min(block.bbox[3]);
                                let block_bottom = block.bbox[1].max(block.bbox[3]);
                                let block_left = block.bbox[0].min(block.bbox[2]);
                                let block_right = block.bbox[0].max(block.bbox[2]);
                                let block_width = (block_right - block_left).abs().max(1.0);
                                let block_height = (block_bottom - block_top).abs();
                                if block_bottom >= top_cursor {
                                    continue;
                                }
                                let gap = top_cursor - block_bottom;
                                let overlap = (block_right.min(body_right)
                                    - block_left.max(body_left))
                                .max(0.0);
                                let overlap_ratio = overlap / block_width.min(body_width);
                                if overlap_ratio < 0.5 {
                                    continue;
                                }
                                let block_text: String = block
                                    .all_subs()
                                    .iter()
                                    .flat_map(|sub| &sub.lines)
                                    .flat_map(|line| &line.spans)
                                    .filter_map(|span| span.content.as_ref())
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .join(" ");
                                // Branch 1: panel label / list legend
                                // (gap ≤ 5pt).  Lists are accepted unless
                                // numbered ("1. ...") — those are ordinary
                                // itemized lists, not figure legends.  Text
                                // blocks must carry a panel label "(a)/(b)/...".
                                let mut absorb = false;
                                if gap <= 5.0 {
                                    if block.block_type == "list" {
                                        let trimmed = block_text.trim_start();
                                        let first_word = trimmed.split_whitespace().next();
                                        let is_numbered = first_word.is_some_and(|w| {
                                            w.ends_with('.')
                                                && w[..w.len() - 1].parse::<u32>().is_ok()
                                        });
                                        absorb = !is_numbered;
                                    } else if has_sub_panel_label(&block_text) {
                                        absorb = true;
                                    }
                                }
                                // Branch 2: stacked legend line — short,
                                // narrow, fully inside the body's x-range,
                                // not a caption.  The gap ceiling (12pt) is
                                // larger than branch 1 to bridge the natural
                                // padding between the topmost legend row and
                                // the chart's plot area (e.g. ~9pt gap above
                                // Figure 4 in 2201.11903).
                                if !absorb
                                    && block.block_type == "text"
                                    && gap <= 12.0
                                    && block_height <= 12.0
                                    && block_left >= body_left - 1.0
                                    && block_right <= body_right + 1.0
                                    && block_width <= 0.7 * body_width
                                    && extract_caption_number(&block_text).is_none()
                                {
                                    absorb = true;
                                }
                                if !absorb {
                                    continue;
                                }
                                c.bbox = union_bbox(
                                    c.bbox,
                                    [block_left, block_top, block_right, block_bottom],
                                );
                                top_cursor = block_top;
                                pp_info(&format!(
                                    "[mineru] Absorbed panel/legend block into figure {:?} on page {}: bbox expanded to [{:.1},{:.1},{:.1},{:.1}] text='{}'",
                                    c.caption_number,
                                    page.page_idx + 1,
                                    c.bbox[0],
                                    c.bbox[1],
                                    c.bbox[2],
                                    c.bbox[3],
                                    &block_text[..block_text
                                        .char_indices()
                                        .nth(60)
                                        .map(|(i, _)| i)
                                        .unwrap_or(block_text.len())]
                                ));
                            }
                        }
                    }
                    // Absorb text blocks that sit directly to the LEFT or
                    // RIGHT of a figure body and overlap it vertically.
                    // MinerU sometimes splits a text-heavy figure (e.g. a
                    // diagram whose left half is rendered as text) into
                    // separate `text` and `image` blocks (2005.11401 Figure 2:
                    // left text blocks at x=108..285, image body at
                    // x=288..502, gap=3pt, full vertical overlap).
                    for page in &doc.pdf_info {
                        for c in processed.iter_mut().filter(|c| {
                            c.caption_number.is_some()
                                && c.page_idx == page.page_idx
                                && (c.block_type == "image" || c.block_type == "chart")
                        }) {
                            let body_top = c.body_bbox[1].min(c.body_bbox[3]);
                            let body_bottom = c.body_bbox[1].max(c.body_bbox[3]);
                            let body_left = c.body_bbox[0].min(c.body_bbox[2]);
                            let body_right = c.body_bbox[0].max(c.body_bbox[2]);
                            let body_height = (body_bottom - body_top).abs().max(1.0);

                            for block in &page.para_blocks {
                                if block.block_type != "text" || block.bbox.len() < 4 {
                                    continue;
                                }
                                let block_top = block.bbox[1].min(block.bbox[3]);
                                let block_bottom = block.bbox[1].max(block.bbox[3]);
                                let block_left = block.bbox[0].min(block.bbox[2]);
                                let block_right = block.bbox[0].max(block.bbox[2]);
                                let block_height = (block_bottom - block_top).abs();

                                // Strong vertical overlap (the text must be
                                // aligned with the body, not just nearby).
                                let v_overlap = (block_bottom.min(body_bottom)
                                    - block_top.max(body_top))
                                .max(0.0);
                                if v_overlap / body_height < 0.25 || v_overlap / block_height < 0.5
                                {
                                    continue;
                                }

                                // Must be immediately adjacent horizontally
                                // (left or right), with a tiny gap.
                                let h_gap = if block_right <= body_left {
                                    body_left - block_right
                                } else if block_left >= body_right {
                                    block_left - body_right
                                } else {
                                    continue;
                                };
                                if h_gap > 5.0 {
                                    continue;
                                }

                                // Must be short — not a full paragraph.
                                if block_height > 60.0 {
                                    continue;
                                }

                                let block_text: String = block
                                    .all_subs()
                                    .iter()
                                    .flat_map(|sub| &sub.lines)
                                    .flat_map(|line| &line.spans)
                                    .filter_map(|span| span.content.as_ref())
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .join(" ");
                                if extract_caption_number(&block_text).is_some() {
                                    continue;
                                }

                                c.bbox = union_bbox(
                                    c.bbox,
                                    [block_left, block_top, block_right, block_bottom],
                                );
                                c.body_bbox = union_bbox(
                                    c.body_bbox,
                                    [block_left, block_top, block_right, block_bottom],
                                );
                                pp_info(&format!(
                                    "[mineru] Absorbed side text block into figure {:?} on page {}: bbox=[{:.1},{:.1},{:.1},{:.1}] text='{}'",
                                    c.caption_number,
                                    page.page_idx + 1,
                                    block_left, block_top, block_right, block_bottom,
                                    &block_text[..block_text.char_indices().nth(60).map(|(i, _)| i).unwrap_or(block_text.len())]
                                ));
                            }
                        }
                    }
                    // Page-header signature set.  MinerU classifies recurring
                    // page headers (e.g. "James Pan, Guoliang Li" / "A Survey
                    // of LLM Inference Systems" alternating across pages of
                    // 2506.21901, or "DRAW: A Recurrent Neural Network ..."
                    // on every page of 1502.04623) AND figure context boxes
                    // (e.g. the "Problem: ..." yellow box of 2605.10889) into
                    // the discarded list.  Different papers tag these
                    // differently (`type=header` vs `type=discarded`) so we
                    // can't rely on the tag — instead we use **recurrence**:
                    // a discarded block whose (rough y-position, text) pair
                    // appears on ≥ 2 pages is a real page header that must
                    // NOT be absorbed into a figure crop.  One-off discarded
                    // text (page-specific footnotes, single-page context
                    // boxes) is fair game.
                    let extract_discarded_text =
                        |d: &crate::mineru::types::LayoutParaBlock| -> String {
                            d.lines
                                .iter()
                                .flat_map(|l| &l.spans)
                                .filter_map(|s| s.content.as_ref())
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(" ")
                                .trim()
                                .to_string()
                        };
                    let page_header_keys: std::collections::HashSet<String> = {
                        let mut counts: std::collections::HashMap<String, usize> =
                            std::collections::HashMap::new();
                        for page in &doc.pdf_info {
                            for d in &page.discarded_blocks {
                                if d.bbox.len() < 4 {
                                    continue;
                                }
                                let y_top = d.bbox[1].min(d.bbox[3]);
                                let text = extract_discarded_text(d);
                                if text.is_empty() {
                                    continue;
                                }
                                let key = format!("{}|{}", (y_top / 10.0).round() as i32, text);
                                *counts.entry(key).or_default() += 1;
                            }
                        }
                        counts
                            .into_iter()
                            .filter_map(|(k, n)| if n >= 2 { Some(k) } else { None })
                            .collect()
                    };
                    if !page_header_keys.is_empty() {
                        pp_info(&format!(
                            "[mineru] Detected {} recurring page-header signature(s): {:?}",
                            page_header_keys.len(),
                            page_header_keys.iter().take(3).collect::<Vec<_>>()
                        ));
                    }
                    // Top snap-up for image/chart figures.  MinerU's image_body
                    // bbox sometimes slices through thin label rows that the
                    // vision model treated as image content but did not include
                    // in the body envelope (e.g. column headers "VGG-19" /
                    // "34-layer plain" / "34-layer residual" above Figure 3 in
                    // 1512.03385).  It also sometimes mis-classifies actual
                    // figure content as a "header" discarded block — e.g. the
                    // "Problem: ..." yellow box above Figure 1 in 2605.10889
                    // that lives in `discarded_blocks` instead of `para_blocks`.
                    // We push body_top up to:
                    //   - the top of the topmost substantive discarded text
                    //     contiguously above the figure body (if any), or
                    //   - up to 10pt into empty space (the original behavior).
                    // We never cross:
                    //   - another para_block (text/title/list/...) that
                    //     horizontally overlaps the image's column, or
                    //   - another captioned candidate's final bbox, or
                    //   - a recurring page-header signature, or
                    //   - the y=50 page header margin.
                    let mut expansions: Vec<(usize, f32, f32)> = Vec::new();
                    for page in &doc.pdf_info {
                        for (idx, c) in processed.iter().enumerate() {
                            if c.caption_number.is_none()
                                || c.page_idx != page.page_idx
                                || (c.block_type != "image" && c.block_type != "chart")
                            {
                                continue;
                            }
                            let body_top = c.body_bbox[1].min(c.body_bbox[3]);
                            let body_left = c.body_bbox[0].min(c.body_bbox[2]);
                            let body_right = c.body_bbox[0].max(c.body_bbox[2]);
                            // Does this page carry a recurring page-header?
                            let page_has_header = page
                                .discarded_blocks
                                .iter()
                                .filter(|d| d.bbox.len() >= 4)
                                .any(|d| {
                                    let dy_top = d.bbox[1].min(d.bbox[3]);
                                    let text = extract_discarded_text(d);
                                    if text.is_empty() {
                                        return false;
                                    }
                                    let key =
                                        format!("{}|{}", (dy_top / 10.0).round() as i32, text);
                                    page_header_keys.contains(&key)
                                });
                            let mut obstacle_bottom: f32 = 0.0;
                            for other in &page.para_blocks {
                                if other.bbox.len() < 4 {
                                    continue;
                                }
                                let ox1 = other.bbox[0].min(other.bbox[2]);
                                let ox2 = other.bbox[0].max(other.bbox[2]);
                                let oy1 = other.bbox[1].min(other.bbox[3]);
                                let oy2 = other.bbox[1].max(other.bbox[3]);
                                if oy1 >= body_top {
                                    continue;
                                }
                                let h_overlap = ox2.min(body_right) - ox1.max(body_left);
                                if h_overlap <= 0.0 {
                                    continue;
                                }
                                if oy2 > obstacle_bottom {
                                    obstacle_bottom = oy2;
                                }
                            }
                            for (j, other_c) in processed.iter().enumerate() {
                                if j == idx || other_c.page_idx != page.page_idx {
                                    continue;
                                }
                                let ox1 = other_c.bbox[0].min(other_c.bbox[2]);
                                let ox2 = other_c.bbox[0].max(other_c.bbox[2]);
                                let oy1 = other_c.bbox[1].min(other_c.bbox[3]);
                                let oy2 = other_c.bbox[1].max(other_c.bbox[3]);
                                if oy1 >= body_top {
                                    continue;
                                }
                                let h_overlap = ox2.min(body_right) - ox1.max(body_left);
                                if h_overlap <= 0.0 {
                                    continue;
                                }
                                if oy2 > obstacle_bottom {
                                    obstacle_bottom = oy2;
                                }
                            }
                            let free = body_top - obstacle_bottom;
                            // Walk discarded blocks above body_top, sorted by
                            // bottom-Y descending (closest to body first).
                            // Include each block if it has substantive text,
                            // sits at least 50% inside the body's horizontal
                            // column, and chains within 15pt of the previous
                            // cursor.  The result is `discarded_top`: the top
                            // of the furthest chained discarded block.
                            let mut above_discarded: Vec<(f32, f32, usize)> = page
                                .discarded_blocks
                                .iter()
                                .filter_map(|d| {
                                    if d.bbox.len() < 4 {
                                        return None;
                                    }
                                    let dx1 = d.bbox[0].min(d.bbox[2]);
                                    let dx2 = d.bbox[0].max(d.bbox[2]);
                                    let dy1 = d.bbox[1].min(d.bbox[3]);
                                    let dy2 = d.bbox[1].max(d.bbox[3]);
                                    if dy2 >= body_top || dy1 <= obstacle_bottom {
                                        return None;
                                    }
                                    let dw = (dx2 - dx1).abs().max(1.0);
                                    let h_ovr = (dx2.min(body_right) - dx1.max(body_left)).max(0.0);
                                    if h_ovr / dw < 0.5 {
                                        return None;
                                    }
                                    let text: String = d
                                        .lines
                                        .iter()
                                        .flat_map(|l| &l.spans)
                                        .filter_map(|s| s.content.as_ref())
                                        .cloned()
                                        .collect::<Vec<_>>()
                                        .join(" ");
                                    let trimmed = text.trim();
                                    let len = trimmed.chars().count();
                                    if len < MIN_DISCARDED_TEXT_CHARS {
                                        return None;
                                    }
                                    // Skip recurring page-header blocks.
                                    let header_key =
                                        format!("{}|{}", (dy1 / 10.0).round() as i32, trimmed);
                                    if page_header_keys.contains(&header_key) {
                                        return None;
                                    }
                                    Some((dy1, dy2, len))
                                })
                                .collect();
                            above_discarded.sort_by(|a, b| {
                                b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                            });
                            let mut cursor = body_top;
                            let mut discarded_top: Option<f32> = None;
                            for (dy1, dy2, _) in &above_discarded {
                                let gap = cursor - dy2;
                                if gap > MAX_DISCARDED_STEP_GAP {
                                    break;
                                }
                                if *dy1 <= obstacle_bottom {
                                    break;
                                }
                                discarded_top = Some(*dy1);
                                cursor = *dy1;
                            }
                            let new_top = if let Some(d_top) = discarded_top {
                                // Capture discarded content.  Cap at the
                                // page-header margin and never cross
                                // obstacle_bottom.
                                d_top.max(MIN_TOP_FROM_PAGE).max(obstacle_bottom + 1.0)
                            } else if free >= MIN_FREE_FOR_EXPANSION && !page_has_header {
                                // Default 10pt expansion into empty space —
                                // suppressed when the page carries a real
                                // recurring header, because the gap between
                                // header text and figure body often contains
                                // a decorative horizontal rule (e.g. LNCS
                                // page-header rule on 2506.21901) that we
                                // don't want to bleed into the crop.
                                (body_top - MAX_EXPANSION).max(MIN_TOP_FROM_PAGE)
                            } else {
                                continue;
                            };
                            if new_top >= body_top {
                                continue;
                            }
                            expansions.push((idx, body_top, new_top));
                        }
                    }
                    for (idx, body_top_orig, new_top) in expansions {
                        let c = &mut processed[idx];
                        if c.body_bbox[1] <= c.body_bbox[3] {
                            if c.body_bbox[1] > new_top {
                                c.body_bbox[1] = new_top;
                            }
                        } else if c.body_bbox[3] > new_top {
                            c.body_bbox[3] = new_top;
                        }
                        if c.bbox[1] <= c.bbox[3] {
                            if c.bbox[1] > new_top {
                                c.bbox[1] = new_top;
                            }
                        } else if c.bbox[3] > new_top {
                            c.bbox[3] = new_top;
                        }
                        pp_info(&format!(
                            "[mineru] Top snap-up for figure {:?} on page {}: body_top {:.1} -> {:.1}",
                            c.caption_number,
                            c.page_idx + 1,
                            body_top_orig,
                            new_top
                        ));
                    }
                    swap_mismatched_captions(&mut processed);
                    // Second-pass orphan rebind: after merging sub-panels and
                    // swapping mismatched captions, merged candidates are wider
                    // and can match full-width orphan captions that failed the
                    // first pass (e.g. 2408.15998 Figure 6/7, 1607.06450).
                    if !all_orphan_caps.is_empty() {
                        let before_second = processed
                            .iter()
                            .filter(|c| {
                                c.desc.starts_with("image on page")
                                    || c.desc.starts_with("table on page")
                                    || c.desc.starts_with("chart on page")
                            })
                            .count();
                        rebind_orphan_captions(&mut processed, &all_orphan_caps, split_figures);
                        let after_second = processed
                            .iter()
                            .filter(|c| {
                                c.desc.starts_with("image on page")
                                    || c.desc.starts_with("table on page")
                                    || c.desc.starts_with("chart on page")
                            })
                            .count();
                        if after_second < before_second {
                            pp_info(&format!(
                                "[mineru] Second-pass orphan rebind: {} placeholder(s) resolved ({} -> {})",
                                before_second - after_second, before_second, after_second
                            ));
                        }
                    }
                    pp_info(&format!(
                        "[mineru] Post-process: {} raw -> {} final ({} filtered/merged)",
                        raw_count,
                        processed.len(),
                        raw_count.saturating_sub(processed.len())
                    ));

                    // Stage 3: read images for processed candidates
                    for c in processed {
                        let mut img_file = extract_dir.join(&c.img_path);
                        if !img_file.exists() {
                            img_file = extract_dir.join("images").join(&c.img_path);
                        }
                        if img_file.exists() {
                            let bytes = tokio::fs::read(&img_file).await?;
                            images.push((c.desc, bytes));
                            bboxes.push(ImageBboxInfo {
                                bbox: c.bbox,
                                page_idx: c.page_idx,
                                content_type: c.block_type.clone(),
                            });
                            body_bboxes.push(ImageBboxInfo {
                                bbox: c.body_bbox,
                                page_idx: c.page_idx,
                                content_type: c.block_type,
                            });
                        } else {
                            pp_warn(&format!(
                                "[mineru] Image file not found: {}",
                                img_file.display()
                            ));
                        }
                    }

                    layout_doc = Some(doc);
                }
                Err(e) => {
                    pp_warn(&format!(
                        "[mineru] Failed to parse layout.json: {}, will try v2 fallback",
                        e
                    ));
                }
            }
        }

        // Fallback: scan images directory directly if layout.json was not parsed
        if images.is_empty() {
            if let Some(ref img_dir) = images_dir {
                for entry in walkdir::WalkDir::new(img_dir).max_depth(1) {
                    let entry = entry?;
                    let path = entry.path();
                    if path.is_file() {
                        let bytes = tokio::fs::read(path).await?;
                        let name = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("figure");
                        images.push((name.to_string(), bytes));
                        bboxes.push(ImageBboxInfo {
                            bbox: [0.0, 0.0, 0.0, 0.0],
                            page_idx: 0,
                            content_type: "image".to_string(),
                        });
                        body_bboxes.push(ImageBboxInfo {
                            bbox: [0.0, 0.0, 0.0, 0.0],
                            page_idx: 0,
                            content_type: "image".to_string(),
                        });
                    }
                }
            }
        }

        // Sort by caption number: Figure before Table, then by number.
        // Items without a caption number keep their original relative order
        // and are placed at the end.
        let mut combined: Vec<_> = images
            .into_iter()
            .zip(bboxes.into_iter())
            .zip(body_bboxes.into_iter())
            .map(|(((desc, bytes), bbox), body)| (desc, bytes, bbox, body))
            .collect();
        combined.sort_by(|(desc_a, _, bbox_a, _), (desc_b, _, bbox_b, _)| {
            fn key(desc: &str, page_idx: i32) -> (u8, u32, i32) {
                let cap = extract_caption_number(desc);
                let (kind, num) = match cap {
                    Some(ref c) if c.starts_with("F:") => (0u8, c[2..].parse().unwrap_or(u32::MAX)),
                    Some(ref c) if c.starts_with("T:") => (1u8, c[2..].parse().unwrap_or(u32::MAX)),
                    _ => (2u8, u32::MAX),
                };
                (kind, num, page_idx)
            }
            key(desc_a, bbox_a.page_idx).cmp(&key(desc_b, bbox_b.page_idx))
        });
        let mut images = Vec::with_capacity(combined.len());
        let mut bboxes = Vec::with_capacity(combined.len());
        let mut body_bboxes = Vec::with_capacity(combined.len());
        for (desc, bytes, bbox, body) in combined {
            images.push((desc, bytes));
            bboxes.push(bbox);
            body_bboxes.push(body);
        }

        let extracted = ExtractedFigures {
            images,
            markdown,
            image_bboxes: bboxes,
            body_bboxes,
            layout_doc,
        };

        Ok(extracted)
    }
}
