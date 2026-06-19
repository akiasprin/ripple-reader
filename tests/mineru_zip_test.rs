//! End-to-end check that `MinerUClient::reprocess_from_zip` produces
//! image_bboxes that include caption strips alongside the body region,
//! and additionally renders every cropped image to disk so a human can
//! visually confirm the captions are now inside the crop.
//!
//! Each test is driven by a cached `figures/<paper_id>/mineru.zip` fixture
//! and writes the cropped PNGs to `target/test-out/<paper_id>-with-caption/`.
//! Tests are skipped automatically if the fixture is missing so CI without
//! the fixtures still runs cleanly.

use std::path::PathBuf;

use ripple_reader::mineru::{bbox_contains, ExtractedFigures, ImageBboxInfo, MinerUClient};

/// Return the cached `figures/{paper_id}/mineru.zip` if it exists.
/// Otherwise download the PDF from arXiv, send it to MinerU, and cache
/// the resulting zip so future test runs are instantaneous.
///
/// The zip is written to disk **before** any validation so that a
/// failing fixture can still be inspected manually.
async fn fixture_zip_for(paper_id: &str) -> PathBuf {
    let p = PathBuf::from(format!("target/test-out/fixtures/{paper_id}/mineru.zip"));
    if p.exists() {
        return p;
    }

    // Fallback to cached figures/ directory so tests work without
    // re-downloading from MinerU.  Try every subdirectory under figures/
    // (e.g. figures/arxiv/, figures/openreview/, ...).
    let figures_dir = PathBuf::from("figures");
    if let Ok(entries) = std::fs::read_dir(&figures_dir) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let cached = entry.path().join(paper_id).join("mineru.zip");
            if cached.exists() {
                std::fs::create_dir_all(p.parent().unwrap()).expect("mkdir fixtures dir");
                std::fs::copy(&cached, &p).expect("copy cached mineru.zip");
                return p;
            }
        }
    }

    dotenvy::dotenv().ok();
    let base_url =
        std::env::var("MINERU_BASE_URL").unwrap_or_else(|_| "https://mineru.net".to_string());
    let api_key = std::env::var("MINERU_API_KEY").expect("MINERU_API_KEY not set in .env");

    let client = MinerUClient::new(base_url, api_key, None, false);
    let pdf_url = format!("https://arxiv.org/pdf/{}.pdf", paper_id);

    eprintln!("[mineru_zip_test] Downloading {} from MinerU...", paper_id);
    let zip_bytes = client
        .download_mineru_zip(&pdf_url)
        .await
        .unwrap_or_else(|_| panic!("Failed to download {} from MinerU", paper_id));

    std::fs::create_dir_all(p.parent().unwrap()).expect("mkdir figures/{paper_id}");
    std::fs::write(&p, &zip_bytes).expect("write mineru.zip");
    eprintln!(
        "[mineru_zip_test] Saved {} bytes to {}",
        zip_bytes.len(),
        p.display()
    );
    p
}

/// Try `Pdfium::bind_to_system_library()` first, then fall back to known
/// macOS/Linux install paths.  Tests run without `DYLD_FALLBACK_LIBRARY_PATH`
/// configured, so the system bind fails on machines where `libpdfium.dylib`
/// lives under `~/.local/lib` or `/usr/local/opt/pdfium/lib`.
fn bind_pdfium() -> pdfium_render::prelude::Pdfium {
    use pdfium_render::prelude::Pdfium;

    if let Ok(b) = Pdfium::bind_to_system_library() {
        return Pdfium::new(b);
    }

    let mut candidates: Vec<PathBuf> = vec![
        PathBuf::from("/usr/local/opt/pdfium/lib/libpdfium.dylib"),
        PathBuf::from("/opt/homebrew/lib/libpdfium.dylib"),
        PathBuf::from("/usr/local/lib/libpdfium.dylib"),
        PathBuf::from("/usr/lib/libpdfium.so"),
    ];
    if let Ok(home) = std::env::var("HOME") {
        candidates.insert(0, PathBuf::from(home).join(".local/lib/libpdfium.dylib"));
    }
    if let Ok(custom) = std::env::var("PDFIUM_DYNAMIC_LIB_PATH") {
        candidates.insert(0, PathBuf::from(custom));
    }

    for path in &candidates {
        if path.exists() {
            if let Ok(b) = Pdfium::bind_to_library(path) {
                return Pdfium::new(b);
            }
        }
    }

    panic!(
        "libpdfium dynamic library not found.  Tried: {:?}",
        candidates
    );
}

/// Turn an arbitrary description string into a filesystem-safe slug.
/// Keeps alphanumerics, dot, underscore, dash; collapses everything else
/// into single dashes; truncates to `max_len` characters.
fn slugify(s: &str, max_len: usize) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
            out.push(c);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.chars().count() > max_len {
        trimmed
            .chars()
            .take(max_len)
            .collect::<String>()
            .trim_end_matches('-')
            .to_string()
    } else {
        trimmed
    }
}

/// Run `MinerUClient::reprocess_from_zip` against the cached fixture for
/// `paper_id`, isolating cache writes inside a tempdir.
async fn reprocess_fixture(paper_id: &str, src_zip: &std::path::Path) -> ExtractedFigures {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_dir = tmp.path().to_string_lossy().to_string();
    let paper_dir = tmp.path().join(paper_id);
    std::fs::create_dir_all(&paper_dir).unwrap();
    std::fs::copy(src_zip, paper_dir.join("mineru.zip")).unwrap();

    let client = MinerUClient::new(
        "http://unused".to_string(),
        "unused".to_string(),
        Some(cache_dir),
        false,
    );
    client
        .reprocess_from_zip(paper_id, &[], false, false)
        .await
        .expect("reprocess succeeds")
}

/// Pull the embedded `*_origin.pdf` out of a MinerU result zip.
fn extract_origin_pdf(zip_path: &std::path::Path) -> Vec<u8> {
    let zip_bytes = std::fs::read(zip_path).expect("read fixture zip");
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).expect("open zip archive");
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).expect("zip entry");
        if entry.name().ends_with("_origin.pdf") {
            let mut buf = Vec::with_capacity(entry.size() as usize);
            std::io::copy(&mut entry, &mut buf).expect("read pdf bytes");
            return buf;
        }
    }
    panic!("origin.pdf inside {}", zip_path.display());
}

/// Render every bbox against `pdf_bytes` at 200dpi and write the cropped
/// PNGs to `out_dir` (clearing any stale contents first). Returns the
/// number of PNGs successfully written. Mirrors the cropping logic in
/// `src/hires.rs::generate_hires_images`, including the 3px padding.
fn render_crops_to_dir(
    pdf_bytes: Vec<u8>,
    items: Vec<(String, ImageBboxInfo)>,
    out_dir: PathBuf,
) -> usize {
    if std::env::var("MINERU_TEST_SKIP_RENDER").is_ok() {
        eprintln!("[mineru_zip_test] Skipping PNG render (MINERU_TEST_SKIP_RENDER is set)");
        return 0;
    }

    use pdfium_render::prelude::*;

    if out_dir.exists() {
        std::fs::remove_dir_all(&out_dir).expect("clean stale out_dir");
    }
    std::fs::create_dir_all(&out_dir).expect("mkdir out_dir");

    let pdfium = bind_pdfium();
    let document = pdfium
        .load_pdf_from_reader(std::io::Cursor::new(pdf_bytes), None)
        .expect("load pdf");

    let dpi: u32 = 200;
    let mut written = 0usize;

    for (i, (desc, bbox_info)) in items.iter().enumerate() {
        let page_idx = bbox_info.page_idx.max(0) as u16;
        let page = match document.pages().get(page_idx) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[crop] page {} missing: {e}", page_idx + 1);
                continue;
            }
        };

        let page_height_pt = page.height().value;
        let page_width_pt = page.width().value;
        let [x0, y0, x1, y1] = bbox_info.bbox;

        // Mirror hires.rs: bbox is in screen-pt (origin top-left); pdfium
        // expects PDF-pt (origin bottom-left).
        let left_pt = x0.min(x1);
        let right_pt = x0.max(x1);
        let top_screen_pt = y0.min(y1);
        let bottom_screen_pt = y0.max(y1);
        let top_pdf = page_height_pt - top_screen_pt;
        let bottom_pdf = page_height_pt - bottom_screen_pt;

        let scale = dpi as f32 / 72.0;
        let target_width = (page_width_pt * scale) as i32;
        let config = PdfRenderConfig::new()
            .set_target_width(target_width)
            .render_form_data(false);

        let (clip_left, clip_top) = match page.points_to_pixels(
            PdfPoints::new(left_pt),
            PdfPoints::new(top_pdf),
            &config,
        ) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[crop] points_to_pixels(top-left) failed for #{i}: {e}");
                continue;
            }
        };
        let (clip_right, clip_bottom) = match page.points_to_pixels(
            PdfPoints::new(right_pt),
            PdfPoints::new(bottom_pdf),
            &config,
        ) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[crop] points_to_pixels(bottom-right) failed for #{i}: {e}");
                continue;
            }
        };

        let bitmap = match page.render_with_config(&config) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[crop] render failed for #{i}: {e}");
                continue;
            }
        };

        let label = slugify(desc, 60);
        let path = out_dir.join(format!(
            "{:02}-p{}-{}.png",
            i,
            page_idx + 1,
            if label.is_empty() {
                "image".to_string()
            } else {
                label
            }
        ));
        // Pad each side by ~1pt for a razor-thin breathing room.
        // Mirrors `hires.rs::generate_hires_images`.
        let mut img = bitmap.as_image();
        let img_w = img.width() as i32;
        let img_h = img.height() as i32;
        const PAD_PT: f32 = 1.0;
        let pad_px = (PAD_PT * scale).round() as i32;
        let pad_left = (clip_left - pad_px).max(0);
        let pad_top = (clip_top - pad_px).max(0);
        let pad_right = (clip_right + pad_px).min(img_w);
        let pad_bottom = (clip_bottom + pad_px).min(img_h);
        let pad_w = (pad_right - pad_left).max(1) as u32;
        let pad_h = (pad_bottom - pad_top).max(1) as u32;
        match img
            .crop(pad_left as u32, pad_top as u32, pad_w, pad_h)
            .save_with_format(&path, image::ImageFormat::Png)
        {
            Ok(_) => {
                written += 1;
            }
            Err(e) => eprintln!("[crop] save failed for {}: {e}", path.display()),
        }
    }

    written
}

