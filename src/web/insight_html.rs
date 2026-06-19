// SPDX-License-Identifier: MIT OR Apache-2.0

use ammonia::Builder;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use regex::Regex;
use std::sync::{Arc, LazyLock};
use std::time::Instant;
use tracing::{error, info};

use crate::web::types::{AppState, ErrorResponse, InsightHtmlResponse, TocItem};

static MATH_DISPLAY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\$[\s\S]*?\$\$").unwrap());

static MATH_INLINE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$([^$\n]+?)\$").unwrap());

static ALGORITHM_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\begin\{algorithm\}[\s\S]*?\\end\{algorithm\}").unwrap());

static BOLD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*([\s\S]+?)\*\*").unwrap());

static FENCED_CODE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"```[a-zA-Z0-9]*\n[\s\S]*?\n```").unwrap());

static INLINE_CODE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`[^`\n]+?`").unwrap());

static EXTERNAL_IMG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"!\[([\s\S]*?)\]\((https?://[^)\s]+)(?:(?:\s*=\s*(\d*(?:\.\d+)?(?:%|px)?)(?:x(\d*(?:\.\d+)?(?:%|px)?))?)?(?:\s+(left|right|inline|center))?(?:\s+"([^"]*)")?)?\)"#).unwrap()
});

static PAGE_IMG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"!\[([\s\S]*?)\]\(page://(\d+)(?:(?:\s*=\s*(\d*(?:\.\d+)?(?:%|px)?)(?:x(\d*(?:\.\d+)?(?:%|px)?))?)?(?:\s+(left|right|inline|center))?(?:\s+"([^"]*)")?)?\)"#).unwrap()
});

static FIGURE_IMG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"!\[([\s\S]*?)\]\(figure/([a-zA-Z0-9_.-]+)(?:(?:\s*=\s*(\d*(?:\.\d+)?(?:%|px)?)(?:x(\d*(?:\.\d+)?(?:%|px)?))?)?(?:\s+(left|right|inline|center))?(?:\s+"([^"]*)")?)?\)"#).unwrap()
});

static TABLE_IMG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"!\[([\s\S]*?)\]\(table/([a-zA-Z0-9_.-]+)(?:(?:\s*=\s*(\d*(?:\.\d+)?(?:%|px)?)(?:x(\d*(?:\.\d+)?(?:%|px)?))?)?(?:\s+(left|right|inline|center))?(?:\s+"([^"]*)")?)?\)"#).unwrap()
});

static ROTATE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"rotate\s*=\s*(90|180|270)").unwrap());

static CJK_AUTOLINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<a([^>]*)>(https?://[^\s<]+?)([\u{4e00}-\u{9fff}\u{3000}-\u{303f}\u{ff00}-\u{ffef}][^<]*?)</a>"#).unwrap()
});

static HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<h([1-3])([^>]*)>(.+?)</h[1-3]>"#).unwrap());

static LINE_MATH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|\n)\s*\$([^$\n]+?)\$\s*(\n|$)").unwrap());

static PRICE_LIKE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9,.\s]+$").unwrap());

static DIM_NUMERIC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+(\.\d+)?$").unwrap());

static HREF_ATTR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"href\s*=\s*["'][^"']*["']"#).unwrap());

static ID_ATTR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"id\s*=\s*["']([^"']*)["']"#).unwrap());

static STRIP_HTML_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());
pub(crate) async fn insight_html(
    State(state): State<Arc<AppState>>,
    Path((source, id)): Path<(String, String)>,
) -> Result<Json<InsightHtmlResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Check cache first
    let t0 = Instant::now();
    if let Ok(Some((html, toc))) = state.db.get_insight_html_cache(&id).await {
        info!(
            "[insight_html] {} cache hit, {} chars html, {} toc items, {:.0}ms",
            id,
            html.len(),
            toc.len(),
            t0.elapsed().as_secs_f64() * 1000.0
        );
        return Ok(Json(InsightHtmlResponse { html, toc }));
    }

    // Cache miss: render and write back
    let paper = state
        .db
        .get_paper(&id)
        .await
        .map_err(|e| {
            error!("{}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    code: 500,
                    reason: "Database query failed".into(),
                }),
            )
        })?
        .ok_or((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                code: 404,
                reason: "Paper not found".into(),
            }),
        ))?;

    let t0 = Instant::now();
    let response = render_insight_html(&source, &id, &paper.insight);
    info!(
        "[insight_html] {} ({} chars) -> {} chars html, {} toc items, {:.0}ms (cache miss)",
        id,
        paper.insight.len(),
        response.html.len(),
        response.toc.len(),
        t0.elapsed().as_secs_f64() * 1000.0
    );

    // Write back to cache (best-effort, don't fail the request)
    if !paper.insight.is_empty() {
        if let Err(e) = state
            .db
            .upsert_insight_html_cache(&id, &response.html, &response.toc)
            .await
        {
            info!("[insight_html] {} cache write failed: {}", id, e);
        }
    }

    Ok(Json(response))
}

/// Call this before every write that modifies insight content.
/// Renders HTML and populates the cache fields in the update struct.
pub(crate) async fn refresh_insight_html_cache(
    db: &crate::db::Db,
    source: &str,
    paper_id: &str,
    insight: &str,
) {
    if insight.is_empty() {
        let _ = db.delete_insight_html_cache(paper_id).await;
        return;
    }
    let resp = render_insight_html(source, paper_id, insight);
    if let Err(e) = db
        .upsert_insight_html_cache(paper_id, &resp.html, &resp.toc)
        .await
    {
        info!("[insight_html] {} cache refresh failed: {}", paper_id, e);
    }
}

pub fn render_insight_html(source: &str, paper_id: &str, insight: &str) -> InsightHtmlResponse {
    if insight.is_empty() {
        return InsightHtmlResponse {
            html: String::new(),
            toc: Vec::new(),
        };
    }

    let mut text = insight.to_string();

    // Step 1: protect algorithm blocks (must be before math, so math
    // inside \begin{algorithm}...\end{algorithm} is left for pseudocode.js)
    let mut algo_blocks: Vec<String> = Vec::new();
    text = protect_algorithms(&text, &mut algo_blocks);

    // Step 2: protect page/figure/table/external images (must be before math,
    // so that math inside image alt text stays as raw $...$ and does not get
    // turned into %%MATH_i%% which would later be restored inside the alt attribute)
    let mut page_images: Vec<PageImage> = Vec::new();
    text = protect_page_images(&text, &mut page_images);

    // Step 3: protect math blocks
    let mut math_blocks: Vec<String> = Vec::new();
    text = protect_math(&text, &mut math_blocks);

    // Step 3.5: protect code blocks so normalize_bold does not misinterpret
    // `**` inside fenced code / inline code as bold markers.
    let mut code_blocks: Vec<String> = Vec::new();
    text = protect_code_blocks(&text, &mut code_blocks);

    // Step 4: normalize **bold** to <strong> (before markdown parsing, to avoid
    // CommonMark CJK delimiter issues)
    text = normalize_bold(&text);

    // Step 4.5: restore code blocks before markdown parsing
    text = restore_code_blocks(&text, &code_blocks);

    // Step 5: parse markdown → HTML
    let mut html = String::new();
    let parser = pulldown_cmark::Parser::new_ext(&text, pulldown_cmark::Options::all());
    pulldown_cmark::html::push_html(&mut html, parser);

    // Step 5.5: pulldown-cmark emits plain `<pre><code>` for fenced blocks
    // without a language tag. Prism.js matches via `pre code[class^="language-"]`;
    // add `class="language-text"` so these blocks still get CSS styling.
    html = html.replace(
        "<pre><code>",
        "<pre class=\"language-text\"><code class=\"language-text\">",
    );

    // Step 6: restore math blocks as .math-deferred elements
    html = restore_math(&html, &math_blocks);

    // Step 7: restore page/figure/table/external images
    html = restore_page_images(&html, source, paper_id, &page_images);

    // Step 8: restore algorithm blocks
    html = restore_algorithms(&html, &algo_blocks);

    // Step 9: fix CJK autolinks
    html = fix_cjk_autolinks(&html);

    // Step 10: wrap tables
    html = wrap_tables(&html);

    // Step 11: heading numbering + TOC extraction
    let (html, toc) = number_headings_and_toc(&html);

    // Restore escaped dollar signs so \$ becomes literal $
    let html = html.replace("\u{7f}ESCDOLLAR\u{7f}", "$");

    // Sanitize the final HTML to prevent XSS
    let html = sanitize_markdown_html(&html);

    InsightHtmlResponse { html, toc }
}

struct PageImage {
    alt: String,
    kind: PageImageKind,
    width: Option<String>,
    height: Option<String>,
    align: String,
    rotate: Option<i32>,
}

enum PageImageKind {
    Page(String),
    Figure(String),
    Table(String),
    External(String),
}

fn protect_math(text: &str, blocks: &mut Vec<String>) -> String {
    // Protect escaped dollar signs so \$ is not treated as math delimiter
    let text = text.replace(r"\$", "\u{7f}ESCDOLLAR\u{7f}");

    // Upgrade standalone inline math on its own line to display math
    let text = LINE_MATH_RE
        .replace_all(&text, |caps: &regex::Captures| {
            format!(
                "{}\u{7f}MATHUPGRADE\u{7f}$${}$$\u{7f}MATHUPGRADE\u{7f}{}",
                &caps[1],
                caps[2].trim(),
                &caps[3]
            )
        })
        .to_string();

    // Protect display math $$...$$
    let text = MATH_DISPLAY_RE
        .replace_all(&text, |caps: &regex::Captures| {
            blocks.push(caps[0].to_string());
            format!("%%MATH_{}%%", blocks.len() - 1)
        })
        .to_string();

    // Protect inline math $...$ (skip price-like content and content with
    // leading/trailing spaces that are unlikely to be real math)
    let text = MATH_INLINE_RE
        .replace_all(&text, |caps: &regex::Captures| {
            let content = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let trimmed = content.trim();
            if PRICE_LIKE_RE.is_match(trimmed) {
                return caps[0].to_string();
            }
            if content != trimmed {
                return caps[0].to_string();
            }
            blocks.push(caps[0].to_string());
            format!("%%MATH_{}%%", blocks.len() - 1)
        })
        .to_string();

    // Remove upgrade markers
    text.replace("\u{7f}MATHUPGRADE\u{7f}", "")
}

/// Convert **bold** markers to <strong> HTML before markdown parsing to avoid
/// CommonMark CJK delimiter issues (e.g. **中文** followed by a CJK character
/// fails the right-flanking delimiter rule).
fn normalize_bold(text: &str) -> String {
    BOLD_RE
        .replace_all(text, |caps: &regex::Captures| {
            format!("<strong>{}</strong>", &caps[1])
        })
        .to_string()
}

fn protect_code_blocks(text: &str, blocks: &mut Vec<String>) -> String {
    // Fenced code blocks first (multi-line)
    let text = FENCED_CODE_RE
        .replace_all(text, |caps: &regex::Captures| {
            blocks.push(caps[0].to_string());
            format!("%%CODE_{}%%", blocks.len() - 1)
        })
        .to_string();
    // Then inline code (single-line, must not contain backticks or newlines)
    INLINE_CODE_RE
        .replace_all(&text, |caps: &regex::Captures| {
            blocks.push(caps[0].to_string());
            format!("%%CODE_{}%%", blocks.len() - 1)
        })
        .to_string()
}

fn restore_code_blocks(text: &str, blocks: &[String]) -> String {
    let mut result = text.to_string();
    for (i, block) in blocks.iter().enumerate() {
        result = result.replace(&format!("%%CODE_{}%%", i), block);
    }
    result
}

fn protect_algorithms(text: &str, blocks: &mut Vec<String>) -> String {
    ALGORITHM_RE
        .replace_all(text, |caps: &regex::Captures| {
            blocks.push(caps[0].to_string());
            format!("%%ALGO_{}%%", blocks.len() - 1)
        })
        .to_string()
}

fn dim_unit(v: &str) -> String {
    if v.is_empty() {
        String::new()
    } else if DIM_NUMERIC_RE.is_match(v) {
        format!("{}px", v)
    } else {
        v.to_string()
    }
}

/// Parse the image title for `rotate=90/180/270` and return the angle in
/// degrees. Only 90/180/270 are supported.
fn parse_rotate(title: &Option<String>) -> Option<i32> {
    let t = title.as_deref()?;
    ROTATE_RE.captures(t).and_then(|c| c[1].parse().ok())
}

/// Build CSS styles for a 90/270-degree rotated image when its natural
/// dimensions are known. Returns `(wrapper_prefix, box_style, rot_style,
/// img_style)`.
///
/// The user-supplied dimensions describe the **final on-screen (visual) box**,
/// i.e. what the reader sees after rotation — so `=47%` always means the
/// displayed image is 47% wide, rotated or not.
///
/// Layout is three nested layers, each with a single responsibility:
/// - **box** (`box_style`): the visual box, sized to the swapped aspect ratio
///   (a 90/270 turn swaps width and height) with `overflow:hidden` clipping.
/// - **rot** (`rot_style`): an absolutely-centered, un-rotated box that keeps
///   the *original* aspect ratio and is rotated to fill the box. Its
///   `width:100%` + `aspect-ratio` make its area equal to the box, so it fills
///   it exactly after rotation.
/// - **img** (`img_style`): a plain `100%×100%` fill of the rot box — no
///   `calc`, no `max-width` hack.
///
/// Using `aspect-ratio` (the same mechanism) for both box and rot, and pure
/// `100%` for the img, avoids the two independent sizing calculations
/// (`aspect-ratio` on the box vs `calc(x/y*100%)` on the img) that caused
/// sub-pixel mismatches and visible stretching.
///
/// When `width_on_wrapper` is true (floated left/right), the user width is
/// emitted on the *wrapper* via `wrapper_prefix` and the box uses `width:100%`
/// — this keeps the percentage resolving against the text column. When false
/// (centered), the user width stays on the box itself.
fn rotated_image_styles(
    user_w: &str,
    user_h: &str,
    img_w: u32,
    img_h: u32,
    deg: i32,
    width_on_wrapper: bool,
) -> Option<(String, String, String, String)> {
    if img_w == 0 || img_h == 0 {
        return None;
    }
    let has_w = !user_w.is_empty();
    let has_h = !user_h.is_empty();

    // Visual (rotated) box aspect ratio = img_h / img_w (swapped): a 90/270
    // turn swaps the original width and height.
    let visual_ratio = format!("{} / {}", img_h, img_w);

    // For floated images the percentage width must live on the wrapper (so it
    // resolves against the text column); the box then fills the wrapper. For
    // centered images the box carries the width directly (resolving against the
    // full-width centered container).
    let wrapper_prefix = if width_on_wrapper && has_w {
        format!("width:{};", user_w)
    } else {
        String::new()
    };
    let box_w = if width_on_wrapper {
        "width:100%;".to_string()
    } else if has_w {
        format!("width:{};", user_w)
    } else {
        String::new()
    };

    let box_sizing = match (has_w, has_h) {
        (true, true) => format!("{}height:{};", box_w, user_h),
        (true, false) => format!("{}aspect-ratio:{};", box_w, visual_ratio),
        (false, true) => format!("height:{};aspect-ratio:{};", user_h, visual_ratio),
        (false, false) => format!("width:{}px;aspect-ratio:{};", img_h, visual_ratio),
    };
    // display:block + margin:auto centers a fixed-width block inside the
    // centered wrapper (text-align:center has no effect on block boxes). For
    // floated images the box is width:100% so centering is a no-op.
    let center = if width_on_wrapper { "" } else { "margin:auto;" };
    let box_style = format!(
        "{}overflow:hidden;position:relative;display:block;{}",
        box_sizing, center
    );

    // The rot box must be the box's dimensions with width/height swapped
    // (H_box x W_box), so that a 90/270 rotation fills the box exactly:
    //   box:  W_box x H_box,  H_box = W_box * (img_w / img_h)
    //   rot:  H_box x W_box  =  calc(img_w/img_h * 100%) wide
    //                            x calc(img_h/img_w * 100%) tall
    // (percentages resolve against the box: width % vs box width, height % vs
    // box height). Both factors reproduce the original img_w:img_h ratio, so
    // the un-rotated rot box has the image's true aspect and is not distorted.
    let rot_w = format!("calc({} / {} * 100%)", img_w, img_h);
    let rot_h = format!("calc({} / {} * 100%)", img_h, img_w);
    let rot_style = format!(
        "position:absolute;left:50%;top:50%;width:{};height:{};transform:translate(-50%,-50%) rotate({}deg);transform-origin:center;",
        rot_w, rot_h, deg
    );

    // Plain fill of the rot box. width/height:100% stay within the rot box, so
    // the `.progressive-img` `max-width:100%` never clamps; the inline
    // height:100% overrides its `height:auto`.
    let img_style = "position:absolute;inset:0;width:100%;height:100%;".to_string();

    Some((wrapper_prefix, box_style, rot_style, img_style))
}

fn protect_page_images(text: &str, images: &mut Vec<PageImage>) -> String {
    // page://N
    let text = PAGE_IMG_RE
        .replace_all(text, |caps: &regex::Captures| {
            images.push(PageImage {
                alt: caps[1].to_string(),
                kind: PageImageKind::Page(caps[2].to_string()),
                width: caps.get(3).map(|m| m.as_str().to_string()),
                height: caps.get(4).map(|m| m.as_str().to_string()),
                align: caps
                    .get(5)
                    .map(|m| m.as_str())
                    .unwrap_or("center")
                    .to_string(),
                rotate: parse_rotate(&caps.get(6).map(|m| m.as_str().to_string())),
            });
            format!("%%PAGEIMG_{}%%", images.len() - 1)
        })
        .to_string();

    // figure/name
    let text = FIGURE_IMG_RE
        .replace_all(&text, |caps: &regex::Captures| {
            images.push(PageImage {
                alt: caps[1].to_string(),
                kind: PageImageKind::Figure(caps[2].to_string()),
                width: caps.get(3).map(|m| m.as_str().to_string()),
                height: caps.get(4).map(|m| m.as_str().to_string()),
                align: caps
                    .get(5)
                    .map(|m| m.as_str())
                    .unwrap_or("center")
                    .to_string(),
                rotate: parse_rotate(&caps.get(6).map(|m| m.as_str().to_string())),
            });
            format!("%%PAGEIMG_{}%%", images.len() - 1)
        })
        .to_string();

    // table/name
    let text = TABLE_IMG_RE
        .replace_all(&text, |caps: &regex::Captures| {
            images.push(PageImage {
                alt: caps[1].to_string(),
                kind: PageImageKind::Table(caps[2].to_string()),
                width: caps.get(3).map(|m| m.as_str().to_string()),
                height: caps.get(4).map(|m| m.as_str().to_string()),
                align: caps
                    .get(5)
                    .map(|m| m.as_str())
                    .unwrap_or("center")
                    .to_string(),
                rotate: parse_rotate(&caps.get(6).map(|m| m.as_str().to_string())),
            });
            format!("%%PAGEIMG_{}%%", images.len() - 1)
        })
        .to_string();

    // external https://...
    EXTERNAL_IMG_RE
        .replace_all(&text, |caps: &regex::Captures| {
            images.push(PageImage {
                alt: caps[1].to_string(),
                kind: PageImageKind::External(caps[2].to_string()),
                width: caps.get(3).map(|m| m.as_str().to_string()),
                height: caps.get(4).map(|m| m.as_str().to_string()),
                align: caps
                    .get(5)
                    .map(|m| m.as_str())
                    .unwrap_or("center")
                    .to_string(),
                rotate: parse_rotate(&caps.get(6).map(|m| m.as_str().to_string())),
            });
            format!("%%PAGEIMG_{}%%", images.len() - 1)
        })
        .to_string()
}

// ---- image dimension helpers (for LQIP CLS prevention) ----

fn png_dimensions(path: &str) -> Option<(u32, u32)> {
    let mut f = std::fs::File::open(path).ok()?;
    use std::io::Read;
    let mut sig = [0u8; 8];
    f.read_exact(&mut sig).ok()?;
    if sig != [137, 80, 78, 71, 13, 10, 26, 10] {
        return None;
    } // not PNG
    let mut ihdr_len = [0u8; 4];
    f.read_exact(&mut ihdr_len).ok()?;
    let mut ihdr = [0u8; 4];
    f.read_exact(&mut ihdr).ok()?;
    if &ihdr != b"IHDR" {
        return None;
    }
    let mut w = [0u8; 4];
    let mut h = [0u8; 4];
    f.read_exact(&mut w).ok()?;
    f.read_exact(&mut h).ok()?;
    Some((u32::from_be_bytes(w), u32::from_be_bytes(h)))
}

fn page_dimensions(source: &str, paper_id: &str, page_num: &str, dpi: u32) -> Option<(u32, u32)> {
    use std::process::Command;
    let pdf_path = crate::web::paper_pdf_path(source, paper_id);
    if !std::path::Path::new(&pdf_path).exists() {
        return None;
    }
    // mutool info: <page id="N" width="W" height="H" />
    let info_re = regex::Regex::new(&format!(
        r#"<page id="{}"\s+width="([\d.]+)"\s+height="([\d.]+)""#,
        regex::escape(page_num)
    ))
    .ok()?;
    let output = Command::new("mutool")
        .args(["info", "-M", &pdf_path])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let caps = info_re.captures(&text)?;
    let w_pt: f64 = caps[1].parse().ok()?;
    let h_pt: f64 = caps[2].parse().ok()?;
    let scale = dpi as f64 / 72.0;
    Some(((w_pt * scale).round() as u32, (h_pt * scale).round() as u32))
}

/// Render inline/display math in plain text to .math-deferred elements
/// for image captions (where math was left raw because protect_page_images
/// runs before protect_math).
fn render_math_in_text(text: &str) -> String {
    // Protect escaped dollar signs so \$ is not treated as math delimiter
    let text = text.replace(r"\$", "\u{7f}ESCDOLLAR\u{7f}");

    // Display math $$...$$
    let text = MATH_DISPLAY_RE
        .replace_all(&text, |caps: &regex::Captures| {
            let full = caps.get(0).map(|m| m.as_str()).unwrap_or("");
            let latex = full[2..full.len() - 2].trim();
            format!(
                r#"<div class="math-deferred" data-latex="{}" data-display="true">{}</div>"#,
                escape_html(latex),
                full
            )
        })
        .to_string();

    // Inline math $...$ (skip price-like content)
    let text = MATH_INLINE_RE
        .replace_all(&text, |caps: &regex::Captures| {
            let full = caps.get(0).map(|m| m.as_str()).unwrap_or("");
            let content = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let trimmed = content.trim();
            if PRICE_LIKE_RE.is_match(trimmed) {
                return full.to_string();
            }
            if content != trimmed {
                return full.to_string();
            }
            format!(
                r#"<span class="math-deferred" data-latex="{}" data-display="false">{}</span>"#,
                escape_html(trimmed),
                full
            )
        })
        .to_string();

    // Restore escaped dollar signs so \$ becomes literal $
    text.replace("\u{7f}ESCDOLLAR\u{7f}", "$")
}

/// Inputs for rendering a single block (left/right/center) image.
struct BlockImage<'a> {
    rotate: Option<i32>,
    /// Pre-built `transform:rotate(...);transform-origin:center;` for the `<img>`.
    img_rotate_style: &'a str,
    /// User width/height (already unit-suffixed, e.g. "47%", "300px").
    w: &'a str,
    h: &'a str,
    /// Width/height swapped for 90/270 (used by the no-natural-dims fallback).
    render_w: &'a str,
    render_h: &'a str,
    /// Natural pixel dimensions of the source image (0,0 when unknown).
    img_w: u32,
    img_h: u32,
    img_attrs: &'a str,
    alt: &'a str,
    /// `true` for left/right (width + `max-width:100%` go on the wrapper),
    /// `false` for centered (width goes on the `<img>`).
    floated: bool,
}

