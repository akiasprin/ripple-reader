// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::Result;
use regex::Regex;
use serde_json;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, LazyLock};
use tracing::{info, warn};

use super::provider::{ContentPart, ImageUrl, Message, MessageContent};
use super::Processor;

static RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<think(?:ing)?>.*?</think(?:ing)?>").expect("invalid think regex")
});

static SCORE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Score[:\s]*(\d{1,2}(?:\.\d+)?)(?:/10(?:\.00)?)?").expect("invalid score regex")
});

static SUMMARY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)Summary[:\s]*(?:Score[:\s]*\d{1,2}(?:\.\d+)?(?:/10(?:\.00)?)?\s*)?(.*)")
        .expect("invalid summary regex")
});

static TAG_LINE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*[-*]\s*([^:]+)[:\s]*(\d{1,2}(?:\.\d+)?)\s*$").expect("invalid tag line regex")
});

static TAGS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)Tags[:\s]*\n(.*)").expect("invalid tags regex"));

static TYPE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Type[:\s]*([^\n]+)").expect("invalid type regex"));

#[derive(Debug, Clone)]
pub struct ReviseResult {
    pub explanation: String,
    pub old_text: String,
    pub new_text: String,
}

#[derive(Debug, Clone)]
pub struct Summary {
    pub score: f32,
    pub paper_type: String,
    pub summary: String,
    pub tags: Vec<(String, f32)>,
    #[allow(dead_code)]
    pub raw: String,
}

#[derive(Debug, Clone)]
pub struct ReviewResult {
    pub content: String,
    pub qualified: bool,
}

impl Processor {
    pub async fn summarize(&self, title: &str, text: &str) -> Result<Summary> {
        let truncated = text.chars().take(30000).collect::<String>();
        info!(
            "[processor] Summarize input title='{}', text_len={} (truncated to {})",
            title,
            text.len(),
            truncated.len()
        );
        let user_prompt = super::PromptTemplate::render(
            &self.summarize_template.user,
            &[("title", title), ("content", &truncated)],
        );

        let system_prompt = self.summarize_template.system.clone();
        let messages = vec![
            Message {
                role: "system".to_string(),
                content: MessageContent::Text(system_prompt),
            },
            Message {
                role: "user".to_string(),
                content: MessageContent::Text(user_prompt),
            },
        ];

        let mut last_parse_err = None;
        for attempt in 0..=5 {
            let content = self
                .call_api("summarize", messages.clone(), None, None)
                .await?;
            let content = strip_think_tags(&content);

            match (parse_score(&content), parse_summary(&content)) {
                (Some(score), Some(summary)) => {
                    let paper_type = parse_paper_type(&content).unwrap_or_default();
                    let tags = parse_tags(&content);
                    info!(
                        "[processor] Parsed score={:.1}/5.0, type='{}', summary_len={} chars, tags={:?}",
                        score,
                        paper_type,
                        summary.len(),
                        tags
                    );
                    return Ok(Summary {
                        score,
                        paper_type,
                        summary,
                        tags,
                        raw: content,
                    });
                }
                (None, _) => {
                    warn!(
                        "[processor] LLM response missing Score on attempt {}: {}",
                        attempt + 1,
                        content.chars().take(200).collect::<String>()
                    );
                    last_parse_err = Some("LLM response missing Score field".to_string());
                }
                (_, None) => {
                    warn!(
                        "[processor] LLM response missing Summary on attempt {}: {}",
                        attempt + 1,
                        content.chars().take(200).collect::<String>()
                    );
                    last_parse_err = Some("LLM response missing Summary field".to_string());
                }
            }
        }

        anyhow::bail!(
            "LLM summarize failed after parse retries: {}",
            last_parse_err.unwrap_or_default()
        )
    }

    pub async fn translate(&self, text: &str) -> Result<String> {
        let truncated = text.chars().take(30000).collect::<String>();
        info!(
            "[processor] Translate input length={} (truncated to {})",
            text.len(),
            truncated.len()
        );
        let user_prompt =
            super::PromptTemplate::render(&self.translate_template.user, &[("text", &truncated)]);

        let messages = vec![
            Message {
                role: "system".to_string(),
                content: MessageContent::Text(self.translate_template.system.clone()),
            },
            Message {
                role: "user".to_string(),
                content: MessageContent::Text(user_prompt),
            },
        ];
        let content = self.call_api("translate", messages, None, None).await?;
        let content = strip_think_tags(&content);
        info!(
            "[processor] Translated length={} chars (after stripping think tags)",
            content.len()
        );
        Ok(content.trim().to_string())
    }

