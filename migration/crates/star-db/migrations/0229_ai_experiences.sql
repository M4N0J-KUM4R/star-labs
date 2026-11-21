-- Migration 0229: the AI experiences, their conversations and their context
-- files.
--
-- Canvas `AiExperience` is the course scoped configuration an AI tool reads
-- for a course; `AiConversation` is one conversation of a user with that
-- experience, and `AiExperienceContextFile` links the files the experience
-- is indexed from.

CREATE TABLE IF NOT EXISTS ai_experiences (
    id                         BIGSERIAL PRIMARY KEY,
    course_id                  BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    account_id                 BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    root_account_id            BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    title                      TEXT NOT NULL DEFAULT '',
    description                TEXT,
    facts                      TEXT,
    learning_objective         TEXT NOT NULL DEFAULT '',
    pedagogical_guidance       TEXT,
    workflow_state             TEXT NOT NULL DEFAULT 'unpublished',
    llm_conversation_context_id TEXT,
    context_index_status       TEXT NOT NULL DEFAULT 'not_started',
    created_at                 TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                 TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_ai_experiences_course ON ai_experiences (course_id);

CREATE TABLE IF NOT EXISTS ai_conversations (
    id                  BIGSERIAL PRIMARY KEY,
    ai_experience_id    BIGINT NOT NULL REFERENCES ai_experiences(id) ON DELETE CASCADE,
    user_id             BIGINT REFERENCES users(id) ON DELETE SET NULL,
    course_id           BIGINT REFERENCES courses(id) ON DELETE CASCADE,
    account_id          BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    root_account_id     BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    llm_conversation_id TEXT,
    workflow_state      TEXT NOT NULL DEFAULT 'active',
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_ai_conversations_experience
    ON ai_conversations (ai_experience_id, user_id);

CREATE TABLE IF NOT EXISTS ai_experience_context_files (
    id              BIGSERIAL PRIMARY KEY,
    ai_experience_id BIGINT NOT NULL REFERENCES ai_experiences(id) ON DELETE CASCADE,
    attachment_id   BIGINT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    root_account_id BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    position        INTEGER NOT NULL DEFAULT 1,
    UNIQUE (ai_experience_id, attachment_id)
);
