// SPDX-License-Identifier: MIT OR Apache-2.0

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Json,
};
use std::sync::Arc;
use tracing::error;

use super::auth::check_auth;
use super::insight_html::refresh_insight_html_cache;
use super::types::{AiCommentRequest, AppState, CommentRequest, CommentResponse, ErrorResponse};
use crate::db::DbPaperUpdate;
use crate::processor::Processor;

/// Find `needle` inside `haystack` (the paper insight). Tries exact match first,
/// then normalises whitespace, then fuzzy word-by-word match.
/// Returns `(matched_substring, vec_of_start_indices)` on success.
fn find_text_in_insight(haystack: &str, needle: &str) -> Option<(String, Vec<usize>)> {
    // 1. Exact match
    let exact: Vec<usize> = haystack.match_indices(needle).map(|(i, _)| i).collect();
    if !exact.is_empty() {
        return Some((needle.to_string(), exact));
    }

    // Helper: collapse any run of whitespace to a single space.
    fn normalize_ws(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    // 2. Normalised-whitespace match
    let norm_hay = normalize_ws(haystack);
    let norm_needle = normalize_ws(needle);
    if !norm_needle.is_empty() {
        let norm_matches: Vec<usize> = norm_hay
            .match_indices(&norm_needle)
            .map(|(i, _)| i)
            .collect();
        if !norm_matches.is_empty() {
            let mut starts = Vec::new();
            for &norm_start in &norm_matches {
                let mut orig_pos = 0;
                let mut norm_pos = 0;
                // Skip leading whitespace in original
                for ch in haystack.chars() {
                    if !ch.is_whitespace() {
                        break;
                    }
                    orig_pos += ch.len_utf8();
                }
                while norm_pos < norm_start {
                    // Advance one word in normalized
                    let word_len = norm_hay[norm_pos..]
                        .find(' ')
                        .unwrap_or(norm_hay.len() - norm_pos);
                    norm_pos += word_len;
                    // Advance past same word in original
                    let word_end = haystack[orig_pos..]
                        .find(|c: char| c.is_whitespace())
                        .map(|i| orig_pos + i)
                        .unwrap_or(haystack.len());
                    orig_pos = word_end;
                    // Skip whitespace in original
                    for ch in haystack[orig_pos..].chars() {
                        if !ch.is_whitespace() {
                            break;
                        }
                        orig_pos += ch.len_utf8();
                    }
                    // Skip single space in normalized
                    if norm_pos < norm_hay.len() && norm_hay[norm_pos..].starts_with(' ') {
                        norm_pos += 1;
                    }
                }
                starts.push(orig_pos);
            }
            if let Some(&first) = starts.first() {
                // Walk from `first`, consuming the same number of words as needle
                let needle_word_count = needle.split_whitespace().count();
                let mut end = first;
                let mut words_seen = 0;
                let mut in_word = false;
                for ch in haystack[first..].chars() {
                    if ch.is_whitespace() {
                        if in_word {
                            in_word = false;
                            words_seen += 1;
                        }
                    } else {
                        in_word = true;
                    }
                    end += ch.len_utf8();
                    if words_seen >= needle_word_count {
                        break;
                    }
                }
                let matched = haystack[first..end].trim_end().to_string();
                return Some((matched, starts));
            }
        }
    }

    // 3. Fuzzy match: word-by-word
    let hay_words: Vec<&str> = haystack.split_whitespace().collect();
    let needle_words: Vec<&str> = needle.split_whitespace().collect();
    if needle_words.is_empty() {
        return None;
    }
    let mut positions = Vec::new();
    for i in 0..hay_words.len() {
        if i + needle_words.len() > hay_words.len() {
            break;
        }
        if hay_words[i..i + needle_words.len()] == needle_words[..] {
            // Map word index back to character index in original
            let mut word_count = 0;
            for (idx, ch) in haystack.char_indices() {
                if word_count == i && !ch.is_whitespace() {
                    positions.push(idx);
                    break;
                }
                if ch.is_whitespace() {
                    let prev = haystack[..idx].chars().next_back();
                    if prev.is_some_and(|c| !c.is_whitespace()) {
                        word_count += 1;
                    }
                }
            }
            break; // Only first match
        }
    }
    if positions.is_empty() {
        return None;
    }
    // Extract substring from original
    let first = positions[0];
    let mut end = first;
    let mut words_seen = 0;
    let mut in_word = false;
    for ch in haystack[first..].chars() {
        if ch.is_whitespace() {
            if in_word {
                in_word = false;
                words_seen += 1;
            }
        } else {
            in_word = true;
        }
        end += ch.len_utf8();
        if words_seen >= needle_words.len() {
            break;
        }
    }
    let matched = haystack[first..end].trim_end().to_string();
    Some((matched, positions))
}

/// Background task: call LLM to revise insight based on comment instruction.
/// When `is_heading` is true, the original paper text (from mineru.json) is
/// sent to the LLM instead of the interpreted insight content.
#[allow(clippy::too_many_arguments)]
/// Strip the "@AGENT " prefix from a comment for use in conversation history.
fn clean_history_message(comment: &str) -> String {
    comment
        .strip_prefix("@AGENT")
        .unwrap_or(comment)
        .trim()
        .to_string()
}

#[allow(clippy::too_many_arguments)]
fn spawn_ai_reply_task(
    db: crate::db::Db,
    processor: Option<Arc<Processor>>,
    comment_id: i64,
    parent_id: Option<i64>,
    quote: String,
    before_ctx: String,
    after_ctx: String,
    instruction: String,
    insight: String,
    source: String,
    paper_id: String,
    is_heading: bool,
) {
    tokio::spawn(async move {
        let Some(processor) = processor else {
            if let Err(e) = db
                .update_ai_comment_reply(comment_id, "AI service not configured", "", "", "failed")
                .await
            {
                tracing::error!(
                    "[ai_comment] Failed to update comment {} with 'not configured': {}",
                    comment_id,
                    e
                );
            }
            return;
        };

        // Build structured multi-turn conversation history by walking the
        // parent_id chain. Returns ordered (role, content) tuples where role
        // is "user" or "assistant". Handles any conversation topology including
        // AI-initiated root comments (e.g. from heading prompts).
        let conversation_history: Vec<(String, String)> = if let Some(pid) = parent_id {
            match db.get_comment_thread(pid).await {
                Ok(thread) => {
                    tracing::info!(
                        "[ai_comment] comment={} parent_id={} thread_len={}",
                        comment_id,
                        pid,
                        thread.len()
                    );
                    for (idx, c) in thread.iter().enumerate() {
                        tracing::info!(
                            "[ai_comment]   thread[{}]: id={} parent_id={:?} is_ai={} comment=\"{}\" ai_reply=\"{}\"",
                            idx,
                            c.id,
                            c.parent_id,
                            c.is_ai,
                            &c.comment,
                            &c.ai_reply
                        );
                    }
                    let mut history = Vec::new();
                    for c in &thread {
                        if c.is_ai {
                            // AI comment: the `comment` field is the user's instruction
                            // (with @AGENT prefix), and `ai_reply` is the assistant's response.
                            let user_msg = clean_history_message(&c.comment);
                            if !user_msg.is_empty() {
                                history.push(("user".to_string(), user_msg));
                                tracing::info!(
                                    "[ai_comment]   history +user(from ai) id={} msg=\"{}\"",
                                    c.id,
                                    &history.last().unwrap().1
                                );
                            }
                            if !c.ai_reply.is_empty() {
                                history.push(("assistant".to_string(), c.ai_reply.clone()));
                                tracing::info!(
                                    "[ai_comment]   history +assistant id={} len={}",
                                    c.id,
                                    c.ai_reply.len()
                                );
                            }
                        } else {
                            let msg = clean_history_message(&c.comment);
                            if !msg.is_empty() {
                                history.push(("user".to_string(), msg));
                                tracing::info!(
                                    "[ai_comment]   history +user id={} msg=\"{}\"",
                                    c.id,
                                    &history.last().unwrap().1
                                );
                            }
                        }
                    }
                    tracing::info!(
                        "[ai_comment] built {} history messages for comment {}",
                        history.len(),
                        comment_id
                    );
                    history
                }
                Err(e) => {
                    tracing::warn!(
                        "[ai_comment] Failed to load thread for comment {}: {}",
                        comment_id,
                        e
                    );
                    Vec::new()
                }
            }
        } else {
            tracing::info!(
                "[ai_comment] comment={} has no parent_id, first turn",
                comment_id
            );
            Vec::new()
        };

        if !conversation_history.is_empty() {
            tracing::info!(
                "[ai_comment] history for comment {}: {}",
                comment_id,
                conversation_history
                    .iter()
                    .map(|(r, c)| {
                        let preview = if c.len() > 100 {
                            let end = c.char_indices().nth(100).map(|(i, _)| i).unwrap_or(c.len());
                            format!("{}...", &c[..end])
                        } else {
                            c.clone()
                        };
                        format!("[{}] {}", r, preview)
                    })
                    .collect::<Vec<_>>()
                    .join(" → ")
            );
        }
        // Fetch descendants (what comes AFTER the regenerated reply) so the
        // model can bridge the gap naturally.
        let mut instruction_with_context = instruction.clone();
        match db.get_comment_subthread(comment_id).await {
            Ok(descendants) if !descendants.is_empty() => {
                let mut tail = String::from("\n\n<following_conversation>\n");
                for c in &descendants {
                    if c.is_ai && !c.ai_reply.is_empty() {
                        tail.push_str(&format!("assistant: {}\n", c.ai_reply));
                    } else if !c.is_ai {
                        let msg = clean_history_message(&c.comment);
                        if !msg.is_empty() {
                            tail.push_str(&format!("user: {}\n", msg));
                        }
                    }
                }
                tail.push_str("</following_conversation>\n\n");
                tail.push_str(
                    "The above conversation happened AFTER the reply you are about to generate. ",
                );
                tail.push_str(
                    "Your response must bridge naturally from the preceding conversation INTO this following conversation. ",
                );
                instruction_with_context.push_str(&tail);
                tracing::info!(
                    "[ai_comment] comment={} descendants={} appended to instruction",
                    comment_id,
                    descendants.len()
                );
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(
                    "[ai_comment] Failed to fetch descendants for {}: {}",
                    comment_id,
                    e
                );
            }
        }

        tracing::info!(
            "[ai_comment] comment={} instruction=\"{}\" history_msgs={} insight_len={} mode={}",
            comment_id,
            instruction,
            conversation_history.len(),
            insight.len(),
            if conversation_history.is_empty() {
                "first-turn (full text included)"
            } else {
                "multi-turn (instruction only)"
            }
        );

        if is_heading {
            tracing::info!(
                "[ai_comment] Heading selection for {} (insight len={})",
                paper_id,
                insight.len()
            );
        } else {
            tracing::info!(
                "[ai_comment] Non-heading selection for {} (insight len={})",
                paper_id,
                insight.len()
            );
        }

        match processor
            .revise(
                &insight,
                &quote,
                &before_ctx,
                &after_ctx,
                &instruction_with_context,
                &conversation_history,
                None,
                None,
                Some(&source),
                Some(&paper_id),
            )
            .await
        {
            Ok(result) => {
                if let Err(e) = db
                    .update_ai_comment_reply(
                        comment_id,
                        &result.explanation,
                        &result.old_text,
                        &result.new_text,
                        "completed",
                    )
                    .await
                {
                    tracing::error!(
                        "[ai_comment] Failed to update comment {} with result: {}",
                        comment_id,
                        e
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    "[ai_comment] LLM call failed for comment {}: {}",
                    comment_id,
                    e
                );
                if let Err(db_err) = db
                    .update_ai_comment_reply(
                        comment_id,
                        &format!("AI call failed: {}", e),
                        "",
                        "",
                        "failed",
                    )
                    .await
                {
                    tracing::error!(
                        "[ai_comment] Failed to update comment {} with error message: {}",
                        comment_id,
                        db_err
                    );
                }
            }
        }
    });
}

/// On server startup, re-spawn tasks for any AI comments left in "pending" state.
pub(crate) fn resume_pending_ai_comments(state: &Arc<AppState>) {
    let db = state.db.clone();
    let state_clone = Arc::clone(state);

    tokio::spawn(async move {
        let processor = {
            let comment_proc = state_clone.comment_processor.read().await;
            if comment_proc.is_some() {
                comment_proc.clone()
            } else {
                let processors = state_clone.insight_processors.read().await;
                processors.first().map(|(_, s)| Arc::clone(s))
            }
        };
        let pending = match db.list_pending_ai_comments().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("[ai_comment] Failed to query pending comments: {}", e);
                return;
            }
        };

        if pending.is_empty() {
            return;
        }

        tracing::info!(
            "[ai_comment] Resuming {} pending AI comment(s) after restart",
            pending.len()
        );

        for c in pending {
            // Extract instruction from "@AGENT ..." comment text
            let instruction = c
                .comment
                .strip_prefix("@AGENT")
                .unwrap_or(&c.comment)
                .trim()
                .to_string();
            if instruction.is_empty() {
                // Shouldn't happen, but skip if no instruction
                if let Err(e) = db
                    .update_ai_comment_reply(
                        c.id,
                        "Resume failed: missing instruction",
                        "",
                        "",
                        "failed",
                    )
                    .await
                {
                    tracing::error!(
                        "[ai_comment] Failed to mark comment {} as failed: {}",
                        c.id,
                        e
                    );
                }
                continue;
            }

            // Load paper insight for the AI prompt context
            let paper = match db.get_paper(&c.paper_id).await {
                Ok(Some(p)) => p,
                Ok(None) => {
                    if let Err(e) = db
                        .update_ai_comment_reply(
                            c.id,
                            "Resume failed: paper deleted",
                            "",
                            "",
                            "failed",
                        )
                        .await
                    {
                        tracing::error!(
                            "[ai_comment] Failed to mark comment {} as failed: {}",
                            c.id,
                            e
                        );
                    }
                    continue;
                }
                Err(e) => {
                    tracing::warn!(
                        "[ai_comment] Failed to load paper {} for pending comment {}: {}",
                        c.paper_id,
                        c.id,
                        e
                    );
                    continue;
                }
            };

            spawn_ai_reply_task(
                db.clone(),
                processor.clone(),
                c.id,
                c.parent_id,
                c.quote,
                c.before_ctx,
                c.after_ctx,
                instruction,
                paper.insight,
                paper.source_type,
                c.paper_id,
                false, // is_heading: unknown on recovery, default to false
            );
        }
    });
}