    pub async fn insight_analyze(
        &self,
        title: &str,
        images: &[(String, String, String)],
        text: Option<&str>,
        custom_prompt: Option<&str>,
    ) -> Result<String> {
        info!(
            "[processor] Insight analyze input title='{}', images={}, text={}",
            title,
            images.len(),
            text.is_some()
        );
        let system_prompt = if let Some(cp) = custom_prompt {
            cp.to_string()
        } else {
            self.insight_template.read().unwrap().system.clone()
        };

        let user_text = {
            let tpl = self.insight_template.read().unwrap();
            super::PromptTemplate::render(
                &tpl.user,
                &[("title", title), ("content", text.unwrap_or(""))],
            )
        };
        let user_content = if !images.is_empty() {
            let mut parts = vec![ContentPart {
                content_type: "text".to_string(),
                text: Some(user_text),
                image_url: None,
            }];
            for (name, label, _url) in images.iter() {
                parts.push(ContentPart {
                    content_type: "text".to_string(),
                    text: Some(format!(
                        "\n[Extracted Figure: {} | Caption(Possible disorder): {}]\n",
                        name, label
                    )),
                    image_url: None,
                });
            }
            MessageContent::Parts(parts)
        } else {
            MessageContent::Text(user_text)
        };

        let messages = vec![
            Message {
                role: "system".to_string(),
                content: MessageContent::Text(system_prompt),
            },
            Message {
                role: "user".to_string(),
                content: user_content,
            },
        ];

        let logid = format!("{:06x}", rand::random::<u32>());
        info!(
            "[processor] [{}] Acquiring insight semaphore for insight_analyze",
            logid
        );
        let _permit = self.insight_semaphore.acquire().await?;
        info!("[processor] [{}] Insight semaphore acquired", logid);

        let content = self
            .call_api_inner("insight_analyze", &logid, messages, None, None)
            .await?;
        info!(
            "[processor] Insight analyze output length={} chars",
            content.len()
        );
        let mut text = crate::db::normalize_text(&content);
        text.push_str(&format!(
            "\n\n<div style=\"text-align:right; color:var(--text3); font-size:12px;\">（本文使用 {} 模型生成，内容仅供参考）</div>",
            self.provider_name
        ));
        Ok(crate::db::normalize_insight_heading(&text))
    }

