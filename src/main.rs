// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures::stream::{FuturesUnordered, StreamExt};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use ripple_reader::config::Config;
use ripple_reader::db::Db;
use ripple_reader::pipeline::process_paper_with_cache;
use ripple_reader::processor::{Processor, Summary};
use ripple_reader::query::build_query;
use ripple_reader::source::arxiv::{fetch_papers, fetch_papers_by_ids, fetch_papers_by_ids_oai};
use ripple_reader::source::Paper;
use ripple_reader::web::run_server;
use std::env;
use std::fmt::Write as _;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::{info, warn};
use tracing_subscriber::prelude::*;

/// LLM-driven academic paper discovery, screening and deep reading.
#[derive(Parser)]
#[command(name = "ripple-reader", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<SubCmd>,
}

#[derive(Subcommand)]
enum SubCmd {
    /// Start the web server (default when no subcommand given)
    Web {
        /// Port to listen on (fallback: WEB_PORT env, then 8181)
        port: Option<u16>,
    },
    /// Fetch papers from arXiv and print summaries (one-shot)
    ArxivFetch,
    /// Add papers by arXiv or OpenReview ID
    Add {
        /// Paper IDs (arXiv or OpenReview forum IDs)
        #[arg(num_args = 0..)]
        ids: Vec<String>,
        /// Force re-summarize and re-translate
        #[arg(short = 'f', long)]
        force: bool,
        /// Re-process all untagged papers from database
        #[arg(long)]
        force_all: bool,
    },
    /// Physically delete soft-deleted papers
    CleanupDeleted,
    /// Export authors from arXiv to a file
    ExportAuthors {
        /// arXiv query override
        #[arg(short = 'q', long)]
        query: Option<String>,
        /// Output file path
        #[arg(short = 'o', long, default_value = "authors.txt")]
        output: String,
        /// Max results to fetch
        #[arg(short = 'n', long, default_value = "5000")]
        max_results: usize,
    },
    /// Initialize authors via Semantic Scholar
    InitAuthors {
        /// Input file (one author per line)
        #[arg(short = 'f', long, default_value = "authors.txt")]
        file: String,
    },
    /// Estimate local author statistics
    EstimateAuthorStats,
    /// Enrich authors with external citation/h-index data
    EnrichAuthors,
    /// Scrape OpenReview conference papers
    ScrapeOpenreview {
        /// Venue ID (e.g., NeurIPS.cc/2024/Conference)
        venue: String,
        /// Conference year
        year: i32,
        /// Preview only, don't write to database
        #[arg(long)]
        dry_run: bool,
    },
    /// Import OpenReview papers from a JSON file
    ImportOpenreview {
        /// Path to papers.json
        file: String,
    },
}

struct FileLogger {
    logger: env_logger::Logger,
    file: Arc<Mutex<File>>,
}

