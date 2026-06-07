// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::{Context, Result};
use regex::Regex;
use std::process::Command;
use std::sync::LazyLock;
use tracing::{info, warn};

static CONCLUSION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:\d+\.?\s*)?(?:conclusion|conclusions|discussion|summary|future work|limitations)$",
    )
    .expect("invalid conclusion regex")
});

static INTRO_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:\d+\.?\s*)?(?:introduction|background|related work|problem statement|overview|motivation)$")
        .expect("invalid intro regex")
});

static STOP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:\d+\.?\s*)?(?:method|methods|methodology|approach|model|architecture|experiments|experimental|results|evaluation|references|acknowledgments|appendix|biblio|proof|theorem|lemma)\b")
        .expect("invalid stop regex")
});

pub fn extract_text(pdf_bytes: &[u8]) -> Result<String> {
    info!(
        "[extractor] Writing {} bytes to temporary PDF",
        pdf_bytes.len()
    );
    let mut tmp = tempfile::NamedTempFile::with_suffix(".pdf")?;
    std::io::Write::write_all(&mut tmp, pdf_bytes)?;
    let path = tmp.into_temp_path();
    info!("[extractor] Temporary PDF path: {}", path.display());

    info!("[extractor] Running mutool draw -F text");
    let output = Command::new("mutool")
        .args(["draw", "-F", "text", path.to_str().unwrap()])
        .output()?;

    let text = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // mutool returns a non-zero exit code on recoverable PDF syntax warnings
        // (e.g. the bogus `minorversion` keyword) while still extracting usable text.
        // Only fail when nothing came back; otherwise log and proceed with what we got.
        if text.trim().is_empty() {
            warn!("[extractor] mutool stderr: {}", stderr);
            anyhow::bail!("mutool failed: {}", stderr);
        }
        warn!(
            "[extractor] mutool exited non-zero but produced text; continuing: {}",
            stderr
        );
    }

    info!(
        "[extractor] Extracted text length={} chars, lines={}",
        text.len(),
        text.lines().count()
    );
    Ok(text.into_owned())
}

