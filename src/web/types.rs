// SPDX-License-Identifier: MIT OR Apache-2.0

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

use crate::db::{Db, DbPaper};

pub type InsightCancellationMap =
    RwLock<BTreeMap<String, (Arc<AtomicBool>, tokio::task::JoinHandle<()>)>>;
use crate::processor::Processor;

#[derive(Clone, Debug)]
pub(crate) enum InsightStreamEvent {
    Chunk(String),
    Reset,
    Done,
    Error(String),
}

#[derive(Clone)]
pub struct InsightStreamState {
    /// `std::sync::Mutex` is intentional: `accumulated` is written from a
    /// synchronous callback (`on_chunk`) inside `run_insight_analysis`, so it
    /// cannot use `tokio::sync::Mutex`. Lock duration is always sub-millisecond
    /// and never crosses an `.await` point, so blocking the worker thread is
    /// negligible.
    pub accumulated: Arc<std::sync::Mutex<String>>,
    pub tx: broadcast::Sender<InsightStreamEvent>,
    pub completed: Arc<std::sync::atomic::AtomicBool>,
    pub error: Arc<tokio::sync::Mutex<Option<String>>>,
}

impl InsightStreamState {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            accumulated: Arc::new(std::sync::Mutex::new(String::new())),
            tx,
            completed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            error: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }
}

/// Alias for the shared, hot-reloadable list of insight processors.
pub type InsightProcessors = Arc<tokio::sync::RwLock<Vec<(String, Arc<Processor>)>>>;

pub struct AppState {
    pub db: Db,
    /// bcrypt hash of the configured password (hashed once at startup)
    pub password_hash: Option<String>,
    /// random 32-byte token generated at startup for Bearer auth
    pub auth_token: Option<String>,
    /// Insight processors are rebuilt whenever provider config or worker-count
    /// settings change, so new tasks pick up the latest configuration without
    /// a server restart. In-flight tasks keep their existing `Arc<Processor>`.
    pub insight_processors: InsightProcessors,
    /// Dedicated processor for insight comments. When a request does not
    /// explicitly specify a provider, this processor is used.
    /// Wrapped in RwLock so admin UI changes can update it without restart.
    pub comment_processor: Arc<tokio::sync::RwLock<Option<Arc<Processor>>>>,
    pub summarize_prompt: String,
    pub translate_prompt: String,
    pub insight_prompts: Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
    pub insight_prompts_mtime:
        Arc<tokio::sync::RwLock<std::collections::HashMap<String, chrono::DateTime<chrono::Utc>>>>,
    pub insight_streams: Arc<RwLock<BTreeMap<String, InsightStreamState>>>,
    pub insight_cancellations: Arc<InsightCancellationMap>,
    /// Editable runtime config managed by admin UI
    pub editable_config: Arc<tokio::sync::RwLock<EditableConfig>>,
    /// arXiv fetch task status
    pub fetch_status: Arc<tokio::sync::RwLock<FetchStatus>>,
    /// Broadcast channel for fetch task progress events (SSE).
    pub fetch_tx: tokio::sync::broadcast::Sender<String>,
    /// Cancel signal for fetch task.
    pub fetch_cancel: Arc<std::sync::atomic::AtomicBool>,
}

/// arXiv fetch task status.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FetchStatus {
    pub running: bool,
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    pub message: String,
    pub started_at: Option<String>,
    pub phase: String,
}

impl Default for FetchStatus {
    fn default() -> Self {
        Self {
            running: false,
            total: 0,
            completed: 0,
            failed: 0,
            message: "Ready".into(),
            started_at: None,
            phase: "idle".into(),
        }
    }
}

/// Editable config items managed by admin UI.
/// All fields are hot-reloaded at runtime without server restart.
#[derive(Clone, Debug)]
pub struct EditableConfig {
    pub arxiv_fetch_cron: String,
    pub arxiv_query: String,
    pub arxiv_max_results: usize,
    pub arxiv_page_size: usize,
    pub llm_max_workers: usize,
    pub llm_max_retries: usize,
    pub pdf_max_workers: usize,
    pub insight_max_workers: usize,
    pub insight_review_max_attempts: usize,
    pub auto_review_insight: bool,
    pub mineru_base_url: String,
    pub mineru_no_cache: bool,
    pub mineru_api_key: Option<String>,
}