impl FileLogger {
    fn new() -> Result<Self> {
        std::fs::create_dir_all("logs").context("Failed to create logs directory")?;
        let date = chrono::Local::now().format("%Y-%m-%d");
        let path = format!("logs/run-{}.log", date);
        let file = File::options()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("Failed to open log file: {}", path))?;
        let logger =
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
                .build();
        Ok(FileLogger {
            logger,
            file: Arc::new(Mutex::new(file)),
        })
    }
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.logger.enabled(metadata)
    }
    fn log(&self, record: &log::Record) {
        self.logger.log(record);
        if self.enabled(record.metadata()) {
            if let Ok(mut file) = self.file.lock() {
                let _ = writeln!(
                    file,
                    "[{} {} {}] {}",
                    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S"),
                    record.level(),
                    record.target(),
                    record.args()
                );
            }
        }
    }
    fn flush(&self) {
        self.logger.flush();
        if let Ok(mut file) = self.file.lock() {
            let _ = file.flush();
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    let is_web = args.len() <= 1 || args.get(1).map(|s| s.as_str()) == Some("web");
    let multi = if is_web {
        std::fs::create_dir_all("logs").ok();
        let file_appender = tracing_appender::rolling::daily("logs", "run.log");
        let file_layer = tracing_subscriber::fmt::layer()
            .with_file(true)
            .with_line_number(true)
            .with_writer(file_appender)
            .with_ansi(false);
        let stderr_layer = tracing_subscriber::fmt::layer()
            .with_file(true)
            .with_line_number(true);
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,ripple_reader=debug"));
        tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .with(stderr_layer)
            .init();
        None
    } else {
        let logger = FileLogger::new().context("Failed to init file logger")?;
        let multi = MultiProgress::new();
        indicatif_log_bridge::LogWrapper::new(multi.clone(), logger)
            .try_init()
            .unwrap();
        log::set_max_level(log::LevelFilter::Info);
        Some(multi)
    };

    info!("[main] Starting ripple-reader");
    let cli = Cli::parse();
    match cli.command.unwrap_or(SubCmd::Web { port: None }) {
        SubCmd::Web { port } => {
            info!("[main] Web mode");
            let cfg = Config::from_env()?;
            let port = port
                .or_else(|| env::var("WEB_PORT").ok().and_then(|p| p.parse().ok()))
                .unwrap_or(8181);
            let mut insight_processors = Vec::new();
            let mut comment_processor = None;
            for provider in &cfg.insight_providers {
                if !provider.enabled {
                    info!(
                        "[main] Skipping disabled insight provider '{}'",
                        provider.name
                    );
                    continue;
                }
                let processor = Processor::new(
                    ripple_reader::processor::ProviderType::parse(&provider.provider_type),
                    provider.name.clone(),
                    provider.base_url.clone(),
                    provider.api_keys.clone(),
                    provider.model.clone(),
                    provider.max_tokens,
                    provider.reasoning_effort.clone(),
                    provider.output_config_effort.clone(),
                    provider.user_agent.clone(),
                    provider.temperature,
                    provider.top_p,
                    cfg.llm_max_workers,
                    cfg.insight_max_workers,
                    cfg.llm_max_retries,
                    cfg.summarize_prompt.clone(),
                    cfg.translate_prompt.clone(),
                    cfg.insight_prompt.clone(),
                    cfg.review_prompt.clone(),
                    None,
                );
                let arc_proc = Arc::new(processor);
                if provider.is_comment {
                    comment_processor = Some(Arc::clone(&arc_proc));
                }
                insight_processors.push((provider.name.clone(), arc_proc));
            }
            let insight_prompts = Arc::new(tokio::sync::RwLock::new(cfg.insight_prompts));
            let insight_prompts_mtime =
                Arc::new(tokio::sync::RwLock::new(cfg.insight_prompts_mtime));
            let editable_config = ripple_reader::web::EditableConfig {
                arxiv_fetch_cron: cfg.arxiv_fetch_cron.clone(),
                arxiv_query: cfg.arxiv_query.clone(),
                arxiv_max_results: cfg.arxiv_max_results,
                arxiv_page_size: cfg.arxiv_page_size,
                arxiv_cache_ttl_hours: cfg.arxiv_cache_ttl_hours,
                llm_max_workers: cfg.llm_max_workers,
                llm_max_retries: cfg.llm_max_retries,
                pdf_max_workers: cfg.pdf_max_workers,
                insight_max_workers: cfg.insight_max_workers,
                insight_review_max_attempts: cfg.insight_review_max_attempts,
                auto_review_insight: cfg.auto_review_insight,
                mineru_base_url: cfg.mineru_base_url.clone(),
                mineru_no_cache: cfg.mineru_no_cache,
                mineru_api_key: cfg.mineru_api_key.clone(),
            };
            return run_server(
                cfg.database_url,
                cfg.web_password,
                port,
                insight_processors,
                comment_processor,
                cfg.summarize_prompt.clone(),
                cfg.translate_prompt.clone(),
                insight_prompts,
                insight_prompts_mtime,
                editable_config,
            )
            .await;
        }
        SubCmd::ArxivFetch => {
            info!("[main] ArXiv fetch mode");
            let cfg = Config::from_env()?;
            print_config(&cfg);
            run_once(&cfg, multi.as_ref()).await
        }
        SubCmd::Add {
            ids,
            force,
            force_all,
        } => {
            let cfg = Config::from_env()?;
            print_config(&cfg);
            if force_all {
                info!("[main] Force-all mode: re-summarizing all untagged papers");
                let db = Db::new(&cfg.database_url)
                    .await
                    .context("Failed to initialize database")?;
                let ids = db
                    .list_paper_ids_without_tags()
                    .await
                    .context("Failed to list paper ids from database")?;
                info!("[main] Found {} untagged papers to re-process", ids.len());
                if ids.is_empty() {
                    eprintln!("No papers found in database.");
                    std::process::exit(1);
                }
                return add_papers(&ids, &cfg, true, multi.as_ref(), false).await;
            }
            if ids.is_empty() {
                eprintln!("Usage: cargo run -- add [--force] <id1> [id2] ...");
                eprintln!("       cargo run -- add --force-all");
                std::process::exit(1);
            }
            if force {
                info!("[main] Force refresh: re-summarize all specified papers");
            }
            add_papers(&ids, &cfg, force, multi.as_ref(), false).await
        }
        SubCmd::CleanupDeleted => {
            let cfg = Config::from_env()?;
            let db = Db::new(&cfg.database_url)
                .await
                .context("Failed to initialize database")?;
            match db.cleanup_deleted_papers().await {
                Ok(count) => {
                    if count > 0 {
                        println!(
                            "Physically deleted {} paper(s) and cleared deleted_papers.",
                            count
                        );
                    } else {
                        println!("No papers to clean up.");
                    }
                }
                Err(e) => {
                    eprintln!("Failed to cleanup deleted papers: {}", e);
                    std::process::exit(1);
                }
            }
            Ok(())
        }
        SubCmd::ExportAuthors {
            query,
            output,
            max_results,
        } => {
            let q = match query {
                Some(q) => q,
                None => {
                    let cfg = Config::from_env()?;
                    build_query(&cfg.arxiv_query, &cfg.arxiv_categories, &cfg.arxiv_keywords)
                }
            };
            println!("Fetching papers from arXiv with query: {}", q);
            let papers = fetch_papers(&q, max_results, 1000).await?;
            println!("Fetched {} papers, extracting authors...", papers.len());
            let mut authors: std::collections::HashSet<String> = std::collections::HashSet::new();
            for paper in &papers {
                for author in &paper.authors {
                    let author = author.trim();
                    if !author.is_empty() {
                        authors.insert(author.to_string());
                    }
                }
            }
            let mut author_list: Vec<String> = authors.into_iter().collect();
            author_list.sort();
            std::fs::write(&output, author_list.join("\n"))?;
            println!(
                "Exported {} unique authors to {}",
                author_list.len(),
                output
            );
            Ok(())
        }
        SubCmd::InitAuthors { file } => {
            let content = std::fs::read_to_string(&file)
                .with_context(|| format!("Failed to read author list from {}", file))?;
            let authors: Vec<String> = content
                .lines()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if authors.is_empty() {
                println!("No authors found in {}.", file);
                return Ok(());
            }
            let cfg = Config::from_env()?;
            let db = Db::new(&cfg.database_url)
                .await
                .context("Failed to initialize database")?;
            println!(
                "Initializing {} authors from Semantic Scholar...",
                authors.len()
            );
            let mut enriched = 0usize;
            let mut failed = 0usize;
            for author in &authors {
                match ripple_reader::author_reputation::fetch_author_semantic_scholar(author).await
                {
                    Ok(Some(data)) => {
                        if let Err(e) = db
                            .update_author_external_stats(
                                &data.name,
                                data.citation_count,
                                data.h_index,
                            )
                            .await
                        {
                            warn!("[init-authors] Failed to update {}: {}", author, e);
                            failed += 1;
                        } else {
                            enriched += 1;
                            info!(
                                "[init-authors] {}: h-index={}, citations={}",
                                data.name,
                                data.h_index.unwrap_or(0),
                                data.citation_count.unwrap_or(0)
                            );
                        }
                    }
                    Ok(None) => {
                        warn!("[init-authors] No Semantic Scholar match for {}", author);
                        failed += 1;
                    }
                    Err(e) => {
                        warn!("[init-authors] API error for {}: {}", author, e);
                        failed += 1;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
            println!(
                "Initialized {} / {} authors ({} failed).",
                enriched,
                authors.len(),
                failed
            );
            Ok(())
        }
        SubCmd::EstimateAuthorStats => {
            let cfg = Config::from_env()?;
            let db = Db::new(&cfg.database_url)
                .await
                .context("Failed to initialize database")?;
            match db.estimate_local_stats().await {
                Ok(count) => println!("Estimated local stats for {} authors.", count),
                Err(e) => {
                    eprintln!("Failed to estimate author stats: {}", e);
                    std::process::exit(1);
                }
            }
            Ok(())
        }
        SubCmd::EnrichAuthors => {
            let cfg = Config::from_env()?;
            let db = Db::new(&cfg.database_url)
                .await
                .context("Failed to initialize database")?;
            let existing_count = db
                .get_authors_without_external_stats(1)
                .await
                .map(|v| v.len())
                .unwrap_or(0);
            if existing_count == 0 {
                info!("[enrich-authors] Building author stats from existing papers...");
                match db.build_author_stats_from_papers().await {
                    Ok(count) => {
                        println!("Built {} author stats from existing papers.", count)
                    }
                    Err(e) => {
                        eprintln!("Failed to build author stats: {}", e);
                        std::process::exit(1);
                    }
                }
            }
            let authors = db
                .get_authors_without_external_stats(500)
                .await
                .context("Failed to list authors without external stats")?;
            if authors.is_empty() {
                println!("No authors need enrichment.");
                return Ok(());
            }
            println!(
                "Enriching {} authors from Semantic Scholar...",
                authors.len()
            );
            let mut enriched = 0usize;
            let mut failed = 0usize;
            for author in &authors {
                match ripple_reader::author_reputation::fetch_author_semantic_scholar(author).await
                {
                    Ok(Some(data)) => {
                        if let Err(e) = db
                            .update_author_external_stats(
                                &data.name,
                                data.citation_count,
                                data.h_index,
                            )
                            .await
                        {
                            warn!("[enrich-authors] Failed to update {}: {}", author, e);
                            failed += 1;
                        } else {
                            enriched += 1;
                            info!(
                                "[enrich-authors] {}: h-index={}, citations={}",
                                data.name,
                                data.h_index.unwrap_or(0),
                                data.citation_count.unwrap_or(0)
                            );
                        }
                    }
                    Ok(None) => {
                        warn!("[enrich-authors] No Semantic Scholar match for {}", author);
                        failed += 1;
                    }
                    Err(e) => {
                        warn!("[enrich-authors] API error for {}: {}", author, e);
                        failed += 1;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
            println!(
                "Enriched {} / {} authors ({} failed).",
                enriched,
                authors.len(),
                failed
            );
            Ok(())
        }
        SubCmd::ScrapeOpenreview {
            venue,
            year,
            dry_run,
        } => {
            let cfg = Config::from_env()?;
            print_config(&cfg);
            scrape_openreview(&venue, year, dry_run, &cfg).await
        }
        SubCmd::ImportOpenreview { file } => {
            let cfg = Config::from_env()?;
            print_config(&cfg);
            import_openreview(&file, &cfg, multi.as_ref()).await
        }
    }
}

async fn run_once(cfg: &Config, multi: Option<&MultiProgress>) -> Result<()> {
    info!("[main] Building arXiv query");
    let query = build_query(&cfg.arxiv_query, &cfg.arxiv_categories, &cfg.arxiv_keywords);
    info!("[main] Final query: '{}'", query);

    // Fetch progress bar
    let fetch_pb_raw = ProgressBar::new_spinner();
    fetch_pb_raw.set_style(
        ProgressStyle::default_spinner()
            .template("[{elapsed_precise}] {spinner:.cyan} {msg}")
            .unwrap(),
    );
    let fetch_pb = multi
        .map(|m| m.add(fetch_pb_raw.clone()))
        .unwrap_or(fetch_pb_raw);
    fetch_pb.set_message(format!(
        "Fetching papers from arXiv (max_results={})...",
        cfg.arxiv_max_results
    ));
    fetch_pb.enable_steady_tick(Duration::from_millis(120));

    info!(
        "[main] Fetching papers from arXiv (max_results={}, page_size={})",
        cfg.arxiv_max_results, cfg.arxiv_page_size
    );
    let papers = fetch_papers(&query, cfg.arxiv_max_results, cfg.arxiv_page_size).await?;
    fetch_pb.finish_with_message(format!("Fetched {} papers from arXiv", papers.len()));
    info!("[main] Fetched {} papers from arXiv", papers.len());

    // Reverse so oldest papers are processed first (arXiv returns newest-first).
    let mut papers = papers;
    papers.reverse();

    if papers.is_empty() {
        warn!("[main] No papers to output.");
        return Ok(());
    }

    let db = Db::new(&cfg.database_url)
        .await
        .context("Failed to initialize database")?;
    info!("[main] Database initialized at {}", cfg.database_url);

    info!(
        "[main] Initializing Processor with llm_max_workers={}",
        cfg.llm_max_workers
    );
    let summarizer = cfg
        .summarizer
        .as_ref()
        .expect("No summarizer provider configured");
    let processor = Processor::new(
        ripple_reader::processor::ProviderType::parse(&summarizer.provider_type),
        summarizer.name.clone(),
        summarizer.base_url.clone(),
        summarizer.api_keys.clone(),
        summarizer.model.clone(),
        summarizer.max_tokens,
        summarizer.reasoning_effort.clone(),
        summarizer.output_config_effort.clone(),
        summarizer.user_agent.clone(),
        summarizer.temperature,
        summarizer.top_p,
        cfg.llm_max_workers,
        cfg.insight_max_workers,
        cfg.llm_max_retries,
        cfg.summarize_prompt.clone(),
        cfg.translate_prompt.clone(),
        cfg.insight_prompt.clone(),
        cfg.review_prompt.clone(),
        None,
    );

    let pdf_semaphore = Arc::new(Semaphore::new(cfg.pdf_max_workers));
    let db_semaphore = Arc::new(Semaphore::new(cfg.llm_max_workers));
    info!(
        "[main] PDF semaphore initialized with pdf_max_workers={}, DB semaphore with llm_max_workers={}",
        cfg.pdf_max_workers,
        cfg.llm_max_workers
    );

    info!(
        "[main] Processing {} papers concurrently (llm_max_workers={})",
        papers.len(),
        cfg.llm_max_workers
    );

    let total = papers.len();
    let progress_completed = Arc::new(AtomicUsize::new(0));
    let progress_cached = Arc::new(AtomicUsize::new(0));
    let progress_new = Arc::new(AtomicUsize::new(0));
    let progress_failed = Arc::new(AtomicUsize::new(0));
    let completed_dates = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    // Processing progress bar
    let proc_pb_raw = ProgressBar::new(total as u64);
    proc_pb_raw.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40.green/white} {pos}/{len} {msg} ETA:{eta}")
            .unwrap()
            .progress_chars("##-"),
    );
    let proc_pb = multi
        .map(|m| m.add(proc_pb_raw.clone()))
        .unwrap_or(proc_pb_raw);
    proc_pb.set_message("Processing papers...");

    let progress_handle = {
        let progress_completed = Arc::clone(&progress_completed);
        let progress_cached = Arc::clone(&progress_cached);
        let progress_new = Arc::clone(&progress_new);
        let progress_failed = Arc::clone(&progress_failed);
        let completed_dates = Arc::clone(&completed_dates);
        let all_papers = papers.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let c = progress_completed.load(Ordering::Relaxed);
                let cache = progress_cached.load(Ordering::Relaxed);
                let new = progress_new.load(Ordering::Relaxed);
                let failed = progress_failed.load(Ordering::Relaxed);
                let done_dates = completed_dates.lock().await;
                let done_range = date_range(&done_dates);
                let remaining_dates: Vec<String> = all_papers
                    .iter()
                    .filter(|p| !done_dates.contains(&p.published))
                    .map(|p| p.published.clone())
                    .collect();
                let remain_range = date_range(&remaining_dates);
                info!(
                    "[main] Progress: {}/{} completed (cached={}, new={}, failed={}) | done_dates={} | remain_dates={}",
                    c, total, cache, new, failed, done_range, remain_range
                );
            }
        })
    };

    let mut results: Vec<(Paper, Summary, String, bool)> = Vec::with_capacity(papers.len());
    let mut cached_count = 0usize;
    let mut new_count = 0usize;
    let mut skipped_count = 0usize;
    // (id, reason) for papers that errored out (non-scanned) so the completed total reconciles.
    let mut failed: Vec<(String, String)> = Vec::new();
    // (id, reason) for scanned PDFs we deliberately skip.
    let mut skipped: Vec<(String, String)> = Vec::new();

    let mut tasks = FuturesUnordered::new();
    for paper in &papers {
        let s = &processor;
        let db_ref = &db;
        let pdf_sem = Arc::clone(&pdf_semaphore);
        let db_sem = Arc::clone(&db_semaphore);
        let paper = paper.clone();
        tasks.push(async move {
            let _db_permit = db_sem.acquire().await.unwrap();
            let res = process_paper_with_cache(s, db_ref, &paper, &pdf_sem, false, false).await;
            (paper, res)
        });
    }
    info!("[main] Queued {} paper processing tasks", papers.len());

    while let Some((paper, res)) = tasks.next().await {
        progress_completed.fetch_add(1, Ordering::Relaxed);
        proc_pb.inc(1);
        match res {
            Ok((sum, abs, is_new)) => {
                completed_dates.lock().await.push(paper.published.clone());
                if is_new {
                    new_count += 1;
                    progress_new.fetch_add(1, Ordering::Relaxed);
                    info!(
                        "[main] Paper {} processed successfully (new, score={:.1}/10.0)",
                        paper.id, sum.score
                    );
                } else {
                    cached_count += 1;
                    progress_cached.fetch_add(1, Ordering::Relaxed);
                    info!(
                        "[main] Paper {} loaded from cache (score={:.1}/10.0)",
                        paper.id, sum.score
                    );
                }
                results.push((paper, sum, abs, is_new));
            }
            Err(e) => {
                let err_msg = e.to_string();
                if err_msg.contains("scanned") {
                    warn!("[main] Skipped {} (scanned PDF): {}", paper.id, e);
                    skipped_count += 1;
                    skipped.push((paper.id.clone(), err_msg));
                } else {
                    warn!("[main] Failed to summarize {}: {:#}", paper.id, e);
                    progress_failed.fetch_add(1, Ordering::Relaxed);
                    failed.push((paper.id.clone(), format!("{:#}", e)));
                }
            }
        }
    }

    proc_pb.finish_with_message(format!(
        "Done: {} cached, {} new, {} skipped, {} failed",
        cached_count,
        new_count,
        skipped_count,
        failed.len()
    ));

    progress_handle.abort();
    let final_done_dates = completed_dates.lock().await;
    let final_done_range = date_range(&final_done_dates);
    let final_remain_dates: Vec<String> = papers
        .iter()
        .filter(|p| !final_done_dates.contains(&p.published))
        .map(|p| p.published.clone())
        .collect();
    let final_remain_range = date_range(&final_remain_dates);
    info!(
        "[main] Progress: {}/{} completed (cached={}, new={}, failed={}) | done_dates={} | remain_dates={}",
        progress_completed.load(Ordering::Relaxed),
        total,
        progress_cached.load(Ordering::Relaxed),
        progress_new.load(Ordering::Relaxed),
        progress_failed.load(Ordering::Relaxed),
        final_done_range,
        final_remain_range
    );
    info!(
        "[main] Cached {} papers, newly processed {} papers, skipped {} (scanned), failed {}",
        cached_count,
        new_count,
        skipped_count,
        failed.len()
    );
    // List the papers that did not make it into the counts above, so none go unaccounted for.
    for (id, reason) in &skipped {
        warn!("[main]   skipped (scanned): {} -> {}", id, reason);
    }
    for (id, reason) in &failed {
        warn!("[main]   failed: {} -> {}", id, reason);
    }

    info!("[main] Sorting {} results by published time", results.len());
    results.sort_by(|a, b| b.0.published.cmp(&a.0.published));

    let mut output = String::new();
    for (i, (paper, summary, abs, _)) in results.iter().enumerate() {
        writeln!(output, "{}. {}", i + 1, paper.title).unwrap();
        writeln!(output, "   链接: https://arxiv.org/abs/{}", paper.id).unwrap();
        let authors_str = truncate_authors(&paper.authors, 6);
        writeln!(output, "   作者: {}", authors_str).unwrap();
        writeln!(output, "   发布时间: {}", paper.published).unwrap();
        writeln!(output, "   评分: {:.1}/10.0", summary.score).unwrap();
        writeln!(output, "   类型: {}", summary.paper_type).unwrap();
        writeln!(output, "   摘要: {}", abs).unwrap();
        writeln!(output, "   详细信息:").unwrap();
        for line in summary.summary.lines() {
            if !line.trim().is_empty() {
                writeln!(output, "    {}", line).unwrap();
            }
        }
        output.push('\n');
    }

    print!("{}", output);

    info!("[main] Run complete, output {} papers", results.len());
    Ok(())
}

fn is_arxiv_id(id: &str) -> bool {
    id.contains('.') || id.contains('/')
}

async fn add_papers(
    ids: &[String],
    cfg: &Config,
    force_refresh: bool,
    multi: Option<&MultiProgress>,
    use_oai: bool,
) -> Result<()> {
    info!("[main] Custom add mode, ids={:?}", ids);

    let (mut arxiv_ids, mut or_ids): (Vec<String>, Vec<String>) =
        ids.iter().cloned().partition(|id| is_arxiv_id(id));
    arxiv_ids.sort();
    or_ids.sort();

    let total_ids = arxiv_ids.len() + or_ids.len();

    // Progress bars
    let fetch_pb_raw = ProgressBar::new(total_ids as u64);
    fetch_pb_raw.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("##-"),
    );
    let fetch_pb = multi
        .map(|m| m.add(fetch_pb_raw.clone()))
        .unwrap_or(fetch_pb_raw);

    let proc_pb_raw = ProgressBar::new(0);
    proc_pb_raw.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40.green/white} {pos}/{len} {msg} ETA:{eta}")
            .unwrap()
            .progress_chars("##-"),
    );
    let proc_pb = multi
        .map(|m| m.add(proc_pb_raw.clone()))
        .unwrap_or(proc_pb_raw);

    // Channel for streaming papers from fetcher to processor
    let (paper_tx, mut paper_rx) = tokio::sync::mpsc::channel::<Paper>(32);

    // Clone data needed for fetch task
    let fetch_pb_clone = fetch_pb.clone();
    let _proc_pb_clone = proc_pb.clone();
    let enable_html_fallback = false;
    let arxiv_ids_owned = arxiv_ids.clone();
    let or_ids_owned = or_ids.clone();

    // Spawn fetch task
    let fetch_handle = tokio::spawn(async move {
        fetch_pb_clone.set_message("Fetching paper metadata...");

        // Fetch arxiv papers
        if !arxiv_ids_owned.is_empty() {
            fetch_pb_clone.set_message(format!(
                "Fetching {} arXiv papers...",
                arxiv_ids_owned.len()
            ));
            let result = if use_oai {
                fetch_papers_by_ids_oai(&arxiv_ids_owned, 5, Some(&fetch_pb_clone)).await
            } else {
                fetch_papers_by_ids(
                    &arxiv_ids_owned,
                    enable_html_fallback,
                    Some(&fetch_pb_clone),
                )
                .await
            };
            match result {
                Ok(papers) => {
                    info!("[main] Fetched {} papers from arXiv", papers.len());
                    for paper in papers {
                        let _ = paper_tx.send(paper).await;
                    }
                }
                Err(e) => {
                    warn!("[main] Failed to fetch arXiv papers: {}", e);
                }
            }
        }

        // Fetch openreview papers
        if !or_ids_owned.is_empty() {
            for id in &or_ids_owned {
                fetch_pb_clone.set_message(format!("Fetching OpenReview: {}", id));
                match ripple_reader::source::openreview::fetch_paper_by_forum(id).await {
                    Ok(p) => {
                        info!("[main] Fetched paper from OpenReview: {}", p.title);
                        let _ = paper_tx.send(p).await;
                    }
                    Err(e) => {
                        warn!("[main] Failed to fetch OpenReview paper {}: {}", id, e);
                    }
                }
                fetch_pb_clone.inc(1);
            }
        }

        fetch_pb_clone.finish_with_message("Fetch complete");
    });

    // Database and processor setup
    let db = Db::new(&cfg.database_url)
        .await
        .context("Failed to initialize database")?;
    info!("[main] Database initialized at {}", cfg.database_url);

    let summarizer = cfg
        .summarizer
        .as_ref()
        .expect("No summarizer provider configured");
    let processor = Processor::new(
        ripple_reader::processor::ProviderType::parse(&summarizer.provider_type),
        summarizer.name.clone(),
        summarizer.base_url.clone(),
        summarizer.api_keys.clone(),
        summarizer.model.clone(),
        summarizer.max_tokens,
        summarizer.reasoning_effort.clone(),
        summarizer.output_config_effort.clone(),
        summarizer.user_agent.clone(),
        summarizer.temperature,
        summarizer.top_p,
        cfg.llm_max_workers,
        cfg.insight_max_workers,
        cfg.llm_max_retries,
        cfg.summarize_prompt.clone(),
        cfg.translate_prompt.clone(),
        cfg.insight_prompt.clone(),
        cfg.review_prompt.clone(),
        None,
    );

    let pdf_semaphore = Arc::new(Semaphore::new(cfg.pdf_max_workers));
    info!(
        "[main] PDF semaphore initialized with pdf_max_workers={}",
        cfg.pdf_max_workers
    );

    // Process papers as they arrive (streaming pipeline)
    let mut results: Vec<(Paper, Summary, String, bool)> = Vec::new();
    let mut cached_count = 0usize;
    let mut new_count = 0usize;
    let mut skipped_count = 0usize;
    let mut total_fetched = 0usize;

    let mut tasks = FuturesUnordered::new();

    loop {
        tokio::select! {
            biased;

            // Process completed tasks first
            Some(result) = tasks.next() => {
                let (paper, res): (Paper, Result<(Summary, String, bool)>) = result;
                match res {
                    Ok((sum, abs, is_new)) => {
                        if is_new {
                            new_count += 1;
                            info!(
                                "[main] Paper {} processed successfully (new, score={:.1}/10.0)",
                                paper.id, sum.score
                            );
                        } else {
                            cached_count += 1;
                            info!(
                                "[main] Paper {} loaded from cache (score={:.1}/10.0)",
                                paper.id, sum.score
                            );
                        }
                        results.push((paper, sum, abs, is_new));
                    }
                    Err(e) => {
                        let err_msg = e.to_string();
                        if err_msg.contains("scanned") {
                            warn!("[main] Skipped {} (scanned PDF): {}", paper.id, e);
                            skipped_count += 1;
                        } else {
                            warn!("[main] Failed to summarize {}: {:#}", paper.id, e);
                        }
                    }
                }
                proc_pb.inc(1);
                proc_pb.set_message(format!(
                    "new:{new_count} cached:{cached_count} skip:{skipped_count}"
                ));
            }

            // Receive new papers from fetcher
            paper = paper_rx.recv() => {
                match paper {
                    Some(paper) => {
                        total_fetched += 1;
                        proc_pb.set_length(total_fetched as u64);

                        let s = &processor;
                        let db_ref = &db;
                        let pdf_sem = Arc::clone(&pdf_semaphore);
                        tasks.push(async move {
                            let res = process_paper_with_cache(s, db_ref, &paper, &pdf_sem, force_refresh, true).await;
                            (paper, res)
                        });
                    }
                    None => {
                        // Channel closed, fetcher is done
                        break;
                    }
                }
            }
        }
    }

    // Drain remaining tasks
    while let Some((paper, res)) = tasks.next().await {
        match res {
            Ok((sum, abs, is_new)) => {
                if is_new {
                    new_count += 1;
                    info!(
                        "[main] Paper {} processed successfully (new, score={:.1}/10.0)",
                        paper.id, sum.score
                    );
                } else {
                    cached_count += 1;
                    info!(
                        "[main] Paper {} loaded from cache (score={:.1}/10.0)",
                        paper.id, sum.score
                    );
                }
                results.push((paper, sum, abs, is_new));
            }
            Err(e) => {
                let err_msg = e.to_string();
                if err_msg.contains("scanned") {
                    warn!("[main] Skipped {} (scanned PDF): {}", paper.id, e);
                    skipped_count += 1;
                } else {
                    warn!("[main] Failed to summarize {}: {:#}", paper.id, e);
                }
            }
        }
        proc_pb.inc(1);
        proc_pb.set_message(format!(
            "new:{new_count} cached:{cached_count} skip:{skipped_count}"
        ));
    }

    proc_pb.finish_with_message(format!(
        "Done (new:{new_count} cached:{cached_count} skip:{skipped_count})"
    ));

    // Wait for fetch task
    let _ = fetch_handle.await;

    info!(
        "[main] Cached {} papers, newly processed {} papers, skipped {} (scanned)",
        cached_count, new_count, skipped_count
    );

    info!("[main] Sorting {} results by published time", results.len());
    results.sort_by(|a, b| b.0.published.cmp(&a.0.published));

    let mut output = String::new();
    for (i, (paper, summary, abs, _)) in results.iter().enumerate() {
        writeln!(output, "{}. {}", i + 1, paper.title).unwrap();
        writeln!(output, "   链接: https://arxiv.org/abs/{}", paper.id).unwrap();
        let authors_str = truncate_authors(&paper.authors, 6);
        writeln!(output, "   作者: {}", authors_str).unwrap();
        writeln!(output, "   发布时间: {}", paper.published).unwrap();
        writeln!(output, "   评分: {:.1}/10.0", summary.score).unwrap();
        writeln!(output, "   类型: {}", summary.paper_type).unwrap();
        writeln!(output, "   摘要: {}", abs).unwrap();
        writeln!(output, "   详细信息:").unwrap();
        for line in summary.summary.lines() {
            if !line.trim().is_empty() {
                writeln!(output, "    {}", line).unwrap();
            }
        }
        output.push('\n');
    }

    print!("{}", output);
    info!("[main] Add complete, output {} papers", results.len());
    Ok(())
}