pub(crate) async fn list_comments(
    State(state): State<Arc<AppState>>,
    Path((_source, id)): Path<(String, String)>,
) -> Result<Json<Vec<CommentResponse>>, StatusCode> {
    let comments = state.db.list_paper_comments(&id).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let responses: Vec<CommentResponse> = comments
        .into_iter()
        .map(|c| CommentResponse {
            id: c.id,
            paper_id: c.paper_id,
            quote: c.quote,
            before_ctx: c.before_ctx,
            after_ctx: c.after_ctx,
            comment: c.comment,
            ai_reply: c.ai_reply,
            ai_old_text: c.ai_old_text,
            ai_new_text: c.ai_new_text,
            ai_status: c.ai_status,
            is_ai: c.is_ai,
            parent_id: c.parent_id,
            created_at: c.created_at,
            updated_at: c.updated_at,
        })
        .collect();
    Ok(Json(responses))
}

pub(crate) async fn add_comment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((_source, id)): Path<(String, String)>,
    Json(req): Json<CommentRequest>,
) -> Result<Json<CommentResponse>, StatusCode> {
    if !check_auth(&headers, &state.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let comment = state
        .db
        .add_paper_comment(
            &id,
            &req.quote,
            &req.before_ctx,
            &req.after_ctx,
            &req.comment,
            false,
            "",
            req.parent_id,
        )
        .await
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(CommentResponse {
        id: comment.id,
        paper_id: comment.paper_id,
        quote: comment.quote,
        before_ctx: comment.before_ctx,
        after_ctx: comment.after_ctx,
        comment: comment.comment,
        ai_reply: comment.ai_reply,
        ai_old_text: comment.ai_old_text,
        ai_new_text: comment.ai_new_text,
        ai_status: comment.ai_status,
        is_ai: comment.is_ai,
        parent_id: comment.parent_id,
        created_at: comment.created_at,
        updated_at: comment.updated_at,
    }))
}

