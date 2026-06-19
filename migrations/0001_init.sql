-- Consolidated schema: final state of all tables after migrations 0001-0007.
-- For existing DBs, run: DELETE FROM _sqlx_migrations WHERE version >= 1;

CREATE TABLE IF NOT EXISTS papers (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    authors JSONB NOT NULL,
    published TIMESTAMPTZ NOT NULL,
    score REAL NOT NULL,
    paper_type TEXT NOT NULL DEFAULT '',
    summary TEXT NOT NULL,
    abstract_zh TEXT NOT NULL,
    abstract_en TEXT NOT NULL DEFAULT '',
    processed_at TIMESTAMPTZ NOT NULL,
    source_type TEXT NOT NULL DEFAULT 'arxiv',
    source_url TEXT,
    external_id TEXT
);

CREATE TABLE IF NOT EXISTS paper_insights (
    paper_id TEXT PRIMARY KEY,
    insight TEXT NOT NULL DEFAULT '',
    processed_at TIMESTAMPTZ,
    review TEXT NOT NULL DEFAULT '',
    reviewed_at TIMESTAMPTZ,
    checked_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS paper_marks (
    paper_id TEXT PRIMARY KEY,
    mark TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS paper_tags (
    paper_id TEXT NOT NULL,
    tag TEXT NOT NULL,
    weight REAL NOT NULL,
    PRIMARY KEY (paper_id, tag)
);

CREATE TABLE IF NOT EXISTS deleted_papers (
    id TEXT PRIMARY KEY,
    deleted_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS users (
    id SERIAL PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT,
    auth_token TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS user_tag_preferences (
    tag TEXT PRIMARY KEY,
    preference_type TEXT NOT NULL
);

-- Idempotent migration: add user_id and switch to composite PK.
-- Safe to re-run on databases that already have the column/PK.
ALTER TABLE user_tag_preferences
    ADD COLUMN IF NOT EXISTS user_id INTEGER REFERENCES users(id) ON DELETE CASCADE;

INSERT INTO users (id, username, password_hash, auth_token)
VALUES (1, 'default', NULL, NULL)
ON CONFLICT (id) DO NOTHING;

UPDATE user_tag_preferences SET user_id = 1 WHERE user_id IS NULL;

ALTER TABLE user_tag_preferences ALTER COLUMN user_id SET NOT NULL;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'user_tag_preferences_pkey'
        AND conrelid = 'user_tag_preferences'::regclass
    ) THEN
        IF NOT EXISTS (
            SELECT 1 FROM information_schema.key_column_usage
            WHERE table_name = 'user_tag_preferences'
            AND constraint_name = 'user_tag_preferences_pkey'
            AND column_name = 'user_id'
        ) THEN
            ALTER TABLE user_tag_preferences DROP CONSTRAINT user_tag_preferences_pkey;
            ALTER TABLE user_tag_preferences ADD PRIMARY KEY (user_id, tag);
        END IF;
    ELSE
        ALTER TABLE user_tag_preferences ADD PRIMARY KEY (user_id, tag);
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS idx_user_tag_preferences_user_id ON user_tag_preferences(user_id);

CREATE TABLE IF NOT EXISTS paper_comments (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    paper_id TEXT NOT NULL,
    quote TEXT NOT NULL,
    before_ctx TEXT NOT NULL,
    after_ctx TEXT NOT NULL,
    comment TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    ai_reply TEXT NOT NULL DEFAULT '',
    ai_old_text TEXT NOT NULL DEFAULT '',
    ai_new_text TEXT NOT NULL DEFAULT '',
    ai_status TEXT NOT NULL DEFAULT '',
    is_ai BOOLEAN NOT NULL DEFAULT FALSE,
    parent_id BIGINT REFERENCES paper_comments(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS author_stats (
    name TEXT PRIMARY KEY,
    paper_count BIGINT NOT NULL DEFAULT 0,
    avg_score REAL NOT NULL DEFAULT 0.0,
    critical_count BIGINT NOT NULL DEFAULT 0,
    citation_count BIGINT,
    h_index BIGINT,
    first_seen TIMESTAMPTZ NOT NULL,
    last_updated TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS paper_insight_backups (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    paper_id TEXT NOT NULL,
    insight TEXT NOT NULL DEFAULT '',
    insight_processed_at TIMESTAMPTZ,
    insight_review TEXT NOT NULL DEFAULT '',
    insight_reviewed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS _cache_insight_html (
    paper_id TEXT PRIMARY KEY,
    html TEXT NOT NULL,
    toc TEXT NOT NULL DEFAULT '[]',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_papers_published ON papers(published);
CREATE INDEX IF NOT EXISTS idx_papers_source_type ON papers(source_type);
-- No B-tree index on insight TEXT — exceeds PG 8191-byte row limit.
-- Queries only use LIKE '# %' or != '' which don't benefit from B-tree.
CREATE INDEX IF NOT EXISTS idx_paper_insights_checked ON paper_insights(checked_at);
CREATE INDEX IF NOT EXISTS idx_marks_mark ON paper_marks(mark);
CREATE INDEX IF NOT EXISTS idx_tags_tag ON paper_tags(tag);
CREATE INDEX IF NOT EXISTS idx_tags_paper ON paper_tags(paper_id);
CREATE INDEX IF NOT EXISTS idx_comments_paper ON paper_comments(paper_id);
CREATE INDEX IF NOT EXISTS idx_comments_ai_status ON paper_comments(ai_status);
CREATE INDEX IF NOT EXISTS idx_comments_parent ON paper_comments(parent_id);
CREATE INDEX IF NOT EXISTS idx_backups_paper ON paper_insight_backups(paper_id);

-- GIN trigram indexes for ILIKE '%keyword%' search acceleration
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE INDEX IF NOT EXISTS idx_papers_title_trgm ON papers USING GIN (title gin_trgm_ops);
CREATE INDEX IF NOT EXISTS idx_papers_summary_trgm ON papers USING GIN (summary gin_trgm_ops);
CREATE INDEX IF NOT EXISTS idx_papers_abstract_zh_trgm ON papers USING GIN (abstract_zh gin_trgm_ops);
CREATE INDEX IF NOT EXISTS idx_paper_insights_insight_trgm ON paper_insights USING GIN (insight gin_trgm_ops);
CREATE INDEX IF NOT EXISTS idx_paper_insights_review_trgm ON paper_insights USING GIN (review gin_trgm_ops);
