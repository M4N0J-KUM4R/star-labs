--
-- Canvas schema parity: tables a.
--
-- Generated from the Canvas LMS migration set (db/migrate squashed baseline
-- 20101210192618_init_canvas_db.rb plus the post-baseline migrations, master
-- as of 2026-04): every Canvas table star-db does not already define, with
-- the Rails column types, NULL constraints, defaults and CHECK constraints.
-- Foreign keys and secondary indexes live in the two follow-up migrations so
-- every referenced table exists before its constraints are added.
-- NOT included by design: pgvector tables (assignment_embeddings,
-- discussion_topic_embeddings, wiki_page_embeddings — Canvas itself skips
-- them without the vector extension), plpgsql functions/triggers, replica
-- identities, and the runtime-managed auditors/aua log partitions beyond
-- the seven created eagerly by the baseline.
--

-- pg_trgm is required by the trigram indexes in the indexes migration
-- (pseudonyms unique_id / sis_user_id / integration_id, mirroring Canvas).
CREATE EXTENSION IF NOT EXISTS pg_trgm;


CREATE TABLE abstract_courses (
    id                               BIGSERIAL PRIMARY KEY,
    sis_source_id                    varchar(255),
    sis_batch_id                     bigint,
    account_id                       bigint NOT NULL,
    root_account_id                  bigint NOT NULL,
    short_name                       varchar(255),
    name                             varchar(255),
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    enrollment_term_id               bigint NOT NULL,
    workflow_state                   varchar(255) NOT NULL,
    stuck_sis_fields                 text
);

CREATE TABLE accessibility_issues (
    id                               BIGSERIAL PRIMARY KEY,
    root_account_id                  bigint NOT NULL,
    course_id                        bigint NOT NULL,
    wiki_page_id                     bigint,
    assignment_id                    bigint,
    attachment_id                    bigint,
    rule_type                        varchar NOT NULL,
    node_path                        text,
    metadata                         jsonb NOT NULL,
    workflow_state                   varchar NOT NULL DEFAULT 'active',
    updated_by_id                    bigint,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    accessibility_resource_scan_id   bigint NOT NULL,
    discussion_topic_id              bigint,
    announcement_id                  bigint,
    is_syllabus                      boolean NOT NULL DEFAULT false,
    CONSTRAINT chk_rule_type_enum CHECK (rule_type IN ('adjacent-links', 'headings-sequence', 'headings-start-at-h2', 'img-alt-filename', 'img-alt-length', 'img-alt', 'large-text-contrast', 'list-structure', 'paragraphs-for-headings', 'small-text-contrast', 'table-caption', 'table-header', 'table-header-scope')),
    CONSTRAINT chk_workflow_state_enum CHECK (workflow_state IN ('active', 'resolved', 'dismissed')),
    CONSTRAINT chk_context_disjunction CHECK (((wiki_page_id IS NOT NULL)::int + (assignment_id IS NOT NULL)::int + (attachment_id IS NOT NULL)::int + (discussion_topic_id IS NOT NULL)::int + (announcement_id IS NOT NULL)::int) <= 1),
    CONSTRAINT chk_workflow_state_enum_2 CHECK (workflow_state IN ('active', 'resolved', 'dismissed', 'closed'))
);

CREATE TABLE accessibility_resource_scans (
    id                               BIGSERIAL PRIMARY KEY,
    root_account_id                  bigint NOT NULL,
    course_id                        bigint NOT NULL,
    wiki_page_id                     bigint,
    assignment_id                    bigint,
    attachment_id                    bigint,
    error_message                    varchar(255),
    workflow_state                   varchar NOT NULL DEFAULT 'queued',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    resource_name                    varchar(255),
    resource_workflow_state          varchar NOT NULL DEFAULT 'unpublished',
    issue_count                      integer NOT NULL DEFAULT 0,
    resource_updated_at              timestamp,
    discussion_topic_id              bigint,
    announcement_id                  bigint,
    is_syllabus                      boolean NOT NULL DEFAULT false,
    closed_at                        timestamp,
    CONSTRAINT chk_workflow_state_enum_3 CHECK (workflow_state IN ('queued', 'in_progress', 'completed', 'failed')),
    CONSTRAINT chk_resource_workflow_state_enum CHECK (resource_workflow_state IN ('unpublished', 'published')),
    CONSTRAINT chk_context_disjunction_2 CHECK (((wiki_page_id IS NOT NULL)::int + (assignment_id IS NOT NULL)::int + (attachment_id IS NOT NULL)::int + (discussion_topic_id IS NOT NULL)::int + (announcement_id IS NOT NULL)::int) <= 1)
);