/// Render the inner markup for a block (left/right/center) image plus any extra
/// CSS that must be merged into the alignment wrapper.
///
/// Returns `(extra_wrapper_style, inner_html)`. When the image is rotated
/// 90/270 with known natural dimensions, `inner_html` is a clipped box nesting
/// a rotating rot box that holds the `<img>`; otherwise it is the bare `<img>`
/// and any width/max-width lives on `extra_wrapper_style`.
fn render_block_image(img: &BlockImage) -> (String, String) {
    let BlockImage {
        rotate,
        img_rotate_style,
        w,
        h,
        render_w,
        render_h,
        img_w,
        img_h,
        img_attrs,
        alt,
        floated,
    } = *img;

    let mut extra_wrapper = String::new();

    // 90/270 rotation with known natural dims: three-layer box > rot > img.
    if let Some(deg) = rotate.filter(|d| *d == 90 || *d == 270) {
        if let Some((wrapper_prefix, box_style, rot_style, img_style)) =
            rotated_image_styles(w, h, img_w, img_h, deg, floated)
        {
            extra_wrapper.push_str(&wrapper_prefix);
            let img_html = format!(
                "<img alt=\"{}\" {} style=\"{}\" loading=\"lazy\" decoding=\"async\">",
                alt, img_attrs, img_style
            );
            let inner = format!(
                "<div style=\"{}\"><div style=\"{}\">{}</div></div>",
                box_style, rot_style, img_html
            );
            return (extra_wrapper, inner);
        }
    }

    // Everything else (no rotation, 180, or 90/270 without natural dims): a
    // bare `<img>` whose width/max-width go on the wrapper or the img itself.
    let mut img_style = String::new();
    if rotate == Some(90) || rotate == Some(270) {
        // 90/270 fallback (external image, no natural dims): swapped box.
        push_dim(&mut extra_wrapper, "width", render_w);
        push_dim(&mut extra_wrapper, "height", render_h);
        push_dim(&mut img_style, "width", w);
        push_dim(&mut img_style, "height", h);
        img_style.push_str(img_rotate_style);
    } else {
        if floated {
            push_dim(&mut extra_wrapper, "width", w);
            extra_wrapper.push_str("max-width:100%;");
            push_dim(&mut img_style, "height", h);
            img_style.push_str("max-width:100%;");
        } else {
            push_dim(&mut img_style, "width", w);
            push_dim(&mut img_style, "height", h);
        }
        if rotate == Some(180) {
            img_style.push_str(img_rotate_style);
        }
    }

    let img_html = format!(
        "<img alt=\"{}\" {} style=\"{}\" loading=\"lazy\" decoding=\"async\">",
        alt, img_attrs, img_style
    );
    (extra_wrapper, img_html)
}

