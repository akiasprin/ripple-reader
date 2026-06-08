// SPDX-License-Identifier: MIT OR Apache-2.0

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::info;

mod provider;
mod tasks;

pub use provider::ProviderType;
pub use tasks::{ReviewResult, ReviseResult, Summary};

/// A parsed prompt file containing system and user templates.
///
/// Prompt files use section markers:
/// ```text
/// [SYSTEM]
/// ... system prompt ...
///
/// [USER]
/// ... user template with {placeholder} ...
///
/// [USER_IMAGES]   (optional, for insight/review with images)
/// ...
///
/// [IMAGE_SYSTEM]  (optional, for revise image mode)
/// ...
///
/// [IMAGE_USER]    (optional, for revise image mode)
/// ...
/// ```
#[derive(Debug, Clone)]
pub struct PromptTemplate {
    pub system: String,
    pub user: String,
    pub user_images: Option<String>,
    pub image_system: Option<String>,
    pub image_user: Option<String>,
}

impl PromptTemplate {
    /// Render a template by replacing `{key}` placeholders.
    pub fn render(template: &str, vars: &[(&str, &str)]) -> String {
        let mut result = template.to_string();
        for (key, value) in vars {
            result = result.replace(&format!("{{{}}}", key), value);
        }
        result
    }
}

/// Parse a prompt file into a `PromptTemplate`.
pub fn parse_prompt_file(content: &str) -> PromptTemplate {
    let mut sections: HashMap<String, String> = HashMap::new();
    let mut current_key = String::new();
    let mut current_content = String::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() > 2 {
            if !current_key.is_empty() {
                sections.insert(current_key.clone(), current_content.trim().to_string());
            }
            current_key = trimmed[1..trimmed.len() - 1].to_string();
            current_content.clear();
        } else {
            if !current_content.is_empty() {
                current_content.push('\n');
            }
            current_content.push_str(line);
        }
    }
    if !current_key.is_empty() {
        sections.insert(current_key, current_content.trim().to_string());
    }

    PromptTemplate {
        system: sections.get("SYSTEM").cloned().unwrap_or_default(),
        user: sections.get("USER").cloned().unwrap_or_default(),
        user_images: sections.get("USER_IMAGES").cloned(),
        image_system: sections.get("IMAGE_SYSTEM").cloned(),
        image_user: sections.get("IMAGE_USER").cloned(),
    }
}

pub struct Processor {
    pub(super) provider_type: ProviderType,
    pub(super) http_client: reqwest::Client,
    pub(super) anthropic_oauth_token: Option<String>,
    pub(super) api_keys: Vec<String>,
    pub(super) provider_name: String,
    pub(super) base_url: String,
    pub(super) model: String,
    pub(super) max_tokens: u32,
    pub(super) reasoning_effort: Option<String>,
    pub(super) output_config_effort: Option<String>,
    pub(super) thinking_budget_tokens: Option<u32>,
    pub(super) max_retries: usize,
    pub(super) summarize_template: PromptTemplate,
    pub(super) translate_template: PromptTemplate,
    pub(super) insight_template: std::sync::RwLock<PromptTemplate>,
    pub(super) review_template: std::sync::RwLock<PromptTemplate>,
    pub(super) revise_template: std::sync::RwLock<PromptTemplate>,
    pub(super) user_agent: Option<String>,
    pub(super) tools: Vec<serde_json::Value>,
    pub(super) semaphore: Arc<Semaphore>,
    pub(super) insight_semaphore: Arc<Semaphore>,
    pub(super) key_index: AtomicUsize,
}

impl Processor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider_type: ProviderType,
        provider_name: String,
        base_url: String,
        api_keys: Vec<String>,
        model: String,
        max_tokens: u32,
        reasoning_effort: Option<String>,
        output_config_effort: Option<String>,
        thinking_budget_tokens: Option<u32>,
        user_agent: Option<String>,
        llm_max_workers: usize,
        insight_max_workers: usize,
        max_retries: usize,
        summarize_prompt: String,
        translate_prompt: String,
        insight_prompt: String,
        review_prompt: String,
        anthropic_oauth_token: Option<String>,
    ) -> Self {
        info!(
            "[processor] Initializing provider={} type={:?} model={}, llm_max_workers={}, insight_max_workers={}, max_retries={}, keys={}, reasoning_effort={:?}, output_config_effort={:?}, thinking_budget_tokens={:?}, user_agent={:?}",
            provider_name, provider_type, model, llm_max_workers, insight_max_workers, max_retries, api_keys.len(), reasoning_effort, output_config_effort, thinking_budget_tokens, user_agent
        );

        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(14400))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let tools = if let Ok(content) = std::fs::read_to_string("prompts/tools/edit_text.json") {
            if let Ok(tool) = serde_json::from_str::<serde_json::Value>(&content) {
                vec![tool]
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        // Parse prompt files that contain [SYSTEM] and [USER] sections.
        let summarize_template = parse_prompt_file(&summarize_prompt);
        let translate_template = parse_prompt_file(&translate_prompt);
        let insight_template = parse_prompt_file(&insight_prompt);
        let review_template = parse_prompt_file(&review_prompt);

        // Load optional prompt files; warn but don't fail if missing.
        let load_prompt = |path: &str| -> String {
            match std::fs::read_to_string(path) {
                Ok(content) => content,
                Err(e) => {
                    tracing::warn!(
                        "[processor] Failed to load prompt file '{}': {}. Using empty string.",
                        path,
                        e
                    );
                    String::new()
                }
            }
        };
        let revise_template = parse_prompt_file(&load_prompt("prompts/revise.md"));

        Self {
            provider_type,
            http_client,
            anthropic_oauth_token,
            api_keys,
            provider_name,
            base_url,
            model,
            max_tokens,
            reasoning_effort,
            output_config_effort,
            thinking_budget_tokens,
            user_agent,
            max_retries,
            summarize_template,
            translate_template,
            insight_template: std::sync::RwLock::new(insight_template),
            review_template: std::sync::RwLock::new(review_template),
            revise_template: std::sync::RwLock::new(revise_template),
            tools,
            semaphore: Arc::new(Semaphore::new(llm_max_workers)),
            insight_semaphore: Arc::new(Semaphore::new(insight_max_workers)),
            key_index: AtomicUsize::new(0),
        }
    }

    /// Reload hot-updatable prompt templates at runtime.
    pub fn reload_templates(&self, insight: &str, review: &str, revise: &str) {
        if let Ok(mut t) = self.insight_template.write() {
            *t = parse_prompt_file(insight);
        }
        if let Ok(mut t) = self.review_template.write() {
            *t = parse_prompt_file(review);
        }
        if let Ok(mut t) = self.revise_template.write() {
            *t = parse_prompt_file(revise);
        }
        info!("[processor] Reloaded insight/review/revise templates");
    }

    pub fn insight_prompt(&self) -> String {
        self.insight_template.read().unwrap().system.clone()
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub(super) fn pick_key(&self) -> (usize, &str) {
        let idx = self.key_index.fetch_add(1, Ordering::Relaxed);
        let idx = idx % self.api_keys.len();
        let key = &self.api_keys[idx];
        let masked = provider::mask_key(key);
        info!(
            "[processor] Picking API key index {} masked={}",
            idx, masked
        );
        (idx, key)
    }
}