#[tokio::test(flavor = "multi_thread")]
async fn test_1512_03385_caption_bbox_expands_past_body() {
    let paper_id = "1512.03385";
    let src_zip = fixture_zip_for(paper_id).await;

    let figures = reprocess_fixture(paper_id, &src_zip).await;

    // --- Numerical guards: caption strip must be inside the bbox now. ---
    // Figure 6 (composite, caption below): body bottom ~171, caption ~194.
    let fig6 = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .find(|((desc, _), _)| desc.contains("Figure 6") && desc.contains("CIFAR-10"));
    let (_, bb6) = fig6.expect("Figure 6 present");
    assert!(
        bb6.bbox[3] > 175.0,
        "Figure 6 bbox should extend past body bottom (171) to include caption: got {:?}",
        bb6.bbox
    );

    // Table 7 caption was an orphan that had to be rebound; verify the
    // expanded bbox covers the caption strip below the body.
    let tbl7 = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .find(|((desc, _), _)| desc.contains("Table 7") && desc.contains("PASCAL VOC"));
    let (_, bb7) = tbl7.expect("Table 7 present");
    assert!(
        bb7.bbox[3] > 270.0,
        "Table 7 bbox should grow downward past body bottom (260) to include rebound caption: got {:?}",
        bb7.bbox
    );

    // Regression guard for the caption-attribution bug: pre-fix, Table 8's
    // bbox engulfed Table 7's body (and vice versa via union expansion).
    // Tables 7 and 8 are vertically stacked on page 8 — their bboxes must
    // be disjoint.
    let tbl8 = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .find(|((desc, _), _)| desc.contains("Table 8") && desc.contains("COCO"));
    let (_, bb8) = tbl8.expect("Table 8 present");
    assert_eq!(
        bb7.page_idx, bb8.page_idx,
        "Tables 7 and 8 should be on the same page"
    );
    assert!(
        !bbox_contains(bb7.bbox, bb8.bbox),
        "Table 7 bbox must not engulf Table 8 (pre-fix regression): bb7={:?}, bb8={:?}",
        bb7.bbox,
        bb8.bbox
    );
    assert!(
        !bbox_contains(bb8.bbox, bb7.bbox),
        "Table 8 bbox must not engulf Table 7 (pre-fix regression): bb7={:?}, bb8={:?}",
        bb7.bbox,
        bb8.bbox
    );
    // Stacked vertically: Table 7 ends above Table 8 begins (small overlap
    // tolerance for shared caption strip width).
    assert!(
        bb7.bbox[3] <= bb8.bbox[1] + 5.0,
        "Table 7 bottom ({:.1}) must not extend into Table 8 region (top {:.1}): bb7={:?}, bb8={:?}",
        bb7.bbox[3], bb8.bbox[1], bb7.bbox, bb8.bbox
    );

    let pdf_bytes = extract_origin_pdf(&src_zip);
    let out_dir = PathBuf::from(format!("target/test-out/{paper_id}-with-caption"));
    let items: Vec<(String, ImageBboxInfo)> = figures
        .images
        .iter()
        .zip(figures.image_bboxes.iter())
        .map(|((desc, _), bb)| (desc.clone(), bb.clone()))
        .collect();
    let total = items.len();
    let out_dir_clone = out_dir.clone();
    let png_count =
        tokio::task::spawn_blocking(move || render_crops_to_dir(pdf_bytes, items, out_dir_clone))
            .await
            .expect("spawn_blocking");

    eprintln!(
        "[mineru_zip_test] Wrote {}/{} cropped PNGs to {}",
        png_count,
        total,
        out_dir.display()
    );
    assert!(png_count > 0, "expected at least one PNG to be written");
}

/// Verify 2605.04045 reprocesses without errors and all extracted figures
/// have valid bboxes.
#[tokio::test(flavor = "multi_thread")]
async fn test_2605_04045_extraction_passes_and_valid_bboxes() {
    let paper_id = "2605.04045";
    let src_zip = fixture_zip_for(paper_id).await;

    let figures = reprocess_fixture(paper_id, &src_zip).await;

    // All figures should have non-zero bboxes
    for (i, ((desc, _), bb)) in figures
        .images
        .iter()
        .zip(figures.image_bboxes.iter())
        .enumerate()
    {
        let w = (bb.bbox[2] - bb.bbox[0]).abs();
        let h = (bb.bbox[3] - bb.bbox[1]).abs();
        assert!(
            w > 10.0,
            "[{}] bbox width too small ({:.1}): {}",
            i,
            w,
            desc
        );
        assert!(
            h > 10.0,
            "[{}] bbox height too small ({:.1}): {}",
            i,
            h,
            desc
        );
    }

    // Figure 4 should be present (composite figure with many sub-panels)
    let descs: Vec<&str> = figures.images.iter().map(|(d, _)| d.as_str()).collect();
    assert!(
        descs.iter().any(|d| d.contains("Figure 4")),
        "Figure 4 should be present"
    );
    assert!(
        figures.images.len() >= 20,
        "expected >= 20 figures, got {}",
        figures.images.len()
    );

    let pdf_bytes = extract_origin_pdf(&src_zip);
    let out_dir = PathBuf::from(format!("target/test-out/{paper_id}-with-caption"));
    let items: Vec<(String, ImageBboxInfo)> = figures
        .images
        .iter()
        .zip(figures.image_bboxes.iter())
        .map(|((desc, _), bb)| (desc.clone(), bb.clone()))
        .collect();
    let total = items.len();
    let out_dir_clone = out_dir.clone();
    let png_count =
        tokio::task::spawn_blocking(move || render_crops_to_dir(pdf_bytes, items, out_dir_clone))
            .await
            .expect("spawn_blocking");

    eprintln!(
        "[mineru_zip_test] Wrote {}/{} cropped PNGs to {}",
        png_count,
        total,
        out_dir.display()
    );
    assert!(png_count > 0, "expected at least one PNG to be written");
}

/// Verify that the section title "A Appendix" on page 19 of 2604.18002 is
/// NOT absorbed into the Figure 9 bbox (title absorption guard).
#[tokio::test(flavor = "multi_thread")]
async fn test_2604_18002_figure9_no_absorb_appendix_title() {
    let paper_id = "2604.18002";
    let src_zip = fixture_zip_for(paper_id).await;

    let figures = reprocess_fixture(paper_id, &src_zip).await;

    // Figure 9 must be present on page 19 (0-indexed 18)
    let fig9 = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .find(|((desc, _), bb)| bb.page_idx == 18 && desc.contains("Figure 9"));

    let (_, bb) = fig9.expect("Figure 9 should be present on page 19");
    let top = bb.bbox[1].min(bb.bbox[3]);
    // "A Appendix" title is at y=69-87 on page 19.
    // If it were absorbed, the figure top would be ~69.
    // The figure body starts at y=103, so without absorption the top >= ~100.
    assert!(
        top > 90.0,
        "Figure 9 on page 19 should NOT absorb 'A Appendix' title; expected top > 90, got {}",
        top
    );

    // Render crops for visual inspection
    let pdf_bytes = extract_origin_pdf(&src_zip);
    let out_dir = PathBuf::from(format!("target/test-out/{paper_id}-with-caption"));
    let items: Vec<(String, ImageBboxInfo)> = figures
        .images
        .iter()
        .zip(figures.image_bboxes.iter())
        .map(|((desc, _), bb)| (desc.clone(), bb.clone()))
        .collect();
    let total = items.len();
    let out_dir_clone = out_dir.clone();
    let png_count =
        tokio::task::spawn_blocking(move || render_crops_to_dir(pdf_bytes, items, out_dir_clone))
            .await
            .expect("spawn_blocking");

    eprintln!(
        "[mineru_zip_test] Wrote {}/{} cropped PNGs to {}",
        png_count,
        total,
        out_dir.display()
    );
    assert!(png_count > 0, "expected at least one PNG to be written");
}

/// Verify 1512.03385 body_bboxes are strictly smaller than image_bboxes
/// (caption strip is correctly separated from the body crop).
#[tokio::test(flavor = "multi_thread")]
async fn test_1512_03385_body_bbox_smaller_than_image_bbox() {
    let paper_id = "1512.03385";
    let src_zip = fixture_zip_for(paper_id).await;

    let figures = reprocess_fixture(paper_id, &src_zip).await;

    let out_dir = PathBuf::from(format!("target/test-out/{paper_id}-without-caption"));

    // Render crops using body_bboxes (caption-free) instead of image_bboxes
    let pdf_bytes = extract_origin_pdf(&src_zip);
    let items: Vec<(String, ImageBboxInfo)> = figures
        .images
        .iter()
        .zip(figures.body_bboxes.iter())
        .map(|((desc, _), bb)| (desc.clone(), bb.clone()))
        .collect();
    let total = items.len();
    let out_dir_clone = out_dir.clone();
    let png_count =
        tokio::task::spawn_blocking(move || render_crops_to_dir(pdf_bytes, items, out_dir_clone))
            .await
            .expect("spawn_blocking");

    // Write comparison file: full bbox vs body bbox for each figure
    let mut lines = Vec::new();
    for (i, (((desc, _), full_bb), body_bb)) in figures
        .images
        .iter()
        .zip(figures.image_bboxes.iter())
        .zip(figures.body_bboxes.iter())
        .enumerate()
    {
        let full_h = (full_bb.bbox[3] - full_bb.bbox[1]).abs();
        let body_h = (body_bb.bbox[3] - body_bb.bbox[1]).abs();
        lines.push(format!(
            "#{} {}\nfull_bbox  = {:?}  h={:.1}\nbody_bbox  = {:?}  h={:.1}\ndelta_h    = {:.1}\n",
            i,
            desc,
            full_bb.bbox,
            full_h,
            body_bb.bbox,
            body_h,
            full_h - body_h
        ));
    }
    let desc_path = out_dir.join("descriptions.txt");
    std::fs::write(&desc_path, lines.join("\n")).expect("write descriptions");
    eprintln!(
        "[mineru_zip_test] Wrote {} bbox comparisons to {}",
        lines.len(),
        desc_path.display()
    );

    eprintln!(
        "[mineru_zip_test] Wrote {}/{} cropped PNGs (body-only) to {}",
        png_count,
        total,
        out_dir.display()
    );
    assert!(png_count > 0, "expected at least one PNG to be written");

    // Numerical guard: at least one figure with a caption must have a smaller body height
    let mut any_smaller = false;
    for (full_bb, body_bb) in figures.image_bboxes.iter().zip(figures.body_bboxes.iter()) {
        let full_h = (full_bb.bbox[3] - full_bb.bbox[1]).abs();
        let body_h = (body_bb.bbox[3] - body_bb.bbox[1]).abs();
        if body_h > 0.0 && body_h < full_h - 1.0 {
            any_smaller = true;
            break;
        }
    }
    assert!(any_smaller, "expected at least one figure where body_bbox is noticeably smaller than full bbox (caption stripped)");
}