    #[allow(clippy::too_many_arguments)]
    /// `conversation_history`: ordered (role, content) tuples from previous turns
    /// in this thread (root first). Role is "user" or "assistant". @AGENT prefixes
    /// are already stripped from user messages. Empty for first-turn requests.
    // TODO: IMAGE_SYSTEM / IMAGE_USER prompt sections are reserved but not wired up
    // end-to-end. Front-end does not upload images and back-end always passes None
    // for image_path / image_base64. Add file upload + base64 conversion when needed.
    pub async fn revise(
        &self,
        full_text: &str,
        selected: &str,
        before_ctx: &str,
        after_ctx: &str,
        instruction: &str,
        conversation_history: &[(String, String)],
        image_path: Option<&str>,
        image_base64: Option<&str>,
        source: Option<&str>,
        paper_id: Option<&str>,
    ) -> Result<ReviseResult> {
        let (system_prompt, user_text) = if image_base64.is_some() {
            let tpl = self.revise_template.read().unwrap();
            let sys = tpl.image_system.clone().unwrap_or_default();
            let user_tpl = tpl.image_user.clone().unwrap_or_else(|| tpl.user.clone());
            drop(tpl);
            let user = super::PromptTemplate::render(
                &user_tpl,
                &[
                    ("full_text", full_text),
                    ("image_path", image_path.unwrap_or("")),
                    ("before_ctx", before_ctx),
                    ("after_ctx", after_ctx),
                    ("instruction", instruction),
                ],
            );
            (sys, user)
        } else {
            let tpl = self.revise_template.read().unwrap();
            let sys = tpl.system.clone();
            let user_tpl = tpl.user.clone();
            drop(tpl);
            let user = super::PromptTemplate::render(
                &user_tpl,
                &[
                    ("full_text", full_text),
                    ("selected", selected),
                    ("before_ctx", before_ctx),
                    ("after_ctx", after_ctx),
                    ("instruction", instruction),
                ],
            );
            (sys, user)
        };

        let user_content = if let Some(b64) = image_base64 {
            let parts = vec![
                ContentPart {
                    content_type: "text".to_string(),
                    text: Some(user_text),
                    image_url: None,
                },
                ContentPart {
                    content_type: "image_url".to_string(),
                    text: None,
                    image_url: Some(ImageUrl {
                        url: b64.to_string(),
                    }),
                },
            ];
            MessageContent::Parts(parts)
        } else {
            MessageContent::Text(user_text)
        };

        // Build messages with structured multi-turn conversation history.
        //
        // First turn (no history):
        //   system + user(full_text + context + instruction)
        //
        // Multi-turn:
        //   system + user(full_text + context) + history... + user(instruction only)
        // The full text is prepended as a standalone user message so the model
        // always has the paper content, even in long conversations.
        let has_history = !conversation_history.is_empty();
        let mut messages = Vec::with_capacity(1 + conversation_history.len() + 2);
        messages.push(Message {
            role: "system".to_string(),
            content: MessageContent::Text(system_prompt),
        });
        if has_history {
            // Multi-turn: re-render the grounding message with the instruction
            // placeholder replaced by a reference to the conversation below.
            // This avoids duplicating the current instruction in both the
            // grounding message and the final Continue: follow-up, and
            // prevents the current instruction from overwriting the original
            // instruction's structural context in the template.
            let tpl = self.revise_template.read().unwrap();
            let user_tpl = tpl.user.clone();
            drop(tpl);
            let grounding = super::PromptTemplate::render(
                &user_tpl,
                &[
                    ("full_text", full_text),
                    ("selected", selected),
                    ("before_ctx", before_ctx),
                    ("after_ctx", after_ctx),
                    ("instruction", "(see conversation below)"),
                ],
            );
            let grounding_content = if image_base64.is_some() {
                // Preserve image parts if present
                match &user_content {
                    MessageContent::Parts(parts) => {
                        let mut new_parts = parts.clone();
                        // Replace text part with grounding text
                        for part in &mut new_parts {
                            if part.content_type == "text" {
                                part.text = Some(grounding);
                                break;
                            }
                        }
                        MessageContent::Parts(new_parts)
                    }
                    _ => MessageContent::Text(grounding),
                }
            } else {
                MessageContent::Text(grounding)
            };
            messages.push(Message {
                role: "user".to_string(),
                content: grounding_content,
            });
        }
        for (role, content) in conversation_history {
            messages.push(Message {
                role: role.clone(),
                content: MessageContent::Text(content.clone()),
            });
        }
        if has_history {
            // The current instruction appears only here, not duplicated in
            // the grounding message above.
            let followup = format!(
                "{}. Please continue your reply based on the context.\n",
                instruction
            );
            messages.push(Message {
                role: "user".to_string(),
                content: MessageContent::Text(followup),
            });
        } else {
            messages.push(Message {
                role: "user".to_string(),
                content: user_content,
            });
        }

        tracing::info!(
            "[revise] messages_count={} history_pairs={} instruction=\"{}\"",
            messages.len(),
            conversation_history.len(),
            instruction
        );
        for (idx, msg) in messages.iter().enumerate() {
            let content_preview = match &msg.content {
                MessageContent::Text(t) => {
                    if t.len() > 200 {
                        let end = t.char_indices().nth(200).map(|(i, _)| i).unwrap_or(t.len());
                        format!("{}...({} chars)", &t[..end], t.len())
                    } else {
                        t.clone()
                    }
                }
                MessageContent::Parts(_) => "[multipart]".to_string(),
            };
            tracing::info!(
                "[revise]   msg[{}]: role={} content={}",
                idx,
                msg.role,
                content_preview
            );
        }

        let context = match (source, paper_id) {
            (Some(s), Some(id)) => Some(format!("{} {}", s, id)),
            _ => None,
        };
        let content = self
            .call_api("revise", messages, None, context.as_deref())
            .await?;
        let text = crate::db::normalize_text(&content);

        if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(tool_calls) = json_val.get("tool_calls").and_then(|t| t.as_array()) {
                for tc in tool_calls {
                    if let Some(function) = tc.get("function") {
                        if let Some(name) = function.get("name").and_then(|n| n.as_str()) {
                            if name == "edit_text" {
                                if let Some(args_str) =
                                    function.get("arguments").and_then(|a| a.as_str())
                                {
                                    if let Ok(args) =
                                        serde_json::from_str::<serde_json::Value>(args_str)
                                    {
                                        return Ok(ReviseResult {
                                            explanation: args
                                                .get("explanation")
                                                .and_then(|e| e.as_str())
                                                .unwrap_or("")
                                                .to_string(),
                                            old_text: args
                                                .get("old_text")
                                                .and_then(|e| e.as_str())
                                                .unwrap_or("")
                                                .to_string(),
                                            new_text: args
                                                .get("new_text")
                                                .and_then(|e| e.as_str())
                                                .unwrap_or("")
                                                .to_string(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
                return Ok(ReviseResult {
                    explanation: json_val
                        .get("content")
                        .and_then(|c| c.as_str())
                        .unwrap_or(&text)
                        .to_string(),
                    old_text: String::new(),
                    new_text: String::new(),
                });
            }
        }

        Ok(ReviseResult {
            explanation: text,
            old_text: String::new(),
            new_text: String::new(),
        })
    }

    pub async fn insight_analyze_with_callback<F>(
        &self,
        title: &str,
        images: &[(String, String, String)],
        text: Option<&str>,
        custom_prompt: Option<&str>,
        cancel_flag: Option<Arc<AtomicBool>>,
        on_chunk: F,
    ) -> Result<String>
    where
        F: FnMut(&str),
    {
        info!(
            "[processor] Insight analyze stream input title='{}', images={}, text={}",
            title,
            images.len(),
            text.is_some()
        );
        let system_prompt = if let Some(cp) = custom_prompt {
            cp.to_string()
        } else {
            self.insight_template.read().unwrap().system.clone()
        };

        let user_text = {
            let tpl = self.insight_template.read().unwrap();
            super::PromptTemplate::render(
                &tpl.user,
                &[("title", title), ("content", text.unwrap_or(""))],
            )
        };
        let user_content = if !images.is_empty() {
            let mut parts = vec![ContentPart {
                content_type: "text".to_string(),
                text: Some(user_text),
                image_url: None,
            }];
            for (name, label, _url) in images.iter() {
                info!(
                    "[processor] Adding image part for insight_analyze stream: name='{}', label='{}'",
                    name, label
                );
                parts.push(ContentPart {
                    content_type: "text".to_string(),
                    text: Some(format!(
                        "\n[Extracted Figure: {} | Caption(Possible disorder): {}]\n",
                        name, label
                    )),
                    image_url: None,
                });
            }
            MessageContent::Parts(parts)
        } else {
            MessageContent::Text(user_text)
        };

        let messages = vec![
            Message {
                role: "system".to_string(),
                content: MessageContent::Text(system_prompt),
            },
            Message {
                role: "user".to_string(),
                content: user_content,
            },
        ];

        let logid = format!("{:06x}", rand::random::<u32>());
        info!(
            "[processor] [{}] Acquiring insight semaphore for insight_analyze_stream",
            logid
        );
        let _permit = self.insight_semaphore.acquire().await?;
        info!("[processor] [{}] Insight semaphore acquired", logid);

        let content = self
            .call_api_inner_with_callback(
                "insight_analyze",
                &logid,
                messages,
                cancel_flag,
                None,
                on_chunk,
            )
            .await?;
        info!(
            "[processor] Insight analyze stream output length={} chars",
            content.len()
        );
        let mut text = crate::db::normalize_text(&content);
        text.push_str(&format!(
            "\n\n<div style=\"text-align:right; color:var(--text3); font-size:12px;\">（本文使用 {} 模型生成，内容仅供参考）</div>",
            self.provider_name
        ));
        Ok(crate::db::normalize_insight_heading(&text))
    }

    pub async fn review_insight(
        &self,
        title: &str,
        insight: &str,
        images: &[(String, String)],
        text: Option<&str>,
        custom_prompt: Option<&str>,
        cancel_flag: Option<Arc<AtomicBool>>,
    ) -> Result<ReviewResult> {
        let system_prompt = if let Some(cp) = custom_prompt {
            cp.to_string()
        } else {
            self.review_template.read().unwrap().system.clone()
        };
        if system_prompt.is_empty() {
            return Ok(ReviewResult {
                content: String::new(),
                qualified: true,
            });
        }
        info!(
            "[processor] Review insight for '{}', insight_len={}, images={}",
            title,
            insight.len(),
            images.len()
        );

        if images.is_empty() {
            anyhow::bail!("Review requires PDF screenshots; none available");
        }

        let prefix = {
            let tpl = self.review_template.read().unwrap();
            super::PromptTemplate::render(
                &tpl.user,
                &[
                    ("title", title),
                    ("insight", insight),
                    ("text", text.unwrap_or("")),
                ],
            )
        };
        let mut parts = vec![ContentPart {
            content_type: "text".to_string(),
            text: Some(prefix),
            image_url: None,
        }];
        for (i, (label, url)) in images.iter().enumerate() {
            parts.push(ContentPart {
                content_type: "text".to_string(),
                text: Some(format!(
                    "\n[Extracted Figure: {} | Caption(Possible disorder): {}]\n",
                    i + 1,
                    label
                )),
                image_url: None,
            });
            parts.push(ContentPart {
                content_type: "image_url".to_string(),
                text: None,
                image_url: Some(ImageUrl { url: url.clone() }),
            });
        }
        let user_content = MessageContent::Parts(parts);

        let messages = vec![
            Message {
                role: "system".to_string(),
                content: MessageContent::Text(system_prompt),
            },
            Message {
                role: "user".to_string(),
                content: user_content,
            },
        ];
        let content = self
            .call_api("review_insight", messages, cancel_flag, None)
            .await?;
        let content = strip_think_tags(&content);
        info!(
            "[processor] Review output length={} chars (after stripping think tags)",
            content.len()
        );

        let cleaned = crate::db::normalize_text(&content);
        let qualified = Self::parse_qualified(&cleaned);
        let review_text = Self::strip_qualified_line(&cleaned);
        info!("[processor] Review qualified={} for '{}'", qualified, title);
        Ok(ReviewResult {
            content: review_text,
            qualified,
        })
    }

    fn parse_qualified(text: &str) -> bool {
        for line in text.lines().rev() {
            let trimmed = line.trim();
            if trimmed.starts_with("QUALIFIED:") {
                return trimmed.contains("true");
            }
        }
        true
    }

    fn strip_qualified_line(text: &str) -> String {
        let mut lines: Vec<&str> = text.lines().collect();
        while let Some(last) = lines.last() {
            if last.trim().starts_with("QUALIFIED:") {
                lines.pop();
            } else {
                break;
            }
        }
        lines.join("\n").trim().to_string()
    }
}

pub(super) fn strip_think_tags(text: &str) -> String {
    RE.replace_all(text, "").trim().to_string()
}

fn parse_score(text: &str) -> Option<f32> {
    let score: f32 = SCORE_RE.captures(text)?.get(1)?.as_str().parse().ok()?;
    if (0.0..=8.0).contains(&score) {
        Some(score)
    } else {
        Some(8.0)
    }
}

fn parse_summary(text: &str) -> Option<String> {
    Some(
        SUMMARY_RE
            .captures(text)?
            .get(1)?
            .as_str()
            .trim()
            .to_string(),
    )
}

fn parse_paper_type(text: &str) -> Option<String> {
    Some(TYPE_RE.captures(text)?.get(1)?.as_str().trim().to_string())
}

fn parse_tags(text: &str) -> Vec<(String, f32)> {
    let tags_section = match TAGS_RE.captures(text) {
        Some(cap) => cap.get(1).map(|m| m.as_str()).unwrap_or(""),
        None => return Vec::new(),
    };
    let mut tags = Vec::new();
    for line in tags_section.lines() {
        if line.trim().is_empty() || line.starts_with("Tag ") {
            continue;
        }
        if let Some(cap) = TAG_LINE_RE.captures(line.trim()) {
            let tag = cap.get(1).unwrap().as_str().trim().to_string();
            let weight: f32 = cap.get(2).unwrap().as_str().parse().unwrap_or(0.0);
            if !tag.is_empty() && weight > 0.0 {
                tags.push((tag, weight.clamp(0.0, 10.0)));
            }
        }
    }
    tags.truncate(5);
    tags
}
