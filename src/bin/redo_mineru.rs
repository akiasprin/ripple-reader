// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::{Context, Result};
use ripple_reader::config::Config;
use ripple_reader::mineru::MinerUClient;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use tracing::{info, warn};

async fn write_mineru_log(id: &str, msg: &str, append: bool) {
    let path = format!("figures/{id}/mineru.log");
    let mut opts = tokio::fs::OpenOptions::new();
    opts.create(true).write(true);
    if append {
        opts.append(true);
    } else {
        opts.truncate(true);
    }
    let mut file = match opts.open(&path).await {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[redo_mineru] failed to open {path}: {e}");
            return;
        }
    };
    let _ = file.write_all(format!("{msg}\n").as_bytes()).await;
    let _ = file.flush().await;
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,ripple_reader=debug")),
        )
        .with_file(true)
        .with_line_number(true)
        .init();

    let args: Vec<String> = std::env::args().collect();

    let mut offset = 0usize;
    let mut limit = 0usize;
    let mut paper_id: Option<String> = None;
    let mut no_img = false;
    let mut hires_only: Option<String> = None;
    let mut workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let figures_dir = "figures".to_string();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                println!("Usage: redo_mineru [OPTIONS]");
                println!();
                println!("Re-process MinerU extraction for papers in figures/");
                println!();
                println!("Options:");
                println!(
                    "  --paper <ID>    Process only this paper (requires figures/<ID>/mineru.zip)"
                );
                println!("  --offset <N>    Skip first N papers");
                println!("  --limit <N>     Process at most N papers");
                println!("  --no-img         Skip hires PNG generation");
                println!(
                    "  --hires-only <ID>  Generate only hires PNG for this paper (child mode)"
                );
                println!("  --workers <N>   Concurrent workers (default: CPU count)");
                println!("  -h, --help       Print this help");
                return Ok(());
            }
            "--offset" => {
                i += 1;
                if i < args.len() {
                    offset = args[i].parse().unwrap_or(0);
                }
            }
            "--limit" => {
                i += 1;
                if i < args.len() {
                    limit = args[i].parse().unwrap_or(0);
                }
            }
            "--paper" => {
                i += 1;
                if i < args.len() {
                    paper_id = Some(args[i].clone());
                }
            }
            "--no-img" => no_img = true,
            "--hires-only" => {
                i += 1;
                if i < args.len() {
                    hires_only = Some(args[i].clone());
                }
            }
            "--workers" => {
                i += 1;
                if i < args.len() {
                    workers = args[i].parse().unwrap_or(workers);
                }
            }
            _ => {}
        }
        i += 1;
    }

    // Child-process mode: only generate hires for a single paper and exit.
    if let Some(ref pid) = hires_only {
        write_mineru_log(pid, "generating hires screenshots...", true).await;
        let hires_dir = format!("figures/{}/hires", pid);
        if Path::new(&hires_dir).exists() {
            if let Err(e) = tokio::fs::remove_dir_all(&hires_dir).await {
                warn!(
                    "[redo_mineru] {} failed to remove old hires dir: {}",
                    pid, e
                );
                write_mineru_log(pid, &format!("failed to remove old hires dir: {e}"), true).await;
            }
        }
        // Parse source and bare id from path like "arxiv/2401.12345"
        let (source, raw_id) = pid.rsplit_once('/').unwrap_or(("arxiv", pid.as_str()));
        let pdf_url = format!("https://arxiv.org/pdf/{}.pdf", raw_id);
        match ripple_reader::hires::generate_hires_images(source, raw_id, &pdf_url, 600, false)
            .await
        {
            Ok(_) => {
                info!("[redo_mineru] --hires-only {} done", pid);
                write_mineru_log(pid, "hires screenshots generated", true).await;
                return Ok(());
            }
            Err(e) => {
                warn!("[redo_mineru] --hires-only {} failed: {}", pid, e);
                write_mineru_log(
                    pid,
                    &format!("hires screenshot generation failed: {e}"),
                    true,
                )
                .await;
                return Err(e);
            }
        }
    }

    // Scan figures/ directory recursively for papers with mineru.zip.
    // Relative path from figures/ becomes the paper id (e.g. arxiv/2401.12345).
    let mut ids: Vec<String> = Vec::new();
    fn walk(dir: &std::path::Path, base: &std::path::Path, ids: &mut Vec<String>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.join("mineru.zip").exists() {
                        if let Ok(rel) = path.strip_prefix(base) {
                            ids.push(rel.to_string_lossy().to_string());
                        }
                    }
                    walk(&path, base, ids);
                }
            }
        }
    }
    walk(Path::new(&figures_dir), Path::new(&figures_dir), &mut ids);
    ids.sort();

    if let Some(ref id) = paper_id {
        if ids.contains(id) {
            ids.retain(|x| x == id);
        } else {
            // Try suffix match: bare id like "2605.28691" -> "arxiv/2605.28691"
            let matches: Vec<String> = ids
                .iter()
                .filter(|full| {
                    full.strip_prefix('/')
                        .unwrap_or(full)
                        .split('/')
                        .next_back()
                        == Some(id)
                })
                .cloned()
                .collect();
            if matches.len() == 1 {
                let matched = matches[0].clone();
                ids.retain(|x| x == &matched);
            } else if matches.is_empty() {
                anyhow::bail!(
                    "Paper '{}' not found in {}/ (no mineru.zip)",
                    id,
                    figures_dir
                );
            } else {
                anyhow::bail!(
                    "Paper '{}' is ambiguous: found multiple matches: {}",
                    id,
                    matches.join(", ")
                );
            }
        }
    }

    let total = ids.len();
    let end = if limit == 0 {
        total
    } else {
        (offset + limit).min(total)
    };
    let ids = &ids[offset..end];

    info!(
        "[redo_mineru] Found {} papers in {}/, processing [{}, {}), img={}, workers={}",
        total, figures_dir, offset, end, !no_img, workers
    );

    let cfg = Config::from_env()?;
    let api_key = cfg
        .mineru_api_key
        .ok_or_else(|| anyhow::anyhow!("MINERU_API_KEY not set"))?;

    let client = Arc::new(MinerUClient::new(
        cfg.mineru_base_url,
        api_key,
        Some(figures_dir),
        cfg.mineru_no_cache,
    ));

    let semaphore = Arc::new(Semaphore::new(workers));
    let mut handles = Vec::with_capacity(ids.len());
    let done_counter = Arc::new(AtomicUsize::new(0));

    for id in ids.iter() {
        let id = id.clone();
        let client = client.clone();
        let permit = semaphore.clone();
        let done = done_counter.clone();

        let split_figures: Vec<String> = {
            let meta_path = format!("figures/{}/mineru.json", id);
            if Path::new(&meta_path).exists() {
                match tokio::fs::read_to_string(&meta_path).await {
                    Ok(json) => serde_json::from_str::<ripple_reader::mineru::CacheMeta>(&json)
                        .map(|m| m.split_figures)
                        .unwrap_or_default(),
                    Err(_) => Vec::new(),
                }
            } else {
                Vec::new()
            }
        };

        handles.push(tokio::spawn(async move {
            let _guard = permit.acquire().await.unwrap();
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            info!("[redo_mineru] [{}/{}] Processing {} ...", n, total, id);
            write_mineru_log(&id, &format!("[{n}/{total}] starting reprocess..."), false).await;

            match client
                .reprocess_from_zip(&id, &split_figures, true, no_img)
                .await
            {
                Ok(extracted) => {
                    info!(
                        "[redo_mineru] [{}/{}] {} OK ({} figures)",
                        n,
                        total,
                        id,
                        extracted.images.len()
                    );
                    write_mineru_log(
                        &id,
                        &format!(
                            "[{n}/{total}] reprocess done ({} figures)",
                            extracted.images.len()
                        ),
                        true,
                    )
                    .await;
                    Ok(())
                }
                Err(e) => {
                    warn!("[redo_mineru] [{}/{}] {} failed: {}", n, total, id, e);
                    write_mineru_log(&id, &format!("[{n}/{total}] reprocess failed: {e}"), true)
                        .await;
                    Err(e)
                }
            }
        }));
    }

    let mut success_count = 0usize;
    let mut fail_count = 0usize;

    for handle in handles {
        match handle.await {
            Ok(Ok(())) => success_count += 1,
            Ok(Err(_)) => fail_count += 1,
            Err(e) => {
                warn!("[redo_mineru] Join error: {}", e);
                fail_count += 1;
            }
        }
    }

    info!(
        "[redo_mineru] Reprocess done. Range [{}, {}). Success: {}, Failed: {}",
        offset, end, success_count, fail_count
    );

    // Hires generation via multi-process (pdfium has a global lock per process).
    if !no_img {
        let exe = std::env::current_exe().context("Failed to get current exe path")?;

        // Remove old hires dirs first.
        for id in ids {
            let hires_dir = format!("figures/{}/hires", id);
            if Path::new(&hires_dir).exists() {
                if let Err(e) = tokio::fs::remove_dir_all(&hires_dir).await {
                    warn!("[redo_mineru] {} failed to remove old hires dir: {}", id, e);
                }
            }
        }

        let semaphore = Arc::new(Semaphore::new(workers));
        let mut handles = Vec::with_capacity(ids.len());
        let total = ids.len();
        let done_counter = Arc::new(AtomicUsize::new(0));

        for id in ids.iter() {
            let id = id.clone();
            let exe = exe.clone();
            let permit = semaphore.clone();
            let done = done_counter.clone();

            handles.push(tokio::spawn(async move {
                let _guard = permit.acquire().await.unwrap();
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                info!("[redo_mineru] hires [{}/{}] Starting {} ...", n, total, id);

                let output = tokio::process::Command::new(&exe)
                    .arg("--hires-only")
                    .arg(&id)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .output()
                    .await;

                match output {
                    Ok(out) if out.status.success() => {
                        info!("[redo_mineru] hires [{}/{}] {} OK", n, total, id);
                        Ok(())
                    }
                    Ok(out) => {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        warn!(
                            "[redo_mineru] hires [{}/{}] {} FAILED: {}",
                            n,
                            total,
                            id,
                            stderr.trim()
                        );
                        Err(anyhow::anyhow!("{}", stderr.trim()))
                    }
                    Err(e) => {
                        warn!(
                            "[redo_mineru] hires [{}/{}] {} spawn error: {}",
                            n, total, id, e
                        );
                        Err(anyhow::anyhow!("spawn: {}", e))
                    }
                }
            }));
        }

        let mut hires_ok = 0usize;
        let mut hires_fail = 0usize;

        for handle in handles {
            match handle.await {
                Ok(Ok(())) => hires_ok += 1,
                Ok(Err(_)) => hires_fail += 1,
                Err(e) => {
                    warn!("[redo_mineru] hires join error: {}", e);
                    hires_fail += 1;
                }
            }
        }

        info!(
            "[redo_mineru] Hires done. OK: {}, Failed: {}",
            hires_ok, hires_fail
        );
    }

    Ok(())
}