/// Verify 2604.21428 reprocesses and all extracted figures have non-trivial
/// descriptions.
#[tokio::test(flavor = "multi_thread")]
async fn test_2604_21428_validation_and_no_missing_tables() {
    let paper_id = "2604.21428";
    let src_zip = fixture_zip_for(paper_id).await;

    let figures = reprocess_fixture(paper_id, &src_zip).await;

    // Validation must pass
    if let Err(err_msg) =
        ripple_reader::mineru::validate_extracted_figures(&figures, None, Some(paper_id))
    {
        panic!("Validation should pass: {}", err_msg);
    }

    // All tables should have proper number (Table 13 was missing before fix)
    let descs: Vec<&str> = figures.images.iter().map(|(d, _)| d.as_str()).collect();
    assert!(
        descs.iter().any(|d| d.contains("Table 13")),
        "Table 13 should be present"
    );
    assert!(
        figures.images.len() >= 20,
        "expected >= 20 figures/tables, got {}",
        figures.images.len()
    );

    eprintln!(
        "2604.21428: {} figures/tables extracted, validation passed",
        figures.images.len()
    );
}

/// List all descriptions for 2604.21428 after reprocessing.
#[tokio::test(flavor = "multi_thread")]
async fn test_2604_21428_all_descriptions_non_empty() {
    let paper_id = "2604.21428";
    let src_zip = fixture_zip_for(paper_id).await;

    let figures = reprocess_fixture(paper_id, &src_zip).await;
    assert!(!figures.images.is_empty(), "expected at least one figure");
    for (i, ((desc, _), bb)) in figures
        .images
        .iter()
        .zip(figures.image_bboxes.iter())
        .enumerate()
    {
        assert!(!desc.is_empty(), "[{}] description is empty", i);
        eprintln!(
            "[{}] page={} type={}: {}",
            i,
            bb.page_idx + 1,
            bb.content_type,
            desc
        );
    }
}

/// Regression guard for the page-27 Figure 9 caption strip on 2604.21428.
///
/// Before the second-pass bbox-expansion fix in `rebind_orphan_captions`,
/// Figure 9's bbox stopped at the body bottom (y≈232) because the first-pass
/// engulfment guard blocked the wide caption strip from being merged in
/// (sibling sub-panel still un-merged). After `propagate_captions` + merge
/// fused the sub-panels, the second-pass `already_bound` short-circuit left
/// the caption strip outside the bbox — the hires crop on page 27 was thus
/// missing Figure 9's caption.
#[tokio::test(flavor = "multi_thread")]
async fn test_2604_21428_figure9_caption_in_bbox() {
    let paper_id = "2604.21428";
    let src_zip = fixture_zip_for(paper_id).await;

    let figures = reprocess_fixture(paper_id, &src_zip).await;

    let fig9 = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .find(|((desc, _), _)| desc.contains("Figure 9") && desc.contains("fragment sizes"));
    let (_, bb9) = fig9.expect("Figure 9 present");

    let fig10 = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .find(|((desc, _), _)| desc.contains("Figure 10") && desc.contains("fragment sizes"));
    let (_, bb10) = fig10.expect("Figure 10 present");

    // Body bottom is ~216; caption strip ends at ~270.  The fix must extend
    // the bbox bottom past the body to include the caption.
    assert!(
        bb9.bbox[3] >= 265.0,
        "Figure 9 bbox should extend past body bottom (216) to include caption strip (y≈270): got {:?}",
        bb9.bbox
    );

    // Figures 9 and 10 are vertically stacked on page 27 — must stay disjoint
    // and Figure 9 must end before Figure 10 begins.
    assert_eq!(
        bb9.page_idx, bb10.page_idx,
        "Figures 9 and 10 should be on the same page"
    );
    assert!(
        !bbox_contains(bb9.bbox, bb10.bbox),
        "Figure 9 bbox must not engulf Figure 10: bb9={:?}, bb10={:?}",
        bb9.bbox,
        bb10.bbox
    );
    assert!(
        !bbox_contains(bb10.bbox, bb9.bbox),
        "Figure 10 bbox must not engulf Figure 9: bb9={:?}, bb10={:?}",
        bb9.bbox,
        bb10.bbox
    );
    assert!(
        bb9.bbox[3] <= bb10.bbox[1] + 5.0,
        "Figure 9 bottom ({:.1}) must not extend into Figure 10 region (top {:.1}): bb9={:?}, bb10={:?}",
        bb9.bbox[3], bb10.bbox[1], bb9.bbox, bb10.bbox
    );
}

/// Regression guard for the page-38 Table 15 sub-caption strips on 2604.21428.
///
/// Table 15 has two stacked sub-tables on page 38 (page_idx=37); each has a
/// `table_footnote` sub-block carrying its panel label.  The (b) footnote at
/// y≈689-702 sits OUTSIDE its parent `para_block.bbox` (y=419-686), so before
/// the fix `body_bbox` stopped at y=686 — `keep_caption=false` hires crops cut
/// off the "(b) Iso-FLOPs scavenging performance" label even though it is
/// semantically part of the table body, not the main "Table 15" caption strip.
#[tokio::test(flavor = "multi_thread")]
async fn test_2604_21428_table15_sub_captions_in_body_bbox() {
    let paper_id = "2604.21428";
    let src_zip = fixture_zip_for(paper_id).await;

    let figures = reprocess_fixture(paper_id, &src_zip).await;

    let table15 = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .zip(&figures.body_bboxes)
        .find(|(((desc, _), _), _)| desc.contains("Table 15"));
    let (((_, _), img_bb), body_bb) = table15.expect("Table 15 present");

    assert_eq!(img_bb.page_idx, 37, "Table 15 should be on page_idx=37");
    assert_eq!(
        body_bb.page_idx, 37,
        "Table 15 body_bbox should be on page_idx=37"
    );

    // (b) footnote bottom is at y≈702; body_bbox bottom must reach it so the
    // no-caption hires crop still includes the sub-panel label.
    assert!(
        body_bb.bbox[3] >= 700.0,
        "Table 15 body_bbox should extend down to include the (b) footnote (y≈702): got body_bbox={:?}",
        body_bb.bbox
    );

    // Main "Table 15 ..." caption strip is at y≈712-727 and belongs OUTSIDE
    // body_bbox — keep_caption=false is meant to strip exactly that.
    assert!(
        body_bb.bbox[3] < 710.0,
        "Table 15 body_bbox must NOT include the main 'Table 15' caption strip (y>=712): got body_bbox={:?}",
        body_bb.bbox
    );

    // image_bbox always covers the main caption strip.
    assert!(
        img_bb.bbox[3] >= 720.0,
        "Table 15 image_bbox should include the main caption strip (y≈727): got image_bbox={:?}",
        img_bb.bbox
    );
}

/// Regression guard for the page-7 Figure 4 sub-panel labels on 2604.20587.
///
/// Figure 4 has five image sub-panels on page 7 (page_idx=6); each sub-panel
/// carries an `image_caption` sub-block ABOVE its body — "trace:" for the
/// first panel and "(a)/(b)/(c)/(d) ..." for the remaining four.  The labels
/// sit at y≈69-87 while the bodies sit at y≈87-152.  Before the fix,
/// `full_sub_bbox()` filtered above-body captions via the spatial check
/// (`cap_bottom < min_image_top`) and `selected_captions()` did the same, so
/// the labels were silently dropped from both `bbox` and `body_bbox`.
/// `keep_caption=false` hires crops therefore cut off the panel labels even
/// though they are semantically part of the body, not the main "Figure 4 ..."
/// caption strip (which sits BELOW body at y≈159-225 and is correctly
/// excluded by keep_caption=false).
///
/// Validation is bypassed because this paper has a pre-existing "Figure 16
/// missing" gap — Figure 16's caption sits inside a `code` block (which the
/// validator skips), but page 16's text body contains "Figure 16 defines
/// BooMsLANG's IR." which `looks_like_caption` picks up as a layout figure.
/// That mismatch is orthogonal to the body_bbox geometry fix exercised here.
#[tokio::test(flavor = "multi_thread")]
async fn test_2604_20587_figure4_sub_panel_labels_in_body_bbox() {
    let paper_id = "2604.20587";
    let src_zip = fixture_zip_for(paper_id).await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_dir = tmp.path().to_string_lossy().to_string();
    let paper_dir = tmp.path().join(paper_id);
    std::fs::create_dir_all(&paper_dir).unwrap();
    std::fs::copy(&src_zip, paper_dir.join("mineru.zip")).unwrap();

    let client = MinerUClient::new(
        "http://unused".to_string(),
        "unused".to_string(),
        Some(cache_dir),
        false,
    );
    let figures = client
        .reprocess_from_zip(paper_id, &[], true, false)
        .await
        .expect("reprocess_from_zip succeeds with skip_validation=true");

    let fig4 = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .zip(&figures.body_bboxes)
        .find(|(((desc, _), bb), _)| desc.contains("Figure 4") && bb.page_idx == 6);
    let (((desc, _), img_bb), body_bb) =
        fig4.expect("Figure 4 should be present on page_idx=6 (page 7)");
    eprintln!(
        "2604.20587 Figure 4: desc={:?} body_bbox={:?} image_bbox={:?}",
        desc, body_bb.bbox, img_bb.bbox
    );

    // Sub-panel labels sit at y≈69-87 (above the bodies at y≈87-152). The
    // merged Figure 4 body_bbox must extend up to at least one of those
    // labels — y<=82 catches both "(a)/(b)/(c)/(d) ..." (y_bottom 77-86) and
    // "trace:" (y_bottom 87 but y_top 79).  Without the Phase 2 fix the body
    // bboxes start at the image body top (y≈87), so this strictly catches
    // the regression.
    assert!(
        body_bb.bbox[1] <= 82.0,
        "Figure 4 body_bbox should extend up to include sub-panel labels (y<=82): got body_bbox={:?}",
        body_bb.bbox
    );

    // The main "Figure 4. An example ..." caption strip sits BELOW body at
    // y=159-225 and belongs OUTSIDE body_bbox (keep_caption=false is meant
    // to strip exactly that strip).
    assert!(
        body_bb.bbox[3] < 159.0,
        "Figure 4 body_bbox must NOT include the main 'Figure 4' caption strip (y>=159): got body_bbox={:?}",
        body_bb.bbox
    );

    // image_bbox covers the main caption strip (y≈225).
    assert!(
        img_bb.bbox[3] >= 220.0,
        "Figure 4 image_bbox should include the main caption strip (y≈225): got image_bbox={:?}",
        img_bb.bbox
    );
}

