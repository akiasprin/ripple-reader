// SPDX-License-Identifier: MIT OR Apache-2.0

use ripple_reader::config::Config;
use ripple_reader::db::Db;
use ripple_reader::web::insight_html::render_insight_html;
use std::time::Instant;
use tracing::{info, warn};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = Config::from_env().expect("Failed to load config");
    let db = Db::new(&config.database_url)
        .await
        .expect("Failed to connect to DB");

    let paper_ids = db
        .list_paper_ids_with_insight()
        .await
        .expect("Failed to query papers");

    let total = paper_ids.len();
    info!("Found {} papers with insight content", total);

    if total == 0 {
        info!("Nothing to do.");
        return;
    }

    let mut success = 0usize;
    let mut skipped = 0usize;

    for (i, paper_id) in paper_ids.iter().enumerate() {
        let paper = match db.get_paper(paper_id).await {
            Ok(Some(p)) => p,
            Ok(None) => {
                warn!("[{}/{}] {} not found, skipping", i + 1, total, paper_id);
                skipped += 1;
                continue;
            }
            Err(e) => {
                warn!(
                    "[{}/{}] {} DB error: {}, skipping",
                    i + 1,
                    total,
                    paper_id,
                    e
                );
                skipped += 1;
                continue;
            }
        };

        if paper.insight.is_empty() || paper.insight == "__ANALYZING__" {
            skipped += 1;
            continue;
        }

        let t0 = Instant::now();
        let resp = render_insight_html(&paper.source_type, paper_id, &paper.insight);
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;

        match db
            .upsert_insight_html_cache(paper_id, &resp.html, &resp.toc)
            .await
        {
            Ok(()) => {
                success += 1;
                info!(
                    "[{}/{}] {} ({} chars) -> {} chars, {} toc, {:.0}ms",
                    i + 1,
                    total,
                    paper_id,
                    paper.insight.len(),
                    resp.html.len(),
                    resp.toc.len(),
                    elapsed_ms
                );
            }
            Err(e) => {
                warn!(
                    "[{}/{}] {} cache write failed: {}",
                    i + 1,
                    total,
                    paper_id,
                    e
                );
                skipped += 1;
            }
        }
    }

    info!(
        "Done: {} success, {} skipped, {} total",
        success, skipped, total
    );
}