/// Append `prop:value;` to `style` only when `value` is non-empty.
fn push_dim(style: &mut String, prop: &str, value: &str) {
    if !value.is_empty() {
        style.push_str(&format!("{}:{};", prop, value));
    }
}

fn restore_page_images(html: &str, source: &str, paper_id: &str, images: &[PageImage]) -> String {
    let mut result = html.to_string();
    let enc_source = urlencoding::encode(source);
    let enc_id = urlencoding::encode(paper_id);
    for (i, img) in images.iter().enumerate() {
        let (src, thumb) = match &img.kind {
            PageImageKind::Page(p) => (
                format!("/api/papers/{}/{}/page/{}", enc_source, enc_id, p),
                format!("/api/papers/{}/{}/page/{}/thumb", enc_source, enc_id, p),
            ),
            PageImageKind::Figure(n) => (
                format!("/api/papers/{}/{}/figure/{}", enc_source, enc_id, n),
                format!("/api/papers/{}/{}/figure/{}/thumb", enc_source, enc_id, n),
            ),
            PageImageKind::Table(n) => (
                format!("/api/papers/{}/{}/table/{}", enc_source, enc_id, n),
                format!("/api/papers/{}/{}/table/{}/thumb", enc_source, enc_id, n),
            ),
            PageImageKind::External(url) => (url.clone(), url.clone()),
        };
        let placeholder = format!("%%PAGEIMG_{}%%", i);
        let w = dim_unit(img.width.as_deref().unwrap_or(""));
        let h = dim_unit(img.height.as_deref().unwrap_or(""));

        // Swap width/height for 90/270 degree rotations so the rendered box
        // matches the rotated image's bounding box.
        let (render_w, render_h) = if img.rotate == Some(90) || img.rotate == Some(270) {
            (h.clone(), w.clone())
        } else {
            (w.clone(), h.clone())
        };

        // Build rotation style for the image itself. For block images the
        // wrapper reserves the swapped bounding box; inline rotation is left
        // as a TODO because layout semantics are undefined.
        let rotate = img.rotate;
        let img_rotate_style = rotate
            .map(|deg| format!("transform:rotate({}deg);transform-origin:center;", deg))
            .unwrap_or_default();

        // LQIP: use thumb as src, full version in data-full-src
        // Include width/height from actual image for CLS-free layout
        let fig_dir = crate::web::paper_figures_dir(source, paper_id);
        let (img_w, img_h) = match &img.kind {
            PageImageKind::Page(p) => page_dimensions(source, paper_id, p, 150),
            PageImageKind::Figure(n) | PageImageKind::Table(n) => {
                let hires_path = format!("{}/hires/{}.png", fig_dir, n);
                png_dimensions(&hires_path)
            }
            PageImageKind::External(_) => None,
        }
        .unwrap_or((0, 0));
        // Swap intrinsic dimensions for 90/270 degree rotations so the browser
        // reserves layout space for the rotated visual box.
        let (attr_w, attr_h) = if img.rotate == Some(90) || img.rotate == Some(270) {
            (img_h, img_w)
        } else {
            (img_w, img_h)
        };
        let dims_attr = if attr_w > 0 && attr_h > 0 {
            format!(" width=\"{}\" height=\"{}\"", attr_w, attr_h)
        } else {
            String::new()
        };
        let img_attrs = format!(
            "class=\"progressive-img\" src=\"{}\" data-full-src=\"{}\"{}",
            thumb, src, dims_attr
        );

        // For non-rotated images we preserve the original layout behaviour:
        // center: width/height on the <img>, wrapper has no sizing;
        // left/right: width on the wrapper, height on the <img> with max-width:100%.
        // For rotated images the wrapper gets the swapped bounding box and the
        // <img> keeps the original dimensions plus the transform.
        let replacement = if img.align == "inline" {
            // TODO: support rotation for inline images.
            let mut img_style = String::new();
            if !w.is_empty() {
                img_style.push_str(&format!("width:{};", w));
            }
            if !h.is_empty() {
                img_style.push_str(&format!("height:{};", h));
            }
            if !img_style.is_empty() {
                img_style.push_str("vertical-align:middle;");
            } else {
                img_style = "vertical-align:middle;".to_string();
            }
            format!(
                "<img alt=\"{}\" {} style=\"{}\" loading=\"lazy\" decoding=\"async\">",
                escape_html(&img.alt),
                img_attrs,
                img_style
            )
        } else {
            // Block image: left/right (floated) or center.
            let floated = img.align == "left" || img.align == "right";
            let mut wrapper_style = if floated {
                let mut s = format!("float:{};text-align:center;", img.align);
                if img.align == "left" {
                    s.push_str("margin:0 16px 8px 0;");
                } else {
                    s.push_str("margin:0 0 8px 16px;");
                }
                s
            } else {
                "text-align:center;margin:16px 0;".to_string()
            };
            let (extra, inner) = render_block_image(&BlockImage {
                rotate,
                img_rotate_style: &img_rotate_style,
                w: &w,
                h: &h,
                render_w: &render_w,
                render_h: &render_h,
                img_w,
                img_h,
                img_attrs: &img_attrs,
                alt: &escape_html(&img.alt),
                floated,
            });
            wrapper_style.push_str(&extra);
            let caption = render_math_in_text(&img.alt);
            // word-break helps long captions wrap inside a floated (narrow) column.
            let caption_style = if floated {
                "font-size:13px;color:var(--text3);margin-top:6px;word-break:break-word;"
            } else {
                "font-size:13px;color:var(--text3);margin-top:6px;"
            };
            format!(
                "<div style=\"{}\">{}<div style=\"{}\">{}</div></div>",
                wrapper_style, inner, caption_style, caption
            )
        };

        result = result.replace(&placeholder, &replacement);
    }
    result
}