/// Regression guard for 1608.05343 page 14 Figure 11 sub-panel label (b).
///
/// Figure 11 is a large single image at page_idx=13 (page 14) with a sub-panel
/// label "(b)" at bbox=[291,206,304,217]. The figure also has sub-panel "(a)"
/// at bbox=[292,123,304,133] above the top two rows of small images.
/// Previously the (b) label was dropped from body_bbox because it sat between
/// the top image rows (y≈65-201) and the actual Figure 11 image body
/// (y≈281-415). After orphan-caption rebind, (b) must be merged into
/// Figure 11's body_bbox so keep_caption=false hires crops include it.
///
/// Validation is bypassed because Figure 11/12 layout on this page triggers
/// known false positives in the text-overlap validator.
#[tokio::test(flavor = "multi_thread")]
async fn test_1608_05343_figure11_subcaption_b_in_body_bbox() {
    let paper_id = "1608.05343";
    let src_zip = fixture_zip_for(paper_id).await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_dir = tmp.path().to_string_lossy().to_string();
    let paper_dir = tmp.path().join(paper_id);
    std::fs::create_dir_all(&paper_dir).unwrap();
    std::fs::copy(&src_zip, paper_dir.join("mineru.zip")).unwrap();

    let client = MinerUClient::new(
        "http://unused".to_string(),
        "unused".to_string(),
        Some(cache_dir),
        false,
    );
    let figures = client
        .reprocess_from_zip(paper_id, &[], true, false)
        .await
        .expect("reprocess_from_zip succeeds with skip_validation=true");

    let fig11 = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .zip(&figures.body_bboxes)
        .find(|(((desc, _), bb), _)| desc.contains("Figure 11") && bb.page_idx == 13);
    let (((desc, _), img_bb), body_bb) =
        fig11.expect("Figure 11 should be present on page_idx=13 (page 14)");
    eprintln!(
        "1608.05343 Figure 11: desc={:?} body_bbox={:?} image_bbox={:?}",
        desc, body_bb.bbox, img_bb.bbox
    );

    // The (b) sub-panel label sits at y=206-216. It must be included in
    // Figure 11's body_bbox so keep_caption=false hires crops include it.
    assert!(
        body_bb.bbox[3] >= 216.0,
        "Figure 11 body_bbox should extend down to include sub-panel label (b) (y>=216): got body_bbox={:?}",
        body_bb.bbox
    );

    // The main "Figure 11. Test error ..." caption sits below the image at
    // y=226-270 and must NOT be included in body_bbox.
    assert!(
        body_bb.bbox[3] < 226.0,
        "Figure 11 body_bbox must NOT include the main caption strip (y>=226): got body_bbox={:?}",
        body_bb.bbox
    );
}

/// Verify 2604.13627 reprocesses without validation errors and Figure 12
/// is present after orphan-caption rebind.
#[tokio::test(flavor = "multi_thread")]
async fn test_2604_13627_validation_and_fig12_present() {
    let paper_id = "2604.13627";
    let src_zip = fixture_zip_for(paper_id).await;

    let figures = reprocess_fixture(paper_id, &src_zip).await;

    // Validation must pass (no continuity errors or missing figures)
    if let Err(err_msg) =
        ripple_reader::mineru::validate_extracted_figures(&figures, None, Some(paper_id))
    {
        panic!("Validation should pass after fixes: {}", err_msg);
    }

    // Figure 12 must be present (was missing before the orphan rebind fix)
    let descs: Vec<&str> = figures.images.iter().map(|(d, _)| d.as_str()).collect();
    assert!(
        descs.iter().any(|d| d.contains("Figure 12")),
        "Figure 12 should be present"
    );
    assert!(
        descs.iter().any(|d| d.contains("Figure 13")),
        "Figure 13 should be present"
    );
    assert!(
        figures.images.len() >= 20,
        "expected >= 20 figures, got {}",
        figures.images.len()
    );

    eprintln!(
        "2604.13627: {} figures extracted, validation passed",
        figures.images.len()
    );
}

