// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::{Context, Result};
use clap::Parser;
use ripple_reader::config::Config;
use ripple_reader::db::Db;
use ripple_reader::mdfmt::{print_results, transform};
use std::fs;
use std::path::PathBuf;
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "mdfmt", about = "Adjust markdown figure/table layout")]
struct Args {
    /// Input markdown file (or paper ID when --db is used)
    input: Option<String>,

    /// Output file (file mode only)
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Read from / write to database instead of files
    #[arg(long)]
    db: bool,

    /// Process all papers in database (requires --db)
    #[arg(long)]
    all: bool,

    /// Actually write changes back to database (without this, dry-run only)
    #[arg(long)]
    write: bool,

    /// Overwrite existing layouts (by default images with layout are skipped)
    #[arg(long)]
    force: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    if args.db {
        run_db_mode(args).await?;
    } else {
        run_file_mode(args)?;
    }

    Ok(())
}

fn run_file_mode(args: Args) -> Result<()> {
    let input_path = args.input.context("Input file required in file mode")?;
    let markdown = fs::read_to_string(&input_path)
        .with_context(|| format!("Failed to read {}", input_path))?;

    let input_pb = PathBuf::from(&input_path);
    let paper_id = input_pb
        .parent()
        .and_then(|p| {
            let p_str = p.to_string_lossy();
            p_str
                .find("figures/")
                .map(|idx| p_str[idx + 8..].to_string())
        })
        .unwrap_or_else(|| {
            input_pb
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string()
        });

    let (new_md, results) = transform(&paper_id, &markdown, args.force, None);
    print_results(&results);

    if let Some(out) = args.output {
        fs::write(&out, new_md).with_context(|| format!("Failed to write {}", out.display()))?;
        info!("Written to {}", out.display());
    } else {
        println!("\n--- Preview (use -o to save) ---\n{}", new_md);
    }

    Ok(())
}

async fn run_db_mode(args: Args) -> Result<()> {
    if args.all {
        run_db_all(args.write, args.force).await?;
    } else {
        let id = args.input.context("Paper ID required in DB mode")?;
        run_db_single(&id, args.write, args.force).await?;
    }
    Ok(())
}

/// Backup current insight before overwriting.
/// Returns Ok(()) on success; propagates errors so the caller can
/// decide whether it's safe to proceed with the write.
async fn backup_paper_insight(db: &Db, paper: &ripple_reader::db::types::DbPaper) -> Result<()> {
    let insight_processed_at_str = paper
        .insight_processed_at
        .as_ref()
        .map(|dt| dt.to_rfc3339());
    let insight_reviewed_at_str = paper.insight_reviewed_at.as_ref().map(|dt| dt.to_rfc3339());
    db.backup_insight(
        &paper.id,
        &paper.insight,
        insight_processed_at_str.as_deref(),
        &paper.insight_review,
        insight_reviewed_at_str.as_deref(),
    )
    .await
    .with_context(|| format!("Failed to backup insight for {}", paper.id))?;
    Ok(())
}

async fn run_db_single(id: &str, write: bool, force: bool) -> Result<()> {
    let cfg = Config::from_env().context("Failed to load config")?;
    let db = Db::new(&cfg.database_url)
        .await
        .context("Failed to connect to database")?;

    let paper = db
        .get_paper(id)
        .await
        .with_context(|| format!("Failed to fetch paper {}", id))?
        .context("Paper not found")?;

    let markdown = &paper.insight;
    if markdown.is_empty() || markdown == "__ANALYZING__" {
        println!("Paper {} has no insight content, skipping.", id);
        return Ok(());
    }

    let paper_id = format!("{}/{}", paper.source_type, paper.id);
    let zip_path = std::path::PathBuf::from(format!("figures/{}/mineru.zip", paper_id));
    let page_width_px = ripple_reader::mdfmt::page_width_from_layout(&zip_path, 600);
    let (new_md, results) = transform(&paper_id, markdown, force, page_width_px);
    print_results(&results);

    let changed = new_md != *markdown;
    if changed {
        if write {
            let mut updated = paper;

            // Backup current insight before overwriting.
            backup_paper_insight(&db, &updated).await?;
            println!("  [BACKUP] Saved pre-mdfmt snapshot");

            updated.insight = new_md;
            db.save_paper(&updated)
                .await
                .context("Failed to save paper")?;
            println!("\n[WRITTEN] Paper {}", id);
        } else {
            println!(
                "\n[DRY-RUN] Use --write to persist changes for paper {}",
                id
            );
        }
    } else {
        println!("\n[UNCHANGED] Paper {}", id);
    }

    Ok(())
}

async fn run_db_all(write: bool, force: bool) -> Result<()> {
    let cfg = Config::from_env().context("Failed to load config")?;
    let db = Db::new(&cfg.database_url)
        .await
        .context("Failed to connect to database")?;

    let ids = db
        .list_all_paper_ids()
        .await
        .context("Failed to list papers")?;

    let mut processed = 0;
    let mut changed = 0;

    for id in ids {
        let Some(paper) = db.get_paper(&id).await? else {
            continue;
        };
        let markdown = &paper.insight;
        if markdown.is_empty() || markdown == "__ANALYZING__" {
            continue;
        }

        let paper_id = format!("{}/{}", paper.source_type, paper.id);
        let zip_path = std::path::PathBuf::from(format!("figures/{}/mineru.zip", paper_id));
        let page_width_px = ripple_reader::mdfmt::page_width_from_layout(&zip_path, 600);
        let (new_md, results) = transform(&paper_id, markdown, force, page_width_px);
        let has_changes = new_md != *markdown;

        if has_changes {
            println!("\nPaper {}:", paper.id);
            print_results(&results);

            if write {
                let mut updated = paper;

                // Backup current insight before overwriting.
                backup_paper_insight(&db, &updated).await?;

                updated.insight = new_md;
                db.save_paper(&updated)
                    .await
                    .context("Failed to save paper")?;
                println!("[WRITTEN]");
            } else {
                println!("[DRY-RUN] Use --write to persist");
            }
            changed += 1;
        }
        processed += 1;
    }

    println!(
        "\nDone: {} papers processed, {} papers changed.",
        processed, changed
    );

    Ok(())
}
