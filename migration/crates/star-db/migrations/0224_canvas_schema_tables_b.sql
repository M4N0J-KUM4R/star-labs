--
-- Canvas schema parity: tables b.
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

CREATE TABLE custom_gradebook_columns (
    id                               BIGSERIAL PRIMARY KEY,
    title                            varchar(255) NOT NULL,
    position                         integer NOT NULL,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    course_id                        bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    teacher_notes                    boolean NOT NULL DEFAULT false,
    read_only                        boolean NOT NULL DEFAULT false,
    root_account_id                  bigint
);

CREATE TABLE delayed_jobs (
    id                               BIGSERIAL PRIMARY KEY,
    priority                         integer DEFAULT 0,
    attempts                         integer DEFAULT 0,
    handler                          text,
    last_error                       text,
    queue                            varchar(255) NOT NULL,
    run_at                           timestamp NOT NULL,
    locked_at                        timestamp,
    failed_at                        timestamp,
    locked_by                        varchar(255),
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    tag                              varchar(255),
    max_attempts                     integer,
    strand                           varchar(255),
    next_in_strand                   boolean NOT NULL DEFAULT true,
    shard_id                         bigint,
    source                           varchar(255),
    max_concurrent                   integer NOT NULL DEFAULT 1,
    expires_at                       timestamp,
    strand_order_override            integer NOT NULL DEFAULT 0,
    singleton                        varchar
);

CREATE TABLE delayed_messages (
    id                               BIGSERIAL PRIMARY KEY,
    notification_id                  bigint,
    notification_policy_id           bigint,
    context_id                       bigint,
    context_type                     varchar(255),
    communication_channel_id         bigint,
    frequency                        varchar(255),
    workflow_state                   varchar(255),
    batched_at                       timestamp,
    created_at                       timestamp,
    updated_at                       timestamp,
    send_at                          timestamp,
    link                             text,
    name_of_topic                    text,
    summary                          text,
    root_account_id                  bigint,
    notification_policy_override_id  bigint
);

