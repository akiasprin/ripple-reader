use anyhow::Result;
use ripple_reader::config::Config;
use ripple_reader::pipeline::process_paper;
use ripple_reader::processor::Processor;
use ripple_reader::source::arxiv::fetch_papers;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// 集成测试：指定单篇论文，调用真实 LLM API 输出结果。
///
/// 运行方式：
///   cargo test --test llm_paper_test -- --ignored --nocapture
///
/// 指定论文 ID：
///   TEST_PAPER_ID=2401.12345 cargo test --test llm_paper_test -- --ignored --nocapture
#[tokio::test]
#[ignore = "requires real LLM API key"]
async fn test_llm_with_specific_paper() -> Result<()> {
    let cfg = Config::from_env()?;

    let paper_id = std::env::var("TEST_PAPER_ID").unwrap_or_else(|_| "2604.14148".to_string());

    let query = format!("id:{}", paper_id);
    let papers = fetch_papers(&query, 1, 100).await?;
    assert!(!papers.is_empty(), "Paper {} not found on arXiv", paper_id);

    let paper = papers.into_iter().next().unwrap();

    let provider = cfg
        .insight_providers
        .into_iter()
        .find(|p| p.enabled)
        .expect("no enabled insight provider configured");

    let processor = Processor::new(
        ripple_reader::processor::ProviderType::parse(&provider.provider_type),
        provider.name,
        provider.base_url,
        provider.api_keys,
        provider.model,
        provider.max_tokens,
        provider.reasoning_effort,
        provider.output_config_effort,
        None,
        provider.user_agent,
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
    let (summary, translated_abs) = process_paper(&processor, &paper, &pdf_semaphore).await?;

    println!("\n========== LLM Test Result ==========");
    println!("Paper ID:    {}", paper.id);
    println!("Title:       {}", paper.title);
    println!("Authors:     {}", paper.authors.join(", "));
    println!("Published:   {}", paper.published);
    println!("Score:       {:.1}/10.0", summary.score);
    println!("Type:        {}", summary.paper_type);
    println!("Abstract:    {}", translated_abs);
    println!("Summary:\n{}", summary.summary);
    println!("=====================================\n");

    assert!(summary.summary.len() > 10, "Summary should not be empty");
    assert!(
        translated_abs.len() > 5,
        "Translated abstract should not be empty"
    );

    Ok(())
}