fn restore_math(html: &str, blocks: &[String]) -> String {
    let mut result = html.to_string();
    for (i, block) in blocks.iter().enumerate() {
        let is_display = block.starts_with("$$");
        let latex = if is_display {
            &block[2..block.len() - 2]
        } else {
            &block[1..block.len() - 1]
        }
        .trim()
        .to_string();
        let tag = if is_display { "div" } else { "span" };
        let replacement = format!(
            "<{} class=\"math-deferred\" data-latex=\"{}\" data-display=\"{}\">{}</{}>",
            tag,
            escape_html(&latex),
            is_display,
            escape_html(block),
            tag
        );
        result = result.replace(&format!("%%MATH_{}%%", i), &replacement);
    }
    result
}

fn restore_algorithms(html: &str, blocks: &[String]) -> String {
    let mut result = html.to_string();
    for (i, block) in blocks.iter().enumerate() {
        result = result.replace(&format!("%%ALGO_{}%%", i), block);
    }
    result
}

fn fix_cjk_autolinks(html: &str) -> String {
    CJK_AUTOLINK_RE
        .replace_all(html, |caps: &regex::Captures| {
            let attrs = &caps[1];
            let url = &caps[2];
            let rest = &caps[3];
            // Reconstruct href pointing to the correct URL
            let fixed_attrs = HREF_ATTR_RE.replace(attrs, format!("href=\"{}\"", url));
            format!("<a{}>{}</a>{}", fixed_attrs, url, rest)
        })
        .to_string()
}