pub(crate) async fn delete_comment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    if !check_auth(&headers, &state.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    state.db.delete_paper_comment(id).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn add_ai_comment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((_source, id)): Path<(String, String)>,
    Json(req): Json<AiCommentRequest>,
) -> Result<Json<CommentResponse>, StatusCode> {
    if !check_auth(&headers, &state.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let paper = state
        .db
        .get_paper(&id)
        .await
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;
    let comment_text = format!("@AGENT {}", req.instruction);
    let comment = state
        .db
        .add_paper_comment(
            &id,
            &req.quote,
            &req.before_ctx,
            &req.after_ctx,
            &comment_text,
            true,
            "pending",
            req.parent_id,
        )
        .await
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let comment_id = comment.id;
    let db = state.db.clone();
    let processor = {
        let comment_proc = state.comment_processor.read().await;
        if comment_proc.is_some() {
            comment_proc.clone()
        } else {
            let processors = state.insight_processors.read().await;
            processors.first().map(|(_, s)| Arc::clone(s))
        }
    };

    spawn_ai_reply_task(
        db,
        processor,
        comment_id,
        req.parent_id,
        req.quote.clone(),
        req.before_ctx.clone(),
        req.after_ctx.clone(),
        req.instruction.clone(),
        paper.insight.clone(),
        paper.source_type.clone(),
        paper.id.clone(),
        req.is_heading,
    );

    Ok(Json(CommentResponse {
        id: comment.id,
        paper_id: comment.paper_id,
        quote: comment.quote,
        before_ctx: comment.before_ctx,
        after_ctx: comment.after_ctx,
        comment: comment.comment,
        ai_reply: comment.ai_reply,
        ai_old_text: comment.ai_old_text,
        ai_new_text: comment.ai_new_text,
        ai_status: comment.ai_status,
        is_ai: comment.is_ai,
        parent_id: comment.parent_id,
        created_at: comment.created_at,
        updated_at: comment.updated_at,
    }))
}

pub(crate) async fn apply_ai_comment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let err = |code: u16, reason: String| -> (StatusCode, Json<ErrorResponse>) {
        (
            StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(ErrorResponse { code, reason }),
        )
    };
    if !check_auth(&headers, &state.auth_token) {
        return Err(err(401, "Unauthorized".into()));
    }
    let comment = state
        .db
        .get_comment_by_id(id)
        .await
        .map_err(|e| {
            error!("{}", e);
            err(500, "Database error".into())
        })?
        .ok_or(err(404, "Comment not found".into()))?;

    let paper = state
        .db
        .get_paper(&comment.paper_id)
        .await
        .map_err(|e| {
            error!("{}", e);
            err(500, "Database error".into())
        })?
        .ok_or(err(404, "Paper not found".into()))?;

    let old_text = &comment.ai_old_text;
    let new_text = &comment.ai_new_text;
    if old_text.is_empty() || new_text.is_empty() {
        return Err(err(400, "Missing modification content".into()));
    }

    // Try exact match first, then normalized-whitespace match, then fuzzy match.
    let (matched_old, matches) = match find_text_in_insight(&paper.insight, old_text) {
        Some(result) => result,
        None => {
            tracing::warn!(
                "[apply_ai_comment] No match for comment {}. old_text len={}, insight len={}",
                id,
                old_text.len(),
                paper.insight.len()
            );
            return Err(err(
                409,
                "Original text changed; matching fragment not found".into(),
            ));
        }
    };
    if matches.len() > 1 {
        return Err(err(
            409,
            "Matching fragment appears multiple times; cannot determine unique replacement position".into(),
        ));
    }

    // Backup original insight before applying change
    let processed_at_str = paper.insight_processed_at.map(|dt| dt.to_rfc3339());
    let reviewed_at_str = paper.insight_reviewed_at.map(|dt| dt.to_rfc3339());
    state
        .db
        .backup_insight(
            &comment.paper_id,
            &paper.insight,
            processed_at_str.as_deref(),
            &paper.insight_review,
            reviewed_at_str.as_deref(),
        )
        .await
        .map_err(|e| {
            error!("{}", e);
            err(500, "Backup failed".into())
        })?;

    let new_insight = paper.insight.replacen(&matched_old, new_text, 1);
    let updates = DbPaperUpdate {
        insight: Some(new_insight.clone()),
        ..Default::default()
    };
    state
        .db
        .update_paper(&comment.paper_id, &updates)
        .await
        .map_err(|e| {
            error!("{}", e);
            err(500, "Update failed".into())
        })?;
    state.db.apply_ai_comment(id).await.map_err(|e| {
        error!("{}", e);
        err(500, "Failed to update status".into())
    })?;
    refresh_insight_html_cache(
        &state.db,
        &paper.source_type,
        &comment.paper_id,
        &new_insight,
    )
    .await;

    Ok(StatusCode::OK)
}