CREATE TABLE account_report_rows (
    id                               BIGSERIAL PRIMARY KEY,
    account_report_id                bigint NOT NULL,
    account_report_runner_id         bigint NOT NULL,
    row_number                       integer,
    row                              varchar[] DEFAULT '{}',
    created_at                       timestamp NOT NULL,
    file                             varchar
);

CREATE TABLE account_report_runners (
    id                               BIGSERIAL PRIMARY KEY,
    account_report_id                bigint NOT NULL,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'created',
    batch_items                      varchar[] DEFAULT '{}',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    started_at                       timestamp,
    ended_at                         timestamp,
    job_ids                          bigint[] NOT NULL DEFAULT '{}'
);

CREATE TABLE account_reports (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint NOT NULL,
    message                          text,
    account_id                       bigint NOT NULL,
    attachment_id                    bigint,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'created',
    report_type                      varchar(255),
    progress                         integer,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    parameters                       text,
    current_line                     integer,
    total_lines                      integer,
    start_at                         timestamp,
    end_at                           timestamp,
    job_ids                          bigint[] NOT NULL DEFAULT '{}'
);

CREATE TABLE ai_conversations (
    id                               BIGSERIAL PRIMARY KEY,
    root_account_id                  bigint NOT NULL,
    account_id                       bigint NOT NULL,
    course_id                        bigint NOT NULL,
    user_id                          bigint NOT NULL,
    ai_experience_id                 bigint NOT NULL,
    llm_conversation_id              varchar(255) NOT NULL,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    CONSTRAINT chk_workflow_state_enum_4 CHECK (workflow_state IN ('active', 'completed', 'deleted'))
);

CREATE TABLE ai_experience_context_files (
    id                               BIGSERIAL PRIMARY KEY,
    ai_experience_id                 bigint NOT NULL,
    attachment_id                    bigint NOT NULL,
    root_account_id                  bigint NOT NULL,
    position                         integer,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    llm_conversation_service_document_id varchar(255)
);

CREATE TABLE ai_experiences (
    id                               BIGSERIAL PRIMARY KEY,
    root_account_id                  bigint NOT NULL,
    account_id                       bigint NOT NULL,
    course_id                        bigint NOT NULL,
    title                            varchar(255) NOT NULL,
    description                      text,
    facts                            text,
    learning_objective               text NOT NULL,
    pedagogical_guidance             text NOT NULL,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'unpublished',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    llm_conversation_context_id      varchar(255),
    context_index_status             varchar(255) NOT NULL DEFAULT 'not_started',
    CONSTRAINT ai_experiences_context_index_status_check CHECK (context_index_status IN ('not_started', 'in_progress', 'completed', 'failed'))
);

CREATE TABLE alert_criteria (
    id                               BIGSERIAL PRIMARY KEY,
    alert_id                         bigint,
    criterion_type                   varchar(255),
    threshold                        double precision
);