async fn scrape_openreview(venue: &str, year: i32, dry_run: bool, cfg: &Config) -> Result<()> {
    let papers = ripple_reader::source::openreview::fetch_papers(venue, year).await?;
    if papers.is_empty() {
        println!("No papers found for {} {}", venue, year);
        return Ok(());
    }
    if dry_run {
        println!(
            "Would import {} papers from {} {}:",
            papers.len(),
            venue,
            year
        );
        for paper in &papers {
            println!("  {} - {}", paper.id, paper.title);
            if !paper.authors.is_empty() {
                println!("    Authors: {}", paper.authors.join(", "));
            }
        }
        return Ok(());
    }
    println!("Importing {} papers from {} {}", papers.len(), venue, year);
    add_openreview_papers(&papers, cfg, None).await
}

async fn import_openreview(path: &str, cfg: &Config, multi: Option<&MultiProgress>) -> Result<()> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("Failed to read {}", path))?;
    let papers: Vec<ripple_reader::source::Paper> = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse JSON in {}", path))?;
    if papers.is_empty() {
        println!("No papers found in {}", path);
        return Ok(());
    }
    println!("Importing {} papers from {}", papers.len(), path);
    add_openreview_papers(&papers, cfg, multi).await
}

async fn add_openreview_papers(
    papers: &[Paper],
    cfg: &Config,
    multi: Option<&MultiProgress>,
) -> Result<()> {
    let db = Db::new(&cfg.database_url)
        .await
        .context("Failed to initialize database")?;

    let summarizer = cfg
        .summarizer
        .as_ref()
        .expect("No summarizer provider configured");
    let processor = Processor::new(
        ripple_reader::processor::ProviderType::parse(&summarizer.provider_type),
        summarizer.name.clone(),
        summarizer.base_url.clone(),
        summarizer.api_keys.clone(),
        summarizer.model.clone(),
        summarizer.max_tokens,
        summarizer.reasoning_effort.clone(),
        summarizer.output_config_effort.clone(),
        summarizer.user_agent.clone(),
        summarizer.temperature,
        summarizer.top_p,
        cfg.llm_max_workers,
        cfg.insight_max_workers,
        cfg.llm_max_retries,
        cfg.summarize_prompt.clone(),
        cfg.translate_prompt.clone(),
        cfg.insight_prompt.clone(),
        cfg.review_prompt.clone(),
        None,
    );

    let pdf_semaphore = Arc::new(Semaphore::new(cfg.pdf_max_workers));

    let proc_pb_raw = ProgressBar::new(papers.len() as u64);
    proc_pb_raw.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40.green/white} {pos}/{len} {msg} ETA:{eta}")
            .unwrap()
            .progress_chars("##-"),
    );
    let proc_pb = multi
        .map(|m| m.add(proc_pb_raw.clone()))
        .unwrap_or(proc_pb_raw);
    proc_pb.set_message("Processing papers...");

    let mut tasks = FuturesUnordered::new();
    for paper in papers {
        let s = &processor;
        let db_ref = &db;
        let pdf_sem = Arc::clone(&pdf_semaphore);
        let paper = paper.clone();
        tasks.push(async move {
            let res = process_paper_with_cache(s, db_ref, &paper, &pdf_sem, false, false).await;
            (paper, res)
        });
    }

    let mut new_count = 0usize;
    let mut cached_count = 0usize;
    let mut skipped_count = 0usize;
    let mut failed_count = 0usize;
    while let Some((paper, res)) = tasks.next().await {
        proc_pb.inc(1);
        match res {
            Ok((sum, _abs, is_new)) => {
                if is_new {
                    new_count += 1;
                } else {
                    cached_count += 1;
                }
                info!("[import] {} processed (score={:.1})", paper.id, sum.score);
            }
            Err(e) => {
                let err_msg = e.to_string();
                if err_msg.contains("scanned") {
                    warn!("[import] Skipped {} (scanned PDF): {}", paper.id, e);
                    skipped_count += 1;
                } else {
                    warn!("[import] Failed to process {}: {}", paper.id, e);
                    failed_count += 1;
                }
            }
        }
    }
    proc_pb.finish_with_message(format!(
        "Done: {} cached, {} new, {} skipped, {} failed",
        cached_count, new_count, skipped_count, failed_count
    ));
    println!(
        "Done. New: {}, Cached: {}, Skipped (scanned): {}, Failed: {}",
        new_count, cached_count, skipped_count, failed_count
    );
    Ok(())
}