/// Render all PDF pages to PNG images using mutool draw at `dpi` resolution.
pub fn pdf_to_images(pdf_bytes: &[u8], dpi: u32) -> Result<Vec<Vec<u8>>> {
    info!(
        "[extractor] Writing {} bytes to temporary PDF for image rendering",
        pdf_bytes.len()
    );
    let mut tmp = tempfile::NamedTempFile::with_suffix(".pdf")?;
    std::io::Write::write_all(&mut tmp, pdf_bytes)?;
    let path = tmp.into_temp_path();
    info!("[extractor] Temporary PDF path: {}", path.display());

    let out_dir = tempfile::tempdir()?;
    let out_pattern = out_dir.path().join("page-%d.png");
    info!(
        "[extractor] Running mutool draw -o {} -r {}",
        out_pattern.display(),
        dpi
    );

    let output = Command::new("mutool")
        .args([
            "draw",
            "-o",
            out_pattern.to_str().unwrap(),
            "-r",
            &dpi.to_string(),
            path.to_str().unwrap(),
        ])
        .output()?;

    // Recoverable PDF warnings (e.g. the bogus `minorversion` keyword) make mutool exit
    // non-zero even though pages still render, so defer the verdict until we see the output.
    let status_ok = output.status.success();
    if !status_ok {
        warn!(
            "[extractor] mutool draw exited non-zero: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let mut images = Vec::new();
    let mut page_num = 1usize;
    loop {
        let page_path = out_dir.path().join(format!("page-{}.png", page_num));
        if page_path.exists() {
            let data = std::fs::read(&page_path)
                .with_context(|| format!("Failed to read {}", page_path.display()))?;
            let len = data.len();
            images.push(data);
            info!("[extractor] Loaded page {} ({} bytes)", page_num, len);
            page_num += 1;
        } else {
            info!("[extractor] No more pages after {}", page_num - 1);
            break;
        }
    }

    // Only a non-zero exit *and* zero rendered pages counts as a real failure.
    if images.is_empty() && !status_ok {
        anyhow::bail!(
            "mutool draw failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    info!("[extractor] Extracted {} pages as images", images.len());
    Ok(images)
}

#[derive(Debug, Clone)]
pub struct ExtractedSections {
    pub intro: String,
    pub conclusion: String,
    pub header: String,
}

/// Extract the raw header text (before Abstract/Introduction) to let the LLM parse authors/affiliations.
fn extract_header(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();

    let boundary = lines
        .iter()
        .position(|l| {
            let trimmed = l.trim();
            let lower = trimmed.to_lowercase();
            lower == "abstract"
                || lower.starts_with("abstract ")
                || INTRO_RE.is_match(&lower)
                || lower == "keywords"
                || lower.starts_with("keywords ")
                || lower.starts_with("key words")
        })
        .unwrap_or(lines.len().min(30));

    let header: String = lines[..boundary].join("\n");
    header.chars().take(5000).collect()
}

pub fn extract_sections(text: &str) -> ExtractedSections {
    info!("[extractor] Starting section extraction");
    let paragraphs: Vec<String> = text
        .split('\n')
        .map(|s| s.trim().to_string())
        .collect::<Vec<_>>()
        .split(|s: &String| s.is_empty())
        .map(|chunk| chunk.join(" ").trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let n = paragraphs.len().max(1);
    info!("[extractor] Non-empty paragraphs: {}", n);

    let mut best_intro_idx = None;
    let mut best_intro_score = 0.0f32;
    let mut best_conc_idx = None;
    let mut best_conc_score = 0.0f32;

    for (i, p) in paragraphs.iter().enumerate() {
        let intro_match = intro_match_score(p);
        let intro_pos = 1.0 - (i as f32 / n as f32 * 0.5);
        let intro_score = intro_match * intro_pos;
        if intro_score > best_intro_score {
            best_intro_score = intro_score;
            best_intro_idx = Some(i);
        }

        let conc_match = conclusion_match_score(p);
        let conc_pos = 0.5 + (i as f32 / n as f32 * 0.5);
        let conc_score = conc_match * conc_pos;
        if conc_score > best_conc_score {
            best_conc_score = conc_score;
            best_conc_idx = Some(i);
        }
    }

    if let Some(idx) = best_intro_idx {
        info!(
            "[extractor] Best intro match at paragraph {}/{} (score={})",
            idx + 1,
            n,
            best_intro_score
        );
    } else {
        info!("[extractor] No clear intro match found, will use fallback");
    }
    if let Some(idx) = best_conc_idx {
        info!(
            "[extractor] Best conclusion match at paragraph {}/{} (score={})",
            idx + 1,
            n,
            best_conc_score
        );
    } else {
        info!("[extractor] No clear conclusion match found, will use fallback");
    }

    let intro_text = if let Some(start) = best_intro_idx {
        let end = best_conc_idx
            .filter(|c| *c > start)
            .unwrap_or(n)
            .min(find_next_stop(&paragraphs, start + 1));
        let end = end.max(start + 1);
        info!(
            "[extractor] Intro section: paragraphs {} to {}",
            start + 1,
            end
        );
        paragraphs[start..end].join("\n\n")
    } else {
        let take = (n as f32 * 0.15).ceil() as usize;
        info!(
            "[extractor] Intro fallback: first {} paragraphs",
            take.min(n)
        );
        paragraphs[..take.min(n)].join("\n\n")
    };

    let conc_text = if let Some(start) = best_conc_idx {
        let end = find_next_stop(&paragraphs, start + 1).max(start + 1);
        info!(
            "[extractor] Conclusion section: paragraphs {} to {}",
            start + 1,
            end.min(n)
        );
        paragraphs[start..end.min(n)].join("\n\n")
    } else {
        let skip = n - (n as f32 * 0.12).ceil() as usize;
        info!(
            "[extractor] Conclusion fallback: paragraphs {} to {}",
            skip + 1,
            n
        );
        paragraphs[skip..].join("\n\n")
    };

    let header = extract_header(text);
    info!(
        "[extractor] Extracted intro_len={} chars, conclusion_len={} chars, header_len={} chars",
        intro_text.len(),
        conc_text.len(),
        header.len()
    );
    ExtractedSections {
        intro: intro_text,
        conclusion: conc_text,
        header,
    }
}

pub fn intro_match_score(text: &str) -> f32 {
    let t = text.to_lowercase();
    if INTRO_RE.is_match(&t) {
        return 1.0;
    }
    if t.contains("introduction") || t.contains("background") {
        return 0.6;
    }
    if t.contains("related work") || t.contains("overview") || t.contains("motivation") {
        return 0.45;
    }
    0.0
}

pub fn conclusion_match_score(text: &str) -> f32 {
    let t = text.to_lowercase();
    if CONCLUSION_RE.is_match(&t) {
        return 1.0;
    }
    if t.contains("conclusion") || t.contains("discussion") {
        return 0.6;
    }
    if t.contains("summary") || t.contains("future work") || t.contains("limitations") {
        return 0.45;
    }
    0.0
}

pub fn find_next_stop(paragraphs: &[String], after: usize) -> usize {
    for (i, p) in paragraphs.iter().enumerate().skip(after) {
        if STOP_RE.is_match(&p.to_lowercase()) {
            return i;
        }
    }
    paragraphs.len()
}
