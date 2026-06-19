// SPDX-License-Identifier: MIT OR Apache-2.0

use super::Paper;
use anyhow::{Context, Result};
use feed_rs::parser;
use indicatif::ProgressBar;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use tracing::{info, warn};

const BATCH_SIZE: usize = 100;

/// Configurable cache TTL in seconds (default: 48h = 172800s).
/// Hot-updatable from admin UI without restart.
static CACHE_TTL_SECS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(48 * 60 * 60);

/// Set the cache TTL (value in hours, clamped to 1–8760).
/// Called from both startup config load and admin UI updates.
/// Hot-updatable at runtime without restart.
pub fn set_cache_ttl_hours(hours: usize) {
    let secs = (hours.clamp(1, 8760) as u64) * 60 * 60;
    CACHE_TTL_SECS.store(secs, std::sync::atomic::Ordering::Relaxed);
    info!(
        "[arxiv.cache] TTL set to {}h ({}s); atomic reloaded",
        secs / 3600,
        secs
    );
}

/// Get current cache TTL in seconds.
fn cache_ttl_secs() -> u64 {
    CACHE_TTL_SECS.load(std::sync::atomic::Ordering::Relaxed)
}

static ARXIV_CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/135.0.0.0 Safari/537.36")
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to build reqwest client")
});

static OAI_CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent(concat!("RippleReader/", env!("CARGO_PKG_VERSION")))
        .default_headers({
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                reqwest::header::ACCEPT,
                "text/xml, application/xml".parse().unwrap(),
            );
            headers
        })
        .timeout(Duration::from_secs(15))
        .build()
        .expect("Failed to build OAI_CLIENT")
});

/// Normalize arXiv ID by stripping the version suffix (e.g., "2401.12345v1" -> "2401.12345").
fn normalize_arxiv_id(id: &str) -> String {
    id.split('v').next().unwrap_or(id).to_string()
}

