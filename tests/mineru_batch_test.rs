use anyhow::Result;
use ripple_reader::config::Config;
use ripple_reader::db::Db;
use ripple_reader::mineru::{validate_extracted_figures, MinerUClient};
use std::collections::HashSet;
use std::fs::{read_to_string, OpenOptions};
use std::io::Write;
use std::path::Path;
use tracing::{info, warn};

const SUCCESS_FILE: &str = "tests/mineru_batch_success.txt";
const FAILED_FILE: &str = "tests/mineru_batch_failed.txt";

/// 批量测试：遍历 DB 中所有论文，执行 MinerU 图表提取 + hires 截图流程。
///
/// 成功和失败的论文 ID 分别记录到文件，每次执行自动跳过已记录的论文。
///
/// 运行方式：
///   cargo test --test mineru_batch_test -- --ignored --nocapture
#[tokio::test]
#[ignore = "requires real MinerU API key, pdfium and local DB"]
async fn test_mineru_batch_process_all_papers() -> Result<()> {
    tracing_subscriber::fmt()
        .with_file(true)
        .with_line_number(true)
        .init();

    let cfg = Config::from_env()?;
    let db = Db::new(&cfg.database_url).await?;

    let api_key = cfg
        .mineru_api_key
        .ok_or_else(|| anyhow::anyhow!("MINERU_API_KEY not set"))?;

    let client = MinerUClient::new(
        cfg.mineru_base_url,
        api_key,
        Some("figures".to_string()),
        false,
    );

    // 加载已处理的论文 ID
    let mut success_ids = load_id_set(SUCCESS_FILE)?;
    let mut failed_ids = load_id_set(FAILED_FILE)?;
    let processed: HashSet<String> = success_ids.union(&failed_ids).cloned().collect();

    info!(
        "[batch] Loaded {} success, {} failed, total processed: {}",
        success_ids.len(),
        failed_ids.len(),
        processed.len()
    );

    let mut all_ids = db.list_all_paper_ids().await?;
    all_ids.reverse(); // 从旧到新处理
    let todo: Vec<String> = all_ids
        .into_iter()
        .filter(|id| !processed.contains(id))
        .collect();

    info!(
        "[batch] Total papers in DB: {}, remaining to process: {}",
        processed.len() + todo.len(),
        todo.len()
    );

    let total = todo.len();
    let mut success_count = 0;
    let mut fail_count = 0;

    for (idx, id) in todo.iter().enumerate() {
        let pdf_url = format!("https://arxiv.org/pdf/{}.pdf", id);
        info!("[batch] [{}/{}] Processing {} ...", idx + 1, total, id);

        match client.extract_figures(&pdf_url, Some(id), &[], false).await {
            Ok(extracted) => {
                match validate_extracted_figures(&extracted, None, Some(id)) {
                    Ok(()) => {
                        info!(
                            "[batch] [{}/{}] {} MinerU OK ({} figures), starting hires...",
                            idx + 1,
                            total,
                            id,
                            extracted.images.len()
                        );
                        // Hires 截图
                        match ripple_reader::hires::generate_hires_images(
                            "arxiv", id, &pdf_url, 600, true,
                        )
                        .await
                        {
                            Ok(()) => {
                                info!("[batch] [{}/{}] {} hires OK", idx + 1, total, id);
                                append_line(SUCCESS_FILE, id)?;
                                success_ids.insert(id.clone());
                                success_count += 1;
                            }
                            Err(e) => {
                                warn!("[batch] [{}/{}] {} hires failed: {}", idx + 1, total, id, e);
                                let line =
                                    format!("{}|hires: {}", id, e.to_string().replace('\n', " "));
                                append_line(FAILED_FILE, &line)?;
                                failed_ids.insert(id.clone());
                                fail_count += 1;
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            "[batch] [{}/{}] {} validation failed: {}",
                            idx + 1,
                            total,
                            id,
                            e
                        );
                        let line = format!("{}|{}", id, e.replace('\n', " "));
                        append_line(FAILED_FILE, &line)?;
                        failed_ids.insert(id.clone());
                        fail_count += 1;
                    }
                }
            }
            Err(e) => {
                warn!(
                    "[batch] [{}/{}] {} extraction failed: {}",
                    idx + 1,
                    total,
                    id,
                    e
                );
                let line = format!("{}|{}", id, e.to_string().replace('\n', " "));
                append_line(FAILED_FILE, &line)?;
                failed_ids.insert(id.clone());
                fail_count += 1;
            }
        }
    }

    info!(
        "[batch] Done. Total: {}, Success: +{}, Failed: +{}",
        total, success_count, fail_count
    );

    Ok(())
}

fn load_id_set(path: &str) -> Result<HashSet<String>> {
    if !Path::new(path).exists() {
        return Ok(HashSet::new());
    }
    let content = read_to_string(path)?;
    let ids: HashSet<String> = content
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| {
            // 格式可能是 "id" 或 "id|error_message"，取第一段
            l.split('|').next().unwrap_or(l).to_string()
        })
        .collect();
    Ok(ids)
}

fn append_line(path: &str, line: &str) -> Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", line)?;
    Ok(())
}