fn print_config(cfg: &Config) {
    info!("[config] ========== Configuration ==========");
    if let Some(s) = &cfg.summarizer {
        info!(
            "[config] SUMMARIZER: name={}, type={}, model={}, base_url={}",
            s.name, s.provider_type, s.model, s.base_url
        );
        info!("[config] SUMMARIZER_MAX_TOKENS: {}", s.max_tokens);
        info!("[config] SUMMARIZER_API_KEYS_COUNT: {}", s.api_keys.len());
        for (i, key) in s.api_keys.iter().enumerate() {
            info!("[config]   SUMMARIZER_API_KEY_{}: {}", i + 1, mask_key(key));
        }
    } else {
        info!("[config] SUMMARIZER: (none)");
    }
    let fields: [(&str, String); 8] = [
        ("LLM_MAX_WORKERS", cfg.llm_max_workers.to_string()),
        ("LLM_MAX_RETRIES", cfg.llm_max_retries.to_string()),
        ("PDF_MAX_WORKERS", cfg.pdf_max_workers.to_string()),
        ("ARXIV_MAX_RESULTS", cfg.arxiv_max_results.to_string()),
        ("ARXIV_PAGE_SIZE", cfg.arxiv_page_size.to_string()),
        (
            "SUMMARIZE_PROMPT_LEN",
            cfg.summarize_prompt.len().to_string(),
        ),
        (
            "TRANSLATE_PROMPT_LEN",
            cfg.translate_prompt.len().to_string(),
        ),
        ("DB_PATH", cfg.database_url.clone()),
    ];
    for (k, v) in fields {
        info!("[config] {}: {}", k, v);
    }
    info!("[config] ARXIV_QUERY: '{}'", cfg.arxiv_query);
    info!(
        "[config] ARXIV_CATEGORIES: {}",
        cfg.arxiv_categories.join(", ")
    );
    info!("[config] ARXIV_KEYWORDS: {}", cfg.arxiv_keywords.join(", "));
    info!("[config] ===================================");
}

fn mask_key(key: &str) -> String {
    if key.len() > 8 {
        format!("{}...{}", &key[..4], &key[key.len() - 4..])
    } else {
        "***".to_string()
    }
}

fn truncate_authors(authors: &[String], max: usize) -> String {
    if authors.len() > max {
        format!("{}, ...", authors[..max].join(", "))
    } else {
        authors.join(", ")
    }
}

fn date_range(dates: &[String]) -> String {
    if dates.is_empty() {
        return "none".to_string();
    }
    let mut sorted = dates.to_vec();
    sorted.sort();
    if sorted.len() == 1 {
        sorted[0].clone()
    } else {
        format!("{} ~ {}", sorted[0], sorted[sorted.len() - 1])
    }
}