CREATE TABLE alerts (
    id                               BIGSERIAL PRIMARY KEY,
    context_id                       bigint NOT NULL,
    context_type                     varchar(255) NOT NULL,
    recipients                       text NOT NULL,
    repetition                       integer,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE allocation_rules (
    id                               BIGSERIAL PRIMARY KEY,
    root_account_id                  bigint NOT NULL,
    course_id                        bigint NOT NULL,
    assignment_id                    bigint NOT NULL,
    assessor_id                      bigint NOT NULL,
    assessee_id                      bigint NOT NULL,
    must_review                      boolean NOT NULL DEFAULT true,
    review_permitted                 boolean NOT NULL DEFAULT true,
    applies_to_assessor              boolean NOT NULL DEFAULT true,
    workflow_state                   varchar NOT NULL DEFAULT 'active',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    CONSTRAINT chk_workflow_state_enum_5 CHECK (workflow_state IN ('active', 'deleted'))
);

CREATE TABLE assessment_question_bank_users (
    id                               BIGSERIAL PRIMARY KEY,
    assessment_question_bank_id      bigint NOT NULL,
    user_id                          bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE assessment_question_banks (
    id                               BIGSERIAL PRIMARY KEY,
    context_id                       bigint,
    context_type                     varchar(255),
    title                            text,
    workflow_state                   varchar(255) NOT NULL,
    deleted_at                       timestamp,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    migration_id                     varchar(255),
    root_account_id                  bigint
);

CREATE TABLE assessment_questions (
    id                               BIGSERIAL PRIMARY KEY,
    name                             text,
    question_data                    text,
    context_id                       bigint,
    context_type                     varchar(255),
    workflow_state                   varchar(255),
    created_at                       timestamp,
    updated_at                       timestamp,
    assessment_question_bank_id      bigint,
    deleted_at                       timestamp,
    migration_id                     varchar(255),
    position                         integer,
    root_account_id                  bigint
);

CREATE TABLE asset_user_accesses (
    id                               BIGSERIAL PRIMARY KEY,
    asset_code                       varchar(255),
    asset_group_code                 varchar(255),
    user_id                          bigint,
    context_id                       bigint,
    context_type                     varchar(255),
    last_access                      timestamp,
    created_at                       timestamp,
    updated_at                       timestamp,
    asset_category                   varchar(255),
    view_score                       double precision,
    participate_score                double precision,
    action_level                     varchar(255),
    display_name                     text,
    membership_type                  varchar(255),
    root_account_id                  bigint NOT NULL
);

CREATE TABLE assignment_configuration_tool_lookups (
    id                               BIGSERIAL PRIMARY KEY,
    assignment_id                    bigint NOT NULL,
    tool_id                          bigint,
    tool_type                        varchar(255) NOT NULL,
    subscription_id                  varchar,
    tool_product_code                varchar,
    tool_vendor_code                 varchar,
    tool_resource_type_code          varchar,
    context_type                     varchar NOT NULL DEFAULT 'Account'
);

CREATE TABLE assignment_override_students (
    id                               BIGSERIAL PRIMARY KEY,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    assignment_id                    bigint,
    assignment_override_id           bigint NOT NULL,
    user_id                          bigint NOT NULL,
    quiz_id                          bigint,
    workflow_state                   varchar NOT NULL DEFAULT 'active',
    root_account_id                  bigint,
    context_module_id                bigint,
    wiki_page_id                     bigint,
    discussion_topic_id              bigint,
    attachment_id                    bigint
);

CREATE TABLE attachment_associations (
    id                               BIGSERIAL PRIMARY KEY,
    attachment_id                    bigint,
    context_id                       bigint,
    context_type                     varchar(255),
    root_account_id                  bigint,
    created_at                       timestamp NOT NULL DEFAULT now(),
    updated_at                       timestamp NOT NULL DEFAULT now(),
    user_id                          bigint,
    context_concern                  varchar(255)
);

CREATE TABLE attachment_upload_statuses (
    id                               BIGSERIAL PRIMARY KEY,
    attachment_id                    bigint NOT NULL,
    error                            text NOT NULL,
    created_at                       timestamp NOT NULL
);

CREATE TABLE attachments (
    id                               BIGSERIAL PRIMARY KEY,
    context_id                       bigint,
    context_type                     varchar(255),
    size                             bigint,
    folder_id                        bigint,
    content_type                     varchar(255),
    filename                         text,
    uuid                             varchar(255),
    display_name                     text,
    created_at                       timestamp,
    updated_at                       timestamp,
    workflow_state                   varchar(255),
    user_id                          bigint,
    locked                           boolean DEFAULT false,
    file_state                       varchar(255),
    deleted_at                       timestamp,
    position                         integer,
    lock_at                          timestamp,
    unlock_at                        timestamp,
    could_be_locked                  boolean,
    root_attachment_id               bigint,
    cloned_item_id                   bigint,
    migration_id                     varchar(255),
    namespace                        varchar(255),
    media_entry_id                   varchar(255),
    md5                              varchar(255),
    encoding                         varchar(255),
    need_notify                      boolean,
    upload_error_message             text,
    replacement_attachment_id        bigint,
    usage_rights_id                  bigint,
    modified_at                      timestamp,
    viewed_at                        timestamp,
    instfs_uuid                      varchar,
    root_account_id                  bigint,
    category                         varchar NOT NULL DEFAULT 'uncategorized',
    word_count                       integer,
    visibility_level                 varchar(32) NOT NULL DEFAULT 'inherit',
    only_visible_to_overrides        boolean NOT NULL DEFAULT false
);

CREATE TABLE aua_logs_0 (
    id                               BIGSERIAL PRIMARY KEY,
    asset_user_access_id             bigint NOT NULL,
    created_at                       timestamp NOT NULL
);

CREATE TABLE aua_logs_1 (
    id                               BIGSERIAL PRIMARY KEY,
    asset_user_access_id             bigint NOT NULL,
    created_at                       timestamp NOT NULL
);

CREATE TABLE aua_logs_2 (
    id                               BIGSERIAL PRIMARY KEY,
    asset_user_access_id             bigint NOT NULL,
    created_at                       timestamp NOT NULL
);

CREATE TABLE aua_logs_3 (
    id                               BIGSERIAL PRIMARY KEY,
    asset_user_access_id             bigint NOT NULL,
    created_at                       timestamp NOT NULL
);

CREATE TABLE aua_logs_4 (
    id                               BIGSERIAL PRIMARY KEY,
    asset_user_access_id             bigint NOT NULL,
    created_at                       timestamp NOT NULL
);

CREATE TABLE aua_logs_5 (
    id                               BIGSERIAL PRIMARY KEY,
    asset_user_access_id             bigint NOT NULL,
    created_at                       timestamp NOT NULL
);

CREATE TABLE aua_logs_6 (
    id                               BIGSERIAL PRIMARY KEY,
    asset_user_access_id             bigint NOT NULL,
    created_at                       timestamp NOT NULL
);

CREATE TABLE auditor_account_user_records (
    id                               BIGSERIAL PRIMARY KEY,
    performing_user_id               bigint,
    root_account_id                  bigint NOT NULL,
    account_user_id                  bigint NOT NULL,
    action                           varchar(64) NOT NULL,
    hostname                         varchar(255) NOT NULL,
    uuid                             varchar(255) NOT NULL,
    event_type                       varchar(64) NOT NULL,
    request_id                       varchar(255),
    created_at                       timestamp NOT NULL
);

CREATE TABLE auditor_authentication_records (
    id                               BIGSERIAL PRIMARY KEY,
    uuid                             varchar NOT NULL,
    account_id                       bigint NOT NULL,
    event_type                       varchar NOT NULL,
    pseudonym_id                     bigint NOT NULL,
    request_id                       varchar NOT NULL,
    user_id                          bigint NOT NULL,
    created_at                       timestamp NOT NULL
);

CREATE TABLE auditor_course_records (
    id                               BIGSERIAL PRIMARY KEY,
    uuid                             varchar NOT NULL,
    account_id                       bigint NOT NULL,
    course_id                        bigint NOT NULL,
    data                             text,
    event_source                     varchar NOT NULL,
    event_type                       varchar NOT NULL,
    request_id                       varchar NOT NULL,
    sis_batch_id                     bigint,
    user_id                          bigint,
    created_at                       timestamp NOT NULL
);

CREATE TABLE auditor_feature_flag_records (
    id                               BIGSERIAL PRIMARY KEY,
    uuid                             varchar NOT NULL,
    feature_flag_id                  bigint NOT NULL,
    root_account_id                  bigint NOT NULL,
    context_id                       bigint,
    context_type                     varchar,
    feature_name                     varchar,
    event_type                       varchar NOT NULL,
    state_before                     varchar NOT NULL,
    state_after                      varchar NOT NULL,
    request_id                       varchar NOT NULL,
    user_id                          bigint,
    created_at                       timestamp NOT NULL
);

CREATE TABLE auditor_grade_change_records (
    id                               BIGSERIAL PRIMARY KEY,
    uuid                             varchar NOT NULL,
    account_id                       bigint NOT NULL,
    root_account_id                  bigint NOT NULL,
    assignment_id                    bigint,
    context_id                       bigint NOT NULL,
    context_type                     varchar NOT NULL,
    event_type                       varchar NOT NULL,
    excused_after                    boolean NOT NULL,
    excused_before                   boolean NOT NULL,
    grade_after                      varchar,
    grade_before                     varchar,
    graded_anonymously               boolean,
    grader_id                        bigint,
    points_possible_after            double precision,
    points_possible_before           double precision,
    request_id                       varchar NOT NULL,
    score_after                      double precision,
    score_before                     double precision,
    student_id                       bigint NOT NULL,
    submission_id                    bigint,
    submission_version_number        integer,
    created_at                       timestamp NOT NULL,
    grading_period_id                bigint
);

CREATE TABLE auditor_pseudonym_records (
    id                               BIGSERIAL PRIMARY KEY,
    pseudonym_id                     bigint NOT NULL,
    root_account_id                  bigint NOT NULL,
    performing_user_id               bigint NOT NULL,
    action                           varchar NOT NULL,
    hostname                         varchar NOT NULL,
    pid                              varchar NOT NULL,
    uuid                             varchar NOT NULL,
    event_type                       varchar NOT NULL,
    request_id                       varchar,
    created_at                       timestamp NOT NULL
);

CREATE TABLE auto_grade_results (
    id                               BIGSERIAL PRIMARY KEY,
    submission_id                    bigint NOT NULL,
    root_account_id                  bigint NOT NULL,
    attempt                          integer NOT NULL,
    grade_data                       jsonb NOT NULL,
    error_message                    varchar(255),
    grading_attempts                 integer NOT NULL DEFAULT 0,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE block_editors (
    id                               BIGSERIAL PRIMARY KEY,
    root_account_id                  bigint NOT NULL,
    context_id                       bigint NOT NULL,
    context_type                     varchar NOT NULL,
    time                             bigint,
    blocks                           jsonb NOT NULL,
    editor_version                   varchar,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE bookmarks_bookmarks (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint NOT NULL,
    name                             text NOT NULL,
    url                              text NOT NULL,
    position                         integer,
    json                             text
);

CREATE TABLE brand_configs (
    md5                              varchar(32) NOT NULL,
    variables                        text,
    share                            boolean NOT NULL DEFAULT false,
    name                             varchar(255),
    created_at                       timestamp NOT NULL,
    js_overrides                     text,
    css_overrides                    text,
    mobile_js_overrides              text,
    mobile_css_overrides             text,
    parent_md5                       varchar(255),
    PRIMARY KEY (md5)
);

CREATE TABLE canvadocs (
    id                               BIGSERIAL PRIMARY KEY,
    document_id                      varchar(255),
    process_state                    varchar(255),
    attachment_id                    bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    has_annotations                  boolean
);

CREATE TABLE canvadocs_annotation_contexts (
    id                               BIGSERIAL PRIMARY KEY,
    attachment_id                    bigint NOT NULL,
    submission_id                    bigint NOT NULL,
    root_account_id                  bigint NOT NULL,
    launch_id                        varchar NOT NULL,
    submission_attempt               integer,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE canvadocs_submissions (
    id                               BIGSERIAL PRIMARY KEY,
    canvadoc_id                      bigint,
    submission_id                    bigint NOT NULL
);

CREATE TABLE canvas_metadata (
    id                               BIGSERIAL PRIMARY KEY,
    key                              varchar NOT NULL,
    payload                          jsonb NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE cloned_items (
    id                               BIGSERIAL PRIMARY KEY,
    original_item_id                 bigint,
    original_item_type               varchar(255),
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE collaborations (
    id                               BIGSERIAL PRIMARY KEY,
    collaboration_type               varchar(255),
    document_id                      varchar(255),
    user_id                          bigint,
    context_id                       bigint,
    context_type                     varchar(255),
    url                              varchar(255),
    uuid                             varchar(255),
    data                             text,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    description                      text,
    title                            varchar(255) NOT NULL,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    deleted_at                       timestamp,
    context_code                     varchar(255),
    type                             varchar(255),
    resource_link_lookup_uuid        uuid,
    root_account_id                  bigint
);

CREATE TABLE collaborators (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint,
    collaboration_id                 bigint,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    authorized_service_user_id       varchar(255),
    group_id                         bigint
);

CREATE TABLE comment_bank_items (
    id                               BIGSERIAL PRIMARY KEY,
    course_id                        bigint NOT NULL,
    root_account_id                  bigint NOT NULL,
    user_id                          bigint NOT NULL,
    comment                          text NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    workflow_state                   varchar NOT NULL DEFAULT 'active',
    assignment_id                    bigint
);

CREATE TABLE conditional_release_assignment_set_actions (
    id                               BIGSERIAL PRIMARY KEY,
    action                           varchar NOT NULL,
    source                           varchar NOT NULL,
    student_id                       bigint NOT NULL,
    actor_id                         bigint NOT NULL,
    assignment_set_id                bigint,
    deleted_at                       timestamp,
    root_account_id                  bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE conditional_release_assignment_set_associations (
    id                               BIGSERIAL PRIMARY KEY,
    assignment_set_id                bigint,
    assignment_id                    bigint,
    position                         integer,
    deleted_at                       timestamp,
    root_account_id                  bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE conditional_release_assignment_sets (
    id                               BIGSERIAL PRIMARY KEY,
    scoring_range_id                 bigint NOT NULL,
    position                         integer,
    deleted_at                       timestamp,
    root_account_id                  bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE conditional_release_rules (
    id                               BIGSERIAL PRIMARY KEY,
    course_id                        bigint,
    trigger_assignment_id            bigint,
    deleted_at                       timestamp,
    root_account_id                  bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE conditional_release_scoring_ranges (
    id                               BIGSERIAL PRIMARY KEY,
    rule_id                          bigint NOT NULL,
    lower_bound                      numeric,
    upper_bound                      numeric,
    position                         integer,
    deleted_at                       timestamp,
    root_account_id                  bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE content_participation_counts (
    id                               BIGSERIAL PRIMARY KEY,
    content_type                     varchar(255),
    context_id                       bigint,
    context_type                     varchar(255),
    user_id                          bigint,
    unread_count                     integer DEFAULT 0,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    root_account_id                  bigint
);

CREATE TABLE content_participations (
    id                               BIGSERIAL PRIMARY KEY,
    content_id                       bigint NOT NULL,
    content_type                     varchar(255) NOT NULL,
    user_id                          bigint NOT NULL,
    workflow_state                   varchar(255) NOT NULL,
    root_account_id                  bigint,
    content_item                     varchar NOT NULL DEFAULT 'grade',
    created_at                       timestamp NOT NULL DEFAULT now(),
    updated_at                       timestamp NOT NULL DEFAULT now()
);

CREATE TABLE content_tags (
    id                               BIGSERIAL PRIMARY KEY,
    content_id                       bigint,
    content_type                     varchar(255),
    context_id                       bigint NOT NULL,
    context_type                     varchar(255) NOT NULL,
    title                            text,
    tag                              varchar(255),
    url                              text,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    comments                         text,
    tag_type                         varchar(255) DEFAULT 'default',
    context_module_id                bigint,
    position                         integer,
    indent                           integer,
    migration_id                     varchar(255),
    learning_outcome_id              bigint,
    context_code                     varchar(255),
    mastery_score                    double precision,
    rubric_association_id            bigint,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    cloned_item_id                   bigint,
    associated_asset_id              bigint,
    associated_asset_type            varchar(255),
    new_tab                          boolean,
    link_settings                    jsonb,
    root_account_id                  bigint NOT NULL,
    external_data                    jsonb
);

CREATE TABLE context_external_tool_placements (
    id                               BIGSERIAL PRIMARY KEY,
    placement_type                   varchar(255),
    context_external_tool_id         bigint NOT NULL
);

CREATE TABLE context_external_tools (
    id                               BIGSERIAL PRIMARY KEY,
    context_id                       bigint,
    context_type                     varchar(255),
    domain                           varchar(255),
    url                              varchar(4096),
    shared_secret                    text NOT NULL,
    consumer_key                     text NOT NULL,
    name                             varchar(255) NOT NULL,
    description                      text,
    settings                         text,
    workflow_state                   varchar(255) NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    migration_id                     varchar(255),
    cloned_item_id                   bigint,
    tool_id                          varchar(255),
    not_selectable                   boolean NOT NULL DEFAULT false,
    app_center_id                    varchar(255),
    allow_membership_service_access  boolean NOT NULL DEFAULT false,
    developer_key_id                 bigint,
    root_account_id                  bigint NOT NULL,
    is_rce_favorite                  boolean NOT NULL DEFAULT false,
    identity_hash                    varchar(64),
    lti_version                      text NOT NULL DEFAULT '1.1',
    unified_tool_id                  varchar(255),
    unified_tool_id_needs_update     boolean NOT NULL DEFAULT false,
    unified_tool_id_last_updated_at  timestamp,
    lti_registration_id              bigint,
    asset_processor_eula_required    boolean,
    app_id                           bigint
);

CREATE TABLE context_module_progressions (
    id                               BIGSERIAL PRIMARY KEY,
    context_module_id                bigint,
    user_id                          bigint,
    requirements_met                 text,
    workflow_state                   varchar(255) NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    collapsed                        boolean,
    current_position                 integer,
    completed_at                     timestamp,
    current                          boolean,
    lock_version                     integer NOT NULL DEFAULT 0,
    evaluated_at                     timestamp,
    incomplete_requirements          text,
    root_account_id                  bigint
);

CREATE TABLE context_modules (
    id                               BIGSERIAL PRIMARY KEY,
    context_id                       bigint NOT NULL,
    context_type                     varchar(255) NOT NULL,
    name                             text,
    position                         integer,
    prerequisites                    text,
    completion_requirements          text,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    deleted_at                       timestamp,
    unlock_at                        timestamp,
    migration_id                     varchar(255),
    require_sequential_progress      boolean,
    cloned_item_id                   bigint,
    completion_events                text,
    requirement_count                integer,
    root_account_id                  bigint
);

CREATE TABLE conversation_message_participants (
    id                               BIGSERIAL PRIMARY KEY,
    conversation_message_id          bigint,
    conversation_participant_id      bigint,
    tags                             text,
    user_id                          bigint,
    workflow_state                   varchar(255),
    deleted_at                       timestamp,
    root_account_ids                 text,
    created_at                       timestamp NOT NULL DEFAULT now(),
    updated_at                       timestamp NOT NULL DEFAULT now()
);

CREATE TABLE course_account_associations (
    id                               BIGSERIAL PRIMARY KEY,
    course_id                        bigint NOT NULL,
    account_id                       bigint NOT NULL,
    depth                            integer NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    course_section_id                bigint,
    root_account_id                  bigint NOT NULL
);

CREATE TABLE course_pace_module_items (
    id                               BIGSERIAL PRIMARY KEY,
    course_pace_id                   bigint,
    duration                         integer NOT NULL DEFAULT 0,
    module_item_id                   bigint,
    root_account_id                  bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    migration_id                     varchar
);

CREATE TABLE course_paces (
    id                               BIGSERIAL PRIMARY KEY,
    course_id                        bigint NOT NULL,
    course_section_id                bigint,
    user_id                          bigint,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'unpublished',
    end_date                         date,
    exclude_weekends                 boolean NOT NULL DEFAULT true,
    hard_end_dates                   boolean NOT NULL DEFAULT false,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    published_at                     timestamp,
    root_account_id                  bigint NOT NULL,
    migration_id                     varchar,
    selected_days_to_skip            varchar[] NOT NULL DEFAULT '{}',
    assignments_weighting            text,
    time_to_complete_calendar_days   integer
);

CREATE TABLE course_reports (
    id                               BIGSERIAL PRIMARY KEY,
    root_account_id                  bigint NOT NULL,
    course_id                        bigint NOT NULL,
    user_id                          bigint NOT NULL,
    attachment_id                    bigint,
    workflow_state                   varchar NOT NULL DEFAULT 'created',
    report_type                      varchar NOT NULL,
    parameters                       text,
    message                          text,
    job_ids                          bigint[] NOT NULL DEFAULT '{}',
    start_at                         timestamp,
    end_at                           timestamp,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE course_score_statistics (
    id                               BIGSERIAL PRIMARY KEY,
    course_id                        bigint NOT NULL,
    average                          numeric(8,2) NOT NULL,
    score_count                      integer NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE custom_data (
    id                               BIGSERIAL PRIMARY KEY,
    namespace                        varchar(255),
    user_id                          bigint,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    data_json                        jsonb NOT NULL
);

CREATE TABLE custom_grade_statuses (
    id                               BIGSERIAL PRIMARY KEY,
    color                            varchar(7) NOT NULL,
    name                             varchar(14) NOT NULL,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    root_account_id                  bigint NOT NULL,
    created_by_id                    bigint NOT NULL,
    deleted_by_id                    bigint,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    applies_to_submissions           boolean NOT NULL DEFAULT true,
    applies_to_final_grade           boolean NOT NULL DEFAULT true,
    allow_final_grade_value          boolean NOT NULL DEFAULT true,
    icon                             varchar(10)
);

CREATE TABLE custom_gradebook_column_data (
    id                               BIGSERIAL PRIMARY KEY,
    content                          varchar(255) NOT NULL,
    user_id                          bigint NOT NULL,
    custom_gradebook_column_id       bigint NOT NULL,
    root_account_id                  bigint,
    created_at                       timestamp NOT NULL DEFAULT now(),
    updated_at                       timestamp NOT NULL DEFAULT now()
);
