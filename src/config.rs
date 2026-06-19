// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::env;
use tracing::info;

#[derive(Debug, Clone)]
pub struct InsightProvider {
    pub name: String,
    pub provider_type: String,
    pub base_url: String,
    pub api_keys: Vec<String>,
    pub model: String,
    pub max_tokens: u32,
    pub reasoning_effort: Option<String>,
    pub output_config_effort: Option<String>,
    pub user_agent: Option<String>,
    /// Sampling temperature for OpenAI / Anthropic chat-completions / messages
    /// requests. `None` means "use the upstream server default" — no
    /// `temperature` key is sent on the wire.
    pub temperature: Option<f32>,
    /// Nucleus-sampling cutoff. Same "unset = server default" contract as
    /// `temperature`. Anthropic recommends setting either `temperature` OR
    /// `top_p`, not both; we surface both and let the user decide.
    pub top_p: Option<f32>,
    pub is_digest: bool,
    pub is_comment: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub llm_max_workers: usize,
    pub llm_max_retries: usize,
    pub pdf_max_workers: usize,

    pub arxiv_query: String,
    pub arxiv_max_results: usize,
    pub arxiv_page_size: usize,
    pub arxiv_categories: Vec<String>,
    pub arxiv_keywords: Vec<String>,
    pub arxiv_fetch_cron: String,
    pub arxiv_cache_ttl_hours: usize,

    pub summarize_prompt: String,
    pub translate_prompt: String,

    pub database_url: String,
    pub web_password: Option<String>,

    pub insight_providers: Vec<InsightProvider>,
    pub summarizer: Option<InsightProvider>,
    pub insight_max_workers: usize,
    pub insight_prompt: String,
    pub insight_prompts: HashMap<String, String>,
    pub insight_prompts_mtime: HashMap<String, DateTime<Utc>>,
    pub review_prompt: String,
    pub insight_review_max_attempts: usize,
    pub auto_review_insight: bool,

    pub mineru_api_key: Option<String>,
    pub mineru_base_url: String,
    pub mineru_no_cache: bool,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        info!("Loading configuration from .env");
        match dotenvy::from_filename_override(".env") {
            Ok(path) => {
                info!(".env loaded from: {:?}", path);
            }
            Err(e) => {
                tracing::error!("Failed to load .env: {}", e);
            }
        }

        // Helper: require a non-empty string env var
        let require = |k: &str| -> Result<String> {
            let v = env::var(k).with_context(|| format!("{} is required in .env", k))?;
            anyhow::ensure!(!v.trim().is_empty(), "{} cannot be empty", k);
            Ok(v)
        };

        // Helper: require a usize env var (no default)
        let require_usize = |k: &str| -> Result<usize> {
            let v = env::var(k).with_context(|| format!("{} is required in .env", k))?;
            let n = v
                .parse()
                .with_context(|| format!("{} must be a valid integer, got: '{}'", k, v))?;
            Ok(n)
        };

        let llm_max_workers = require_usize("LLM_MAX_WORKERS")?;
        let llm_max_retries = env::var("LLM_MAX_RETRIES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(48);
        let pdf_max_workers = require_usize("PDF_MAX_WORKERS")?;

        // --- arXiv config ---
        let arxiv_query = env::var("ARXIV_QUERY").unwrap_or_default();
        let arxiv_max_results = require_usize("ARXIV_MAX_RESULTS")?;
        let arxiv_page_size = require_usize("ARXIV_PAGE_SIZE")?;
        let arxiv_categories = split_csv(&env::var("ARXIV_CATEGORIES").unwrap_or_default());
        let arxiv_keywords = split_csv(&env::var("ARXIV_KEYWORDS").unwrap_or_default());
        let arxiv_fetch_cron = env::var("ARXIV_FETCH_CRON")
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "0 30 14 * * *".to_string());
        let arxiv_cache_ttl_hours = env::var("ARXIV_CACHE_TTL_HOURS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(48);

        // --- Prompts & DB ---
        let load_prompt = |file: &str| -> Result<String> {
            std::fs::read_to_string(file).with_context(|| format!("{} file is required", file))
        };

        let prompt_mtime = |file: &str| -> String {
            std::fs::metadata(file)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| {
                    let secs = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
                    chrono::DateTime::from_timestamp(secs, 0)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                })
                .unwrap_or_else(|| "(not found)".to_string())
        };
        let summarize_prompt = load_prompt("prompts/summarize.md")?;
        let translate_prompt = load_prompt("prompts/translate.md")?;
        let database_url = require("DATABASE_URL")?;
        let web_password = env::var("WEB_PASSWORD").ok().filter(|s| !s.is_empty());

        // --- Insight analysis config (multi-provider) ---
        let insight_max_workers = require_usize("INSIGHT_MAX_WORKERS")?;
        let insight_prompt = load_prompt("prompts/insight.md")?;