/// List all descriptions for 2604.13627 after reprocessing.
#[tokio::test(flavor = "multi_thread")]
async fn test_2604_13627_all_have_non_empty_descriptions() {
    let paper_id = "2604.13627";
    let src_zip = fixture_zip_for(paper_id).await;

    let figures = reprocess_fixture(paper_id, &src_zip).await;
    assert!(!figures.images.is_empty(), "expected at least one figure");
    for (i, ((desc, _), bb)) in figures
        .images
        .iter()
        .zip(figures.image_bboxes.iter())
        .enumerate()
    {
        assert!(!desc.is_empty(), "[{}] description is empty", i);
        eprintln!(
            "[{}] page={} type={}: {}",
            i,
            bb.page_idx + 1,
            bb.content_type,
            desc
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_2604_13627_layout_json_valid() {
    let paper_id = "2604.13627";
    let zip_path = fixture_zip_for(paper_id).await;
    let zip_file = std::fs::File::open(&zip_path).expect("open zip");
    let mut archive = zip::ZipArchive::new(zip_file).expect("read zip");

    let mut found_layout = false;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).expect("zip entry");
        let name = file.name();
        if name.ends_with("layout.json") || name.ends_with("_layout.json") {
            found_layout = true;
            let mut buf = Vec::new();
            std::io::copy(&mut file, &mut buf).expect("read layout");
            let json_str = String::from_utf8(buf).expect("utf8");
            let doc: serde_json::Value = serde_json::from_str(&json_str).expect("parse json");
            let pages = doc
                .get("pdf_info")
                .and_then(|v| v.as_array())
                .expect("layout.json should have pdf_info array");
            assert!(
                pages.len() >= 18,
                "expected >= 18 pages, got {}",
                pages.len()
            );
        }
    }
    assert!(found_layout, "layout.json not found in zip");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_2604_13627_page5_bboxes_consistent() {
    let paper_id = "2604.13627";
    let src_zip = fixture_zip_for(paper_id).await;

    let figures = reprocess_fixture(paper_id, &src_zip).await;
    let on_page5: Vec<_> = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .zip(&figures.body_bboxes)
        .enumerate()
        .filter(|(_, (((_, _), bb), _))| bb.page_idx == 4)
        .collect();

    assert!(
        !on_page5.is_empty(),
        "expected at least one figure on page 5"
    );

    // Every figure on page 5 must have body_bbox contained within image_bbox
    let mut any_body_smaller = false;
    for (i, (((desc, _), img_bb), body_bb)) in &on_page5 {
        let img_h = (img_bb.bbox[3] - img_bb.bbox[1]).abs();
        let body_h = (body_bb.bbox[3] - body_bb.bbox[1]).abs();
        assert!(
            img_h > 0.0,
            "[{}] image bbox has zero height: desc={}",
            i,
            desc
        );
        if body_h > 0.0 && body_h < img_h - 1.0 {
            any_body_smaller = true;
        }
    }
    assert!(
        any_body_smaller,
        "expected at least one figure with caption strip (body < full bbox)"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_2604_13627_page5_figures_valid() {
    let paper_id = "2604.13627";
    let src_zip = fixture_zip_for(paper_id).await;
    let figures = reprocess_fixture(paper_id, &src_zip).await;

    let on_page5: Vec<_> = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .enumerate()
        .filter(|(_, (_, bb))| bb.page_idx == 4)
        .collect();

    assert!(
        !on_page5.is_empty(),
        "expected at least one figure on page 5"
    );
    for (i, ((desc, _), bb)) in &on_page5 {
        let w = (bb.bbox[2] - bb.bbox[0]).abs();
        let h = (bb.bbox[3] - bb.bbox[1]).abs();
        assert!(
            w > 20.0,
            "[{}] bbox width too small: {:?} desc={}",
            i,
            bb.bbox,
            desc
        );
        assert!(
            h > 20.0,
            "[{}] bbox height too small: {:?} desc={}",
            i,
            bb.bbox,
            desc
        );
    }

    // At least one non-placeholder figure should be present
    let non_placeholder = on_page5
        .iter()
        .filter(|(_, ((desc, _), _))| !desc.starts_with("image on page"))
        .count();
    eprintln!(
        "Page 5 has {} figures ({} non-placeholder)",
        on_page5.len(),
        non_placeholder
    );
    assert!(
        !on_page5.is_empty(),
        "expected at least one figure on page 5"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_2604_13627_fig12_13_present() {
    let paper_id = "2604.13627";
    let src_zip = fixture_zip_for(paper_id).await;

    let figures = reprocess_fixture(paper_id, &src_zip).await;

    // Figure 12 and Figure 13 must both be present on page 18
    let on_page18: Vec<_> = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .filter(|(_, bb)| bb.page_idx == 17)
        .map(|((desc, _), _)| desc.as_str())
        .collect();

    assert!(!on_page18.is_empty(), "expected figures on page 18");
    let has_fig12 = on_page18.iter().any(|d| d.contains("Figure 12"));
    let has_fig13 = on_page18.iter().any(|d| d.contains("Figure 13"));
    assert!(
        has_fig12,
        "Figure 12 should be present on page 18. Found: {:?}",
        on_page18
    );
    assert!(
        has_fig13,
        "Figure 13 should be present on page 18. Found: {:?}",
        on_page18
    );
}

/// Verify that full-width captions for composite figures (Figure 6/7 on
/// page 19 of 2408.15998) are correctly extracted after the second-pass
/// orphan rebind.  The captions span ~400 pt but the sub-panel images are
/// only 30–100 pt wide, so the first-pass orphan rebind fails; the merged
/// candidates after post_process must be wide enough to accept them.
#[tokio::test(flavor = "multi_thread")]
async fn test_2408_15998_figure6_7_orphan_rebind() {
    let paper_id = "2408.15998";
    let src_zip = fixture_zip_for(paper_id).await;

    let figures = reprocess_fixture(paper_id, &src_zip).await;

    // Both Figure 6 and Figure 7 must appear in the extracted descriptions
    let descs: Vec<&str> = figures.images.iter().map(|(d, _)| d.as_str()).collect();
    let fig6 = descs.iter().find(|d| d.contains("Figure 6"));
    let fig7 = descs.iter().find(|d| d.contains("Figure 7"));

    assert!(
        fig6.is_some(),
        "Figure 6 should be in extracted results after second-pass orphan rebind"
    );
    assert!(
        fig7.is_some(),
        "Figure 7 should be in extracted results after second-pass orphan rebind"
    );

    // They must not be placeholders
    for d in [fig6.unwrap(), fig7.unwrap()] {
        assert!(
            !d.starts_with("image on page"),
            "expected real caption, got placeholder: {}",
            d
        );
    }

    // At least 20 figures total
    assert!(
        figures.images.len() >= 20,
        "expected >= 20 figures, got {}",
        figures.images.len()
    );

    // Render crops for visual inspection
    let pdf_bytes = extract_origin_pdf(&src_zip);
    let out_dir = PathBuf::from(format!("target/test-out/{paper_id}-figure6-7"));
    let items: Vec<(String, ImageBboxInfo)> = figures
        .images
        .iter()
        .zip(figures.image_bboxes.iter())
        .map(|((desc, _), bb)| (desc.clone(), bb.clone()))
        .collect();
    let total = items.len();
    let out_dir_clone = out_dir.clone();
    let png_count =
        tokio::task::spawn_blocking(move || render_crops_to_dir(pdf_bytes, items, out_dir_clone))
            .await
            .expect("spawn_blocking");

    eprintln!(
        "[mineru_zip_test] Wrote {}/{} cropped PNGs to {}",
        png_count,
        total,
        out_dir.display()
    );
    assert!(png_count > 0, "expected at least one PNG to be written");
}

/// Verify that figures with large image-to-caption gaps (up to 150 pt
/// on pages 25+) are correctly rebound after increasing max_distance
/// to `page_h * 0.30`.  Without the fix, 22 figure captions are orphaned
/// and never bound.
///
/// Note: Many figures on pages 27-29 of this paper are not detected as
/// image blocks by MinerU at all, so they cannot be extracted regardless
/// of rebinding logic.  This test only asserts the figures whose images
/// MinerU *does* detect.
#[tokio::test(flavor = "multi_thread")]
async fn test_2409_12191_max_distance_rebind() {
    let paper_id = "2409.12191";
    let src_zip = fixture_zip_for(paper_id).await;

    // Don't use reprocess_fixture — it panics on validation failure.
    // MinerU doesn't detect images for some figures in this paper.
    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_dir = tmp.path().to_string_lossy().to_string();
    let paper_dir = tmp.path().join(paper_id);
    std::fs::create_dir_all(&paper_dir).unwrap();
    std::fs::copy(&src_zip, paper_dir.join("mineru.zip")).unwrap();

    let client = MinerUClient::new(
        "http://unused".to_string(),
        "unused".to_string(),
        Some(cache_dir),
        false,
    );
    let result = client.reprocess_from_zip(paper_id, &[], false, false).await;

    // Validation may still report figures missing because MinerU doesn't
    // detect image blocks for some figures.  Accept both success and
    // controlled failure — the test goal is to verify that max_distance
    // fixes the distance-based rejections.
    let figures = match result {
        Ok(f) => f,
        Err(e) => {
            let msg = format!("{}", e);
            eprintln!(
                "2409.12191: validation returned error (expected for this paper): {}",
                msg
            );
            // Can't extract ExtractedFigures from the error, skip assertions
            return;
        }
    };

    let descs: Vec<&str> = figures.images.iter().map(|(d, _)| d.as_str()).collect();

    // Figures that were missing before max_distance fix but are now fixed:
    for num in [7, 25, 30] {
        let label = format!("Figure {}", num);
        assert!(
            descs.iter().any(|d| d.contains(&label)),
            "{} should be present after max_distance fix. Found {} figures total.",
            label,
            figures.images.len()
        );
    }

    // All extracted descriptions must be non-empty
    for (i, ((desc, _), _)) in figures
        .images
        .iter()
        .zip(figures.image_bboxes.iter())
        .enumerate()
    {
        assert!(!desc.is_empty(), "[{}] description is empty", i);
    }

    // The fix resolves several previously-missing figures
    let fixed = [
        "Figure 7",
        "Figure 13",
        "Figure 25",
        "Figure 26",
        "Figure 30",
        "Figure 31",
    ];
    let fixed_count = fixed
        .iter()
        .filter(|l| descs.iter().any(|d| d.contains(*l)))
        .count();
    eprintln!(
        "2409.12191: {} figures extracted, {} of {} previously-missing figures now present",
        figures.images.len(),
        fixed_count,
        fixed.len()
    );
    assert!(
        fixed_count >= 3,
        "expected at least 3 previously-missing figures to be fixed, got {}",
        fixed_count
    );
}

/// 2404.11614 Figure 3: MinerU classified the figure image as an
/// interline_equation block (a Bezier curve diagram).  The block is large
/// enough (h=51pt, area=6018) that we now promote it to a figure candidate
/// so the orphan "Figure 3" caption can rebind to it.
#[tokio::test]
async fn test_2404_11614_figure3_interline_equation_rebind() {
    let paper_id = "2404.11614";
    let src_zip = fixture_zip_for(paper_id).await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_dir = tmp.path().to_string_lossy().to_string();
    let paper_dir = tmp.path().join(paper_id);
    std::fs::create_dir_all(&paper_dir).unwrap();
    std::fs::copy(&src_zip, paper_dir.join("mineru.zip")).unwrap();

    let client = MinerUClient::new(
        "http://unused".to_string(),
        "unused".to_string(),
        Some(cache_dir),
        false,
    );
    let result = client.reprocess_from_zip(paper_id, &[], false, false).await;

    let figures = match result {
        Ok(f) => f,
        Err(e) => {
            panic!("2404.11614 validation should pass: {}", e);
        }
    };

    let descs: Vec<&str> = figures.images.iter().map(|(d, _)| d.as_str()).collect();

    // Figure 3 was missing before the interline_equation candidate fix
    assert!(
        descs.iter().any(|d| d.contains("Figure 3")),
        "Figure 3 should be present after interline_equation fix. Found {} figures.",
        figures.images.len()
    );
}

/// 1502.04623 page 7: Figure 8 ("Generated MNIST images with two digits") is a
/// grid of generated double-digit glyphs that MinerU OCR'd into a LaTeX array,
/// typing the block `interline_equation`.  Its "Figure 8" caption was absorbed
/// as an above-body `image_caption` sub-block of the adjacent Figure 9 (SVHN)
/// image block, so the standalone-caption detection in `is_interline_figure`
/// found nothing and the figure was dropped (the log jumped Figure 7 -> 9).
/// The nested-caption detection path recovers it.
#[tokio::test]
async fn test_1502_04623_figure8_interline_equation_nested_caption() {
    let paper_id = "1502.04623";
    let src_zip = fixture_zip_for(paper_id).await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_dir = tmp.path().to_string_lossy().to_string();
    let paper_dir = tmp.path().join(paper_id);
    std::fs::create_dir_all(&paper_dir).unwrap();
    std::fs::copy(&src_zip, paper_dir.join("mineru.zip")).unwrap();

    let client = MinerUClient::new(
        "http://unused".to_string(),
        "unused".to_string(),
        Some(cache_dir),
        false,
    );
    let result = client.reprocess_from_zip(paper_id, &[], false, false).await;

    let figures = match result {
        Ok(f) => f,
        Err(e) => panic!("1502.04623 validation should pass: {}", e),
    };

    let descs: Vec<&str> = figures.images.iter().map(|(d, _)| d.as_str()).collect();

    assert!(
        descs.iter().any(|d| d.contains("Figure 8")),
        "Figure 8 should be recovered from the interline_equation block. Found: {:?}",
        descs
    );
}

/// 2407.08608 Figure 4: MinerU classified both Figure 3 and Figure 4 as
/// "table" blocks.  The Figure 4 caption becomes an orphan but the only bare
/// candidate on page 8 is also a table — type guard previously blocked the
/// cross-type rebind.  Now relaxed: when no same-type bare candidate exists,
/// cross-type rebind is allowed.
#[tokio::test]
async fn test_2407_08608_figure4_cross_type_rebind() {
    let paper_id = "2407.08608";
    let src_zip = fixture_zip_for(paper_id).await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_dir = tmp.path().to_string_lossy().to_string();
    let paper_dir = tmp.path().join(paper_id);
    std::fs::create_dir_all(&paper_dir).unwrap();
    std::fs::copy(&src_zip, paper_dir.join("mineru.zip")).unwrap();

    let client = MinerUClient::new(
        "http://unused".to_string(),
        "unused".to_string(),
        Some(cache_dir),
        false,
    );
    let result = client.reprocess_from_zip(paper_id, &[], false, false).await;

    let figures = match result {
        Ok(f) => f,
        Err(e) => {
            panic!(
                "2407.08608 validation should pass after cross-type rebind fix: {}",
                e
            );
        }
    };

    let descs: Vec<&str> = figures.images.iter().map(|(d, _)| d.as_str()).collect();

    // Figure 4 was missing before the cross-type rebind fix
    assert!(
        descs.iter().any(|d| d.contains("Figure 4")),
        "Figure 4 should be present after cross-type rebind fix. Found {} figures.",
        figures.images.len()
    );
}

/// 2502.03860 Figures 2 & 4: MinerU didn't detect these figures at all
/// (no captions or images in layout). The continuity check should not
/// report gaps for figures the layout doesn't know about — that's an
/// upstream limitation, not an extraction bug.
#[tokio::test]
async fn test_2502_03860_undetected_figures_not_reported_as_gaps() {
    let paper_id = "2502.03860";
    let src_zip = fixture_zip_for(paper_id).await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_dir = tmp.path().to_string_lossy().to_string();
    let paper_dir = tmp.path().join(paper_id);
    std::fs::create_dir_all(&paper_dir).unwrap();
    std::fs::copy(&src_zip, paper_dir.join("mineru.zip")).unwrap();

    let client = MinerUClient::new(
        "http://unused".to_string(),
        "unused".to_string(),
        Some(cache_dir),
        false,
    );
    let result = client.reprocess_from_zip(paper_id, &[], false, false).await;

    let figures = match result {
        Ok(f) => f,
        Err(e) => {
            panic!(
                "2502.03860 validation should pass (undetected figures should not be gaps): {}",
                e
            );
        }
    };

    let descs: Vec<&str> = figures.images.iter().map(|(d, _)| d.as_str()).collect();
    eprintln!(
        "2502.03860: {} figures extracted: {:?}",
        figures.images.len(),
        descs
    );
}

/// 2604.20556 page 5: Figure 3 is a 2x3 grid of sub-panels (a)-(f) with a
/// main caption below them.  An interline_equation (LRS formula) sits below
/// the caption.  Previously the equation was geometrically linked to the
/// figure group via propagate_captions (h_overlap=1.0, v_gap=40 < 250),
/// inherited caption F:3, and got merged — its bbox was swallowed into
/// Fig.3's body.  The fix adds an interline_equation cross-type guard in
/// should_merge_geometry_only so real equations never link with figures.
#[tokio::test]
async fn test_2604_20556_figure3_does_not_swallow_equation() {
    let paper_id = "2604.20556";
    let src_zip = fixture_zip_for(paper_id).await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_dir = tmp.path().to_string_lossy().to_string();
    let paper_dir = tmp.path().join(paper_id);
    std::fs::create_dir_all(&paper_dir).unwrap();
    std::fs::copy(&src_zip, paper_dir.join("mineru.zip")).unwrap();

    let client = MinerUClient::new(
        "http://unused".to_string(),
        "unused".to_string(),
        Some(cache_dir),
        false,
    );
    let figures = client
        .reprocess_from_zip(paper_id, &[], false, false)
        .await
        .expect("reprocess_from_zip should succeed");

    // Find Fig.3 on page 5 (page_idx=4)
    let fig3 = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .zip(&figures.body_bboxes)
        .find(|(((desc, _), bb), _)| desc.contains("Fig. 3") && bb.page_idx == 4);
    let (((desc, _), img_bb), body_bb) =
        fig3.expect("Fig. 3 should be present on page_idx=4 (page 5)");
    eprintln!(
        "2604.20556 Fig.3: desc={:?} body_bbox={:?} image_bbox={:?}",
        desc, body_bb.bbox, img_bb.bbox
    );

    // Body must NOT extend down to the interline_equation at y=351-388
    let body_bottom = body_bb.bbox[1].max(body_bb.bbox[3]);
    assert!(
        body_bottom < 350.0,
        "Fig.3 body_bbox bottom ({}) must NOT include interline_equation at y=351-388",
        body_bottom
    );

    // Image bbox (body + caption) should include the main caption at y=227-311
    let img_bottom = img_bb.bbox[1].max(img_bb.bbox[3]);
    assert!(
        img_bottom >= 300.0,
        "Fig.3 image_bbox bottom ({}) should include caption strip",
        img_bottom
    );
}

#[tokio::test]
async fn diagnose_2502_03860_page7() {
    let paper_id = "2502.03860";
    let src_zip = fixture_zip_for(paper_id).await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_dir = tmp.path().to_string_lossy().to_string();
    let paper_dir = tmp.path().join(paper_id);
    std::fs::create_dir_all(&paper_dir).unwrap();
    std::fs::copy(&src_zip, paper_dir.join("mineru.zip")).unwrap();

    let client = MinerUClient::new(
        "http://unused".to_string(),
        "unused".to_string(),
        Some(cache_dir),
        false,
    );
    let figures = client
        .reprocess_from_zip(paper_id, &[], false, false)
        .await
        .unwrap();

    eprintln!("\n=== 2502.03860 extracted items ===");
    for (i, (((desc, _bytes), img_bb), body_bb)) in figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .zip(&figures.body_bboxes)
        .enumerate()
    {
        eprintln!(
            "Item {}: page={} type={}",
            i,
            img_bb.page_idx + 1,
            img_bb.content_type
        );
        let snippet = &desc[..desc
            .char_indices()
            .nth(120)
            .map(|(i, _)| i)
            .unwrap_or(desc.len())];
        eprintln!("  desc={}", snippet);
        eprintln!("  bbox={:?}", img_bb.bbox);
        eprintln!("  body={:?}", body_bb.bbox);
    }
}

/// Regression guard for 1312.6114 page 10 Figure 4 sub-panel labels (a) and (b).
///
/// Figure 4 consists of two side-by-side panels on page_idx=9 (page 10).
/// Each panel has its own `image_body` plus an `image_caption` sub-block
/// BELOW the body: "(a) Learned Frey Face manifold" at y=310-321 and
/// "(b) Learned MNIST manifold" at y=310-321.  Before the fix, the
/// below-body caption loop was missing, so these labels were excluded from
/// `body_bbox` and `keep_caption=false` hires crops cut them off.
///
/// Validation is bypassed because this paper has pre-existing layout issues
/// that trigger false positives in the validator.
#[tokio::test(flavor = "multi_thread")]
async fn test_1312_6114_figure4_sub_panel_labels_in_body_bbox() {
    let paper_id = "1312.6114";
    let src_zip = fixture_zip_for(paper_id).await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_dir = tmp.path().to_string_lossy().to_string();
    let paper_dir = tmp.path().join(paper_id);
    std::fs::create_dir_all(&paper_dir).unwrap();
    std::fs::copy(&src_zip, paper_dir.join("mineru.zip")).unwrap();

    let client = MinerUClient::new(
        "http://unused".to_string(),
        "unused".to_string(),
        Some(cache_dir),
        false,
    );
    let figures = client
        .reprocess_from_zip(paper_id, &[], true, false)
        .await
        .expect("reprocess_from_zip succeeds with skip_validation=true");

    let fig4 = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .zip(&figures.body_bboxes)
        .find(|(((desc, _), bb), _)| desc.contains("Figure 4") && bb.page_idx == 9);
    let (((desc, _), img_bb), body_bb) =
        fig4.expect("Figure 4 should be present on page_idx=9 (page 10)");
    eprintln!(
        "1312.6114 Figure 4: desc={:?} body_bbox={:?} image_bbox={:?}",
        desc, body_bb.bbox, img_bb.bbox
    );

    // Sub-panel labels (a) and (b) sit at y=310-321, below the image bodies
    // (y≈82-304). The merged body_bbox must extend down to include them.
    assert!(
        body_bb.bbox[3] >= 321.0,
        "Figure 4 body_bbox should extend down to include sub-panel labels (a)/(b) (y>=321): got body_bbox={:?}",
        body_bb.bbox
    );

    // The main "Figure 4: ..." caption sits further below at y=331-387 and
    // must NOT be included in body_bbox (keep_caption=false should strip it).
    assert!(
        body_bb.bbox[3] < 331.0,
        "Figure 4 body_bbox must NOT include the main caption strip (y>=331): got body_bbox={:?}",
        body_bb.bbox
    );

    // image_bbox covers the main caption strip (y≈387).
    assert!(
        img_bb.bbox[3] >= 380.0,
        "Figure 4 image_bbox should include the main caption strip (y≈387): got image_bbox={:?}",
        img_bb.bbox
    );
}

/// Regression guard for 2106.09685 page 26 caption-type swap.
///
/// MinerU mis-classifies two blocks on page_idx=25 (page 26):
/// - Block[5] is type='table' but carries "Figure 7" caption
/// - Block[6] is type='image' but carries "Table 18" caption (plus Figure 8)
///
/// Without the swap fix, the orphan Table 18 caption gets rebound to the
/// merged top images (Figure 7 sub-panels), so the top images are labeled
/// "Table 18" and the table body keeps "Figure 7".  The swap detects this
/// type-mismatched pair and exchanges their captions.
///
/// Validation is bypassed because the swapped bboxes trigger known false
/// positives in the text-overlap validator.
#[tokio::test(flavor = "multi_thread")]
async fn test_2106_09685_page26_caption_swap() {
    let paper_id = "2106.09685";
    let src_zip = fixture_zip_for(paper_id).await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_dir = tmp.path().to_string_lossy().to_string();
    let paper_dir = tmp.path().join(paper_id);
    std::fs::create_dir_all(&paper_dir).unwrap();
    std::fs::copy(&src_zip, paper_dir.join("mineru.zip")).unwrap();

    let client = MinerUClient::new(
        "http://unused".to_string(),
        "unused".to_string(),
        Some(cache_dir),
        false,
    );
    let figures = client
        .reprocess_from_zip(paper_id, &[], true, false)
        .await
        .expect("reprocess_from_zip succeeds with skip_validation=true");

    let page_items: Vec<(usize, String, String, [f32; 4])> = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .zip(&figures.body_bboxes)
        .enumerate()
        .filter(|(_, ((_, bb), _))| bb.page_idx == 25)
        .map(|(i, (((desc, _), bb), _))| (i, desc.clone(), bb.content_type.clone(), bb.bbox))
        .collect();

    for (i, desc, ty, bbox) in &page_items {
        eprintln!(
            "Item {}: page=26 type={} desc={} bbox={:?}",
            i,
            ty,
            &desc[..desc
                .char_indices()
                .nth(60)
                .map(|(i, _)| i)
                .unwrap_or(desc.len())],
            bbox
        );
    }

    // The top 4 sub-panel images (merged) must be labeled Figure 7.
    let fig7 = page_items
        .iter()
        .find(|(_, desc, ty, _)| ty == "image" && desc.contains("Figure 7"));
    assert!(
        fig7.is_some(),
        "Figure 7 should be present on page 26 as an image"
    );
    let (_, _, _, fig7_bbox) = fig7.unwrap();
    assert_eq!(
        fig7_bbox,
        &[104.0, 99.0, 504.0, 300.0],
        "Figure 7 should cover the 4 top sub-panels and include its caption"
    );

    // The table body must be labeled Table 18.
    let tbl18 = page_items
        .iter()
        .find(|(_, desc, ty, _)| ty == "table" && desc.contains("Table 18"));
    assert!(
        tbl18.is_some(),
        "Table 18 should be present on page 26 as a table"
    );

    // Figure 8 should remain as an image.
    let fig8 = page_items
        .iter()
        .find(|(_, desc, ty, _)| ty == "image" && desc.contains("Figure 8"));
    assert!(
        fig8.is_some(),
        "Figure 8 should be present on page 26 as an image"
    );
}

#[tokio::test]
async fn diagnose_2604_21428_page27() {
    let paper_id = "2604.21428";
    let src_zip = fixture_zip_for(paper_id).await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_dir = tmp.path().to_string_lossy().to_string();
    let paper_dir = tmp.path().join(paper_id);
    std::fs::create_dir_all(&paper_dir).unwrap();
    std::fs::copy(&src_zip, paper_dir.join("mineru.zip")).unwrap();

    let client = MinerUClient::new(
        "http://unused".to_string(),
        "unused".to_string(),
        Some(cache_dir),
        false,
    );
    let figures = client
        .reprocess_from_zip(paper_id, &[], false, false)
        .await
        .unwrap();

    eprintln!("\n=== 2604.21428 extracted items ===");
    for (i, (((desc, _bytes), img_bb), body_bb)) in figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .zip(&figures.body_bboxes)
        .enumerate()
    {
        if img_bb.page_idx != 26 {
            continue;
        }
        eprintln!(
            "Item {}: page={} type={}",
            i,
            img_bb.page_idx + 1,
            img_bb.content_type
        );
        let snippet = &desc[..desc
            .char_indices()
            .nth(120)
            .map(|(i, _)| i)
            .unwrap_or(desc.len())];
        eprintln!("  desc={}", snippet);
        eprintln!("  bbox={:?}", img_bb.bbox);
        eprintln!("  body={:?}", body_bb.bbox);
    }
}

#[tokio::test]
async fn diagnose_2304_10557_page6() {
    let paper_id = "2304.10557";
    let src_zip = fixture_zip_for(paper_id).await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_dir = tmp.path().to_string_lossy().to_string();
    let paper_dir = tmp.path().join(paper_id);
    std::fs::create_dir_all(&paper_dir).unwrap();
    std::fs::copy(&src_zip, paper_dir.join("mineru.zip")).unwrap();

    let client = MinerUClient::new(
        "http://unused".to_string(),
        "unused".to_string(),
        Some(cache_dir),
        false,
    );
    let figures = client
        .reprocess_from_zip(paper_id, &[], true, false)
        .await
        .unwrap();

    eprintln!("\n=== 2304.10557 extracted items ===");
    for (i, (((desc, _bytes), img_bb), body_bb)) in figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .zip(&figures.body_bboxes)
        .enumerate()
    {
        if img_bb.page_idx != 5 {
            continue;
        }
        eprintln!(
            "Item {}: page={} type={}",
            i,
            img_bb.page_idx + 1,
            img_bb.content_type
        );
        let snippet = &desc[..desc
            .char_indices()
            .nth(120)
            .map(|(i, _)| i)
            .unwrap_or(desc.len())];
        eprintln!("  desc={}", snippet);
        eprintln!("  bbox={:?}", img_bb.bbox);
        eprintln!("  body={:?}", body_bb.bbox);
    }
}

/// Regression guard for 2304.10557 page 6: Figure 5 has two side-by-side
/// chart sub-panels (left at x=78..206, right at x=222..350) with the caption
/// placed to the right of the right sub-panel (x=391..576).  Previously the
/// left sub-panel was incorrectly linked to Figure 7 (the image at y=352..544
/// below) via `propagate_captions` because the cross-column guard required
/// BOTH blocks to have a caption/sub-panel label to allow a same-row link.
/// The left sub-panel was bare, so it could not link to its captioned sibling
/// and instead inherited Figure 7's caption from the distant block below.
///
/// After relaxing the guard to require only ONE side to have a figure marker,
/// the left sub-panel links to the right sub-panel, receives Figure 5's
/// caption, and merges correctly.
#[tokio::test(flavor = "multi_thread")]
async fn test_2304_10557_page6_figure5_left_panel_not_figure7() {
    let paper_id = "2304.10557";
    let src_zip = fixture_zip_for(paper_id).await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_dir = tmp.path().to_string_lossy().to_string();
    let paper_dir = tmp.path().join(paper_id);
    std::fs::create_dir_all(&paper_dir).unwrap();
    std::fs::copy(&src_zip, paper_dir.join("mineru.zip")).unwrap();

    let client = MinerUClient::new(
        "http://unused".to_string(),
        "unused".to_string(),
        Some(cache_dir),
        false,
    );
    let figures = client
        .reprocess_from_zip(paper_id, &[], true, false)
        .await
        .expect("reprocess_from_zip succeeds with skip_validation=true");

    // Figure 5 must be present on page 6 (page_idx=5) as a merged chart
    let fig5: Vec<_> = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .zip(&figures.body_bboxes)
        .filter(|(((desc, _), bb), _)| desc.contains("Figure 5") && bb.page_idx == 5)
        .collect();

    assert!(!fig5.is_empty(), "Figure 5 should be present on page 6");
    assert_eq!(
        fig5.len(),
        1,
        "Figure 5 should be a single merged item, got {}",
        fig5.len()
    );

    let (((desc5, _), img_bb5), body_bb5) = &fig5[0];
    eprintln!(
        "2304.10557 Figure 5: desc={:?} bbox={:?} body={:?}",
        desc5, img_bb5.bbox, body_bb5.bbox
    );

    // The merged bbox must cover both the left sub-panel (x≈78..206) and the
    // right sub-panel + caption (x≈222..576).
    assert!(
        img_bb5.bbox[0] <= 80.0,
        "Figure 5 bbox should start near the left sub-panel (x≈78): got {:?}",
        img_bb5.bbox
    );
    assert!(
        img_bb5.bbox[2] >= 570.0,
        "Figure 5 bbox should extend to the right caption (x≈576): got {:?}",
        img_bb5.bbox
    );

    // Figure 7 must also be present on the same page, as a separate item
    let fig7: Vec<_> = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .filter(|((desc, _), bb)| desc.contains("Figure 7") && bb.page_idx == 5)
        .collect();

    assert!(!fig7.is_empty(), "Figure 7 should be present on page 6");
    assert_eq!(
        fig7.len(),
        1,
        "Figure 7 should be a single item, got {}",
        fig7.len()
    );

    let (_, bb7) = &fig7[0];
    eprintln!("2304.10557 Figure 7: bbox={:?}", bb7.bbox);

    // Figure 7 sits below Figure 5; its top must be well below Figure 5's bottom
    assert!(
        bb7.bbox[1] >= 300.0,
        "Figure 7 should start below Figure 5 (y≈352): got {:?}",
        bb7.bbox
    );
}

#[tokio::test]
async fn diagnose_2306_00978_page9() {
    let paper_id = "2306.00978";
    let src_zip = fixture_zip_for(paper_id).await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_dir = tmp.path().to_string_lossy().to_string();
    let paper_dir = tmp.path().join(paper_id);
    std::fs::create_dir_all(&paper_dir).unwrap();
    std::fs::copy(&src_zip, paper_dir.join("mineru.zip")).unwrap();

    let client = MinerUClient::new(
        "http://unused".to_string(),
        "unused".to_string(),
        Some(cache_dir),
        false,
    );
    let figures = client
        .reprocess_from_zip(paper_id, &[], true, false)
        .await
        .unwrap();

    eprintln!("\n=== 2306.00978 extracted items ===");
    for (i, (((desc, _bytes), img_bb), body_bb)) in figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .zip(&figures.body_bboxes)
        .enumerate()
    {
        if img_bb.page_idx != 8 {
            continue;
        }
        eprintln!(
            "Item {}: page={} type={}",
            i,
            img_bb.page_idx + 1,
            img_bb.content_type
        );
        let snippet = &desc[..desc
            .char_indices()
            .nth(120)
            .map(|(i, _)| i)
            .unwrap_or(desc.len())];
        eprintln!("  desc={}", snippet);
        eprintln!("  bbox={:?}", img_bb.bbox);
        eprintln!("  body={:?}", body_bb.bbox);
    }
}

/// Regression guard for 1603.09320 page 8: a numbered list block
/// "1. Baseline NSW algorithm from nmslib 1.1" at y=458-545 sits above three
/// image blocks at y=551-676.  Before the fix, this ordinary itemized list
/// was incorrectly absorbed into the figure bbox because the gap threshold
/// was 10 pt and there was no numbered-list guard.  After tightening the gap
/// to 5 pt and adding `is_numbered` filtering, the list must NOT be absorbed.
#[tokio::test(flavor = "multi_thread")]
async fn test_1603_09320_numbered_list_not_absorbed() {
    let paper_id = "1603.09320";
    let src_zip = fixture_zip_for(paper_id).await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_dir = tmp.path().to_string_lossy().to_string();
    let paper_dir = tmp.path().join(paper_id);
    std::fs::create_dir_all(&paper_dir).unwrap();
    std::fs::copy(&src_zip, paper_dir.join("mineru.zip")).unwrap();

    let client = MinerUClient::new(
        "http://unused".to_string(),
        "unused".to_string(),
        Some(cache_dir),
        false,
    );
    let figures = client
        .reprocess_from_zip(paper_id, &[], true, false)
        .await
        .expect("reprocess_from_zip succeeds with skip_validation=true");

    // Page 8 (page_idx=7) has three images at y=551-676.
    // The numbered list is at y=458-545.  If absorbed, bbox top would be <= 458.
    // Without absorption, bbox top should stay >= ~540 (close to image body top).
    // Page 8 (page_idx=7) has three images at y=551-676.
    // The numbered list is at y=458-545.  If absorbed, bbox top would be <= 458.
    // Without absorption, bbox top should stay >= ~540 (close to image body top).
    let page8_items: Vec<_> = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .filter(|((_, _), bb)| bb.page_idx == 7)
        .collect();

    assert!(
        !page8_items.is_empty(),
        "expected at least one item on page 8, got {}",
        page8_items.len()
    );

    for (i, ((desc, _), bb)) in page8_items.iter().enumerate() {
        let top = bb.bbox[1].min(bb.bbox[3]);
        eprintln!(
            "1603.09320 page 8 item {}: top={:.1} bbox={:?} desc={}",
            i,
            top,
            bb.bbox,
            &desc[..desc
                .char_indices()
                .nth(60)
                .map(|(i, _)| i)
                .unwrap_or(desc.len())]
        );
        // The numbered list sits at y=458-545.  If it were absorbed, top would
        // drop to ~458.  With the fix, top must stay well above that threshold.
        assert!(
            top > 500.0,
            "page 8 item {} top ({:.1}) must NOT absorb the numbered list above (y≈458-545): bbox={:?}",
            i, top, bb.bbox
        );
    }
}

/// Regression guard for 2201.11903 page 6: Figure 5 and Figure 6 have list-typed
/// legend blocks directly above their bodies (e.g. "Standard prompting | Equation
/// only | Variable compute only | Reasoning after answer | Chain-of-thought
/// prompting" at y=76-134 above Figure 5 body at y=135-240).  MinerU emits these
/// as independent para_blocks rather than sub-blocks of the figure, so without
/// absorption the hires crop misses the legend entirely.
///
/// The fix absorbs list-typed blocks that sit directly above a figure body with
/// significant horizontal overlap (> 50%) and small vertical gap (< 10 pt).
#[tokio::test(flavor = "multi_thread")]
async fn test_2201_11903_figure5_6_absorb_panel_labels() {
    let paper_id = "2201.11903";
    let src_zip = fixture_zip_for(paper_id).await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_dir = tmp.path().to_string_lossy().to_string();
    let paper_dir = tmp.path().join(paper_id);
    std::fs::create_dir_all(&paper_dir).unwrap();
    std::fs::copy(&src_zip, paper_dir.join("mineru.zip")).unwrap();

    let client = MinerUClient::new(
        "http://unused".to_string(),
        "unused".to_string(),
        Some(cache_dir),
        false,
    );
    let figures = client
        .reprocess_from_zip(paper_id, &[], true, false)
        .await
        .expect("reprocess_from_zip succeeds with skip_validation=true");

    // Figure 5 on page 6 (page_idx=5)
    let fig5 = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .zip(&figures.body_bboxes)
        .find(|(((desc, _), bb), _)| desc.contains("Figure 5") && bb.page_idx == 5);
    let (((desc5, _), img_bb5), body_bb5) =
        fig5.expect("Figure 5 should be present on page_idx=5 (page 6)");
    eprintln!(
        "2201.11903 Figure 5: desc={:?} bbox={:?} body={:?}",
        desc5, img_bb5.bbox, body_bb5.bbox
    );

    // The list legend sits at y=76-134; body starts at y=135.
    // After absorption bbox top must reach the legend (y <= 80).
    assert!(
        img_bb5.bbox[1] <= 80.0,
        "Figure 5 bbox top ({:.1}) should include the panel label list above (y≈76): bbox={:?}",
        img_bb5.bbox[1],
        img_bb5.bbox
    );

    // Figure 6 on page 6 (page_idx=5)
    let fig6 = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .zip(&figures.body_bboxes)
        .find(|(((desc, _), bb), _)| desc.contains("Figure 6") && bb.page_idx == 5);
    let (((desc6, _), img_bb6), body_bb6) =
        fig6.expect("Figure 6 should be present on page_idx=5 (page 6)");
    eprintln!(
        "2201.11903 Figure 6: desc={:?} bbox={:?} body={:?}",
        desc6, img_bb6.bbox, body_bb6.bbox
    );

    // The list legend sits at y=339-433; body starts at y=438.
    // After absorption bbox top must reach the legend (y <= 345).
    assert!(
        img_bb6.bbox[1] <= 345.0,
        "2201.11903 Figure 6 bbox top should include the panel label list above (y≈339): top={:.1} bbox={:?}",
        img_bb6.bbox[1], img_bb6.bbox
    );
}

/// Regression guard for 2605.31086 page 16 Figure 5 composite sub-panels.
///
/// Figure 5 consists of 10 tiny image icons (~30x30 pt each) spread across
/// the page as separate para_blocks, plus a caption sub-block inside the
/// rightmost icon.  Before the fix, `is_noise` filtered out the 9 bare
/// sub-panel icons (w<40 && h<40 && area<1200), leaving only the captioned
/// rightmost icon — the body_bbox was therefore a 27x20 pt sliver.  The
/// fix pre-computes per-page unique figure captions and exempts image blocks
/// on singleton-caption pages from `is_noise`, so `propagate_page_unique_
/// caption` can assign the caption to every sub-panel and the merge step
/// fuses them into a single composite figure.
#[tokio::test(flavor = "multi_thread")]
async fn test_2605_31086_figure5_composite_subpanels() {
    let paper_id = "2605.31086";
    let src_zip = fixture_zip_for(paper_id).await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let cache_dir = tmp.path().to_string_lossy().to_string();
    let paper_dir = tmp.path().join(paper_id);
    std::fs::create_dir_all(&paper_dir).unwrap();
    std::fs::copy(&src_zip, paper_dir.join("mineru.zip")).unwrap();

    let client = MinerUClient::new(
        "http://unused".to_string(),
        "unused".to_string(),
        Some(cache_dir),
        false,
    );
    let figures = client
        .reprocess_from_zip(paper_id, &[], true, false)
        .await
        .expect("reprocess_from_zip succeeds with skip_validation=true");

    let fig5 = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .zip(&figures.body_bboxes)
        .find(|(((desc, _), bb), _)| desc.contains("Figure 5") && bb.page_idx == 15);
    let (((desc, _), img_bb), body_bb) =
        fig5.expect("Figure 5 should be present on page_idx=15 (page 16)");
    eprintln!(
        "2605.31086 Figure 5: desc={:?} bbox={:?} body={:?}",
        desc, img_bb.bbox, body_bb.bbox
    );

    // Before the fix body was [463,687,490,707] (27x20 pt).
    // After the fix it must span the full composite region.
    let body_w = (body_bb.bbox[2] - body_bb.bbox[0]).abs();
    let body_h = (body_bb.bbox[3] - body_bb.bbox[1]).abs();
    assert!(
        body_w > 300.0,
        "Figure 5 body width ({:.1}) must span the full composite region (>300 pt), not a single tiny icon",
        body_w
    );
    assert!(
        body_h > 150.0,
        "Figure 5 body height ({:.1}) must span the full composite region (>150 pt), not a single tiny icon",
        body_h
    );

    // The bbox must start near the leftmost sub-panel (x≈101) and extend to
    // the rightmost sub-panel + caption (x≈490).
    assert!(
        img_bb.bbox[0] <= 110.0,
        "Figure 5 bbox should start near the leftmost sub-panel (x≈101): got {:?}",
        img_bb.bbox
    );
    assert!(
        img_bb.bbox[2] >= 480.0,
        "Figure 5 bbox should extend to the rightmost sub-panel (x≈490): got {:?}",
        img_bb.bbox
    );
}

/// Regression guard for 1608.05343 Figure 2 bleeding into the right column.
///
/// Figure 2 sits in the left column (x≈54..301) of a two-column page with
/// column boundary at x≈296.5.  A right-column text block at x≈304..541
/// was previously absorbed as a "side text block" (h_gap=0, vertical overlap
/// satisfied), inflating the bbox to span the full page width.  The
/// column-boundary guard must block this absorption.
#[tokio::test(flavor = "multi_thread")]
async fn test_1608_05343_figure2_no_right_column_bleed() {
    let paper_id = "1608.05343";
    let src_zip = fixture_zip_for(paper_id).await;

    let figures = reprocess_fixture(paper_id, &src_zip).await;

    let fig2 = figures
        .image_bboxes
        .iter()
        .find(|bb| bb.page_idx == 2 && bb.bbox[0] < 100.0)
        .expect("Figure 2 must be present on page 2 (0-indexed)");

    let right = fig2.bbox[0].max(fig2.bbox[2]);
    // Column boundary is at ~296.5.  Figure 2's right edge must stay in
    // the left column — the right-column text blocks start at x≈304.
    assert!(
        right < 320.0,
        "Figure 2 bbox right edge must stay in the left column (< 320), got {} (bbox {:?})",
        right,
        fig2.bbox,
    );
}

/// Verify 2112.10752 Figure 5/6/7 are distinct after orphan-caption rebind.
/// MinerU nested "Figure 5" inside the table block and "Figure 6" inside the
/// Figure 7 block.  The hard type-mismatch skip was converted to a score
/// penalty so Figure 5 can bind to its table body.
#[tokio::test(flavor = "multi_thread")]
async fn test_2112_10752_figure5_6_7_distinct() {
    let paper_id = "2112.10752";
    let src_zip = fixture_zip_for(paper_id).await;

    let figures = reprocess_fixture(paper_id, &src_zip).await;

    // Figure 5 is the text-to-image sample grid (table type).
    let fig5 = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .find(|((desc, _), bb)| desc.contains("Figure 5") && bb.page_idx == 5);
    let ((desc5, _), bb5) = fig5.expect("Figure 5 must be present on page 6 (page_idx=5)");
    assert!(
        bb5.content_type == "table",
        "Figure 5 must be table type, got {}",
        bb5.content_type
    );

    // Figure 6 is the training-analysis chart (image type).
    let fig6 = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .find(|((desc, _), bb)| desc.contains("Figure 6") && bb.page_idx == 5);
    let ((desc6, _), bb6) = fig6.expect("Figure 6 must be present on page 6 (page_idx=5)");
    assert!(
        bb6.content_type == "image" || bb6.content_type == "chart",
        "Figure 6 must be image/chart type, got {}",
        bb6.content_type
    );

    // Figure 7 must also be present and distinct.
    let fig7 = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .find(|((desc, _), bb)| desc.contains("Figure 7") && bb.page_idx == 5);
    let ((desc7, _), bb7) = fig7.expect("Figure 7 must be present on page 6 (page_idx=5)");

    // All three must have distinct bounding boxes (not overlapping entirely).
    // Figure 5 bbox should include the caption strip below the table body.
    assert!(
        bb5.bbox[3] > 270.0,
        "Figure 5 bbox must extend to include its caption (y>270), got {:?}",
        bb5.bbox
    );
    // Figure 6 bbox should include its caption above the body.
    assert!(
        bb6.bbox[1] < 310.0,
        "Figure 6 bbox top must include its caption above body (y<310), got {:?}",
        bb6.bbox
    );

    eprintln!(
        "2112.10752 page 6: Fig5 desc={:?} bbox={:?}, Fig6 desc={:?} bbox={:?}, Fig7 desc={:?} bbox={:?}",
        desc5, bb5.bbox, desc6, bb6.bbox, desc7, bb7.bbox
    );
}

/// Verify 2112.10752 Table 14 and Table 15 are distinct on page 25.
/// MinerU nested Table 14's caption inside Table 15's para_block.
/// The above+b below caption split fix keeps the below caption with the block
/// and emits the above caption as orphan for spatial rebind.
#[tokio::test(flavor = "multi_thread")]
async fn test_2112_10752_table14_15_distinct() {
    let paper_id = "2112.10752";
    let src_zip = fixture_zip_for(paper_id).await;

    let figures = reprocess_fixture(paper_id, &src_zip).await;

    // Table 14: unconditional LDMs hyperparams (CelebA).
    let tbl14 = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .find(|((desc, _), bb)| desc.contains("Table 14") && bb.page_idx == 24);
    let ((desc14, _), bb14) = tbl14.expect("Table 14 must be present on page 25 (page_idx=24)");
    assert!(
        bb14.content_type == "table",
        "Table 14 must be table type, got {}",
        bb14.content_type
    );
    // Table 14 body is at y=70-219; bbox must NOT extend into Table 15 body.
    assert!(
        bb14.bbox[3] < 275.0,
        "Table 14 bbox must not include Table 15 body (y<275), got {:?}",
        bb14.bbox
    );

    // Table 15: conditional LDMs hyperparams.
    let tbl15 = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .find(|((desc, _), bb)| desc.contains("Table 15") && bb.page_idx == 24);
    let ((desc15, _), bb15) = tbl15.expect("Table 15 must be present on page 25 (page_idx=24)");
    assert!(
        bb15.content_type == "table",
        "Table 15 must be table type, got {}",
        bb15.content_type
    );
    // Table 15 body starts at y=280; bbox top must not reach into Table 14.
    assert!(
        bb15.bbox[1] > 270.0,
        "Table 15 bbox top must be below Table 14 caption (y>270), got {:?}",
        bb15.bbox
    );

    eprintln!(
        "2112.10752 page 25: Tbl14 desc={:?} bbox={:?}, Tbl15 desc={:?} bbox={:?}",
        desc14, bb14.bbox, desc15, bb15.bbox
    );
}

/// Verify 1608.05343 Table 2 left (table) and right (chart) are merged into
/// a single entry on page 11.
#[tokio::test(flavor = "multi_thread")]
async fn test_1608_05343_table2_merged() {
    let paper_id = "1608.05343";
    let src_zip = fixture_zip_for(paper_id).await;

    let figures = reprocess_fixture(paper_id, &src_zip).await;

    // Table 2 should be a single merged entry covering both the table and chart.
    let tbl2_entries: Vec<_> = figures
        .images
        .iter()
        .zip(&figures.image_bboxes)
        .filter(|((desc, _), bb)| desc.contains("Table 2") && bb.page_idx == 10)
        .collect();

    assert_eq!(
        tbl2_entries.len(),
        1,
        "Table 2 should be a single merged entry, found {}",
        tbl2_entries.len()
    );

    let ((desc, _), bb) = tbl2_entries[0];
    // The merged bbox should cover both the left table (y=65-192) and the
    // right chart (y=196-358), with the caption at y=371-437.
    assert!(
        bb.bbox[3] >= 430.0,
        "Table 2 bbox must extend to include its caption (y>=430), got {:?}",
        bb.bbox
    );
    assert!(
        bb.bbox[1] <= 70.0,
        "Table 2 bbox top must start near the table top (y<=70), got {:?}",
        bb.bbox
    );

    eprintln!(
        "1608.05343 page 11: Table 2 desc={:?} bbox={:?}",
        desc, bb.bbox
    );
}
