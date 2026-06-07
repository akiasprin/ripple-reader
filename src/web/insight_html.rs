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
    Regex::new(r#"!\[([\s\S]*?)\]\((https?://[^)\s]+)(?:\s*=\s*(\d*(?:\.\d+)?(?:%|px)?)(?:x(\d*(?:\.\d+)?(?:%|px)?))?(?:\s+(left|right|inline|center))?)?\)"#).unwrap()
});

static PAGE_IMG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"!\[([\s\S]*?)\]\(page://(\d+)(?:\s*=\s*(\d*(?:\.\d+)?(?:%|px)?)(?:x(\d*(?:\.\d+)?(?:%|px)?))?(?:\s+(left|right|inline|center))?)?\)"#).unwrap()
});

static FIGURE_IMG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"!\[([\s\S]*?)\]\(figure/([a-zA-Z0-9_.-]+)(?:\s*=\s*(\d*(?:\.\d+)?(?:%|px)?)(?:x(\d*(?:\.\d+)?(?:%|px)?))?(?:\s+(left|right|inline|center))?)?\)"#).unwrap()
});

static TABLE_IMG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"!\[([\s\S]*?)\]\(table/([a-zA-Z0-9_.-]+)(?:\s*=\s*(\d*(?:\.\d+)?(?:%|px)?)(?:x(\d*(?:\.\d+)?(?:%|px)?))?(?:\s+(left|right|inline|center))?)?\)"#).unwrap()
});

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
        let mut style = String::new();
        if !w.is_empty() {
            style.push_str(&format!("width:{};", w));
        }
        if !h.is_empty() {
            style.push_str(&format!("height:{};", h));
        }
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
        let dims_attr = if img_w > 0 && img_h > 0 {
            format!(" width=\"{}\" height=\"{}\"", img_w, img_h)
        } else {
            String::new()
        };
        let img_attrs = format!(
            "class=\"progressive-img\" src=\"{}\" data-full-src=\"{}\"{}",
            thumb, src, dims_attr
        );

        let replacement = if img.align == "inline" {
            let img_style = if style.is_empty() {
                "vertical-align:middle;".to_string()
            } else {
                format!("{}vertical-align:middle;", style)
            };
            format!(
                "<img alt=\"{}\" {} style=\"{}\" loading=\"lazy\" decoding=\"async\">",
                escape_html(&img.alt),
                img_attrs,
                img_style
            )
        } else if img.align == "left" || img.align == "right" {
            let mut wrapper_style = format!("float:{};text-align:center;", img.align);
            if img.align == "left" {
                wrapper_style.push_str("margin:0 16px 8px 0;");
            } else {
                wrapper_style.push_str("margin:0 0 8px 16px;");
            }
            if !w.is_empty() {
                wrapper_style.push_str(&format!("width:{};", w));
            }
            wrapper_style.push_str("max-width:100%;");
            let img_style = if h.is_empty() {
                "max-width:100%;".to_string()
            } else {
                format!("height:{};max-width:100%;", h)
            };
            let caption = render_math_in_text(&img.alt);
            format!(
                "<div style=\"{}\"><img alt=\"{}\" {} style=\"{}\" loading=\"lazy\" decoding=\"async\"><div style=\"font-size:13px;color:var(--text3);margin-top:6px;word-break:break-word;\">{}</div></div>",
                wrapper_style,
                escape_html(&img.alt),
                img_attrs,
                img_style,
                caption
            )
        } else {
            let wrapper_style = "text-align:center;margin:16px 0;";
            let caption = render_math_in_text(&img.alt);
            format!(
                "<div style=\"{}\"><img alt=\"{}\" {} style=\"{}\" loading=\"lazy\" decoding=\"async\"><div style=\"font-size:13px;color:var(--text3);margin-top:6px;\">{}</div></div>",
                wrapper_style,
                escape_html(&img.alt),
                img_attrs,
                style,
                caption
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