fn wrap_tables(html: &str) -> String {
    static TABLE_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"<table>[\s\S]*?</table>").unwrap());
    TABLE_RE
        .replace_all(html, |caps: &regex::Captures| {
            format!("<div class=\"table-scroll\">{}</div>", &caps[0])
        })
        .to_string()
}

fn number_headings_and_toc(html: &str) -> (String, Vec<TocItem>) {
    let mut toc = Vec::new();
    let mut toc_id_counter = 0u32;

    // Find all current headings to determine min level
    let mut levels: Vec<usize> = Vec::new();
    for caps in HEADING_RE.captures_iter(html) {
        let level: usize = caps[1].parse().unwrap_or(1);
        levels.push(level);
    }
    let min_level = levels.iter().min().copied().unwrap_or(1);

    let mut counts = [0u32; 4]; // h1..h4
    let result = HEADING_RE
        .replace_all(html, |caps: &regex::Captures| {
            let level: usize = caps[1].parse().unwrap_or(1);
            let attrs = caps[2].to_string();
            let text = caps[3].to_string();

            // Increment counter and zero out deeper levels
            counts[level - 1] += 1;
            for count in counts.iter_mut().skip(level).take(4 - level) {
                *count = 0;
            }

            // Build number string
            let mut num_parts = Vec::new();
            for l in min_level..=level {
                num_parts.push(counts[l - 1].to_string());
            }
            let num = num_parts.join(".");

            // Generate id if missing
            let id = if attrs.contains("id=") {
                // Extract existing id
                ID_ATTR_RE
                    .captures(&attrs)
                    .map(|c| c[1].to_string())
                    .unwrap_or_else(|| {
                        let id = format!("toc-h-{}", toc_id_counter);
                        toc_id_counter += 1;
                        id
                    })
            } else {
                let id = format!("toc-h-{}", toc_id_counter);
                toc_id_counter += 1;
                id
            };

            // Clean text for TOC (strip HTML tags)
            let clean_text = STRIP_HTML_RE.replace_all(&text, "").to_string();

            toc.push(TocItem {
                id: id.clone(),
                level: level as u8,
                text: clean_text.clone(),
                num: num.clone(),
            });

            let id_attr = if attrs.contains("id=") {
                String::new()
            } else {
                format!(" id=\"{}\"", id)
            };

            let num_attr = format!(" data-heading-num=\"{}\"", escape_html(&num));

            format!(
                "<h{}{}{}{}>{}</h{}>",
                level, attrs, id_attr, num_attr, text, level
            )
        })
        .to_string();

    (result, toc)
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Sanitize rendered markdown HTML to prevent XSS while preserving
/// markdown/math/image/table/code constructs that the frontend expects.
fn sanitize_markdown_html(html: &str) -> String {
    let mut builder = Builder::default();
    // Allow data-* attributes (used for data-latex, data-display, etc.)
    builder.add_generic_attribute_prefixes(&["data-"]);
    // Allow class/style/id on all elements
    builder.add_generic_attributes(&["class", "id", "style", "loading", "decoding"]);
    // Add math/image-specific attrs to img
    builder.add_tag_attributes("img", &["alt", "src", "width", "height", "data-full-src"]);
    // Allow table-related attrs
    builder.add_tag_attributes("td", &["colspan", "rowspan"]);
    builder.add_tag_attributes("th", &["colspan", "rowspan"]);
    // Allow heading numbering
    builder.add_tag_attributes("h1", &["data-heading-num"]);
    builder.add_tag_attributes("h2", &["data-heading-num"]);
    builder.add_tag_attributes("h3", &["data-heading-num"]);
    // Only allow http/https URL schemes
    builder.url_schemes(["http", "https"].iter().cloned().collect());
    // Add rel="noopener noreferrer" to external links
    builder.link_rel(Some("noopener noreferrer"));
    // Keep relative URLs intact
    builder.url_relative(ammonia::UrlRelative::PassThrough);
    builder.clean(html).to_string()
}

#[cfg(test)]
mod rotation_tests {
    use super::*;

    #[test]
    fn rotated_centered_keeps_user_width_on_box() {
        // Centered: `=47%` is the FINAL displayed width, on the box itself.
        // For a 1000x600 (landscape) image rotated 90deg, the visual box is
        // portrait: width 47%, aspect-ratio 600/1000.
        let (wrapper, box_style, rot_style, img_style) =
            rotated_image_styles("47%", "", 1000, 600, 90, false).expect("natural dims known");
        assert!(
            wrapper.is_empty(),
            "centered image puts width on the box, not the wrapper: {wrapper}"
        );
        assert!(
            box_style.contains("width:47%;aspect-ratio:600 / 1000;"),
            "visual box must use the user width with swapped aspect: {box_style}"
        );
        assert!(
            box_style.contains("overflow:hidden;position:relative;"),
            "box must clip and establish positioning context: {box_style}"
        );
        // rot box is the box with width/height swapped, so it fills the box
        // after rotation. Width = calc(img_w/img_h*100%) (= box height),
        // height = calc(img_h/img_w*100%) (= box width).
        assert!(
            rot_style.contains("width:calc(1000 / 600 * 100%);"),
            "rot width = box height: {rot_style}"
        );
        assert!(
            rot_style.contains("height:calc(600 / 1000 * 100%);"),
            "rot height = box width: {rot_style}"
        );
        assert!(
            rot_style.contains("transform:translate(-50%,-50%) rotate(90deg);"),
            "rot box is centered then rotated: {rot_style}"
        );
        // img is a plain fill — no calc, no max-width hack.
        assert!(
            img_style.contains("width:100%;height:100%;"),
            "img must be a plain 100% fill: {img_style}"
        );
        assert!(
            !img_style.contains("calc("),
            "img must not use calc: {img_style}"
        );
    }

    #[test]
    fn rotated_floated_puts_user_width_on_wrapper() {
        // Floated: the percentage must resolve against the text column, so it
        // lives on the wrapper and the box fills it (width:100%).
        let (wrapper, box_style, _rot, _img) =
            rotated_image_styles("47%", "", 1000, 600, 90, true).unwrap();
        assert!(
            wrapper.contains("width:47%;"),
            "floated image must carry the user width on the wrapper: {wrapper}"
        );
        assert!(
            box_style.contains("width:100%;aspect-ratio:600 / 1000;"),
            "box must fill the floated wrapper: {box_style}"
        );
    }

    #[test]
    fn rotated_both_dims_use_user_box_as_visual_box() {
        // User dims describe the final visual box (not swapped).
        let (_wrapper, box_style, _rot, _img) =
            rotated_image_styles("400px", "300px", 1000, 600, 270, false).unwrap();
        assert!(
            box_style.contains("width:400px;height:300px;"),
            "visual box = user box: {box_style}"
        );
    }

    #[test]
    fn rotated_unknown_dims_returns_none() {
        // External images have no natural dimensions -> caller falls back.
        assert!(rotated_image_styles("47%", "", 0, 0, 90, false).is_none());
    }

    #[test]
    fn floated_rotated_image_nests_three_layers() {
        // The full block render for a floated, rotated image must put the
        // user width on the wrapper, nest a width:100% clipping box, then a
        // rot box holding the plain-fill <img>, with the caption as a sibling
        // of the box (not clipped).
        let (extra, inner) = render_block_image(&BlockImage {
            rotate: Some(90),
            img_rotate_style: "transform:rotate(90deg);transform-origin:center;",
            w: "47%",
            h: "",
            render_w: "",
            render_h: "47%",
            img_w: 1000,
            img_h: 600,
            img_attrs: "class=\"progressive-img\" src=\"t\"",
            alt: "fig",
            floated: true,
        });
        assert!(
            extra.contains("width:47%;"),
            "wrapper carries the user width: {extra}"
        );
        assert!(
            inner.contains("width:100%;aspect-ratio:600 / 1000;"),
            "box fills the wrapper: {inner}"
        );
        assert!(
            inner.contains("width:calc(1000 / 600 * 100%);"),
            "rot width = box height (swapped): {inner}"
        );
        assert!(
            inner.contains("height:calc(600 / 1000 * 100%);"),
            "rot height = box width (swapped): {inner}"
        );
        assert!(
            inner.contains("width:100%;height:100%;"),
            "img is a plain fill: {inner}"
        );
        // Structure: <div box><div rot><img></div></div>
        assert!(
            inner.matches("<div style=\"").count() == 2,
            "exactly two nested divs (box + rot): {inner}"
        );
    }
}
