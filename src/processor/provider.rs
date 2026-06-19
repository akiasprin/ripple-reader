// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::{Context, Result};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn};

use super::tasks::strip_think_tags;
use super::Processor;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProviderType {
    OpenAi,
    Anthropic,
}

impl ProviderType {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "anthropic" => ProviderType::Anthropic,
            _ => ProviderType::OpenAi,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
enum AnthropicStreamEvent {
    #[serde(rename = "message_start")]
    MessageStart,
    #[serde(rename = "content_block_start")]
    ContentBlockStart,
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { delta: AnthropicDelta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop,
    #[serde(rename = "message_delta")]
    MessageDelta,
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "error")]
    Error { error: AnthropicErrorDetail },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum AnthropicDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
struct AnthropicErrorDetail {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub(super) enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ContentPart {
    #[serde(rename = "type")]
    pub(super) content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) image_url: Option<ImageUrl>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ImageUrl {
    pub(super) url: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Message {
    pub(super) role: String,
    pub(super) content: MessageContent,
}

/// Merge a new Anthropic message into the previous one when two consecutive
/// messages share the same role. Anthropic's Messages API requires user and
/// assistant roles to alternate; collapsing repeated roles keeps the request
/// valid without losing content.
fn merge_anthropic_content(existing: &mut serde_json::Value, incoming: &serde_json::Value) {
    let existing_content = existing.get_mut("content").unwrap();
    let incoming_content = incoming.get("content").unwrap();
    match (existing_content, incoming_content) {
        (serde_json::Value::String(a), serde_json::Value::String(b)) => {
            a.push_str("\n\n");
            a.push_str(b);
        }
        (serde_json::Value::Array(a), serde_json::Value::Array(b)) => {
            a.extend(b.clone());
        }
        (serde_json::Value::String(a), serde_json::Value::Array(_)) => {
            let mut arr = vec![json!({"type": "text", "text": a.clone()})];
            if let serde_json::Value::Array(b) = incoming_content {
                arr.extend(b.clone());
            }
            *existing.get_mut("content").unwrap() = json!(arr);
        }
        (serde_json::Value::Array(_), serde_json::Value::String(b)) => {
            existing
                .get_mut("content")
                .unwrap()
                .as_array_mut()
                .unwrap()
                .push(json!({"type": "text", "text": b.as_str()}));
        }
        _ => {}
    }
}

impl Processor {
    pub(super) async fn call_api(
        &self,
        label: &str,
        messages: Vec<Message>,
        cancel_flag: Option<Arc<AtomicBool>>,
        context: Option<&str>,
    ) -> Result<String> {
        let logid = format!("{:06x}", rand::random::<u32>());
        if let Some(ctx) = context {
            info!(
                "[processor] [{}] [{}] Acquiring semaphore for {}",
                logid, ctx, label
            );
        } else {
            info!("[processor] [{}] Acquiring semaphore for {}", logid, label);
        }
        let _permit = self.semaphore.acquire().await?;
        if let Some(ctx) = context {
            info!("[processor] [{}] [{}] Semaphore acquired", logid, ctx);
        } else {
            info!("[processor] [{}] Semaphore acquired", logid);
        }
        self.call_api_inner(label, &logid, messages, cancel_flag, context)
            .await
    }

    pub(super) async fn call_api_inner(
        &self,
        label: &str,
        logid: &str,
        messages: Vec<Message>,
        cancel_flag: Option<Arc<AtomicBool>>,
        context: Option<&str>,
    ) -> Result<String> {
        self.call_api_inner_with_callback(
            label,
            logid,
            messages,
            cancel_flag,
            context,
            |_s: &str| {},
        )
        .await
    }

    pub(super) async fn call_api_inner_with_callback<F>(
        &self,
        label: &str,
        logid: &str,
        messages: Vec<Message>,
        cancel_flag: Option<Arc<AtomicBool>>,
        context: Option<&str>,
        on_chunk: F,
    ) -> Result<String>
    where
        F: FnMut(&str),
    {
        match self.provider_type {
            ProviderType::OpenAi => {
                self.call_openai_with_callback(
                    label,
                    logid,
                    messages,
                    cancel_flag,
                    context,
                    on_chunk,
                )
                .await
            }
            ProviderType::Anthropic => {
                self.call_anthropic_with_callback(
                    label,
                    logid,
                    messages,
                    cancel_flag,
                    context,
                    on_chunk,
                )
                .await
            }
        }
    }

    async fn call_openai_with_callback<F>(
        &self,
        label: &str,
        logid: &str,
        messages: Vec<Message>,
        cancel_flag: Option<Arc<AtomicBool>>,
        context: Option<&str>,
        mut on_chunk: F,
    ) -> Result<String>
    where
        F: FnMut(&str),
    {
        let openai_messages: Vec<serde_json::Value> = messages
            .into_iter()
            .map(|msg| {
                let role = msg.role;
                match msg.content {
                    MessageContent::Text(text) => json!({"role": role, "content": text}),
                    MessageContent::Parts(parts) => {
                        let content: Vec<serde_json::Value> = parts
                            .into_iter()
                            .map(|part| match part.content_type.as_str() {
                                "text" => {
                                    json!({"type": "text", "text": part.text.unwrap_or_default()})
                                }
                                "image_url" => json!({"type": "image_url", "image_url": {"url": part.image_url.map(|u| u.url).unwrap_or_default()}}),
                                _ => json!({"type": "text", "text": ""}),
                            })
                            .collect();
                        json!({"role": role, "content": content})
                    }
                }
            })
            .collect();

        for (i, msg) in openai_messages.iter().enumerate() {
            let role = msg
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("unknown");
            if let Some(content) = msg.get("content") {
                if let Some(text) = content.as_str() {
                    info!(
                        "[processor] [{}] message[{}] role={} text_len={} chars",
                        label,
                        i,
                        role,
                        text.len()
                    );
                } else if let Some(arr) = content.as_array() {
                    for (j, part) in arr.iter().enumerate() {
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            info!(
                                "[processor] [{}] message[{}] part[{}] role={} type=text text_len={} chars",
                                label, i, j, role, text.len()
                            );
                        } else if part.get("image_url").is_some() {
                            info!(
                                "[processor] [{}] message[{}] part[{}] role={} type=image_url image_url=<present>",
                                label, i, j, role
                            );
                        }
                    }
                }
            }
        }

        let has_tools = !self.tools.is_empty() && label == "revise";
        let mut body = json!({
            "model": self.model,
            "messages": openai_messages,
            "max_completion_tokens": self.max_tokens,
        });
        let body_obj = body.as_object_mut().unwrap();

        if has_tools {
            body_obj.insert("tools".to_string(), json!(self.tools));
            body_obj.insert("tool_choice".to_string(), json!("auto"));
        } else {
            body_obj.insert("stream".to_string(), json!(true));
        }
        if let Some(t) = self.temperature {
            body_obj.insert("temperature".to_string(), json!(t));
        }
        if let Some(p) = self.top_p {
            body_obj.insert("top_p".to_string(), json!(p));
        }
        if let Some(ref effort) = self.reasoning_effort {
            body_obj.insert("reasoning_effort".to_string(), json!(effort));
        }

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let mut last_err = None;

        for attempt in 0..=self.max_retries {
            if cancel_flag
                .as_ref()
                .map(|b| b.load(Ordering::Relaxed))
                .unwrap_or(false)
            {
                warn!(
                    "[processor] [{}] OpenAI request cancelled before attempt {}",
                    logid,
                    attempt + 1
                );
                return Err(anyhow::anyhow!("Cancelled"));
            }
            let (key_idx, key) = self.pick_key();
            let masked = mask_key(key);
            info!(
                "[processor] [{}] POST {} (attempt {}/{}, key_idx={}, masked={})",
                logid,
                url,
                attempt + 1,
                self.max_retries + 1,
                key_idx,
                masked,
            );
            let start = std::time::Instant::now();

            let res = self
                .http_client
                .post(&url)
                .header("Authorization", format!("Bearer {}", key))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await;

            match res {
                Ok(response) => {
                    if !response.status().is_success() {
                        let status = response.status();
                        let body_text = response.text().await.unwrap_or_default();
                        warn!(
                            "[processor] [{}] HTTP {} (key_idx={}, masked={}): {}",
                            logid, status, key_idx, masked, body_text
                        );
                        last_err = Some(format!(
                            "HTTP {} (key_idx={}, masked={}): {}",
                            status, key_idx, masked, body_text
                        ));
                    } else if has_tools {
                        let json_resp: serde_json::Value = response
                            .json()
                            .await
                            .map_err(|e| anyhow::anyhow!("JSON parse error: {}", e))?;
                        if let Some(choice) = json_resp
                            .get("choices")
                            .and_then(|c| c.as_array())
                            .and_then(|arr| arr.first())
                        {
                            if let Some(tool_calls) = choice
                                .get("message")
                                .and_then(|m| m.get("tool_calls"))
                                .and_then(|t| t.as_array())
                            {
                                let tool_result = serde_json::to_string(&json!({
                                    "tool_calls": tool_calls,
                                    "content": choice.get("message").and_then(|m| m.get("content")).and_then(|c| c.as_str()).unwrap_or("")
                                }))?;
                                if let Some(ctx) = context {
                                    info!(
                                        "[processor] [{}] [{}] Tool response completed, length={} chars",
                                        logid, ctx, tool_result.len()
                                    );
                                } else {
                                    info!(
                                        "[processor] [{}] Tool response completed, length={} chars",
                                        logid,
                                        tool_result.len()
                                    );
                                }
                                return Ok(tool_result);
                            }
                            if let Some(content) = choice
                                .get("message")
                                .and_then(|m| m.get("content"))
                                .and_then(|c| c.as_str())
                            {
                                let text = strip_think_tags(content);
                                if text.trim().is_empty() {
                                    last_err = Some("LLM returned empty content".to_string());
                                } else {
                                    if let Some(ctx) = context {
                                        info!(
                                            "[processor] [{}] [{}] Response completed, length={} chars",
                                            logid, ctx, text.len()
                                        );
                                    } else {
                                        info!(
                                            "[processor] [{}] Response completed, length={} chars",
                                            logid,
                                            text.len()
                                        );
                                    }
                                    return Ok(text);
                                }
                            } else {
                                last_err = Some("No content in response".to_string());
                            }
                        } else {
                            last_err = Some("No choices in response".to_string());
                        }
                    } else {
                        let bytes_stream = response.bytes_stream();
                        let mut event_stream = bytes_stream.eventsource();
                        let mut content = String::new();
                        let mut last_log_len = 0;
                        let mut stream_err = None;
                        while let Some(event_result) = event_stream.next().await {
                            if cancel_flag
                                .as_ref()
                                .map(|b| b.load(Ordering::Relaxed))
                                .unwrap_or(false)
                            {
                                warn!("[processor] [{}] OpenAI stream cancelled", logid);
                                return Err(anyhow::anyhow!("Cancelled"));
                            }
                            match event_result {
                                Ok(event) => {
                                    let data = event.data.trim();
                                    if data.is_empty() || data == "[DONE]" {
                                        continue;
                                    }
                                    if let Ok(value) =
                                        serde_json::from_str::<serde_json::Value>(data)
                                    {
                                        if let Some(text) = value
                                            .get("choices")
                                            .and_then(|c| c.as_array())
                                            .and_then(|arr| arr.first())
                                            .and_then(|c| c.get("delta"))
                                            .and_then(|d| d.get("content"))
                                            .and_then(|c| c.as_str())
                                        {
                                            content.push_str(text);
                                            on_chunk(text);
                                            if content.len() - last_log_len >= 500 {
                                                if let Some(ctx) = context {
                                                    info!(
                                                        "[processor] [{}] [{}] Streaming... {} chars received",
                                                        logid, ctx, content.len()
                                                    );
                                                } else {
                                                    info!(
                                                        "[processor] [{}] Streaming... {} chars received",
                                                        logid, content.len()
                                                    );
                                                }
                                                last_log_len = content.len();
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    let err_str = format!("{}", e);
                                    warn!(
                                        "[processor] [{}] Stream error on attempt {} (key_idx={}, masked={}): {} (debug: {:?}, received {} chars)",
                                        logid,
                                        attempt + 1,
                                        key_idx,
                                        masked,
                                        err_str,
                                        e,
                                        content.len()
                                    );
                                    stream_err = Some(err_str);
                                    break;
                                }
                            }
                        }
                        if let Some(err) = stream_err {
                            last_err = Some(err);
                        } else {
                            let elapsed = start.elapsed();
                            let content = strip_think_tags(&content);
                            if content.trim().is_empty() {
                                warn!(
                                    "[processor] [{}] API returned empty content on attempt {}",
                                    logid,
                                    attempt + 1
                                );
                                last_err = Some("LLM returned empty content".to_string());
                            } else {
                                if let Some(ctx) = context {
                                    info!(
                                        "[processor] [{}] [{}] Response completed, length={} chars, elapsed: {:?}",
                                        logid, ctx, content.len(), elapsed
                                    );
                                } else {
                                    info!(
                                        "[processor] [{}] Response completed, length={} chars, elapsed: {:?}",
                                        logid, content.len(), elapsed
                                    );
                                }
                                return Ok(content);
                            }
                        }
                    }
                }
                Err(e) => {
                    let err_str = format!("{}", e);
                    warn!(
                        "[processor] [{}] Request failed on attempt {} (key_idx={}, masked={}): {}",
                        logid,
                        attempt + 1,
                        key_idx,
                        masked,
                        err_str
                    );
                    if err_str == "Cancelled" {
                        return Err(anyhow::anyhow!("Cancelled"));
                    }
                    last_err = Some(err_str);
                }
            }

            if attempt < self.max_retries {
                let delay_ms = std::cmp::min(500u64 * (1u64 << attempt.min(6)), 30_000u64);
                warn!("[processor] Retrying in {} ms...", delay_ms);
                sleep(Duration::from_millis(delay_ms)).await;
            }
        }

        anyhow::bail!(
            "LLM request failed after {} attempts: {}",
            self.max_retries + 1,
            last_err.unwrap_or_default()
        )
    }

    /// Single-shot Anthropic HTTP request with SSE parsing.
    /// Does NOT retry; caller is responsible for retry logic.
    #[allow(clippy::too_many_arguments)]
    async fn anthropic_post_once<F>(
        &self,
        logid: &str,
        url: &str,
        headers: &[(String, String)],
        body: &serde_json::Value,
        cancel_flag: Option<Arc<AtomicBool>>,
        context: Option<&str>,
        mut on_chunk: F,
    ) -> Result<String>
    where
        F: FnMut(&str),
    {
        let start = std::time::Instant::now();
        let mut req = self.http_client.post(url);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        let response = req
            .json(body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("HTTP {}: {}", status, body_text));
        }

        let mut content = String::new();
        let mut last_log_len = 0;
        let bytes_stream = response.bytes_stream();
        let mut event_stream = bytes_stream.eventsource();

        while let Some(event_result) = event_stream.next().await {
            if cancel_flag
                .as_ref()
                .map(|b| b.load(Ordering::Relaxed))
                .unwrap_or(false)
            {
                warn!("[processor] [{}] Anthropic stream cancelled", logid);
                return Err(anyhow::anyhow!("Cancelled"));
            }
            match event_result {
                Ok(event) => {
                    let data = event.data.trim();
                    if data.is_empty() || data == "[DONE]" {
                        continue;
                    }
                    match serde_json::from_str::<AnthropicStreamEvent>(data) {
                        Ok(AnthropicStreamEvent::ContentBlockDelta { delta }) => {
                            if let AnthropicDelta::TextDelta { text } = delta {
                                content.push_str(&text);
                                on_chunk(&text);
                                if content.len() - last_log_len >= 500 {
                                    if let Some(ctx) = context {
                                        info!(
                                            "[processor] [{}] [{}] Streaming... {} chars received",
                                            logid,
                                            ctx,
                                            content.len()
                                        );
                                    } else {
                                        info!(
                                            "[processor] [{}] Streaming... {} chars received",
                                            logid,
                                            content.len()
                                        );
                                    }
                                    last_log_len = content.len();
                                }
                            }
                        }
                        Ok(AnthropicStreamEvent::MessageStop) => break,
                        Ok(AnthropicStreamEvent::Error { error }) => {
                            return Err(anyhow::anyhow!(
                                "API error: {} - {}",
                                error.error_type,
                                error.message
                            ));
                        }
                        Ok(AnthropicStreamEvent::Unknown) => {
                            let ty = serde_json::from_str::<serde_json::Value>(data)
                                .ok()
                                .and_then(|v| {
                                    v.get("type").and_then(|t| t.as_str()).map(String::from)
                                });
                            return Err(anyhow::anyhow!(
                                "Unknown SSE event type: {:?}. Data: {}",
                                ty,
                                data
                            ));
                        }
                        Ok(_) => {}
                        Err(e) => {
                            if e.to_string().contains("missing field `signature`") {
                                warn!(
                                    "[processor] [{}] Failed to parse SSE event: {}. Data: {}",
                                    logid, e, data
                                );
                            } else {
                                return Err(anyhow::anyhow!(
                                    "Failed to parse SSE event: {}. Data: {}",
                                    e,
                                    data
                                ));
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "[processor] [{}] Stream error: {} (debug: {:?}, received {} chars)",
                        logid,
                        e,
                        e,
                        content.len()
                    );
                    return Err(anyhow::anyhow!("Stream error: {}", e));
                }
            }
        }

        let content = strip_think_tags(&content);
        if content.trim().is_empty() {
            return Err(anyhow::anyhow!("LLM returned empty content"));
        }
        if let Some(ctx) = context {
            info!(
                "[processor] [{}] [{}] Response completed, length={} chars, elapsed: {:?}",
                logid,
                ctx,
                content.len(),
                start.elapsed()
            );
        } else {
            info!(
                "[processor] [{}] Response completed, length={} chars, elapsed: {:?}",
                logid,
                content.len(),
                start.elapsed()
            );
        }
        Ok(content)
    }

    async fn call_anthropic_with_callback<F>(
        &self,
        label: &str,
        logid: &str,
        messages: Vec<Message>,
        cancel_flag: Option<Arc<AtomicBool>>,
        context: Option<&str>,
        mut on_chunk: F,
    ) -> Result<String>
    where
        F: FnMut(&str),
    {
        let mut system_prompt = None;
        let mut anthropic_messages: Vec<serde_json::Value> = Vec::new();

        for msg in messages {
            let role = msg.role.as_str();
            match role {
                "system" => {
                    if let MessageContent::Text(text) = msg.content {
                        system_prompt = Some(text);
                    }
                }
                "user" | "assistant" => {
                    let anthropic_msg = match msg.content {
                        MessageContent::Text(text) => {
                            json!({"role": role, "content": text})
                        }
                        MessageContent::Parts(parts) => {
                            let content: Vec<serde_json::Value> = parts
                                .into_iter()
                                .map(|part| match part.content_type.as_str() {
                                    "text" => {
                                        json!({"type": "text", "text": part.text.unwrap_or_default()})
                                    }
                                    "image_url" => {
                                        let url =
                                            part.image_url.map(|u| u.url).unwrap_or_default();
                                        if let Ok((media_type, data)) = parse_data_url(&url) {
                                            json!({"type": "image", "source": {"type": "base64", "media_type": media_type, "data": data}})
                                        } else {
                                            json!({"type": "text", "text": format!("[unparseable image: {}]", url)})
                                        }
                                    }
                                    _ => json!({"type": "text", "text": ""}),
                                })
                                .collect();
                            json!({"role": role, "content": content})
                        }
                    };
                    if let Some(last) = anthropic_messages.last_mut() {
                        if last.get("role").and_then(|r| r.as_str()) == Some(role) {
                            merge_anthropic_content(last, &anthropic_msg);
                            continue;
                        }
                    }
                    anthropic_messages.push(anthropic_msg);
                }
                _ => {}
            }
        }

        for (i, msg) in anthropic_messages.iter().enumerate() {
            let role = msg
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("unknown");
            if let Some(content) = msg.get("content") {
                if let Some(text) = content.as_str() {
                    info!(
                        "[processor] [{}] anthropic message[{}] role={} text_len={} chars",
                        label,
                        i,
                        role,
                        text.len()
                    );
                } else if let Some(arr) = content.as_array() {
                    for (j, block) in arr.iter().enumerate() {
                        if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                            info!(
                                "[processor] [{}] anthropic message[{}] part[{}] role={} type=text text_len={} chars",
                                label, i, j, role, text.len()
                            );
                        } else if block.get("image").is_some() || block.get("source").is_some() {
                            info!(
                                "[processor] [{}] anthropic message[{}] part[{}] role={} type=image image=<present>",
                                label, i, j, role
                            );
                        }
                    }
                }
            }
        }

        let mut body = json!({
            "model": self.model,
            "messages": anthropic_messages,
            "max_tokens": self.max_tokens,
            "stream": true,
        });
        if let Some(t) = self.temperature {
            body.as_object_mut()
                .unwrap()
                .insert("temperature".to_string(), json!(t));
        }
        if let Some(p) = self.top_p {
            body.as_object_mut()
                .unwrap()
                .insert("top_p".to_string(), json!(p));
        }
        if let Some(sp) = system_prompt {
            body.as_object_mut()
                .unwrap()
                .insert("system".to_string(), json!(sp));
        }

        if let Some(ref oauth_token) = self.anthropic_oauth_token {
            return self
                .call_anthropic_oauth_with_callback(
                    label,
                    logid,
                    &body,
                    cancel_flag.clone(),
                    on_chunk,
                    oauth_token,
                )
                .await;
        }

        let url = format!("{}/messages", self.base_url.trim_end_matches('/'));
        let use_tools = label == "revise" && !self.tools.is_empty();

        if use_tools {
            let anthropic_tools: Vec<serde_json::Value> = self
                .tools
                .iter()
                .filter_map(|tool| {
                    let func = tool.get("function")?;
                    Some(json!({
                        "name": func.get("name")?.as_str()?,
                        "description": func.get("description")?.as_str()?,
                        "input_schema": func.get("parameters").cloned().unwrap_or(json!({}))
                    }))
                })
                .collect();
            body.as_object_mut()
                .unwrap()
                .insert("tools".to_string(), json!(anthropic_tools));
            body.as_object_mut().unwrap().remove("stream");
        }

        let mut last_err = None;
        let mut use_adaptive = false;
        let mut attempt = 0;

        while attempt <= self.max_retries {
            if cancel_flag
                .as_ref()
                .map(|b| b.load(Ordering::Relaxed))
                .unwrap_or(false)
            {
                warn!(
                    "[processor] [{}] Anthropic request cancelled before attempt {}",
                    logid,
                    attempt + 1
                );
                return Err(anyhow::anyhow!("Cancelled"));
            }
            let mut current_body = body.clone();
            if use_adaptive {
                if let Some(thinking) = current_body.get_mut("thinking") {
                    if let Some(obj) = thinking.as_object_mut() {
                        obj.insert("type".to_string(), serde_json::json!("adaptive"));
                        obj.remove("budget_tokens");
                    }
                }
                let effort = self
                    .output_config_effort
                    .as_ref()
                    .or(self.reasoning_effort.as_ref());
                if let Some(effort) = effort {
                    current_body.as_object_mut().unwrap().insert(
                        "output_config".to_string(),
                        serde_json::json!({"effort": effort}),
                    );
                }
            }

            let (key_idx, key) = self.pick_key();
            let masked = mask_key(key);
            let key = key.to_string();
            let mut headers = vec![
                ("x-api-key".to_string(), key),
                ("anthropic-version".to_string(), "2023-06-01".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
            ];
            if let Some(ref ua) = self.user_agent {
                headers.push(("User-Agent".to_string(), ua.clone()));
            }

            info!(
                "[processor] [{}] POST {} (attempt {}/{}, key_idx={}, masked={})",
                logid,
                url,
                attempt + 1,
                self.max_retries + 1,
                key_idx,
                masked,
            );

            if use_tools {
                let mut req = self.http_client.post(&url);
                for (k, v) in &headers {
                    req = req.header(k, v);
                }
                match req.json(&current_body).send().await {
                    Ok(response) => {
                        if !response.status().is_success() {
                            let status = response.status();
                            let body_text = response.text().await.unwrap_or_default();
                            warn!(
                                "[processor] [{}] HTTP {} (key_idx={}, masked={}): {}",
                                logid, status, key_idx, masked, body_text
                            );
                            last_err = Some(format!(
                                "HTTP {} (key_idx={}, masked={}): {}",
                                status, key_idx, masked, body_text
                            ));
                        } else {
                            let json_resp: serde_json::Value = response
                                .json()
                                .await
                                .map_err(|e| anyhow::anyhow!("JSON parse error: {}", e))?;
                            if let Some(content_arr) =
                                json_resp.get("content").and_then(|c| c.as_array())
                            {
                                let mut text_parts = Vec::new();
                                let mut tool_calls = Vec::new();
                                for block in content_arr {
                                    if let Some(block_type) =
                                        block.get("type").and_then(|t| t.as_str())
                                    {
                                        match block_type {
                                            "text" => {
                                                if let Some(text) =
                                                    block.get("text").and_then(|t| t.as_str())
                                                {
                                                    text_parts.push(text.to_string());
                                                }
                                            }
                                            "tool_use" => {
                                                if let Some(name) =
                                                    block.get("name").and_then(|n| n.as_str())
                                                {
                                                    if let Some(input) = block.get("input") {
                                                        tool_calls.push(json!({
                                                            "function": {
                                                                "name": name,
                                                                "arguments": serde_json::to_string(input).unwrap_or_default()
                                                            }
                                                        }));
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                if !tool_calls.is_empty() {
                                    let tool_result = serde_json::to_string(&json!({
                                        "tool_calls": tool_calls,
                                        "content": text_parts.join("")
                                    }))?;
                                    if let Some(ctx) = context {
                                        info!(
                                            "[processor] [{}] [{}] Tool response completed, length={} chars",
                                            logid, ctx, tool_result.len()
                                        );
                                    } else {
                                        info!(
                                            "[processor] [{}] Tool response completed, length={} chars",
                                            logid, tool_result.len()
                                        );
                                    }
                                    return Ok(tool_result);
                                }
                                let text = text_parts.join("");
                                let text = strip_think_tags(&text);
                                if text.trim().is_empty() {
                                    last_err = Some("LLM returned empty content".to_string());
                                } else {
                                    if let Some(ctx) = context {
                                        info!(
                                            "[processor] [{}] [{}] Response completed, length={} chars",
                                            logid, ctx, text.len()
                                        );
                                    } else {
                                        info!(
                                            "[processor] [{}] Response completed, length={} chars",
                                            logid,
                                            text.len()
                                        );
                                    }
                                    return Ok(text);
                                }
                            } else {
                                last_err = Some("No content in response".to_string());
                            }
                        }
                    }
                    Err(e) => {
                        let err_str = format!("{}", e);
                        if err_str == "Cancelled" {
                            return Err(anyhow::anyhow!("Cancelled"));
                        }
                        warn!(
                            "[processor] [{}] Request failed on attempt {} (key_idx={}, masked={}): {}",
                            logid,
                            attempt + 1,
                            key_idx,
                            masked,
                            err_str
                        );
                        last_err = Some(err_str);
                    }
                }
            } else {
                match self
                    .anthropic_post_once(
                        logid,
                        &url,
                        &headers,
                        &current_body,
                        cancel_flag.clone(),
                        context,
                        &mut on_chunk,
                    )
                    .await
                {
                    Ok(content) => return Ok(content),
                    Err(e) => {
                        let err_str = e.to_string();
                        if !use_adaptive && err_str.contains("thinking.type.enabled") {
                            warn!(
                                "[processor] [{}] Model doesn't support thinking.type.enabled, retrying with adaptive",
                                logid
                            );
                            use_adaptive = true;
                            continue;
                        }
                        if err_str == "Cancelled" {
                            return Err(anyhow::anyhow!("Cancelled"));
                        }
                        warn!(
                            "[processor] [{}] Request failed on attempt {} (key_idx={}, masked={}): {}",
                            logid,
                            attempt + 1,
                            key_idx,
                            masked,
                            err_str
                        );
                        last_err = Some(err_str);
                    }
                }
            }
            attempt += 1;
            if attempt <= self.max_retries {
                let delay_ms = std::cmp::min(500u64 * (1u64 << (attempt - 1).min(6)), 30_000u64);
                warn!("[processor] Retrying in {} ms...", delay_ms);
                sleep(Duration::from_millis(delay_ms)).await;
            }
        }

        anyhow::bail!(
            "LLM request failed after {} attempts: {}",
            self.max_retries + 1,
            last_err.unwrap_or_default()
        )
    }

    async fn call_anthropic_oauth_with_callback<F>(
        &self,
        _label: &str,
        logid: &str,
        body: &serde_json::Value,
        cancel_flag: Option<Arc<AtomicBool>>,
        mut on_chunk: F,
        oauth_token: &str,
    ) -> Result<String>
    where
        F: FnMut(&str),
    {
        let url = format!("{}/messages", self.base_url.trim_end_matches('/'));
        let mut last_err = None;
        let mut use_adaptive = true;
        let mut attempt = 0;

        while attempt <= self.max_retries {
            if cancel_flag
                .as_ref()
                .map(|b| b.load(Ordering::Relaxed))
                .unwrap_or(false)
            {
                warn!(
                    "[processor] [{}] Anthropic OAuth request cancelled before attempt {}",
                    logid,
                    attempt + 1
                );
                return Err(anyhow::anyhow!("Cancelled"));
            }
            let mut current_body = body.clone();
            if use_adaptive {
                if let Some(thinking) = current_body.get_mut("thinking") {
                    if let Some(obj) = thinking.as_object_mut() {
                        obj.insert("type".to_string(), serde_json::json!("adaptive"));
                        obj.remove("budget_tokens");
                    }
                }
                let effort = self
                    .output_config_effort
                    .as_ref()
                    .or(self.reasoning_effort.as_ref());
                if let Some(effort) = effort {
                    current_body.as_object_mut().unwrap().insert(
                        "output_config".to_string(),
                        serde_json::json!({"effort": effort}),
                    );
                }
            }

            let mut headers = vec![
                (
                    "Authorization".to_string(),
                    format!("Bearer {}", oauth_token),
                ),
                ("anthropic-version".to_string(), "2023-06-01".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
            ];
            if let Some(ref ua) = self.user_agent {
                headers.push(("User-Agent".to_string(), ua.clone()));
            }

            match self
                .anthropic_post_once(
                    logid,
                    &url,
                    &headers,
                    &current_body,
                    cancel_flag.clone(),
                    None,
                    &mut on_chunk,
                )
                .await
            {
                Ok(content) => return Ok(content),
                Err(e) => {
                    let err_str = e.to_string();
                    if !use_adaptive && err_str.contains("thinking.type.enabled") {
                        warn!(
                            "[processor] [{}] Model doesn't support thinking.type.enabled, retrying with adaptive",
                            logid
                        );
                        use_adaptive = true;
                        continue;
                    }
                    if err_str == "Cancelled" {
                        return Err(anyhow::anyhow!("Cancelled"));
                    }
                    warn!(
                        "[processor] [{}] Request failed on attempt {}: {}",
                        logid,
                        attempt + 1,
                        err_str
                    );
                    last_err = Some(err_str);
                    attempt += 1;
                    if attempt <= self.max_retries {
                        let delay_ms =
                            std::cmp::min(500u64 * (1u64 << (attempt - 1).min(6)), 30_000u64);
                        warn!("[processor] Retrying in {} ms...", delay_ms);
                        sleep(Duration::from_millis(delay_ms)).await;
                    }
                }
            }
        }

        anyhow::bail!(
            "LLM request failed after {} attempts: {}",
            self.max_retries + 1,
            last_err.unwrap_or_default()
        )
    }
}

/// Parse a data URL like `data:image/png;base64,XXXX` and return (media_type, base64_data).
fn parse_data_url(url: &str) -> Result<(String, String)> {
    let prefix = "data:";
    anyhow::ensure!(url.starts_with(prefix), "Not a data URL: {}", url);
    let rest = &url[prefix.len()..];
    let (meta, data) = rest
        .split_once(",")
        .context("Invalid data URL format: missing comma")?;
    let media_type = meta.split(";").next().unwrap_or("image/png").to_string();
    Ok((media_type, data.to_string()))
}

pub(super) fn mask_key(key: &str) -> String {
    if key.len() > 8 {
        format!("{}...{}", &key[..4], &key[key.len() - 4..])
    } else {
        "***".to_string()
    }
}