#[derive(Serialize)]
pub struct PaperResponse {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub published: DateTime<Utc>,
    pub score: f32,
    pub paper_type: String,
    pub summary: String,
    pub r#abstract: String,
    pub processed_at: DateTime<Utc>,
    pub insight: String,
    pub insight_processed_at: Option<DateTime<Utc>>,
    pub insight_review: String,
    pub insight_reviewed_at: Option<DateTime<Utc>>,
    pub checked_at: Option<DateTime<Utc>>,
    pub mark: Option<String>,
    pub tags: Vec<TagItem>,
    pub source_type: String,
    pub source_url: Option<String>,
    pub external_id: Option<String>,
}

impl PaperResponse {
    pub fn from_db(p: DbPaper, mark: Option<String>, tags: Vec<TagItem>) -> Self {
        Self {
            id: p.id,
            title: p.title,
            authors: p.authors,
            published: p.published,
            score: p.score,
            paper_type: p.paper_type,
            summary: p.summary,
            r#abstract: p.r#abstract,
            processed_at: p.processed_at,
            insight: p.insight,
            insight_processed_at: p.insight_processed_at,
            insight_review: p.insight_review,
            insight_reviewed_at: p.insight_reviewed_at,
            checked_at: p.checked_at,
            mark,
            tags,
            source_type: p.source_type,
            source_url: p.source_url,
            external_id: p.external_id,
        }
    }
}

#[derive(Serialize)]
pub struct TagItem {
    pub tag: String,
    pub weight: f32,
}

#[derive(Serialize)]
pub struct RelatedPaper {
    pub id: String,
    pub title: String,
    pub score: f32,
    pub similarity: f32,
    pub source_type: String,
}

#[derive(Serialize)]
pub struct InsightNeighbor {
    pub id: String,
    pub title: String,
    pub source_type: String,
}

#[derive(Serialize)]
pub struct InsightNeighborsResponse {
    pub prev: Option<InsightNeighbor>,
    pub next: Option<InsightNeighbor>,
}

/// Response item for paper list queries.
/// Carries boolean flags for insight/review status instead of full text.
#[derive(Serialize)]
pub struct PaperListItemResponse {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub published: DateTime<Utc>,
    pub score: f32,
    pub paper_type: String,
    pub summary: String,
    pub r#abstract: String,
    pub processed_at: DateTime<Utc>,
    pub insight_processed_at: Option<DateTime<Utc>>,
    pub insight_reviewed_at: Option<DateTime<Utc>>,
    pub checked_at: Option<DateTime<Utc>>,
    pub mark: Option<String>,
    pub tags: Vec<TagItem>,
    pub source_type: String,
    pub source_url: Option<String>,
    pub external_id: Option<String>,
    pub is_analyzing: bool,
    pub is_reviewing: bool,
    pub has_insight: bool,
    pub has_review: bool,
}

impl PaperListItemResponse {
    pub fn from_db_list_item(
        p: crate::db::DbPaperListItem,
        mark: Option<String>,
        tags: Vec<TagItem>,
    ) -> Self {
        Self {
            id: p.id,
            title: p.title,
            authors: p.authors,
            published: p.published,
            score: p.score,
            paper_type: p.paper_type,
            summary: p.summary,
            r#abstract: p.r#abstract,
            processed_at: p.processed_at,
            insight_processed_at: p.insight_processed_at,
            insight_reviewed_at: p.insight_reviewed_at,
            checked_at: p.checked_at,
            mark,
            tags,
            source_type: p.source_type,
            source_url: p.source_url,
            external_id: p.external_id,
            is_analyzing: p.is_analyzing,
            is_reviewing: p.is_reviewing,
            has_insight: p.has_insight,
            has_review: p.has_review,
        }
    }
}