pub async fn fetch_papers_by_ids(
    ids: &[String],
    enable_html_fallback: bool,
    progress: Option<&ProgressBar>,
) -> Result<Vec<Paper>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut all_papers = Vec::new();
    let mut failed_ids: Vec<String> = Vec::new();

    for chunk in ids.chunks(BATCH_SIZE) {
        let normalized_chunk: Vec<String> = chunk.iter().map(|id| normalize_arxiv_id(id)).collect();
        let id_list = normalized_chunk.join(",");
        let url = format!(
            "https://export.arxiv.org/api/query?id_list={}&max_results={}",
            urlencoding::encode(&id_list),
            normalized_chunk.len()
        );
        info!(
            "[arxiv] Fetching papers by ids (batch {}): {}",
            all_papers.len(),
            id_list
        );

        match fetch_single_page(&url, 3).await {
            Ok((papers, cached)) => {
                let found_ids: std::collections::HashSet<String> =
                    papers.iter().map(|p| p.id.clone()).collect();
                for id in &normalized_chunk {
                    if !found_ids.contains(id) {
                        failed_ids.push(id.clone());
                    }
                }
                all_papers.extend(papers);
                if let Some(pb) = progress {
                    pb.inc(chunk.len() as u64);
                }
                if chunk.len() == BATCH_SIZE && !cached {
                    info!("[arxiv] Rate limit: sleeping 3s between batches");
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            }
            Err(e) => {
                warn!(
                    "[arxiv] Batch failed: {}. Will fallback to Semantic Scholar.",
                    e
                );
                failed_ids.extend(normalized_chunk);
                if let Some(pb) = progress {
                    pb.inc(chunk.len() as u64);
                }
            }
        }
    }

    // Fallback to arxiv.org HTML for failed/missing IDs (only if enabled)
    if enable_html_fallback && !failed_ids.is_empty() {
        info!(
            "[arxiv] Fallback to arxiv.org HTML for {} IDs",
            failed_ids.len()
        );
        let mut html_failed = Vec::new();
        for id in &failed_ids {
            match fetch_paper_from_html(id).await {
                Ok(Some(paper)) => {
                    info!("[arxiv] HTML fallback success for {}", id);
                    all_papers.push(paper);
                }
                Ok(None) => {
                    warn!("[arxiv] HTML fallback returned no data for {}", id);
                    html_failed.push(id.clone());
                }
                Err(e) => {
                    warn!("[arxiv] HTML fallback failed for {}: {}", id, e);
                    html_failed.push(id.clone());
                }
            }
            // Small delay to avoid being rate-limited
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        failed_ids = html_failed;
    }

    if !failed_ids.is_empty() {
        warn!(
            "[arxiv] {} papers could not be fetched: {:?}",
            failed_ids.len(),
            failed_ids
        );
    }

    info!(
        "[arxiv] Fetched {} papers by ids in {} batches",
        all_papers.len(),
        ids.len().div_ceil(BATCH_SIZE)
    );
    Ok(all_papers)
}

/// Fetch papers via OAI-PMH API with concurrency control.
pub async fn fetch_papers_by_ids_oai(
    ids: &[String],
    concurrency: usize,
    progress: Option<&ProgressBar>,
) -> Result<Vec<Paper>> {
    use futures::stream::{self, StreamExt};
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let semaphore = Arc::new(Semaphore::new(concurrency));
    let mut papers = Vec::new();

    info!(
        "[arxiv] Fetching {} papers via OAI-PMH (concurrency={})",
        ids.len(),
        concurrency
    );

    // Convert to owned values to avoid lifetime issues
    let owned_ids: Vec<String> = ids.to_vec();
    let progress = progress.cloned();

    let results: Vec<_> = stream::iter(owned_ids.into_iter())
        .map(move |id| {
            let semaphore = Arc::clone(&semaphore);
            let progress = progress.clone();
            async move {
                let _permit = semaphore.acquire().await?;
                let result = fetch_paper_oai(&id).await;
                if let Some(pb) = &progress {
                    pb.inc(1);
                }
                result
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;

    for result in results {
        match result {
            Ok(Some(paper)) => papers.push(paper),
            Ok(None) => {}
            Err(e) => warn!("[arxiv] OAI-PMH fetch failed: {}", e),
        }
    }

    info!("[arxiv] Fetched {} papers via OAI-PMH", papers.len());
    Ok(papers)
}

/// Fetch a single paper via OAI-PMH GetRecord.
async fn fetch_paper_oai(id: &str) -> Result<Option<Paper>> {
    let normalized_id = normalize_arxiv_id(id);
    let url = format!(
        "https://oaipmh.arxiv.org/oai?verb=GetRecord&metadataPrefix=arXiv&identifier=oai:arXiv.org:{}",
        normalized_id
    );

    // Check cache first
    if let Some(cached_xml) = read_cache(&url) {
        info!("[arxiv] OAI-PMH cache hit for {}", normalized_id);
        return parse_arxiv_record(&cached_xml, &normalized_id);
    }

    info!("[arxiv] OAI-PMH fetching: {}", normalized_id);

    let max_retries = 3000;
    let mut last_err = None;

    for attempt in 1..=max_retries {
        info!(
            "[arxiv] OAI-PMH request attempt {}/{}: {}",
            attempt, max_retries, url
        );
        match OAI_CLIENT.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                info!("[arxiv] OAI-PMH response status: {}", status);
                if status.is_success() {
                    let xml = resp
                        .text()
                        .await
                        .context("Failed to read OAI-PMH response")?;
                    write_cache(&url, &xml);
                    return parse_arxiv_record(&xml, &normalized_id);
                } else if status.as_u16() == 429 || status.is_server_error() {
                    let delay = std::cmp::min(2u64.saturating_pow(attempt as u32), 60);
                    warn!(
                        "[arxiv] OAI-PMH server error {} on attempt {}, retrying in {}s",
                        status, attempt, delay
                    );
                    last_err = Some(anyhow::anyhow!("OAI-PMH returned {}", status));
                    if attempt < max_retries {
                        tokio::time::sleep(Duration::from_secs(delay)).await;
                        continue;
                    }
                    break;
                } else {
                    let err_text = resp.text().await.unwrap_or_default();
                    warn!(
                        "[arxiv] OAI-PMH returned {} for {}: {}",
                        status, normalized_id, err_text
                    );
                    let delay = std::cmp::min(2u64.saturating_pow(attempt as u32), 60);
                    if attempt < max_retries {
                        tokio::time::sleep(Duration::from_secs(delay)).await;
                    }
                    continue;
                }
            }
            Err(e) => {
                warn!(
                    "[arxiv] OAI-PMH request failed on attempt {}: {}",
                    attempt, e
                );
                last_err = Some(e.into());
            }
        }
        if attempt < max_retries {
            let delay = std::cmp::min(2u64.saturating_pow(attempt as u32), 60);
            warn!(
                "[arxiv] OAI-PMH retrying in {}s (exponential backoff)",
                delay
            );
            tokio::time::sleep(Duration::from_secs(delay)).await;
        }
    }

    warn!(
        "[arxiv] OAI-PMH failed for {} after {} attempts: {:?}",
        normalized_id, max_retries, last_err
    );
    Ok(None)
}

/// Parse OAI-PMH arXiv XML response.
fn parse_arxiv_record(xml: &str, expected_id: &str) -> Result<Option<Paper>> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();

    // State tracking for nested elements
    let mut in_arxiv = false;
    let mut in_authors = false;
    let mut in_author = false;
    let mut current_element = String::new();

    // Parsed fields
    let mut arxiv_id = String::new();
    let mut title = String::new();
    let mut abstract_text = String::new();
    let mut created = String::new();
    let mut authors: Vec<String> = Vec::new();
    // Per-author accumulators
    let mut keyname = String::new();
    let mut forenames = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match name.as_str() {
                    "arXiv" => in_arxiv = true,
                    "authors" if in_arxiv => in_authors = true,
                    "author" if in_authors => {
                        in_author = true;
                        keyname.clear();
                        forenames.clear();
                    }
                    _ if in_arxiv => current_element = name,
                    _ => {}
                }
            }
            Ok(Event::Text(e)) if in_arxiv => {
                let text = e.decode().unwrap_or_default().trim().to_string();
                if in_author {
                    match current_element.as_str() {
                        "keyname" => keyname = text,
                        "forenames" => forenames = text,
                        _ => {}
                    }
                } else if !in_authors {
                    match current_element.as_str() {
                        "id" => arxiv_id = text,
                        "title" => title = text,
                        "abstract" => abstract_text = text,
                        "created" => created = text,
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match name.as_str() {
                    "arXiv" => in_arxiv = false,
                    "authors" => in_authors = false,
                    "author" => {
                        if !keyname.is_empty() {
                            let author = if forenames.is_empty() {
                                keyname.clone()
                            } else {
                                format!("{} {}", forenames, keyname)
                            };
                            authors.push(author);
                        }
                        in_author = false;
                    }
                    _ => current_element.clear(),
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                warn!("[arxiv] OAI-PMH XML parse error: {}", e);
                return Ok(None);
            }
            _ => {}
        }
        buf.clear();
    }

    if title.is_empty() && authors.is_empty() {
        warn!(
            "[arxiv] OAI-PMH record for {} has no title or authors",
            expected_id
        );
        return Ok(None);
    }

    let paper_id = if arxiv_id.is_empty() {
        expected_id.to_string()
    } else {
        normalize_arxiv_id(&arxiv_id)
    };

    // Parse date to RFC3339
    let published = if !created.is_empty() {
        if let Ok(dt) = chrono::NaiveDate::parse_from_str(&created, "%Y-%m-%d") {
            dt.and_hms_opt(0, 0, 0)
                .map(|d| d.and_utc().to_rfc3339())
                .unwrap_or_default()
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    Ok(Some(Paper {
        id: expected_id.to_string(),
        title,
        authors,
        summary: abstract_text,
        pdf_url: format!("https://arxiv.org/pdf/{}.pdf", paper_id),
        published,
        source_type: Some("arxiv".to_string()),
        source_url: Some(format!("https://arxiv.org/abs/{}", paper_id)),
        external_id: Some(paper_id),
    }))
}

pub async fn fetch_papers(query: &str, max_results: usize, page_size: usize) -> Result<Vec<Paper>> {
    let mut start = 0usize;
    let mut all_papers = Vec::new();

    loop {
        if max_results > 0 && all_papers.len() >= max_results {
            info!(
                "[arxiv] Reached user-specified max_results={}, stopping",
                max_results
            );
            all_papers.truncate(max_results);
            break;
        }

        let remaining = if max_results > 0 {
            (max_results - all_papers.len()).min(page_size)
        } else {
            page_size
        };

        if remaining == 0 {
            break;
        }

        let url = format!(
            "https://export.arxiv.org/api/query?search_query={}&sortBy=submittedDate&sortOrder=descending&start={}&max_results={}",
            urlencoding::encode(query),
            start,
            remaining
        );
        info!(
            "[arxiv] Fetching page: start={}, page_size={}, collected={}/{}",
            start,
            remaining,
            all_papers.len(),
            if max_results > 0 {
                max_results.to_string()
            } else {
                "all".to_string()
            }
        );

        let (page_papers, cached) = fetch_single_page(&url, 3000).await?;
        let fetched = page_papers.len();

        if fetched == 0 {
            info!("[arxiv] No more papers returned, done");
            break;
        }

        all_papers.extend(page_papers);
        start += fetched;

        if fetched >= remaining {
            if cached {
                info!("[arxiv] Cache hit, skipping sleep");
            } else {
                info!("[arxiv] Sleeping 3s before next page...");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        } else {
            info!("[arxiv] Last page returned {} papers, done", fetched);
            break;
        }
    }

    info!("[arxiv] Total papers fetched: {}", all_papers.len());
    Ok(all_papers)
}

fn cache_dir() -> PathBuf {
    PathBuf::from(".fetchcache")
}

fn cache_key(url: &str) -> String {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn cache_path(url: &str) -> PathBuf {
    cache_dir().join(cache_key(url))
}

/// Format seconds as a human-readable "Xh Ym Zs" / "Xm Zs" / "Zs" string.
fn format_human_secs(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{}h {}m {}s", h, m, s)
    } else if m > 0 {
        format!("{}m {}s", m, s)
    } else {
        format!("{}s", s)
    }
}

fn read_cache(url: &str) -> Option<String> {
    let path = cache_path(url);
    if !path.exists() {
        info!(
            "[arxiv.cache] MISS url={} path={} reason=file_does_not_exist",
            url,
            path.display()
        );
        return None;
    }
    let metadata = std::fs::metadata(&path).ok()?;
    let modified: SystemTime = metadata.modified().ok()?;
    let elapsed = modified.elapsed().ok()?;
    let ttl = cache_ttl_secs();
    let now = SystemTime::now();
    let expires_at = modified
        .checked_add(Duration::from_secs(ttl))
        .unwrap_or(now);
    let modified_rfc = chrono::DateTime::<chrono::Utc>::from(modified).to_rfc3339();
    let expires_rfc = chrono::DateTime::<chrono::Utc>::from(expires_at).to_rfc3339();

    if elapsed > Duration::from_secs(ttl) {
        // Expired: treat as miss, but DO NOT delete the file. The next
        // write_cache() will overwrite it in place, and the file remains
        // on disk for forensics / for when the operator lowers TTL back
        // below the file age.
        info!(
            "[arxiv.cache] EXPIRED url={} path={} written_at={} expires_at={} ttl={}h elapsed={} ({}) — keeping file, will overwrite on next write",
            url,
            path.display(),
            modified_rfc,
            expires_rfc,
            ttl / 3600,
            elapsed.as_secs(),
            format_human_secs(elapsed.as_secs()),
        );
        return None;
    }
    let body = std::fs::read_to_string(&path).ok()?;
    info!(
        "[arxiv.cache] HIT url={} path={} written_at={} expires_at={} ttl={}h elapsed={} ({}) bytes={}",
        url,
        path.display(),
        modified_rfc,
        expires_rfc,
        ttl / 3600,
        elapsed.as_secs(),
        format_human_secs(elapsed.as_secs()),
        body.len(),
    );
    Some(body)
}

fn write_cache(url: &str, body: &str) {
    let dir = cache_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn!("[arxiv.cache] Failed to create cache dir: {}", e);
        return;
    }
    let path = cache_path(url);
    // Atomic write: stage to <path>.tmp, then rename into place. This way
    // concurrent readers either see the old file in full or the new file in
    // full — never a half-written file. On Unix, rename(2) atomically
    // replaces the destination; on Windows, std::fs::rename falls back to
    // MoveFileEx with MOVEFILE_REPLACE_EXISTING semantics.
    let tmp_path = {
        let mut p = path.clone();
        let mut name = p.file_name().map(|n| n.to_os_string()).unwrap_or_default();
        name.push(".tmp");
        p.set_file_name(name);
        p
    };
    if let Err(e) = std::fs::write(&tmp_path, body) {
        warn!("[arxiv.cache] Failed to write tmp cache for {}: {}", url, e);
        return;
    }
    if let Err(e) = std::fs::rename(&tmp_path, &path) {
        warn!(
            "[arxiv.cache] Failed to rename tmp cache into place for {} ({} -> {}): {}",
            url,
            tmp_path.display(),
            path.display(),
            e
        );
        // Best-effort cleanup of leftover tmp file.
        let _ = std::fs::remove_file(&tmp_path);
        return;
    }
    let now = SystemTime::now();
    let ttl = cache_ttl_secs();
    let expires_at = now.checked_add(Duration::from_secs(ttl)).unwrap_or(now);
    let now_rfc = chrono::DateTime::<chrono::Utc>::from(now).to_rfc3339();
    let expires_rfc = chrono::DateTime::<chrono::Utc>::from(expires_at).to_rfc3339();
    info!(
        "[arxiv.cache] WRITE url={} path={} written_at={} expires_at={} ttl={}h bytes={}",
        url,
        path.display(),
        now_rfc,
        expires_rfc,
        ttl / 3600,
        body.len(),
    );
}

async fn fetch_single_page(url: &str, max_retries: usize) -> Result<(Vec<Paper>, bool)> {
    if let Some(cached_body) = read_cache(url) {
        return parse_feed(&cached_body).map(|papers| (papers, true));
    }

    let mut last_err = None;

    for attempt in 1..=max_retries {
        info!(
            "[arxiv] HTTP request attempt {}/{}: {}",
            attempt, max_retries, url
        );
        match ARXIV_CLIENT.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                info!("[arxiv] Response status: {}", status);
                if status.is_success() {
                    match resp.text().await {
                        Ok(body) => {
                            write_cache(url, &body);
                            return parse_feed(&body).map(|papers| (papers, false));
                        }
                        Err(e) => {
                            warn!("[arxiv] Failed to read body: {}", e);
                            last_err = Some(e.into());
                        }
                    }
                } else if status.as_u16() == 429 || status.is_server_error() {
                    let delay = std::cmp::min(2u64.saturating_pow(attempt as u32), 60);
                    warn!(
                        "[arxiv] Server error {} on attempt {}, retrying in {}s",
                        status, attempt, delay
                    );
                    last_err = Some(anyhow::anyhow!("arXiv API returned {}", status));
                    if attempt < max_retries {
                        tokio::time::sleep(Duration::from_secs(delay)).await;
                        continue;
                    }
                    // Last attempt failed, break and return the error
                    break;
                } else {
                    let err_text = resp.text().await.unwrap_or_default();
                    return Err(anyhow::anyhow!(
                        "arXiv API returned {}: {}",
                        status,
                        err_text
                    ));
                }
            }
            Err(e) => {
                warn!("[arxiv] Request failed on attempt {}: {}", attempt, e);
                last_err = Some(e.into());
            }
        }
        if attempt < max_retries {
            let delay = std::cmp::min(2u64.saturating_pow(attempt as u32), 60);
            warn!("[arxiv] Retrying in {}s (exponential backoff)", delay);
            tokio::time::sleep(Duration::from_secs(delay)).await;
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("All retry attempts failed")))
}

fn parse_feed(body: &str) -> Result<Vec<Paper>> {
    let feed = parser::parse(body.as_bytes()).context("Failed to parse arXiv Atom feed")?;

    let mut papers = Vec::new();
    for entry in feed.entries {
        let raw_id = entry.id.replace("http://arxiv.org/abs/", "");
        let id = if raw_id.contains('v') {
            raw_id.split('v').next().unwrap_or(&raw_id).to_string()
        } else {
            raw_id.clone()
        };

        let pdf_url = format!("https://arxiv.org/pdf/{}.pdf", id);
        let title = entry.title.map(|t| t.content).unwrap_or_default();
        let authors: Vec<String> = entry.authors.into_iter().map(|a| a.name).collect();
        let summary = entry.summary.map(|s| s.content).unwrap_or_default();
        let published = entry.published.map(|d| d.to_rfc3339()).unwrap_or_default();

        papers.push(Paper {
            id: id.clone(),
            title,
            authors,
            summary,
            pdf_url,
            published,
            source_type: Some("arxiv".to_string()),
            source_url: Some(format!("https://arxiv.org/abs/{}", id)),
            external_id: Some(id),
        });
    }
    Ok(papers)
}

/// Fallback: scrape paper metadata from arxiv.org/abs/{id} HTML page.
async fn fetch_paper_from_html(id: &str) -> Result<Option<Paper>> {
    let url = format!("https://arxiv.org/abs/{}", id);
    info!("[arxiv] Fetching HTML page: {}", url);

    let resp = ARXIV_CLIENT
        .get(&url)
        .send()
        .await
        .context("HTTP request to arxiv.org abs page failed")?;

    let status = resp.status();
    if !status.is_success() {
        warn!("[arxiv] HTML page returned {} for {}", status, id);
        return Ok(None);
    }

    let html = resp.text().await.context("Failed to read HTML body")?;

    // Extract title from <h1 class="title mathjax">...</h1>
    let title = extract_between(&html, r#"<h1 class="title mathjax">"#, "</h1>")
        .unwrap_or_default()
        .trim()
        .to_string();
    // Remove <span class="descriptor">Title:</span> prefix if present
    let title = if let Some(pos) = title.find("</span>") {
        title[pos + 7..].trim().to_string()
    } else {
        title
    };

    // Extract authors from <div class="authors">Authors:<a href="...">Name</a>, ...</div>
    let authors =
        if let Some(authors_block) = extract_between(&html, r#"<div class="authors">"#, "</div>") {
            let mut authors = Vec::new();
            let mut rest = authors_block.as_str();
            // Skip "Authors:" prefix if present
            if let Some(pos) = rest.find("Authors:") {
                rest = &rest[pos + 8..];
            }
            while let Some(start) = rest.find("<a ") {
                rest = &rest[start..];
                if let Some(tag_end) = rest.find(">") {
                    rest = &rest[tag_end + 1..];
                    if let Some(name_end) = rest.find("</a>") {
                        let name = rest[..name_end].trim();
                        if !name.is_empty() {
                            authors.push(name.to_string());
                        }
                        rest = &rest[name_end + 4..];
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
            authors
        } else {
            Vec::new()
        };

    // Extract abstract from <blockquote class="abstract mathjax"><span class="descriptor">Abstract:</span>...</blockquote>
    let summary = if let Some(abs_block) = extract_between(
        &html,
        r#"<blockquote class="abstract mathjax">"#,
        "</blockquote>",
    ) {
        let mut content = abs_block;
        // Remove <span class="descriptor">Abstract:</span>
        if let Some(start) = content.find("<span class=\"descriptor\">") {
            if let Some(end) = content[start..].find("</span>") {
                content = content[start + end + 7..].to_string();
            }
        }
        content.trim().to_string()
    } else {
        String::new()
    };

    // Extract date from <div class="dateline">[Submitted on 21 Jan 2025]</div>
    let published =
        if let Some(date_line) = extract_between(&html, r#"<div class="dateline">"#, "</div>") {
            parse_arxiv_dateline(&date_line)
        } else {
            String::new()
        };

    if title.is_empty() && authors.is_empty() && summary.is_empty() {
        warn!("[arxiv] HTML page for {} contained no usable metadata", id);
        return Ok(None);
    }

    Ok(Some(Paper {
        id: id.to_string(),
        title,
        authors,
        summary,
        pdf_url: format!("https://arxiv.org/pdf/{}.pdf", id),
        published,
        source_type: Some("arxiv".to_string()),
        source_url: Some(format!("https://arxiv.org/abs/{}", id)),
        external_id: Some(id.to_string()),
    }))
}

/// Extract text between two markers (first occurrence).
fn extract_between(text: &str, start: &str, end: &str) -> Option<String> {
    let start_idx = text.find(start)?;
    let after_start = start_idx + start.len();
    let end_idx = text[after_start..].find(end)?;
    Some(text[after_start..after_start + end_idx].to_string())
}

/// Parse arxiv dateline like "[Submitted on 21 Jan 2025]" or "[v1] Tue, 21 Jan 2025 ..." into RFC3339.
fn parse_arxiv_dateline(line: &str) -> String {
    let line = line.trim();

    // Try "Submitted on 21 Jan 2025"
    if let Some(pos) = line.find("Submitted on ") {
        let date_str = &line[pos + 13..].trim();
        // Remove trailing bracket if present
        let date_str = date_str.trim_end_matches(']').trim();
        if let Ok(dt) = chrono::NaiveDate::parse_from_str(date_str, "%d %b %Y") {
            return dt
                .and_hms_opt(0, 0, 0)
                .map(|d| d.and_utc().to_rfc3339())
                .unwrap_or_default();
        }
    }

    // Try "[v1] Tue, 21 Jan 2025 12:00:00 GMT"
    if let Some(pos) = line.find(", ") {
        let after_comma = &line[pos + 2..].trim();
        if let Some(space_idx) = after_comma.find(' ') {
            let date_part = &after_comma[..space_idx + 11]; // "21 Jan 2025"
            if let Ok(dt) = chrono::NaiveDate::parse_from_str(date_part.trim(), "%d %b %Y") {
                return dt
                    .and_hms_opt(0, 0, 0)
                    .map(|d| d.and_utc().to_rfc3339())
                    .unwrap_or_default();
            }
        }
    }

    String::new()
}