        // Scan prompts/insight_*.md for variant prompts, plus prompts/insight.md as default
        let mut insight_prompts: HashMap<String, String> = HashMap::new();
        let mut insight_prompts_mtime: HashMap<String, DateTime<Utc>> = HashMap::new();
        if let Ok(entries) = std::fs::read_dir("prompts") {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                let (variant, path) = if name == "insight.md" {
                    ("default".to_string(), entry.path())
                } else if name.starts_with("insight_") && name.ends_with(".md") {
                    let variant = name
                        .trim_start_matches("insight_")
                        .trim_end_matches(".md")
                        .to_string();
                    (variant, entry.path())
                } else {
                    continue;
                };
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if !content.is_empty() {
                        insight_prompts.insert(variant.clone(), content);
                        if let Ok(meta) = std::fs::metadata(&path) {
                            if let Ok(modified) = meta.modified() {
                                if let Ok(dur) = modified.duration_since(std::time::UNIX_EPOCH) {
                                    if let Some(dt) =
                                        DateTime::from_timestamp(dur.as_secs() as i64, 0)
                                    {
                                        insight_prompts_mtime.insert(variant, dt);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let review_prompt = std::fs::read_to_string("prompts/review.md").unwrap_or_default();
        let insight_review_max_attempts = env::var("INSIGHT_REVIEW_MAX_ATTEMPTS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        let auto_review_insight = env::var("AUTO_REVIEW_INSIGHT")
            .ok()
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(true);

        let mut insight_providers = Vec::new();
        for i in 1..=10 {
            let name_key = format!("INSIGHT_PROVIDER_{}_NAME", i);
            if let Ok(name) = env::var(&name_key) {
                if name.trim().is_empty() {
                    continue;
                }
                let base_url =
                    env::var(format!("INSIGHT_PROVIDER_{}_BASE_URL", i)).unwrap_or_default();

                let provider_type =
                    env::var(format!("INSIGHT_PROVIDER_{}_TYPE", i)).unwrap_or_default();

                // Collect API keys
                let mut api_keys = Vec::new();
                for ki in 1..=100 {
                    if let Ok(key) = env::var(format!("INSIGHT_PROVIDER_{}_API_KEY_{}", i, ki)) {
                        if !key.is_empty() {
                            api_keys.push(key);
                        }
                    }
                }
                if api_keys.is_empty() {
                    let single =
                        env::var(format!("INSIGHT_PROVIDER_{}_API_KEY", i)).unwrap_or_default();
                    if !single.is_empty() {
                        api_keys.push(single);
                    }
                }

                let model = env::var(format!("INSIGHT_PROVIDER_{}_MODEL", i)).unwrap_or_default();

                let max_tokens = env::var(format!("INSIGHT_PROVIDER_{}_MAX_TOKENS", i))
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0u32);

                let provider_reasoning =
                    env::var(format!("INSIGHT_PROVIDER_{}_REASONING_EFFORT", i))
                        .ok()
                        .filter(|s| !s.is_empty());
                // Same value drives both OpenAI (reasoning_effort) and Anthropic (output_config_effort)
                let provider_output_effort = provider_reasoning.clone();

                let temperature = env::var(format!("INSIGHT_PROVIDER_{}_TEMPERATURE", i))
                    .ok()
                    .filter(|s| !s.is_empty())
                    .and_then(|v| match v.parse::<f32>() {
                        Ok(t) if (0.0..=2.0).contains(&t) => Some(t),
                        Ok(t) => {
                            info!(
                                "INSIGHT_PROVIDER_{}_TEMPERATURE = {} is out of [0,2]; ignoring",
                                i, t
                            );
                            None
                        }
                        Err(e) => {
                            info!(
                                "INSIGHT_PROVIDER_{}_TEMPERATURE = {:?} is not a number: {}",
                                i, v, e
                            );
                            None
                        }
                    });
                let top_p = env::var(format!("INSIGHT_PROVIDER_{}_TOP_P", i))
                    .ok()
                    .filter(|s| !s.is_empty())
                    .and_then(|v| match v.parse::<f32>() {
                        Ok(p) if (0.0..=1.0).contains(&p) => Some(p),
                        Ok(p) => {
                            info!(
                                "INSIGHT_PROVIDER_{}_TOP_P = {} is out of [0,1]; ignoring",
                                i, p
                            );
                            None
                        }
                        Err(e) => {
                            info!(
                                "INSIGHT_PROVIDER_{}_TOP_P = {:?} is not a number: {}",
                                i, v, e
                            );
                            None
                        }
                    });

                let is_digest = env::var(format!("INSIGHT_PROVIDER_{}_IS_DIGEST", i))
                    .ok()
                    .map(|v| v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);

                let is_comment = env::var(format!("INSIGHT_PROVIDER_{}_IS_COMMENT", i))
                    .ok()
                    .map(|v| v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);

                let enabled = env::var(format!("INSIGHT_PROVIDER_{}_ENABLED", i))
                    .ok()
                    .map(|v| !v.eq_ignore_ascii_case("false"))
                    .unwrap_or(true);

                anyhow::ensure!(
                    !base_url.is_empty(),
                    "INSIGHT_PROVIDER_{}_BASE_URL is required",
                    i
                );
                anyhow::ensure!(
                    !model.is_empty(),
                    "INSIGHT_PROVIDER_{}_MODEL is required",
                    i
                );

                insight_providers.push(InsightProvider {
                    name,
                    provider_type,
                    base_url,
                    api_keys,
                    model,
                    max_tokens,
                    reasoning_effort: provider_reasoning,
                    output_config_effort: provider_output_effort,
                    user_agent: None,
                    temperature,
                    top_p,
                    is_digest,
                    is_comment,
                    enabled,
                });
            }
        }

        // --- MinerU config ---
        let mineru_api_key = env::var("MINERU_API_KEY").ok().filter(|s| !s.is_empty());
        let mineru_base_url =
            env::var("MINERU_BASE_URL").unwrap_or_else(|_| "https://mineru.net".to_string());
        let mineru_no_cache = env::var("MINERU_NO_CACHE")
            .ok()
            .map(|s| s.eq_ignore_ascii_case("true") || s == "1")
            .unwrap_or(false);

        // --- Find summarizer ---
        let mut summarizer = insight_providers.iter().find(|p| p.is_digest).cloned();
        if summarizer.is_none() {
            summarizer = insight_providers.iter().find(|p| p.enabled).cloned();
        }

        // --- Log all loaded config ---
        info!("========== Configuration ==========");
        info!("LLM_MAX_WORKERS = {}", llm_max_workers);
        info!("LLM_MAX_RETRIES = {}", llm_max_retries);
        info!("PDF_MAX_WORKERS = {}", pdf_max_workers);
        info!("ARXIV_MAX_RESULTS = {}", arxiv_max_results);
        info!("ARXIV_PAGE_SIZE = {}", arxiv_page_size);
        info!("ARXIV_FETCH_CRON = {}", arxiv_fetch_cron);
        info!("ARXIV_CACHE_TTL_HOURS = {}", arxiv_cache_ttl_hours);
        // Sync the .env-loaded value into the in-process atomic used by
        // read_cache(). Without this, the atomic stays at its static default
        // (48h) until an admin UI update fires, even though .env says
        // ARXIV_CACHE_TTL_HOURS=1000.
        crate::source::arxiv::set_cache_ttl_hours(arxiv_cache_ttl_hours);
        info!("DATABASE_URL = {}", database_url);
        info!(
            "INSIGHT_PROVIDERS = {} provider(s)",
            insight_providers.len()
        );
        for (i, p) in insight_providers.iter().enumerate() {
            info!(
                "  PROVIDER[{}]: name={}, type={}, model={}, base_url={}, is_digest={}, is_comment={}, enabled={}",
                i, p.name, p.provider_type, p.model, p.base_url, p.is_digest, p.is_comment, p.enabled
            );
        }
        info!(
            "SUMMARIZER = {}",
            summarizer
                .as_ref()
                .map(|s| s.name.as_str())
                .unwrap_or("(none)")
        );
        info!("INSIGHT_MAX_WORKERS = {}", insight_max_workers);
        info!(
            "INSIGHT_REVIEW_MAX_ATTEMPTS = {}",
            insight_review_max_attempts
        );
        info!("AUTO_REVIEW_INSIGHT = {}", auto_review_insight);
        info!("MINERU_BASE_URL = {}", mineru_base_url);
        info!(
            "MINERU_API_KEY = {}",
            if mineru_api_key.is_some() {
                "***"
            } else {
                "not set"
            }
        );
        info!("MINERU_NO_CACHE = {}", mineru_no_cache);
        info!("========== PROMPTS ==========");
        info!("SUMMARIZE_PROMPT: {}", prompt_mtime("prompts/summarize.md"));
        info!("TRANSLATE_PROMPT: {}", prompt_mtime("prompts/translate.md"));
        info!("INSIGHT_PROMPT: {}", prompt_mtime("prompts/insight.md"));
        for variant in insight_prompts.keys() {
            let file = if variant == "default" {
                "prompts/insight.md".to_string()
            } else {
                format!("prompts/insight_{}.md", variant)
            };
            info!(
                "INSIGHT_PROMPT_{}: {}",
                variant.to_uppercase(),
                prompt_mtime(&file)
            );
        }
        info!("REVIEW_PROMPT: {}", prompt_mtime("prompts/review.md"));
        info!("===================================");

        Ok(Config {
            llm_max_workers,
            llm_max_retries,
            pdf_max_workers,

            arxiv_query,
            arxiv_max_results,
            arxiv_page_size,
            arxiv_categories,
            arxiv_keywords,
            arxiv_fetch_cron,
            arxiv_cache_ttl_hours,

            summarize_prompt,
            translate_prompt,

            database_url,
            web_password,

            insight_providers,
            summarizer,
            insight_max_workers,
            insight_prompt,
            insight_prompts,
            insight_prompts_mtime,
            review_prompt,
            insight_review_max_attempts,
            auto_review_insight,

            mineru_api_key,
            mineru_base_url,
            mineru_no_cache,
        })
    }
}

fn split_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}