#[derive(Serialize)]
pub struct ListResponse {
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
    pub papers: Vec<PaperListItemResponse>,
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub page: Option<usize>,
    pub page_size: Option<usize>,
    pub search: Option<String>,
    pub mark: Option<String>,
    pub insight: Option<String>,
    pub tag: Option<String>,
    pub checked: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub source: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateRequest {
    pub title: Option<String>,
    pub score: Option<f32>,
    pub paper_type: Option<String>,
    pub summary: Option<String>,
    pub r#abstract: Option<String>,
    pub insight: Option<String>,
    pub insight_review: Option<String>,
    pub checked_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
pub struct MarkRequest {
    pub mark: Option<String>,
}

#[derive(Deserialize)]
pub struct InsightRequest {
    pub prompt: Option<String>,
    pub provider: Option<String>,
    /// Comma-separated caption numbers to keep as separate sub-figures,
    /// e.g. "F:7,F:8".
    pub split_figures: Option<String>,
    /// Override auto-review for this request. If None, falls back to the
    /// server-side default (AUTO_REVIEW_INSIGHT env var).
    pub auto_review: Option<bool>,
    /// Skip MinerU figure extraction validation. Useful when a paper contains
    /// inline tables or figures without formal captions.
    pub skip_validation: Option<bool>,
    /// Keep the main figure/table caption text in the LLM prompt.  When
    /// `false` (or absent and user unchecked the box), captions that start
    /// with "Figure N:" / "Table N:" are stripped, preserving only
    /// sub-panel labels like "(a) ...".
    pub keep_caption: Option<bool>,
}

#[derive(Deserialize)]
pub struct ReviewRequest {
    pub provider: Option<String>,
    pub prompt: Option<String>,
}

#[derive(Deserialize)]
pub struct CommentRequest {
    pub quote: String,
    pub before_ctx: String,
    pub after_ctx: String,
    pub comment: String,
    pub parent_id: Option<i64>,
}

#[derive(Serialize)]
pub struct CommentResponse {
    pub id: i64,
    pub paper_id: String,
    pub quote: String,
    pub before_ctx: String,
    pub after_ctx: String,
    pub comment: String,
    pub ai_reply: String,
    pub ai_old_text: String,
    pub ai_new_text: String,
    pub ai_status: String,
    pub is_ai: bool,
    pub parent_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct AiCommentRequest {
    pub quote: String,
    pub before_ctx: String,
    pub after_ctx: String,
    pub instruction: String,
    pub parent_id: Option<i64>,
    #[serde(default)]
    pub is_heading: bool,
}

#[derive(Deserialize)]
pub struct AuthRequest {
    pub password: String,
}

#[derive(Serialize)]
pub struct AuthResponse {
    pub token: String,
}

#[derive(Serialize)]
pub struct AuthStatus {
    pub authenticated: bool,
}

#[derive(Serialize)]
pub struct InsightProgressResponse {
    pub titles: Vec<InsightProgressItem>,
    pub completed: Vec<InsightProgressItem>,
}

#[derive(Serialize)]
pub struct InsightProgressItem {
    pub id: String,
    pub title: String,
    pub processed_at: Option<i64>,
    pub source_type: String,
}

#[derive(Serialize)]
pub struct DeleteTagResponse {
    pub deleted: usize,
}

#[derive(Serialize)]
pub struct TagInfo {
    pub tag: String,
    pub count: usize,
}

#[derive(Serialize)]
pub struct TagsDashboardResponse {
    pub tags: Vec<TagInfo>,
    pub orphan_count: usize,
    pub uninteresting_count: usize,
    pub disinterest_tags: Vec<TagPreferenceInfo>,
    pub interest_tags: Vec<String>,
}

#[derive(Serialize)]
pub struct TagPreferenceInfo {
    pub tag: String,
    pub count: usize,
}

#[derive(Deserialize)]
pub struct DeleteUninterestingRequest {
    pub tags: Vec<String>,
}

#[derive(Serialize)]
pub struct UninterestingDeleteResponse {
    pub deleted: usize,
    pub skipped: usize,
}

#[derive(Serialize)]
pub(crate) struct InsightProvidersResponse {
    pub providers: Vec<String>,
}

#[derive(Serialize)]
pub(crate) struct InsightBackupItem {
    pub id: i64,
    pub insight: String,
    pub insight_processed_at: Option<DateTime<Utc>>,
    pub insight_review: String,
    pub insight_reviewed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub(crate) struct InsightBackupsResponse {
    pub backups: Vec<InsightBackupItem>,
}

#[derive(Deserialize)]
pub(crate) struct RestoreInsightRequest {
    pub backup_id: i64,
}

#[derive(Serialize)]
pub(crate) struct TokenCountResponse {
    pub insight_tokens: usize,
    pub review_tokens: usize,
}

#[derive(Serialize)]
pub(crate) struct OverlayBbox {
    pub bbox: [f32; 4],
    pub body_bbox: [f32; 4],
    pub page_idx: i32,
    pub content_type: String,
    pub desc: String,
    pub name: String,
    pub caption_number: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct OverlayPage {
    pub page_idx: i32,
    pub page_size: [f32; 2],
    pub bboxes: Vec<OverlayBbox>,
}

#[derive(Serialize)]
pub(crate) struct OverlayResponse {
    pub paper_id: String,
    pub total_pages: i32,
    pub pages: Vec<OverlayPage>,
}

#[derive(Serialize)]
pub struct ErrorResponse {
    pub code: u16,
    pub reason: String,
}

#[derive(Serialize)]
pub struct DateTreeResponse {
    pub year: String,
    pub count: usize,
    pub months: Vec<MonthNode>,
}

#[derive(Serialize)]
pub struct MonthNode {
    pub month: String,
    pub count: usize,
    pub days: Vec<DayNode>,
}

#[derive(Serialize)]
pub struct DayNode {
    pub day: String,
    pub count: usize,
}

#[derive(Serialize)]
pub struct InsightHtmlResponse {
    pub html: String,
    pub toc: Vec<TocItem>,
}

#[derive(Serialize, Deserialize)]
pub struct TocItem {
    pub id: String,
    pub level: u8,
    pub text: String,
    pub num: String,
}

#[derive(Serialize)]
pub struct PaperByDateResponse {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub r#abstract: String,
    pub score: f32,
    pub mark: Option<String>,
    pub source_type: Option<String>,
    pub tags: Vec<TagItem>,
}

#[derive(Deserialize)]
pub struct SaveBboxAdjustmentRequest {
    pub name: String,
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

#[derive(Serialize)]
pub struct BboxAdjustmentsResponse {
    pub adjustments: std::collections::HashMap<String, crate::mineru::BboxAdjustment>,
}

#[derive(Serialize)]
pub struct CaptionCandidate {
    pub text: String,
    pub bbox: [f32; 4],
    pub score: f32,
}

#[derive(Serialize)]
pub struct CaptionCandidatesResponse {
    pub candidates: Vec<CaptionCandidate>,
}

#[derive(Deserialize)]
pub struct SaveManualFigureRequest {
    pub page_idx: i32,
    pub body_bbox: [f32; 4],
    pub caption_bbox: Option<[f32; 4]>,
    pub caption_text: String,
    pub content_type: String,
}

#[derive(Serialize)]
pub struct SaveManualFigureResponse {
    pub name: String,
}

pub use crate::figure::types::SmartDetectResponse;

/// Editable admin config items (GET response)
#[derive(Serialize)]
pub struct AdminConfigResponse {
    pub arxiv_fetch_cron: String,
    pub arxiv_query: String,
    pub arxiv_max_results: usize,
    pub arxiv_page_size: usize,
    pub llm_max_workers: usize,
    pub llm_max_retries: usize,
    pub pdf_max_workers: usize,
    pub insight_max_workers: usize,
    pub insight_review_max_attempts: usize,
    pub auto_review_insight: bool,
    pub mineru_base_url: String,
    pub mineru_no_cache: bool,
    pub mineru_api_key: Option<String>,
}

/// Admin config update request (POST body)
#[derive(Deserialize)]
pub struct AdminConfigUpdateRequest {
    pub arxiv_fetch_cron: Option<String>,
    pub arxiv_query: Option<String>,
    pub arxiv_max_results: Option<usize>,
    pub arxiv_page_size: Option<usize>,
    pub llm_max_workers: Option<usize>,
    pub llm_max_retries: Option<usize>,
    pub pdf_max_workers: Option<usize>,
    pub insight_max_workers: Option<usize>,
    pub insight_review_max_attempts: Option<usize>,
    pub auto_review_insight: Option<bool>,
    pub mineru_base_url: Option<String>,
    pub mineru_no_cache: Option<bool>,
    pub mineru_api_key: Option<String>,
}

/// Provider configuration detail
#[derive(Serialize)]
pub struct ProviderDetail {
    pub index: usize,
    pub name: String,
    pub provider_type: String,
    pub base_url: String,
    pub api_keys: Vec<String>,
    pub model: String,
    pub max_tokens: String,
    pub reasoning_effort: String,
    pub is_digest: String,
    pub is_comment: String,
    pub enabled: String,
}

#[derive(Serialize)]
pub struct ProviderListResponse {
    pub providers: Vec<ProviderDetail>,
}

#[derive(Deserialize)]
pub struct ProviderUpdateRequest {
    pub name: String,
    pub provider_type: String,
    pub base_url: String,
    #[serde(default)]
    pub api_keys: Vec<String>,
    pub model: String,
    pub max_tokens: String,
    #[serde(default)]
    pub reasoning_effort: String,
    #[serde(default)]
    pub is_digest: String,
    #[serde(default)]
    pub is_comment: String,
    #[serde(default)]
    pub enabled: String,
}

#[derive(Deserialize)]
pub struct ReorderProvidersRequest {
    pub order: Vec<usize>,
}