pub(crate) async fn reject_ai_comment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    if !check_auth(&headers, &state.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    state.db.reject_ai_comment(id).await.map_err(|e| {
        error!("{}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(StatusCode::OK)
}

pub(crate) async fn regenerate_ai_comment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<CommentResponse>, StatusCode> {
    if !check_auth(&headers, &state.auth_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let comment = state
        .db
        .get_comment_by_id(id)
        .await
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let paper = state
        .db
        .get_paper(&comment.paper_id)
        .await
        .map_err(|e| {
            error!("{}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let instruction = comment
        .comment
        .strip_prefix("@AGENT ")
        .unwrap_or(&comment.comment);

    if let Err(e) = state
        .db
        .update_ai_comment_reply(id, "", "", "", "pending")
        .await
    {
        error!(
            "[ai_comment] Failed to reset comment {} to pending: {}",
            id, e
        );
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let db = state.db.clone();
    let processor = {
        let comment_proc = state.comment_processor.read().await;
        if comment_proc.is_some() {
            comment_proc.clone()
        } else {
            let processors = state.insight_processors.read().await;
            processors.first().map(|(_, s)| Arc::clone(s))
        }
    };

    // Check if quote matches the paper title from DB.
    let is_heading = comment.quote.trim() == paper.title.trim();

    spawn_ai_reply_task(
        db,
        processor,
        id,
        comment.parent_id,
        comment.quote.clone(),
        comment.before_ctx.clone(),
        comment.after_ctx.clone(),
        instruction.to_string(),
        paper.insight.clone(),
        paper.source_type.clone(),
        comment.paper_id.clone(),
        is_heading,
    );

    Ok(Json(CommentResponse {
        id: comment.id,
        paper_id: comment.paper_id,
        quote: comment.quote,
        before_ctx: comment.before_ctx,
        after_ctx: comment.after_ctx,
        comment: comment.comment,
        ai_reply: comment.ai_reply,
        ai_old_text: comment.ai_old_text,
        ai_new_text: comment.ai_new_text,
        ai_status: comment.ai_status,
        is_ai: comment.is_ai,
        parent_id: comment.parent_id,
        created_at: comment.created_at,
        updated_at: comment.updated_at,
    }))
}
