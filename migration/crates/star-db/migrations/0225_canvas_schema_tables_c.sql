--
-- Canvas schema parity: tables c.
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

CREATE TABLE master_courses_master_content_tags (
    id                               BIGSERIAL PRIMARY KEY,
    master_template_id               bigint NOT NULL,
    content_id                       bigint NOT NULL,
    content_type                     varchar(255) NOT NULL,
    current_migration_id             bigint,
    restrictions                     text,
    migration_id                     varchar,
    use_default_restrictions         boolean NOT NULL DEFAULT false,
    root_account_id                  bigint,
    created_at                       timestamp NOT NULL DEFAULT now(),
    updated_at                       timestamp NOT NULL DEFAULT now()
);

CREATE TABLE master_courses_master_migrations (
    id                               BIGSERIAL PRIMARY KEY,
    master_template_id               bigint NOT NULL,
    user_id                          bigint,
    export_results                   text,
    exports_started_at               timestamp,
    imports_queued_at                timestamp,
    workflow_state                   varchar(255) NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    imports_completed_at             timestamp,
    comment                          text,
    send_notification                boolean NOT NULL DEFAULT false,
    migration_settings               text,
    root_account_id                  bigint
);

CREATE TABLE master_courses_master_templates (
    id                               BIGSERIAL PRIMARY KEY,
    course_id                        bigint NOT NULL,
    full_course                      boolean NOT NULL DEFAULT true,
    workflow_state                   varchar(255),
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    active_migration_id              bigint,
    default_restrictions             text,
    use_default_restrictions_by_type boolean NOT NULL DEFAULT false,
    default_restrictions_by_type     text,
    root_account_id                  bigint
);

CREATE TABLE master_courses_migration_results (
    id                               BIGSERIAL PRIMARY KEY,
    master_migration_id              bigint NOT NULL,
    content_migration_id             bigint NOT NULL,
    child_subscription_id            bigint NOT NULL,
    import_type                      varchar NOT NULL,
    state                            varchar NOT NULL,
    results                          text,
    root_account_id                  bigint,
    created_at                       timestamp NOT NULL DEFAULT now(),
    updated_at                       timestamp NOT NULL DEFAULT now()
);