CREATE TABLE delayed_notifications (
    id                               BIGSERIAL PRIMARY KEY,
    notification_id                  bigint NOT NULL,
    asset_id                         bigint NOT NULL,
    asset_type                       varchar(255) NOT NULL,
    recipient_keys                   text,
    workflow_state                   varchar(255) NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE developer_key_account_bindings (
    id                               BIGSERIAL PRIMARY KEY,
    account_id                       bigint NOT NULL,
    developer_key_id                 bigint NOT NULL,
    workflow_state                   varchar NOT NULL DEFAULT 'off',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    root_account_id                  bigint NOT NULL
);

CREATE TABLE discussion_entry_participants (
    id                               BIGSERIAL PRIMARY KEY,
    discussion_entry_id              bigint NOT NULL,
    user_id                          bigint NOT NULL,
    workflow_state                   varchar(255) NOT NULL,
    forced_read_state                boolean,
    rating                           integer,
    root_account_id                  bigint NOT NULL,
    report_type                      varchar(255),
    read_at                          timestamp,
    created_at                       timestamp NOT NULL DEFAULT now(),
    updated_at                       timestamp NOT NULL DEFAULT now()
);

CREATE TABLE discussion_entry_versions (
    id                               BIGSERIAL PRIMARY KEY,
    discussion_entry_id              bigint NOT NULL,
    root_account_id                  bigint NOT NULL,
    user_id                          bigint,
    version                          bigint,
    message                          text,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE discussion_topic_insight_entries (
    id                               BIGSERIAL PRIMARY KEY,
    discussion_topic_insight_id      bigint NOT NULL,
    discussion_topic_id              bigint NOT NULL,
    discussion_entry_id              bigint NOT NULL,
    discussion_entry_version_id      bigint NOT NULL,
    root_account_id                  bigint NOT NULL,
    locale                           varchar(255) NOT NULL,
    dynamic_content_hash             varchar(64) NOT NULL,
    ai_evaluation                    jsonb NOT NULL,
    ai_evaluation_human_reviewer_id  bigint,
    ai_evaluation_human_feedback_liked boolean NOT NULL DEFAULT false,
    ai_evaluation_human_feedback_disliked boolean NOT NULL DEFAULT false,
    ai_evaluation_human_feedback_notes varchar(1024) NOT NULL DEFAULT '',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE discussion_topic_insights (
    id                               BIGSERIAL PRIMARY KEY,
    discussion_topic_id              bigint NOT NULL,
    user_id                          bigint NOT NULL,
    root_account_id                  bigint NOT NULL,
    workflow_state                   varchar NOT NULL DEFAULT 'created',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE discussion_topic_materialized_views (
    discussion_topic_id              bigint NOT NULL,
    json_structure                   text,
    participants_array               text,
    entry_ids_array                  text,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    generation_started_at            timestamp,
    PRIMARY KEY (discussion_topic_id)
);

CREATE TABLE discussion_topic_participants (
    id                               BIGSERIAL PRIMARY KEY,
    discussion_topic_id              bigint NOT NULL,
    user_id                          bigint NOT NULL,
    unread_entry_count               integer NOT NULL DEFAULT 0,
    workflow_state                   varchar(255) NOT NULL,
    subscribed                       boolean,
    root_account_id                  bigint NOT NULL,
    created_at                       timestamp NOT NULL DEFAULT now(),
    updated_at                       timestamp NOT NULL DEFAULT now(),
    sort_order                       varchar NOT NULL DEFAULT 'inherit',
    expanded                         boolean,
    summary_enabled                  boolean NOT NULL DEFAULT false,
    preferred_language               varchar,
    show_pinned_entries              boolean NOT NULL DEFAULT true,
    has_unread_pinned_entry          boolean NOT NULL DEFAULT false
);

CREATE TABLE discussion_topic_section_visibilities (
    id                               BIGSERIAL PRIMARY KEY,
    discussion_topic_id              bigint NOT NULL,
    course_section_id                bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    workflow_state                   varchar(255) NOT NULL
);

CREATE TABLE enrollment_dates_overrides (
    id                               BIGSERIAL PRIMARY KEY,
    enrollment_term_id               bigint,
    enrollment_type                  varchar(255),
    context_id                       bigint NOT NULL,
    context_type                     varchar(255),
    start_at                         timestamp,
    end_at                           timestamp,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    root_account_id                  bigint,
    stuck_sis_fields                 text
);

CREATE TABLE enrollment_states (
    enrollment_id                    bigint NOT NULL,
    state                            varchar(255),
    state_is_current                 boolean NOT NULL DEFAULT false,
    state_started_at                 timestamp,
    state_valid_until                timestamp,
    restricted_access                boolean NOT NULL DEFAULT false,
    access_is_current                boolean NOT NULL DEFAULT false,
    lock_version                     integer NOT NULL DEFAULT 0,
    updated_at                       timestamp,
    root_account_id                  bigint NOT NULL,
    PRIMARY KEY (enrollment_id)
);

CREATE TABLE enrollment_terms (
    id                               BIGSERIAL PRIMARY KEY,
    root_account_id                  bigint NOT NULL,
    name                             varchar(255),
    term_code                        varchar(255),
    sis_source_id                    varchar(255),
    sis_batch_id                     bigint,
    start_at                         timestamp,
    end_at                           timestamp,
    accepting_enrollments            boolean,
    can_manually_enroll              boolean,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    stuck_sis_fields                 text,
    integration_id                   varchar(255),
    grading_period_group_id          bigint
);

CREATE TABLE eportfolio_categories (
    id                               BIGSERIAL PRIMARY KEY,
    eportfolio_id                    bigint NOT NULL,
    name                             varchar(255),
    position                         integer,
    slug                             varchar(255),
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE eportfolio_entries (
    id                               BIGSERIAL PRIMARY KEY,
    eportfolio_id                    bigint NOT NULL,
    eportfolio_category_id           bigint NOT NULL,
    position                         integer,
    name                             varchar(255),
    allow_comments                   boolean,
    show_comments                    boolean,
    slug                             varchar(255),
    content                          text,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE eportfolios (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint NOT NULL,
    name                             varchar(255),
    public                           boolean,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    uuid                             varchar(255),
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    deleted_at                       timestamp,
    spam_status                      varchar
);

CREATE TABLE estimated_durations (
    id                               BIGSERIAL PRIMARY KEY,
    duration                         interval,
    discussion_topic_id              bigint NOT NULL,
    assignment_id                    bigint NOT NULL,
    attachment_id                    bigint NOT NULL,
    quiz_id                          bigint NOT NULL,
    wiki_page_id                     bigint NOT NULL,
    content_tag_id                   bigint NOT NULL,
    root_account_id                  bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    external_tool_id                 bigint,
    CONSTRAINT chk_require_context CHECK (((assignment_id IS NOT NULL)::int + (attachment_id IS NOT NULL)::int + (content_tag_id IS NOT NULL)::int + (discussion_topic_id IS NOT NULL)::int + (wiki_page_id IS NOT NULL)::int + (quiz_id IS NOT NULL)::int + (external_tool_id IS NOT NULL)::int) = 1)
);

CREATE TABLE event_stream_failures (
    id                               BIGSERIAL PRIMARY KEY,
    operation                        varchar(255) NOT NULL,
    event_stream                     varchar(255) NOT NULL,
    record_id                        varchar(255) NOT NULL,
    payload                          text NOT NULL,
    exception                        text,
    backtrace                        text,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE external_content_references (
    id                               BIGSERIAL PRIMARY KEY,
    root_account_id                  bigint NOT NULL,
    wiki_page_id                     bigint NOT NULL,
    content_id                       varchar(255) NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    CONSTRAINT chk_require_context_2 CHECK (((wiki_page_id IS NOT NULL)::int) = 1)
);

CREATE TABLE external_feed_entries (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint,
    external_feed_id                 bigint NOT NULL,
    title                            text,
    message                          text,
    source_name                      varchar(255),
    source_url                       text,
    posted_at                        timestamp,
    workflow_state                   varchar(255) NOT NULL,
    url                              text,
    asset_id                         bigint,
    asset_type                       varchar(255),
    uuid                             varchar(255),
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE external_feeds (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint,
    context_id                       bigint NOT NULL,
    context_type                     varchar(255) NOT NULL,
    consecutive_failures             integer,
    failures                         integer,
    refresh_at                       timestamp,
    title                            varchar(255),
    url                              varchar(255) NOT NULL,
    header_match                     varchar(255),
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    verbosity                        varchar(255),
    migration_id                     varchar(255)
);

CREATE TABLE external_integration_keys (
    id                               BIGSERIAL PRIMARY KEY,
    context_id                       bigint NOT NULL,
    context_type                     varchar(255) NOT NULL,
    key_value                        varchar(255) NOT NULL,
    key_type                         varchar(255) NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE failed_jobs (
    id                               BIGSERIAL PRIMARY KEY,
    priority                         integer DEFAULT 0,
    attempts                         integer DEFAULT 0,
    handler                          text,
    last_error                       text,
    queue                            varchar(255),
    run_at                           timestamp,
    locked_at                        timestamp,
    failed_at                        timestamp,
    locked_by                        varchar(255),
    created_at                       timestamp,
    updated_at                       timestamp,
    tag                              varchar(255),
    max_attempts                     integer,
    strand                           varchar(255),
    shard_id                         bigint,
    original_job_id                  bigint,
    source                           varchar(255),
    expires_at                       timestamp,
    strand_order_override            integer NOT NULL DEFAULT 0,
    singleton                        varchar,
    requeued_job_id                  bigint
);

CREATE TABLE gradebook_csvs (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint NOT NULL,
    attachment_id                    bigint NOT NULL,
    progress_id                      bigint NOT NULL,
    course_id                        bigint NOT NULL
);

CREATE TABLE gradebook_filters (
    id                               BIGSERIAL PRIMARY KEY,
    course_id                        bigint NOT NULL,
    user_id                          bigint NOT NULL,
    name                             varchar(255) NOT NULL,
    payload                          jsonb NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE gradebook_uploads (
    id                               BIGSERIAL PRIMARY KEY,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    course_id                        bigint NOT NULL,
    user_id                          bigint NOT NULL,
    progress_id                      bigint NOT NULL,
    gradebook                        text
);

CREATE TABLE group_and_membership_importers (
    id                               BIGSERIAL PRIMARY KEY,
    group_category_id                bigint,
    attachment_id                    bigint,
    workflow_state                   varchar NOT NULL DEFAULT 'active',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    course_id                        bigint
);

CREATE TABLE ignores (
    id                               BIGSERIAL PRIMARY KEY,
    asset_id                         bigint NOT NULL,
    asset_type                       varchar(255) NOT NULL,
    user_id                          bigint NOT NULL,
    purpose                          varchar(255) NOT NULL,
    permanent                        boolean NOT NULL DEFAULT false,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE inbox_settings (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          varchar NOT NULL,
    use_signature                    boolean NOT NULL DEFAULT false,
    signature                        varchar(255),
    use_out_of_office                boolean NOT NULL DEFAULT false,
    out_of_office_first_date         timestamp,
    out_of_office_last_date          timestamp,
    out_of_office_subject            varchar(255),
    out_of_office_message            varchar(255),
    root_account_id                  bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE institutional_tag_associations (
    id                               BIGSERIAL PRIMARY KEY,
    institutional_tag_id             bigint NOT NULL,
    user_id                          bigint NOT NULL,
    course_id                        bigint NOT NULL,
    root_account_id                  bigint NOT NULL,
    sis_source_id                    varchar(255),
    sis_batch_id                     bigint,
    stuck_sis_fields                 varchar(255),
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    CONSTRAINT chk_require_context_3 CHECK (((user_id IS NOT NULL)::int + (course_id IS NOT NULL)::int) = 1),
    CONSTRAINT chk_workflow_state_enum_6 CHECK (workflow_state IN ('active', 'deleted'))
);

CREATE TABLE institutional_tag_categories (
    id                               BIGSERIAL PRIMARY KEY,
    name                             varchar(255) NOT NULL,
    description                      text,
    account_id                       bigint NOT NULL,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    sis_source_id                    varchar(255),
    sis_batch_id                     bigint,
    stuck_sis_fields                 varchar(255),
    root_account_id                  bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    CONSTRAINT chk_workflow_state_enum_7 CHECK (workflow_state IN ('active', 'deleted'))
);

CREATE TABLE institutional_tags (
    id                               BIGSERIAL PRIMARY KEY,
    name                             varchar(255) NOT NULL,
    description                      text NOT NULL,
    sis_source_id                    varchar(255),
    category_id                      bigint NOT NULL,
    sis_batch_id                     bigint,
    stuck_sis_fields                 varchar(255),
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    root_account_id                  bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    CONSTRAINT chk_workflow_state_enum_8 CHECK (workflow_state IN ('active', 'deleted'))
);

CREATE TABLE late_policies (
    id                               BIGSERIAL PRIMARY KEY,
    course_id                        bigint NOT NULL,
    missing_submission_deduction_enabled boolean NOT NULL DEFAULT false,
    missing_submission_deduction     numeric(5,2) NOT NULL DEFAULT 100,
    late_submission_deduction_enabled boolean NOT NULL DEFAULT false,
    late_submission_deduction        numeric(5,2) NOT NULL DEFAULT 0,
    late_submission_interval         varchar(16) NOT NULL DEFAULT 'day',
    late_submission_minimum_percent_enabled boolean NOT NULL DEFAULT false,
    late_submission_minimum_percent  numeric(5,2) NOT NULL DEFAULT 0,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    root_account_id                  bigint
);

CREATE TABLE learning_outcome_question_results (
    id                               BIGSERIAL PRIMARY KEY,
    learning_outcome_result_id       bigint,
    learning_outcome_id              bigint,
    associated_asset_id              bigint,
    associated_asset_type            varchar(255),
    score                            double precision,
    possible                         double precision,
    mastery                          boolean,
    percent                          double precision,
    attempt                          integer,
    title                            text,
    original_score                   double precision,
    original_possible                double precision,
    original_mastery                 boolean,
    assessed_at                      timestamp,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    submitted_at                     timestamp,
    root_account_id                  bigint
);

CREATE TABLE live_assessments_assessments (
    id                               BIGSERIAL PRIMARY KEY,
    key                              varchar(255) NOT NULL,
    title                            varchar(255) NOT NULL,
    context_id                       bigint NOT NULL,
    context_type                     varchar(255) NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE live_assessments_results (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint NOT NULL,
    assessor_id                      bigint NOT NULL,
    assessment_id                    bigint NOT NULL,
    passed                           boolean NOT NULL,
    assessed_at                      timestamp NOT NULL
);

CREATE TABLE live_assessments_submissions (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint NOT NULL,
    assessment_id                    bigint NOT NULL,
    possible                         double precision,
    score                            double precision,
    assessed_at                      timestamp,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE llm_responses (
    id                               BIGSERIAL PRIMARY KEY,
    associated_assignment_id         bigint NOT NULL,
    user_id                          bigint NOT NULL,
    prompt_name                      varchar(255) NOT NULL,
    prompt_model_id                  varchar(255) NOT NULL,
    prompt_dynamic_content           jsonb,
    raw_response                     text NOT NULL,
    input_tokens                     integer NOT NULL,
    output_tokens                    integer NOT NULL,
    response_time                    double precision NOT NULL,
    root_account_id                  bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE lti_asset_processor_eula_acceptances (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint NOT NULL,
    accepted                         boolean NOT NULL DEFAULT false,
    timestamp                        timestamp NOT NULL,
    context_external_tool_id         bigint NOT NULL,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    root_account_id                  bigint NOT NULL
);

CREATE TABLE lti_asset_processors (
    id                               BIGSERIAL PRIMARY KEY,
    url                              varchar(4096),
    title                            varchar(255),
    text                             varchar(255),
    custom                           jsonb,
    icon                             jsonb,
    "window"                         jsonb,
    iframe                           jsonb,
    report                           jsonb,
    assignment_id                    bigint NOT NULL,
    context_external_tool_id         bigint NOT NULL,
    workflow_state                   varchar(255) NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    root_account_id                  bigint NOT NULL,
    migration_id                     varchar(255)
);

CREATE TABLE lti_asset_reports (
    id                               BIGSERIAL PRIMARY KEY,
    report_type                      varchar NOT NULL,
    timestamp                        timestamp NOT NULL,
    title                            varchar,
    comment                          varchar,
    indication_color                 varchar(255),
    indication_alt                   varchar(255),
    processing_progress              varchar NOT NULL,
    error_code                       varchar,
    priority                         integer NOT NULL,
    extensions                       jsonb NOT NULL,
    lti_asset_id                     bigint NOT NULL,
    lti_asset_processor_id           bigint NOT NULL,
    workflow_state                   varchar(255) NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    root_account_id                  bigint NOT NULL,
    result                           varchar(255),
    visible_to_owner                 boolean NOT NULL DEFAULT false
);

CREATE TABLE lti_assets (
    id                               BIGSERIAL PRIMARY KEY,
    uuid                             uuid NOT NULL,
    attachment_id                    bigint,
    submission_id                    bigint,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    root_account_id                  bigint NOT NULL,
    sha256_checksum                  varchar(64),
    submission_attempt               integer,
    discussion_entry_version_id      bigint,
    CONSTRAINT chk_one_asset_locator_present CHECK (( (attachment_id IS NOT NULL)::int + (submission_attempt IS NOT NULL)::int + (discussion_entry_version_id IS NOT NULL)::int ) <= 1)
);

CREATE TABLE lti_context_controls (
    id                               BIGSERIAL PRIMARY KEY,
    available                        boolean NOT NULL DEFAULT true,
    path                             varchar(4096) NOT NULL,
    deployment_id                    bigint NOT NULL,
    registration_id                  bigint NOT NULL,
    account_id                       bigint NOT NULL,
    course_id                        bigint NOT NULL,
    created_by_id                    bigint,
    updated_by_id                    bigint,
    workflow_state                   varchar(48) NOT NULL DEFAULT 'active',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    root_account_id                  bigint NOT NULL,
    app_id                           bigint,
    CONSTRAINT chk_require_context_4 CHECK (((account_id IS NOT NULL)::int + (course_id IS NOT NULL)::int) = 1)
);

CREATE TABLE lti_import_histories (
    id                               BIGSERIAL PRIMARY KEY,
    target_lti_id                    varchar(255) NOT NULL,
    source_lti_id                    varchar(255) NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    root_account_id                  bigint NOT NULL
);

CREATE TABLE lti_links (
    id                               BIGSERIAL PRIMARY KEY,
    resource_link_id                 varchar NOT NULL,
    vendor_code                      varchar NOT NULL,
    product_code                     varchar NOT NULL,
    resource_type_code               varchar NOT NULL,
    linkable_id                      bigint,
    linkable_type                    varchar,
    custom_parameters                text,
    resource_url                     text,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE lti_message_handlers (
    id                               BIGSERIAL PRIMARY KEY,
    message_type                     varchar(255) NOT NULL,
    launch_path                      varchar(255) NOT NULL,
    capabilities                     text,
    parameters                       text,
    resource_handler_id              bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    tool_proxy_id                    bigint
);

CREATE TABLE lti_notice_handlers (
    id                               BIGSERIAL PRIMARY KEY,
    account_id                       bigint NOT NULL,
    notice_type                      varchar(255) NOT NULL,
    url                              varchar(4096) NOT NULL,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    root_account_id                  bigint NOT NULL,
    context_external_tool_id         bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    max_batch_size                   integer
);

CREATE TABLE lti_overlay_versions (
    id                               BIGSERIAL PRIMARY KEY,
    account_id                       bigint NOT NULL,
    lti_overlay_id                   bigint NOT NULL,
    created_by_id                    bigint,
    diff                             jsonb NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    root_account_id                  bigint NOT NULL,
    caused_by_reset                  boolean NOT NULL DEFAULT false
);

CREATE TABLE lti_overlays (
    id                               BIGSERIAL PRIMARY KEY,
    registration_id                  bigint NOT NULL,
    account_id                       bigint NOT NULL,
    updated_by_id                    bigint,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    root_account_id                  bigint NOT NULL,
    data                             jsonb NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE lti_product_families (
    id                               BIGSERIAL PRIMARY KEY,
    vendor_code                      varchar(255) NOT NULL,
    product_code                     varchar(255) NOT NULL,
    vendor_name                      varchar(255) NOT NULL,
    vendor_description               text,
    website                          varchar(255),
    vendor_email                     varchar(255),
    root_account_id                  bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    developer_key_id                 bigint
);

CREATE TABLE lti_registration_history_entries (
    id                               BIGSERIAL PRIMARY KEY,
    root_account_id                  bigint NOT NULL,
    lti_registration_id              bigint NOT NULL,
    created_by_id                    bigint,
    diff                             jsonb NOT NULL,
    update_type                      text NOT NULL,
    comment                          text,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    old_configuration                jsonb,
    new_configuration                jsonb,
    old_context_controls             jsonb,
    new_context_controls             jsonb,
    app_id                           bigint
);

CREATE TABLE lti_resource_handlers (
    id                               BIGSERIAL PRIMARY KEY,
    resource_type_code               varchar(255) NOT NULL,
    placements                       varchar(255),
    name                             varchar(255) NOT NULL,
    description                      text,
    icon_info                        text,
    tool_proxy_id                    bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE lti_resource_links (
    id                               BIGSERIAL PRIMARY KEY,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    context_external_tool_id         bigint NOT NULL,
    workflow_state                   varchar NOT NULL DEFAULT 'active',
    root_account_id                  bigint NOT NULL,
    context_id                       bigint NOT NULL,
    context_type                     varchar(255) NOT NULL,
    custom                           jsonb,
    lookup_uuid                      uuid NOT NULL,
    resource_link_uuid               uuid NOT NULL,
    url                              varchar,
    lti_1_1_id                       varchar,
    title                            varchar
);

CREATE TABLE lti_resource_placements (
    id                               BIGSERIAL PRIMARY KEY,
    placement                        varchar(255) NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    message_handler_id               bigint
);

CREATE TABLE lti_results (
    id                               BIGSERIAL PRIMARY KEY,
    result_score                     double precision,
    result_maximum                   double precision,
    comment                          text,
    activity_progress                varchar,
    grading_progress                 varchar,
    lti_line_item_id                 bigint NOT NULL,
    submission_id                    bigint,
    user_id                          bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    workflow_state                   varchar NOT NULL DEFAULT 'active',
    extensions                       jsonb,
    root_account_id                  bigint NOT NULL
);

CREATE TABLE lti_tool_configurations (
    id                               BIGSERIAL PRIMARY KEY,
    developer_key_id                 bigint,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    disabled_placements              varchar[] DEFAULT '{}',
    privacy_level                    varchar,
    unified_tool_id                  varchar(255),
    lti_registration_id              bigint,
    title                            varchar(255),
    description                      text,
    target_link_uri                  varchar(4000),
    domain                           varchar(4000),
    tool_id                          varchar(255),
    public_jwk_url                   varchar(4000),
    oidc_initiation_url              varchar(4000),
    oidc_initiation_urls             jsonb NOT NULL,
    custom_fields                    jsonb NOT NULL,
    launch_settings                  jsonb NOT NULL,
    placements                       jsonb NOT NULL DEFAULT '{}',
    public_jwk                       jsonb,
    scopes                           text[] NOT NULL DEFAULT '{}',
    redirect_uris                    text[] NOT NULL DEFAULT '{}',
    workflow_state                   varchar(32) NOT NULL DEFAULT 'active',
    CONSTRAINT chk_workflow_state_enum_9 CHECK (workflow_state IN ('active', 'deleted'))
);

CREATE TABLE lti_tool_consumer_profiles (
    id                               BIGSERIAL PRIMARY KEY,
    services                         text,
    capabilities                     text,
    uuid                             varchar NOT NULL,
    developer_key_id                 bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE lti_tool_proxy_bindings (
    id                               BIGSERIAL PRIMARY KEY,
    context_id                       bigint NOT NULL,
    context_type                     varchar(255) NOT NULL,
    tool_proxy_id                    bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    enabled                          boolean NOT NULL DEFAULT true
);

CREATE TABLE lti_tool_settings (
    id                               BIGSERIAL PRIMARY KEY,
    tool_proxy_id                    bigint,
    context_id                       bigint,
    context_type                     varchar(255),
    resource_link_id                 text,
    custom                           text,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    product_code                     varchar,
    vendor_code                      varchar,
    resource_type_code               varchar,
    custom_parameters                text,
    resource_url                     text
);

CREATE TABLE master_courses_child_content_tags (
    id                               BIGSERIAL PRIMARY KEY,
    child_subscription_id            bigint NOT NULL,
    content_id                       bigint NOT NULL,
    content_type                     varchar(255) NOT NULL,
    downstream_changes               text,
    migration_id                     varchar,
    root_account_id                  bigint,
    created_at                       timestamp NOT NULL DEFAULT now(),
    updated_at                       timestamp NOT NULL DEFAULT now()
);

CREATE TABLE master_courses_child_subscriptions (
    id                               BIGSERIAL PRIMARY KEY,
    master_template_id               bigint NOT NULL,
    child_course_id                  bigint NOT NULL,
    workflow_state                   varchar(255) NOT NULL,
    use_selective_copy               boolean NOT NULL DEFAULT false,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    root_account_id                  bigint
);
