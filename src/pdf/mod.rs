// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::{Context, Result};
use std::path::Path;
use std::time::Duration;
use tracing::{info, warn};

pub mod mutool;

const PDF_DIR: &str = "pdf";
const PDF_MAX_RETRIES: usize = 12;

fn is_valid_pdf(data: &[u8]) -> bool {
    data.starts_with(b"%PDF")
}

pub async fn download_pdf(source: &str, id: &str, url: &str) -> Result<Vec<u8>> {
    let path = Path::new(PDF_DIR).join(source).join(format!("{}.pdf", id));
    info!("[pdf] Resolved PDF path: {}", path.display());

    if path.exists() {
        info!("[pdf] Cache hit for {}", id);
        let data = tokio::fs::read(&path)
            .await
            .context("Failed to read cached PDF")?;
        if is_valid_pdf(&data) {
            info!("[pdf] Read {} bytes from cache", data.len());
            return Ok(data);
        }
        warn!(
            "[pdf] Cached PDF is incomplete/invalid for {}, re-downloading",
            id
        );
        let _ = tokio::fs::remove_file(&path).await;
    }

    let mut last_err = None;
    for attempt in 0..=PDF_MAX_RETRIES {
        info!(
            "[pdf] Downloading from {} (attempt {}/{})",
            url,
            attempt + 1,
            PDF_MAX_RETRIES + 1
        );
        match try_download(url).await {
            Ok(data) => {
                if !is_valid_pdf(&data) {
                    anyhow::bail!("Downloaded data is not a valid PDF");
                }

                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await.with_context(|| {
                        format!("Failed to create pdf directory: {}", parent.display())
                    })?;
                    info!("[pdf] Ensured directory '{}' exists", parent.display());
                }

                // Write to a temp file and rename atomically to avoid incomplete files on crash
                let tmp_path = path.with_extension("tmp");
                tokio::fs::write(&tmp_path, &data)
                    .await
                    .context("Failed to write PDF temp file")?;
                tokio::fs::rename(&tmp_path, &path)
                    .await
                    .context("Failed to rename PDF temp file")?;
                info!("[pdf] Saved PDF to {}", path.display());

                return Ok(data);
            }
            Err(e) => {
                warn!("[pdf] Download attempt {} failed: {}", attempt + 1, e);
                last_err = Some(e);
                if attempt < PDF_MAX_RETRIES {
                    let delay = std::cmp::min(10_000u64 * (1u64 << attempt), 300_000u64);
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
            }
        }
    }

    let err_msg = last_err
        .map(|e| e.to_string())
        .unwrap_or_else(|| "unknown error".to_string());
    anyhow::bail!(
        "PDF download failed after {} attempts: {}",
        PDF_MAX_RETRIES + 1,
        err_msg
    )
}

async fn try_download(url: &str) -> Result<Vec<u8>> {
    let resp = reqwest::get(url).await.context("PDF request failed")?;
    let status = resp.status();
    info!("[pdf] Download response status: {}", status);
    if !status.is_success() {
        anyhow::bail!("PDF download returned status {}", status);
    }
    let bytes = resp
        .bytes()
        .await
        .context("Failed to read PDF response body")?;
    let data = bytes.to_vec();
    info!("[pdf] Downloaded {} bytes", data.len());
    Ok(data)
}