CREATE TABLE mentions (
    id                               BIGSERIAL PRIMARY KEY,
    discussion_entry_id              bigint NOT NULL,
    user_id                          bigint NOT NULL,
    root_account_id                  bigint NOT NULL,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE messages (
    id                               BIGSERIAL PRIMARY KEY,
    "to"                             text,
    "from"                           text,
    subject                          text,
    body                             text,
    delay_for                        integer DEFAULT 120,
    dispatch_at                      timestamp,
    sent_at                          timestamp,
    workflow_state                   varchar(255),
    transmission_errors              text,
    is_bounced                       boolean,
    notification_id                  bigint,
    communication_channel_id         bigint,
    context_id                       bigint,
    context_type                     varchar(255),
    user_id                          bigint,
    created_at                       timestamp,
    updated_at                       timestamp,
    notification_name                varchar(255),
    url                              text,
    path_type                        varchar(255),
    from_name                        text,
    to_email                         boolean,
    html_body                        text,
    root_account_id                  bigint,
    reply_to_name                    varchar(255)
);

CREATE TABLE microsoft_sync_groups (
    id                               BIGSERIAL PRIMARY KEY,
    course_id                        bigint NOT NULL,
    workflow_state                   varchar NOT NULL DEFAULT 'pending',
    job_state                        varchar,
    last_synced_at                   timestamp,
    last_manually_synced_at          timestamp,
    last_error                       text,
    root_account_id                  bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    ms_group_id                      varchar,
    last_error_report_id             bigint,
    debug_info                       text
);

CREATE TABLE microsoft_sync_partial_sync_changes (
    id                               BIGSERIAL PRIMARY KEY,
    course_id                        bigint NOT NULL,
    user_id                          bigint NOT NULL,
    root_account_id                  bigint NOT NULL,
    enrollment_type                  varchar NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE microsoft_sync_user_mappings (
    id                               BIGSERIAL PRIMARY KEY,
    root_account_id                  bigint NOT NULL,
    user_id                          bigint NOT NULL,
    aad_id                           varchar,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    needs_updating                   boolean NOT NULL DEFAULT false
);

CREATE TABLE moderated_grading_provisional_grades (
    id                               BIGSERIAL PRIMARY KEY,
    grade                            varchar(255),
    score                            double precision,
    graded_at                        timestamp,
    scorer_id                        bigint NOT NULL,
    submission_id                    bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    final                            boolean NOT NULL DEFAULT false,
    source_provisional_grade_id      bigint,
    graded_anonymously               boolean
);

CREATE TABLE nav_menu_links (
    id                               BIGSERIAL PRIMARY KEY,
    account_id                       bigint NOT NULL,
    course_id                        bigint NOT NULL,
    label                            varchar(255) NOT NULL,
    url                              varchar(2048) NOT NULL,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    root_account_id                  bigint NOT NULL,
    course_nav                       boolean NOT NULL DEFAULT false,
    account_nav                      boolean NOT NULL DEFAULT false,
    user_nav                         boolean NOT NULL DEFAULT false,
    migration_id                     varchar(255),
    CONSTRAINT chk_require_context_5 CHECK (((account_id IS NOT NULL)::int + (course_id IS NOT NULL)::int) = 1),
    CONSTRAINT chk_workflow_state_enum_10 CHECK (workflow_state IN ('active', 'deleted')),
    CONSTRAINT chk_at_least_one_nav_type CHECK ((course_nav OR account_nav OR user_nav)),
    CONSTRAINT chk_course_id_implies_only_course_nav CHECK ((course_id IS NULL) OR (account_nav = false AND user_nav = false))
);

CREATE TABLE notification_endpoints (
    id                               BIGSERIAL PRIMARY KEY,
    access_token_id                  bigint NOT NULL,
    token                            varchar(255) NOT NULL,
    arn                              varchar(255) NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    workflow_state                   varchar NOT NULL DEFAULT 'active'
);

CREATE TABLE notification_policy_overrides (
    id                               BIGSERIAL PRIMARY KEY,
    context_id                       bigint NOT NULL,
    context_type                     varchar NOT NULL,
    communication_channel_id         bigint NOT NULL,
    notification_id                  bigint,
    workflow_state                   varchar NOT NULL DEFAULT 'active',
    frequency                        varchar,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE oauth_client_configs (
    id                               BIGSERIAL PRIMARY KEY,
    type                             varchar NOT NULL,
    identifier                       varchar(255) NOT NULL,
    client_name                      varchar(255),
    comment                          varchar(1024),
    root_account_id                  bigint NOT NULL,
    throttle_maximum                 integer,
    throttle_high_water_mark         integer,
    throttle_outflow                 integer,
    throttle_upfront_cost            integer,
    updated_by_id                    bigint NOT NULL,
    workflow_state                   varchar NOT NULL DEFAULT 'active',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    CONSTRAINT chk_type_enum CHECK (type IN ('custom', 'product', 'client_id', 'lti_advantage', 'service_user_key', 'token', 'user', 'tool', 'session', 'ip')),
    CONSTRAINT chk_workflow_state_enum_11 CHECK (workflow_state IN ('active', 'deleted'))
);

CREATE TABLE oauth_requests (
    id                               BIGSERIAL PRIMARY KEY,
    token                            varchar(255),
    secret                           varchar(255),
    user_secret                      varchar(255),
    return_url                       varchar(4096),
    workflow_state                   varchar(255),
    user_id                          bigint,
    original_host_with_port          varchar(255),
    service                          varchar(255),
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE observer_alert_thresholds (
    id                               BIGSERIAL PRIMARY KEY,
    alert_type                       varchar NOT NULL,
    threshold                        varchar,
    workflow_state                   varchar NOT NULL DEFAULT 'active',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    user_id                          bigint NOT NULL,
    observer_id                      bigint NOT NULL
);

CREATE TABLE observer_alerts (
    id                               BIGSERIAL PRIMARY KEY,
    observer_alert_threshold_id      bigint NOT NULL,
    context_id                       bigint,
    context_type                     varchar,
    alert_type                       varchar NOT NULL,
    workflow_state                   varchar NOT NULL DEFAULT 'unread',
    action_date                      timestamp NOT NULL,
    title                            varchar NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    user_id                          bigint NOT NULL,
    observer_id                      bigint NOT NULL
);

CREATE TABLE observer_pairing_codes (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint NOT NULL,
    code                             varchar(10) NOT NULL,
    expires_at                       timestamp NOT NULL,
    workflow_state                   varchar NOT NULL DEFAULT 'active',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE one_time_passwords (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint NOT NULL,
    code                             varchar NOT NULL,
    used                             boolean NOT NULL DEFAULT false,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE originality_reports (
    id                               BIGSERIAL PRIMARY KEY,
    attachment_id                    bigint,
    originality_score                double precision,
    originality_report_attachment_id bigint,
    originality_report_url           text,
    originality_report_lti_url       text,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    submission_id                    bigint NOT NULL,
    workflow_state                   varchar NOT NULL DEFAULT 'pending',
    link_id                          text,
    error_message                    text,
    submission_time                  timestamp,
    root_account_id                  bigint NOT NULL
);

CREATE TABLE outcome_calculation_methods (
    id                               BIGSERIAL PRIMARY KEY,
    context_id                       bigint NOT NULL,
    context_type                     varchar(255) NOT NULL,
    calculation_int                  smallint,
    calculation_method               varchar(255) NOT NULL,
    workflow_state                   varchar NOT NULL DEFAULT 'active',
    root_account_id                  bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE outcome_friendly_descriptions (
    id                               BIGSERIAL PRIMARY KEY,
    context_id                       bigint NOT NULL,
    context_type                     varchar(255) NOT NULL,
    workflow_state                   varchar NOT NULL DEFAULT 'active',
    root_account_id                  bigint,
    description                      text NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    learning_outcome_id              bigint NOT NULL
);

CREATE TABLE outcome_import_errors (
    id                               BIGSERIAL PRIMARY KEY,
    outcome_import_id                bigint NOT NULL,
    message                          varchar(255) NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    row                              integer,
    failure                          boolean NOT NULL DEFAULT false
);

CREATE TABLE outcome_imports (
    id                               BIGSERIAL PRIMARY KEY,
    workflow_state                   varchar NOT NULL,
    context_id                       bigint NOT NULL,
    context_type                     varchar NOT NULL,
    user_id                          bigint,
    attachment_id                    bigint,
    progress                         integer,
    ended_at                         timestamp,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    data                             json,
    learning_outcome_group_id        bigint
);

CREATE TABLE outcome_proficiencies (
    id                               BIGSERIAL PRIMARY KEY,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    root_account_id                  bigint,
    context_id                       bigint NOT NULL,
    context_type                     varchar(255) NOT NULL,
    workflow_state                   varchar NOT NULL DEFAULT 'active'
);

CREATE TABLE outcome_proficiency_ratings (
    id                               BIGSERIAL PRIMARY KEY,
    outcome_proficiency_id           bigint NOT NULL,
    description                      varchar(255) NOT NULL,
    points                           double precision NOT NULL,
    mastery                          boolean NOT NULL,
    color                            varchar NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    root_account_id                  bigint,
    workflow_state                   varchar NOT NULL DEFAULT 'active'
);

CREATE TABLE outcome_rollups (
    id                               BIGSERIAL PRIMARY KEY,
    root_account_id                  bigint NOT NULL,
    course_id                        bigint NOT NULL,
    user_id                          bigint NOT NULL,
    outcome_id                       bigint NOT NULL,
    calculation_method               varchar NOT NULL,
    aggregate_score                  double precision NOT NULL,
    last_calculated_at               timestamp NOT NULL,
    workflow_state                   varchar NOT NULL DEFAULT 'active',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    submitted_at                     timestamp,
    results_count                    integer NOT NULL DEFAULT 0,
    title                            varchar(255),
    hide_points                      boolean NOT NULL DEFAULT false,
    CONSTRAINT chk_calculation_method_enum CHECK (calculation_method IN ('average', 'decaying_average', 'highest', 'latest', 'n_mastery', 'standard_decaying_average', 'weighted_average')),
    CONSTRAINT chk_workflow_state_enum_12 CHECK (workflow_state IN ('active', 'deleted'))
);

CREATE TABLE page_comments (
    id                               BIGSERIAL PRIMARY KEY,
    message                          text,
    page_id                          bigint,
    page_type                        varchar(255),
    user_id                          bigint,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE page_views (
    request_id                       varchar(255),
    session_id                       varchar(255),
    user_id                          bigint NOT NULL,
    url                              text,
    context_id                       bigint,
    context_type                     varchar(255),
    asset_id                         bigint,
    asset_type                       varchar(255),
    controller                       varchar(255),
    action                           varchar(255),
    interaction_seconds              double precision,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    developer_key_id                 bigint,
    user_request                     boolean,
    render_time                      double precision,
    user_agent                       text,
    asset_user_access_id             bigint,
    participated                     boolean,
    summarized                       boolean,
    account_id                       bigint,
    real_user_id                     bigint,
    http_method                      varchar(255),
    remote_ip                        varchar(255),
    PRIMARY KEY (request_id)
);

CREATE TABLE parallel_importers (
    id                               BIGSERIAL PRIMARY KEY,
    sis_batch_id                     bigint NOT NULL,
    workflow_state                   varchar(255) NOT NULL,
    index                            bigint NOT NULL,
    batch_size                       bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    started_at                       timestamp,
    ended_at                         timestamp,
    importer_type                    varchar(255) NOT NULL,
    attachment_id                    bigint NOT NULL,
    rows_processed                   integer NOT NULL DEFAULT 0,
    job_ids                          bigint[] NOT NULL DEFAULT '{}'
);

CREATE TABLE plugin_settings (
    id                               BIGSERIAL PRIMARY KEY,
    name                             varchar(255) NOT NULL DEFAULT '',
    settings                         text,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    disabled                         boolean
);

CREATE TABLE polling_poll_choices (
    id                               BIGSERIAL PRIMARY KEY,
    text                             varchar(255),
    is_correct                       boolean NOT NULL DEFAULT false,
    poll_id                          bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    position                         integer
);

CREATE TABLE polling_poll_sessions (
    id                               BIGSERIAL PRIMARY KEY,
    is_published                     boolean NOT NULL DEFAULT false,
    has_public_results               boolean NOT NULL DEFAULT false,
    course_id                        bigint NOT NULL,
    course_section_id                bigint,
    poll_id                          bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE polling_poll_submissions (
    id                               BIGSERIAL PRIMARY KEY,
    poll_id                          bigint NOT NULL,
    poll_choice_id                   bigint NOT NULL,
    user_id                          bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    poll_session_id                  bigint NOT NULL
);

CREATE TABLE polling_polls (
    id                               BIGSERIAL PRIMARY KEY,
    question                         varchar(255),
    description                      varchar(255),
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    user_id                          bigint NOT NULL
);

CREATE TABLE post_policies (
    id                               BIGSERIAL PRIMARY KEY,
    post_manually                    boolean NOT NULL DEFAULT false,
    course_id                        bigint,
    assignment_id                    bigint,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    root_account_id                  bigint
);

CREATE TABLE profiles (
    id                               BIGSERIAL PRIMARY KEY,
    root_account_id                  bigint NOT NULL,
    context_id                       bigint NOT NULL,
    context_type                     varchar(255) NOT NULL,
    title                            varchar(255),
    path                             varchar(255),
    description                      text,
    data                             text,
    visibility                       varchar(255),
    position                         integer
);

CREATE TABLE progresses (
    id                               BIGSERIAL PRIMARY KEY,
    context_id                       bigint NOT NULL,
    context_type                     varchar(255) NOT NULL,
    user_id                          bigint,
    tag                              varchar(255) NOT NULL,
    completion                       double precision,
    delayed_job_id                   varchar(255),
    workflow_state                   varchar(255) NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    message                          text,
    cache_key_context                varchar(255),
    results                          text
);

CREATE TABLE pseudonyms (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint NOT NULL,
    account_id                       bigint NOT NULL,
    workflow_state                   varchar(255) NOT NULL,
    unique_id                        varchar(255) NOT NULL,
    crypted_password                 varchar(255) NOT NULL,
    password_salt                    varchar(255) NOT NULL,
    persistence_token                varchar(255) NOT NULL,
    single_access_token              varchar(255) NOT NULL,
    perishable_token                 varchar(255) NOT NULL,
    login_count                      integer NOT NULL DEFAULT 0,
    failed_login_count               integer NOT NULL DEFAULT 0,
    last_request_at                  timestamp,
    last_login_at                    timestamp,
    current_login_at                 timestamp,
    last_login_ip                    varchar(255),
    current_login_ip                 varchar(255),
    reset_password_token             varchar(255) NOT NULL DEFAULT '',
    position                         integer,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    password_auto_generated          boolean,
    deleted_at                       timestamp,
    sis_batch_id                     bigint,
    sis_user_id                      varchar(255),
    sis_ssha                         varchar(255),
    communication_channel_id         bigint,
    sis_communication_channel_id     bigint,
    stuck_sis_fields                 text,
    integration_id                   varchar(255),
    authentication_provider_id       bigint,
    declared_user_type               varchar(255),
    unique_ids                       jsonb NOT NULL,
    unique_id_normalized             varchar(255) NOT NULL
);

CREATE TABLE purgatories (
    id                               BIGSERIAL PRIMARY KEY,
    attachment_id                    bigint NOT NULL,
    deleted_by_user_id               bigint,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    workflow_state                   varchar NOT NULL DEFAULT 'active',
    old_filename                     varchar NOT NULL,
    old_display_name                 varchar(255),
    old_content_type                 varchar(255),
    new_instfs_uuid                  varchar,
    old_file_state                   varchar,
    old_workflow_state               varchar
);

CREATE TABLE quiz_groups (
    id                               BIGSERIAL PRIMARY KEY,
    quiz_id                          bigint NOT NULL,
    name                             varchar(255),
    pick_count                       integer,
    question_points                  double precision,
    position                         integer,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    migration_id                     varchar(255),
    assessment_question_bank_id      bigint,
    root_account_id                  bigint
);

CREATE TABLE quiz_migration_alerts (
    id                               BIGSERIAL PRIMARY KEY,
    migration_id                     bigint,
    migration_type                   varchar,
    user_id                          bigint NOT NULL,
    course_id                        bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE quiz_question_regrades (
    id                               BIGSERIAL PRIMARY KEY,
    quiz_regrade_id                  bigint NOT NULL,
    quiz_question_id                 bigint NOT NULL,
    regrade_option                   varchar(255) NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE quiz_regrade_runs (
    id                               BIGSERIAL PRIMARY KEY,
    quiz_regrade_id                  bigint NOT NULL,
    started_at                       timestamp,
    finished_at                      timestamp,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE quiz_regrades (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint NOT NULL,
    quiz_id                          bigint NOT NULL,
    quiz_version                     integer NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE quiz_submission_events (
    id                               BIGSERIAL PRIMARY KEY,
    attempt                          integer NOT NULL,
    event_type                       varchar(255) NOT NULL,
    quiz_submission_id               bigint NOT NULL,
    event_data                       text,
    created_at                       timestamp NOT NULL,
    client_timestamp                 timestamp,
    root_account_id                  bigint
);

CREATE TABLE quiz_submission_snapshots (
    id                               BIGSERIAL PRIMARY KEY,
    quiz_submission_id               bigint,
    attempt                          integer,
    data                             text,
    created_at                       timestamp,
    updated_at                       timestamp
);

CREATE TABLE report_snapshots (
    id                               BIGSERIAL PRIMARY KEY,
    report_type                      varchar(255),
    data                             text,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    account_id                       bigint
);

CREATE TABLE rubric_assessment_imports (
    id                               BIGSERIAL PRIMARY KEY,
    root_account_id                  bigint NOT NULL,
    workflow_state                   varchar NOT NULL,
    user_id                          bigint,
    assignment_id                    bigint,
    attachment_id                    bigint,
    course_id                        bigint,
    progress                         integer NOT NULL DEFAULT 0,
    error_count                      integer NOT NULL DEFAULT 0,
    error_data                       json,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE scheduled_posts (
    id                               BIGSERIAL PRIMARY KEY,
    post_comments_at                 timestamp NOT NULL,
    post_comments_ran_at             timestamp,
    post_grades_at                   timestamp NOT NULL,
    post_grades_ran_at               timestamp,
    root_account_id                  bigint NOT NULL,
    post_policy_id                   bigint NOT NULL,
    assignment_id                    bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE scheduled_smart_alerts (
    id                               BIGSERIAL PRIMARY KEY,
    context_type                     varchar NOT NULL,
    alert_type                       varchar NOT NULL,
    context_id                       bigint NOT NULL,
    due_at                           timestamp NOT NULL,
    root_account_id                  bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE score_metadata (
    id                               BIGSERIAL PRIMARY KEY,
    score_id                         bigint NOT NULL,
    calculation_details              json NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    workflow_state                   varchar NOT NULL DEFAULT 'active'
);

CREATE TABLE score_statistics (
    id                               BIGSERIAL PRIMARY KEY,
    assignment_id                    bigint NOT NULL,
    minimum                          double precision NOT NULL,
    maximum                          double precision NOT NULL,
    mean                             double precision NOT NULL,
    count                            integer NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    root_account_id                  bigint,
    lower_q                          double precision,
    median                           double precision,
    upper_q                          double precision
);

CREATE TABLE scores (
    id                               BIGSERIAL PRIMARY KEY,
    enrollment_id                    bigint NOT NULL,
    grading_period_id                bigint,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    current_score                    double precision,
    final_score                      double precision,
    created_at                       timestamp,
    updated_at                       timestamp,
    assignment_group_id              bigint,
    course_score                     boolean NOT NULL DEFAULT false,
    unposted_current_score           double precision,
    unposted_final_score             double precision,
    current_points                   double precision,
    unposted_current_points          double precision,
    final_points                     double precision,
    unposted_final_points            double precision,
    override_score                   double precision,
    root_account_id                  bigint,
    custom_grade_status_id           bigint
);

CREATE TABLE session_persistence_tokens (
    id                               BIGSERIAL PRIMARY KEY,
    token_salt                       varchar(255) NOT NULL,
    crypted_token                    varchar(255) NOT NULL,
    pseudonym_id                     bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE settings (
    id                               BIGSERIAL PRIMARY KEY,
    name                             varchar(255),
    value                            text,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    secret                           boolean NOT NULL DEFAULT false
);

CREATE TABLE shared_brand_configs (
    id                               BIGSERIAL PRIMARY KEY,
    name                             varchar(255),
    account_id                       bigint,
    brand_config_md5                 varchar(32) NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE sis_batch_errors (
    id                               BIGSERIAL PRIMARY KEY,
    sis_batch_id                     bigint NOT NULL,
    root_account_id                  bigint NOT NULL,
    message                          text NOT NULL,
    backtrace                        text,
    file                             varchar(255),
    failure                          boolean NOT NULL DEFAULT false,
    row                              integer,
    created_at                       timestamp NOT NULL,
    row_info                         text
);

CREATE TABLE sis_batch_roll_back_data (
    id                               BIGSERIAL PRIMARY KEY,
    sis_batch_id                     bigint NOT NULL,
    context_id                       bigint NOT NULL,
    context_type                     varchar(255) NOT NULL,
    previous_workflow_state          varchar(255) NOT NULL,
    updated_workflow_state           varchar(255) NOT NULL,
    batch_mode_delete                boolean NOT NULL DEFAULT false,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE sis_batches (
    id                               BIGSERIAL PRIMARY KEY,
    account_id                       bigint NOT NULL,
    ended_at                         timestamp,
    workflow_state                   varchar(255) NOT NULL,
    data                             text,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    attachment_id                    bigint,
    progress                         integer,
    processing_errors                text,
    processing_warnings              text,
    batch_mode                       boolean,
    batch_mode_term_id               bigint,
    options                          text,
    user_id                          bigint,
    started_at                       timestamp,
    diffing_data_set_identifier      varchar(255),
    diffing_remaster                 boolean,
    generated_diff_id                bigint,
    errors_attachment_id             bigint,
    change_threshold                 integer,
    diffing_threshold_exceeded       boolean NOT NULL DEFAULT false,
    job_ids                          bigint[] NOT NULL DEFAULT '{}'
);

CREATE TABLE sis_post_grades_statuses (
    id                               BIGSERIAL PRIMARY KEY,
    course_id                        bigint NOT NULL,
    course_section_id                bigint,
    user_id                          bigint,
    status                           varchar(255) NOT NULL,
    message                          varchar(255) NOT NULL,
    grades_posted_at                 timestamp NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE standard_grade_statuses (
    id                               BIGSERIAL PRIMARY KEY,
    color                            varchar(7) NOT NULL,
    status_name                      varchar NOT NULL,
    hidden                           boolean NOT NULL DEFAULT false,
    root_account_id                  bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE stream_item_instances (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint NOT NULL,
    stream_item_id                   bigint NOT NULL,
    hidden                           boolean NOT NULL DEFAULT false,
    workflow_state                   varchar(255),
    context_id                       bigint,
    context_type                     varchar(255)
);

CREATE TABLE submission_draft_attachments (
    id                               BIGSERIAL PRIMARY KEY,
    submission_draft_id              bigint NOT NULL,
    attachment_id                    bigint NOT NULL
);

CREATE TABLE submission_drafts (
    id                               BIGSERIAL PRIMARY KEY,
    submission_id                    bigint NOT NULL,
    submission_attempt               integer NOT NULL,
    body                             text,
    url                              text,
    active_submission_type           varchar,
    media_object_id                  varchar,
    context_external_tool_id         bigint,
    lti_launch_url                   text,
    resource_link_lookup_uuid        uuid
);

CREATE TABLE submission_texts (
    id                               BIGSERIAL PRIMARY KEY,
    submission_id                    bigint NOT NULL,
    attachment_id                    bigint,
    root_account_id                  bigint NOT NULL,
    text                             text NOT NULL,
    attempt                          integer NOT NULL,
    contains_images                  boolean NOT NULL DEFAULT false,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    submission_type                  varchar
);

CREATE TABLE submission_versions (
    id                               BIGSERIAL PRIMARY KEY,
    context_id                       bigint,
    context_type                     varchar(255),
    version_id                       bigint,
    user_id                          bigint,
    assignment_id                    bigint,
    root_account_id                  bigint,
    created_at                       timestamp NOT NULL DEFAULT now(),
    updated_at                       timestamp NOT NULL DEFAULT now()
);

CREATE TABLE switchman_shards (
    id                               BIGSERIAL PRIMARY KEY,
    name                             varchar(255),
    database_server_id               varchar(255),
    "default"                        boolean NOT NULL DEFAULT false,
    settings                         text,
    delayed_jobs_shard_id            bigint,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    block_stranded                   boolean DEFAULT false,
    jobs_held                        boolean DEFAULT false,
    migrate_from_delayed_jobs_shard_id bigint
);

CREATE TABLE temporary_enrollment_pairings (
    id                               BIGSERIAL PRIMARY KEY,
    root_account_id                  bigint NOT NULL,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    created_by_id                    bigint,
    deleted_by_id                    bigint,
    ending_enrollment_state          text
);

CREATE TABLE terms_of_service_contents (
    id                               BIGSERIAL PRIMARY KEY,
    content                          text NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    terms_updated_at                 timestamp NOT NULL,
    workflow_state                   varchar NOT NULL,
    account_id                       bigint
);

CREATE TABLE terms_of_services (
    id                               BIGSERIAL PRIMARY KEY,
    terms_type                       varchar NOT NULL DEFAULT 'default',
    passive                          boolean NOT NULL DEFAULT true,
    terms_of_service_content_id      bigint,
    account_id                       bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    workflow_state                   varchar NOT NULL
);

CREATE TABLE thumbnails (
    id                               BIGSERIAL PRIMARY KEY,
    parent_id                        bigint,
    content_type                     varchar(255) NOT NULL,
    filename                         varchar(255) NOT NULL,
    thumbnail                        varchar(255),
    size                             integer NOT NULL,
    width                            integer,
    height                           integer,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    uuid                             varchar(255),
    namespace                        varchar(255)
);

CREATE TABLE translation_feedback (
    id                               BIGSERIAL PRIMARY KEY,
    root_account_id                  bigint NOT NULL,
    user_id                          bigint NOT NULL,
    course_id                        bigint NOT NULL,
    discussion_topic_id              bigint NOT NULL,
    discussion_entry_id              bigint NOT NULL,
    target_language                  varchar(16) NOT NULL,
    liked                            boolean NOT NULL DEFAULT false,
    disliked                         boolean NOT NULL DEFAULT false,
    feedback_notes                   text,
    feature_slug                     varchar(255),
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    CONSTRAINT chk_require_context_6 CHECK (((course_id IS NOT NULL)::int) = 1),
    CONSTRAINT chk_require_content CHECK (((discussion_topic_id IS NOT NULL)::int + (discussion_entry_id IS NOT NULL)::int) = 1),
    CONSTRAINT chk_translation_feedback_liked_disliked CHECK (NOT (liked AND disliked))
);

CREATE TABLE user_account_associations (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint NOT NULL,
    account_id                       bigint NOT NULL,
    depth                            integer,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    root_account_id                  bigint NOT NULL
);

CREATE TABLE user_lmgb_outcome_orderings (
    id                               BIGSERIAL PRIMARY KEY,
    root_account_id                  bigint NOT NULL,
    user_id                          bigint NOT NULL,
    course_id                        bigint NOT NULL,
    learning_outcome_id              bigint NOT NULL,
    position                         integer NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE user_merge_data (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint NOT NULL,
    from_user_id                     bigint NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active'
);

CREATE TABLE user_merge_data_items (
    id                               BIGSERIAL PRIMARY KEY,
    user_merge_data_id               bigint NOT NULL,
    user_id                          bigint NOT NULL,
    item_type                        varchar(255) NOT NULL,
    item                             text NOT NULL
);

CREATE TABLE user_merge_data_records (
    id                               BIGSERIAL PRIMARY KEY,
    user_merge_data_id               bigint NOT NULL,
    context_id                       bigint NOT NULL,
    previous_user_id                 bigint NOT NULL,
    context_type                     varchar(255) NOT NULL,
    previous_workflow_state          varchar(255)
);

CREATE TABLE user_observers (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint NOT NULL,
    observer_id                      bigint NOT NULL,
    workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    sis_batch_id                     bigint,
    root_account_id                  bigint NOT NULL
);

CREATE TABLE user_past_lti_ids (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint NOT NULL,
    context_id                       bigint NOT NULL,
    context_type                     varchar(255) NOT NULL,
    user_uuid                        varchar(255) NOT NULL,
    user_lti_id                      text NOT NULL,
    user_lti_context_id              varchar(255)
);

CREATE TABLE user_preference_values (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint NOT NULL,
    key                              varchar NOT NULL,
    sub_key                          varchar,
    value                            text
);

CREATE TABLE user_profile_links (
    id                               BIGSERIAL PRIMARY KEY,
    url                              varchar(4096),
    title                            varchar(255),
    user_profile_id                  bigint,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE user_services (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint NOT NULL,
    token                            text,
    secret                           varchar(255),
    protocol                         varchar(255),
    service                          varchar(255) NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    service_user_url                 varchar(255),
    service_user_id                  varchar(255) NOT NULL,
    service_user_name                varchar(255),
    service_domain                   varchar(255),
    crypted_password                 varchar(255),
    password_salt                    varchar(255),
    type                             varchar(255),
    workflow_state                   varchar(255) NOT NULL,
    last_result_id                   varchar(255),
    refresh_at                       timestamp,
    visible                          boolean
);

CREATE TABLE versions (
    id                               BIGSERIAL PRIMARY KEY,
    versionable_id                   bigint,
    versionable_type                 varchar(255),
    number                           integer,
    yaml                             text,
    created_at                       timestamp
);

CREATE TABLE viewed_submission_comments (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint NOT NULL,
    submission_comment_id            bigint NOT NULL,
    viewed_at                        timestamp NOT NULL
);

CREATE TABLE web_conference_participants (
    id                               BIGSERIAL PRIMARY KEY,
    user_id                          bigint,
    web_conference_id                bigint,
    participation_type               varchar(255),
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    root_account_id                  bigint NOT NULL
);

CREATE TABLE web_conferences (
    id                               BIGSERIAL PRIMARY KEY,
    title                            varchar(255) NOT NULL,
    conference_type                  varchar(255) NOT NULL,
    conference_key                   varchar(255),
    context_id                       bigint NOT NULL,
    context_type                     varchar(255) NOT NULL,
    user_ids                         varchar(255),
    added_user_ids                   varchar(255),
    user_id                          bigint,
    started_at                       timestamp,
    description                      text,
    duration                         double precision,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    uuid                             varchar(255),
    invited_user_ids                 varchar(255),
    ended_at                         timestamp,
    start_at                         timestamp,
    end_at                           timestamp,
    context_code                     varchar(255),
    type                             varchar(255),
    settings                         text,
    recording_ready                  boolean,
    root_account_id                  bigint NOT NULL
);

CREATE TABLE wiki_page_lookups (
    id                               BIGSERIAL PRIMARY KEY,
    slug                             text NOT NULL,
    wiki_page_id                     bigint NOT NULL,
    root_account_id                  bigint NOT NULL,
    context_id                       bigint NOT NULL,
    context_type                     varchar(255) NOT NULL,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL
);

CREATE TABLE wiki_pages (
    id                               BIGSERIAL PRIMARY KEY,
    wiki_id                          bigint NOT NULL,
    title                            varchar(255),
    body                             text,
    workflow_state                   varchar(255) NOT NULL,
    user_id                          bigint,
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    url                              text,
    protected_editing                boolean DEFAULT false,
    editing_roles                    varchar(255),
    revised_at                       timestamp,
    could_be_locked                  boolean,
    cloned_item_id                   bigint,
    migration_id                     varchar(255),
    assignment_id                    bigint,
    old_assignment_id                bigint,
    todo_date                        timestamp,
    context_id                       bigint NOT NULL,
    context_type                     varchar NOT NULL,
    root_account_id                  bigint,
    publish_at                       timestamp,
    current_lookup_id                bigint,
    unlock_at                        timestamp,
    lock_at                          timestamp,
    only_visible_to_overrides        boolean NOT NULL DEFAULT false
);

CREATE TABLE wikis (
    id                               BIGSERIAL PRIMARY KEY,
    title                            varchar(255),
    created_at                       timestamp NOT NULL,
    updated_at                       timestamp NOT NULL,
    front_page_url                   text,
    has_no_front_page                boolean,
    root_account_id                  bigint
);
