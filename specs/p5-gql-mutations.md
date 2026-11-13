# Spec p5: GraphQL mutations — first batches (assignments, submissions, discussions, enrollments, conversations)

Issue: #21 (epic: specs) · Status: Draft for review · Parity target: canvas-lms `app/graphql/`
(mutations, types) · Implementation target: `migration/crates/star-graphql/` (async-graphql 7,
`EmptyMutation` today).

This spec defines the first GraphQL mutation batches for star-lms with Canvas permission checks,
Canvas validation messages (exact strings, exact `attribute` values), and Canvas payload shapes.
All Canvas behavior below was verified against the canvas-lms sources cited per section; star-lms
line numbers refer to `migration/crates/*/src/lib.rs` on `main` (3dc34f6).

## 1. Overview and scope

The star-graphql foundation (`migration/crates/star-graphql/src/lib.rs`) already provides the
execution shell every mutation will reuse: the router mounted at `/api/graphql`, a `GraphQLContext`
holding the `PgPool` plus the `AuthUser` resolved by the shared extractor **before** execution
(unauthenticated requests get HTTP 401 and never reach the schema), and `db_err` mapping any
sqlx failure to a generic `async_graphql::Error("database error")`. What is missing is the
mutation root (`EmptyMutation`) and the domain payloads — this spec adds them in five batches:

| Batch | Mutations | Count | Canvas source |
| --- | --- | --- | --- |
| A | CreateAssignment, UpdateAssignment (delete = UpdateAssignment `state: deleted`) | 2 (3 operations) | `app/graphql/mutations/create_assignment.rb`, `update_assignment.rb`, `assignment_base.rb` |
| B | CreateSubmission, UpdateSubmissionGrade, CreateSubmissionComment | 3 | `create_submission.rb`, `update_submission_grade.rb`, `create_submission_comment.rb` |
| C | CreateDiscussionTopic, UpdateDiscussionTopic, DeleteDiscussionTopic | 3 | `create_discussion_topic.rb`, `update_discussion_topic.rb`, `delete_discussion_topic.rb`, `discussion_base.rb` |
| D | CreateEnrollment (**new — documented deviation**, no Canvas mutation exists) | 1 | REST parity: `app/controllers/enrollments_api_controller.rb#create` |
| E | CreateConversation | 1 | `create_conversation.rb`, `app/helpers/conversations_helper.rb` |

Total: 10 mutations. Out of scope for these batches: discussion entries, conversation
add-message/delete, assignment-grade posting/hiding, submission drafts, enrollment
accept/reject invitations (the only enrollment mutations Canvas actually has).

Canvas conventions every mutation must follow (`app/graphql/mutations/base_mutation.rb`):

- Exactly **one `input` argument** per mutation (`GraphQL::Schema::HasSingleInputArgument`).
- Every payload carries `errors: [ValidationErrorType]`; `ValidationErrorType` =
  `{ attribute: String (nullable), message: String! }`
  (`app/graphql/types/validation_error_type.rb`).
- `verify_authorized_action!(obj, perm)` raises `GraphQL::ExecutionError "not found"` — authz
  failures intentionally hide existence.
- `validation_error(msg, attribute: nil)` returns `{ errors: [{ attribute, message }] }` inside a
  200 response; Ruby defaults the attribute to the literal string `"message"` when omitted
  (star-lms will emit `attribute: null` for the same messages — cosmetic divergence, see §7).
- Field names are camelCase (`courseId`, `fileIds`), matching the existing star-graphql queries.

## 2. Per-mutation contracts

### 2.1 Batch A — assignments

Ability map (`app/models/abstract_assignment.rb` `set_policy`, line ~2151):

| Action | Canvas ability | Course right it derives from |
| --- | --- | --- |
| create | `:create` | `manage_assignments_add` |
| update | `:update` (`user_can_update?`) | `manage_assignments_edit` |
| delete | `:delete` | `manage_assignments_delete` |
| submit | `:submit` | `participate_as_student` + assignment unlocked/visible + active enrollment |
| grade | `:grade` | `manage_grades` |

star-lms equivalent for all teacher-side abilities: `check_teacher_or_admin`
(`migration/crates/star-assignments/src/lib.rs:461`); student-side: `check_enrolled_or_admin`
(line 439).

**CreateAssignment** (`app/graphql/mutations/create_assignment.rb`)

```graphql
mutation CreateAssignment($input: CreateAssignmentInput!) {
  createAssignment(input: $input) { assignment { id name state } errors { attribute message } }
}
input CreateAssignmentInput {
  courseId: ID!          # Course
  name: String!
  description: String
  pointsPossible: Float
  dueAt: DateTime
  unlockAt: DateTime
  lockAt: DateTime
  state: AssignmentState   # published | unpublished (enum also has deleted/duplicating/...; not accepted here)
  allowedExtensions: [String!]
  submissionTypes: [String!]
}
type CreateAssignmentPayload { assignment: AssignmentType errors: [ValidationErrorType] }
```

- Permission: load course, build assignment, then `raise GraphQL::ExecutionError,
  "invalid course: {courseId}"` unless `assignment.grants_right?(current_user, :create)`. A
  missing course and an unauthorized caller produce the **same** message — Canvas conflates them
  deliberately; star-lms must do the same (no existence leak).
- `state` handling: workflow_state starts `"unpublished"`; `state: "unpublished"` →
  `published = false`, `state: "published"` → `published = true`; any other enum value raises
  internally.
- Model validation failures return as payload `errors` (attribute = input field name, e.g.
  `name`), via `errors_for`.
- star-lms mapping: reuse the insert from `create_assignment` (star-assignments
  `src/lib.rs:1096`) — `check_teacher_or_admin` gate, `title` required, `points_possible >= 0`;
  GraphQL adds the `state` → `published` column mapping (missing today, §4).

**UpdateAssignment** (`app/graphql/mutations/update_assignment.rb`)

```graphql
input UpdateAssignmentInput { id: ID! name: String description: String pointsPossible: Float
  dueAt: DateTime unlockAt: DateTime lockAt: DateTime state: AssignmentState
  allowedExtensions: [String!] submissionTypes: [String!] }
type UpdateAssignmentPayload { assignment: AssignmentType errors: [ValidationErrorType] }
```

- `Assignment.find` misses → `GraphQL::ExecutionError "assignment not found: {id}"`.
- Without `:update` → `GraphQL::ExecutionError "insufficient permission"`.
- `state` routing (`assignment_base.rb` `ensure_destroyed`/`ensure_restored`, lines ~306–325):
  - `"deleted"` → destroy path, **re-checks `:delete`** (`manage_assignments_delete`), raises
    `"insufficient permission"` without it; sets `workflow_state = "deleted"` (soft, idempotent —
    returns early when already deleted).
  - `"published"`/`"unpublished"` → `published` bool **and** `ensure_restored` when the row is
    currently deleted (restore also requires `:delete`).
- **There is no DeleteAssignment mutation in Canvas.** Deletion is `UpdateAssignment` with
  `state: deleted`; star-lms mirrors exactly — do not invent `deleteAssignment`.
- star-lms mapping: `update_assignment` (`src/lib.rs:1164`). Note the REST `delete_assignment`
  (`src/lib.rs:1213`) does a hard `DELETE`; the GraphQL destroy path must be a soft
  `workflow_state = 'deleted'` update once the column exists (§4) — flag REST hard-delete as a
  separate parity gap, out of scope here.

### 2.2 Batch B — submissions (create + grade/comment)

Canvas has **no plain "update submission" mutation**; the update surface is
`UpdateSubmissionGrade` + `CreateSubmissionComment`. This batch is exactly those.

**CreateSubmission** (`app/graphql/mutations/create_submission.rb`)

```graphql
input CreateSubmissionInput {
  assignmentId: ID!
  submissionType: OnlineSubmissionType!   # basic_lti_launch | student_annotation | media_recording | online_text_entry | online_upload | online_url
  body: String
  fileIds: [ID!]
  mediaId: ID
  url: String
  studentId: ID                           # proxy submission (teacher)
  annotatableAttachmentId: ID
  resourceLinkLookupUuid: String
}
type CreateSubmissionPayload { submission: SubmissionType errors: [ValidationErrorType] }
```

- Lookup: `Assignment.active.find` → `ActiveRecord::RecordNotFound` rescued to
  `GraphQL::ExecutionError "not found"`.
- Authorization, in order: `verify_authorized_action!(assignment, :read)` (→ `"not found"`);
  then if `studentId` present `verify_authorized_action!(assignment.course,
  :proxy_assignment_submission)` else `verify_authorized_action!(assignment, :submit)`
  (→ `"not found"` each). `:submit` = `participate_as_student` + assignment unlocked + visible +
  active enrollment.
- Validation errors (exact messages; attribute per Canvas):
  | Message | Attribute | Trigger |
  | --- | --- | --- |
  | `LTI submissions require a URL to submit` | none | `basic_lti_launch` with blank `url` |
  | `media_recording submissions require a media_id to submit` | none | `media_recording` without `mediaId` |
  | `The media_id does not correspond to an existing media object` | none | unknown `mediaId` |
  | `Student Annotation submissions require an annotatable_attachment_id to submit` | none | `student_annotation` without annotatable attachment |
  | `You must attach at least one file to this assignment` | `file_ids` | `online_upload` with no resolvable attachments |
  | `No attachments found for the following ids: {ids}` | `file_ids` | some `fileIds` unresolvable |
  | `Invalid file type` | `file_ids` | extension not in `allowedExtensions` |
- star-lms mapping: own-submission upsert from `create_submission` (`src/lib.rs:1244`, INSERT …
  ON CONFLICT), `online_upload` via the `submission_files` table (migration 0060); attachments
  must belong to the submitting user. `online_url`/`basic_lti_launch` are **deferred** — the
  `submissions.url` column does not exist (§4). Proxy submissions (`studentId`) may ship later in
  the batch behind `check_teacher_or_admin` (§7).

**UpdateSubmissionGrade** (`app/graphql/mutations/update_submission_grade.rb`)

Canvas `graphql_name` is `"UpdateSubmissionsGrade"` (plural — a Canvas quirk; see §7).

```graphql
input UpdateSubmissionGradeInput { score: Int! submissionId: ID! }
type UpdateSubmissionGradePayload {
  submission: SubmissionType
  parentAssignmentSubmission: SubmissionType   # checkpointed discussions only; null in star-lms
  errors: [ValidationErrorType] }
```

- `Submission.find` (RecordNotFound → top-level error). Authorization is **per submission inside
  the payload, not an ExecutionError**: without `:grade` (`manage_grades`) the payload returns
  `errors: [{ attribute: "{submissionId}", message: "Not authorized to score Submission" }]` and
  `submission: null` — HTTP 200. star-lms equivalent: `grade_submission` handler
  (`src/lib.rs:1359`) with `check_teacher_or_admin`; on failure emit the same payload shape.
- Reuse the handler's UPDATE (`score`, `graded_at`, `grader_id`, `workflow_state = 'graded'`).
  Keep `score: Int!` as in Canvas even though the column is NUMERIC(10,2) (§7).

**CreateSubmissionComment** (`app/graphql/mutations/create_submission_comment.rb`)

```graphql
input CreateSubmissionCommentInput { comment: String! submissionId: ID! attempt: Int
  fileIds: [ID!] mediaObjectId: ID mediaObjectType: String draftComment: Boolean = false
  groupComment: Boolean = false reviewerSubmissionId: ID }
type CreateSubmissionCommentPayload { submissionComment: SubmissionCommentType errors: [ValidationErrorType] }
```

- `Submission.find` → RecordNotFound `GraphQL::ExecutionError "not found"`;
  `verify_authorized_action!(submission, :comment)` → `"not found"`.
- Unknown `mediaObjectId` or any `fileIds` count mismatch → `GraphQL::ExecutionError "not found"`;
  attachments additionally need `:attach_to_submission_comment`.
- star-lms mapping: `create_comment_for` in star-submission-comments (`src/lib.rs:260`, REST
  entry `create_comment` at 349) with `check_can_view_or_comment` (`src/lib.rs:229`): enrolled
  students comment on their own submission, teachers/admins on any — that is the `:comment`
  ability equivalent.

### 2.3 Batch C — discussion topics

Create/authorization semantics (`app/models/discussion_topic.rb`):

- `user_can_create` (line ~1543) = `!is_announcement && context.grants_any_right?(user, :create_forum, :moderate_forum) && context_allows_user_to_create?(user)`; `CreateDiscussionTopic` calls `verify_authorized_action!(topic, :create)` → `"not found"` on failure.
- `:update` / `:delete` for teachers = context `read_forum` **and** `moderate_forum`
  (`grants_all_rights?`). For students: own topic (`topic.user == user`) with
  `discussion_entries.active.empty?` (delete) via `user_can_manage_own_discussion_posts?`
  (`course.allow_student_discussion_editing? || user_is_instructor?`) plus
  `participate_as_student` (`set_policy`, lines ~1469–1520).
- star-lms first cut: teacher/admin only via `check_teacher_or_admin`
  (star-discussions `src/lib.rs:254`), matching the existing REST handlers; the student
  own-topic path needs `discussion_topics.user_id` (§4) and ships with it.

**CreateDiscussionTopic** (`app/graphql/mutations/create_discussion_topic.rb`)

```graphql
input CreateDiscussionTopicInput {
  contextId: ID!                    # Course | Group
  contextType: DiscussionTopicContextType!
  title: String
  message: String
  discussionType: DiscussionTopicDiscussionType   # side_comment | threaded | flat | not_threaded
  published: Boolean
  locked: Boolean
  isAnnouncement: Boolean = false
  anonymousState: DiscussionTopicAnonymousState    # deferred (needs anonymous columns)
  delayedPostAt: DateTime  lockAt: DateTime }
type CreateDiscussionTopicPayload { discussionTopic: DiscussionType errors: [ValidationErrorType] }
```

- Context resolution: `find_context(input)`; failure → payload error `"Invalid context"`
  (no attribute).
- `workflow_state` = `"active"` when `published` (or announcement) else `"unpublished"`;
  `discussion_type` defaults to `threaded` (Canvas) — note star-lms REST defaults
  `side_comment`; GraphQL follows Canvas.
- Validation errors (exact): `"Cannot set default thread state locked, when threads are
  collapsed"` when `expanded: false` and `expanded_locked: true`; announcement section errors
  `"You do not have permissions to modify discussion for section(s) {section_ids}"` (defer — no
  section visibilities yet).
- star-lms mapping: `create_topic` (`src/lib.rs:278`) — title non-empty, discussion_type
  whitelist, insert with `workflow_state`; add the `published` mapping already proven in
  `update_topic`. `pinned` is a star-lms REST-only field (no Canvas mutation input) and stays
  REST-only.

**UpdateDiscussionTopic** (`app/graphql/mutations/update_discussion_topic.rb`)

```graphql
input UpdateDiscussionTopicInput { discussionTopicId: ID! title: String message: String
  discussionType: DiscussionTopicDiscussionType published: Boolean locked: Boolean
  removeAttachment: Boolean anonymousState: DiscussionTopicAnonymousState }
type UpdateDiscussionTopicPayload { discussionTopic: DiscussionType errors: [ValidationErrorType] }
```

- `DiscussionTopic.find` then `raise GraphQL::ExecutionError "insufficient permission"` unless
  `grants_right?(current_user, :update)`.
- Validation errors (exact): `"Anonymity settings are locked due to a posted reply"` when
  `anonymous_state` is set and replies exist; `"Cannot set default thread state locked, when
  threads are collapsed"` (same as create); `"Cannot disable threaded replies when there are
  threaded messages under this discussion"`.
- star-lms mapping: `update_topic` (`src/lib.rs:363`) — fetch non-deleted row (404 → top-level
  `"not found"`), `check_teacher_or_admin`, coalesce partial updates; already maps `published`
  onto `workflow_state`.

**DeleteDiscussionTopic** (`app/graphql/mutations/delete_discussion_topic.rb`)

```graphql
input DeleteDiscussionTopicInput { id: ID! }
type DeleteDiscussionTopicPayload { discussionTopicId: ID! errors: [ValidationErrorType] }
```

- `DiscussionTopic.active.find_by(id:)` missing **or** without `:read` →
  `GraphQL::ExecutionError "Unable to find Discussion Topic"`.
- Without `:delete` → `GraphQL::ExecutionError "Insufficient permissions"` (capital I, plural —
  exact strings).
- star-lms mapping: `delete_topic` (`src/lib.rs:432`) — already a soft delete
  (`workflow_state = 'deleted'`); GraphQL returns the id in `discussionTopicId`.

### 2.4 Batch D — CreateEnrollment (documented deviation)

**Canvas has no enrollment-create GraphQL mutation** — its only enrollment mutations are
`accept_enrollment_invitation.rb` / `reject_enrollment_invitation.rb`. This spec therefore
proposes a **new** `CreateEnrollment` whose checks mirror Canvas REST
`EnrollmentsApiController#create` (`app/controllers/enrollments_api_controller.rb`). This is a
documented deviation: no Canvas GraphQL payload semantics exist to copy; the REST right checks
and error strings are the parity source.

```graphql
input CreateEnrollmentInput {
  courseId: ID!
  userId: ID!
  type: EnrollmentType!              # StudentEnrollment | TeacherEnrollment | TaEnrollment | DesignerEnrollment | ObserverEnrollment
  role: String                       # custom role name, optional
  enrollmentState: String            # invited | active | ...
  courseSectionId: ID }
type CreateEnrollmentPayload { enrollment: EnrollmentType errors: [ValidationErrorType] }
```

- Permission (Canvas `User#can_create_enrollment_for?`, `app/models/user.rb:3474`): the caller
  needs the course right matching the type — `add_teacher_to_course`, `add_ta_to_course`,
  `add_designer_to_course`, `add_student_to_course`, `add_observer_to_course`. Canvas REST
  renders unauthorized; in GraphQL map to `GraphQL::ExecutionError "insufficient permission"`
  (consistent with the other star-lms mutations). star-lms equivalent: teacher/admin in the
  course (`check_teacher_or_admin`), refined per-type once role overrides exist.
- Validation errors (exact REST strings, `@@errors` map at controller line ~333):
  | Message | Trigger |
  | --- | --- |
  | `Can't create an enrollment without a user. Include enrollment[user_id] to create an enrollment` | missing user |
  | `Invalid type` | type not one of the five |
  | `Invalid role` | unknown custom role |
  | `Can't add an enrollment to a concluded course.` | course `workflow_state = 'completed'` |
- star-lms mapping: `create_enrollment` (star-enrollments `src/lib.rs:477`) — course/user
  existence checks, section-in-course check, insert with `workflow_state`. **Gap:** its
  `validate_enrollment_role` (`src/lib.rs:357`) accepts `student|teacher|ta` only; Canvas has
  five enrollment types (role strings only in star-lms, no `type` distinction — §4). Until the
  role model widens, the mutation accepts the three and returns Canvas's `"Invalid type"` for
  `DesignerEnrollment`/`ObserverEnrollment`, with the gap flagged in §7.

### 2.5 Batch E — conversations

**CreateConversation** (`app/graphql/mutations/create_conversation.rb`)

```graphql
input CreateConversationInput {
  recipients: [String!]!
  body: String!
  subject: String
  contextCode: String
  groupConversation: Boolean
  bulkMessage: Boolean
  forceNew: Boolean
  attachmentIds: [ID!]
  mediaCommentId: ID  mediaCommentType: String
  tags: [String!] }
type CreateConversationPayload { conversations: [ConversationParticipantType] errors: [ValidationErrorType] }
```

- Recipient count cap: `Conversation.max_group_conversation_size` =
  `Setting.get("max_group_conversation_size", 100)` (`app/models/conversation.rb:160`).
- Validation errors (exact; attribute per Canvas):
  | Message | Attribute | Trigger |
  | --- | --- | --- |
  | `Context cannot be blank` | `context_code` | no resolvable context **and** caller lacks root-account `read_roster` |
  | `Too many recipients for group conversation` | `recipients` | non-batch send with > 100 recipients |
  | `Invalid recipients` | none | recipients blank after normalization |
  | `No context found for the following context code: {code}` | `context_code` | unknown contextCode |
  | `Unable to send messages to users in {context_name}` | `permissions` | no right to message those recipients in the context |
  | `Course concluded, unable to send messages` | none | course `workflow_state = 'completed'` |
- `ActiveRecord::RecordNotFound` → `GraphQL::ExecutionError "not found"` (e.g. bad
  `conversationId` on add-message paths).
- star-lms mapping: `create_conversation` (star-conversations `src/lib.rs:445`) — validates
  recipients/subject/body, transactionally creates conversation + participants (author read,
  recipients unread) + first message. Payload shape differs: REST returns a flat
  `CreatedConversation`; GraphQL must return `ConversationParticipantType` entries per
  recipient. Context-scoped sends need `conversations.context_type/context_id` (§4);
  the context-less core ships without them. Canvas's `read_roster` gate for context-less sends
  is stricter than star-lms REST (any authenticated user) — decision in §7.

## 3. Error mapping

Three error channels, never mixed:

| Channel | Transport | When | star-lms implementation |
| --- | --- | --- | --- |
| Unauthenticated | HTTP 401, non-GraphQL body, **before execution** | missing/garbage bearer token | `AuthUser` extractor in the router (already tested by `graphql_integration.rs`) — mutations inherit it, no code needed |
| Execution error | HTTP 200, top-level `errors[]`, `data.<field>` null | authz failure, record not found, DB failure | `async_graphql::Error::new(msg)`; `db_err(e)` → `"database error"` (never leak SQL) |
| Validation error | HTTP 200, payload `errors: [{attribute, message}]` | input/validation failures | payload field `errors: Option<Vec<ValidationError>>` on every mutation payload type |

`ValidationErrorType` in star-lms:

```rust
#[derive(SimpleObject)]
struct ValidationError { attribute: Option<String>, message: String }
```

Canvas → star-lms error conventions:

| Canvas (Ruby) | Canvas output | star-lms (async-graphql 7) |
| --- | --- | --- |
| `verify_authorized_action!(obj, perm)` | `GraphQL::ExecutionError "not found"` | `Err(Error::new("not found"))` |
| `validation_error(msg, attribute: "file_ids")` | `{errors: [{attribute: "file_ids", message: msg}]}` | `Ok(Payload { errors: Some(vec![ValidationError{attribute: Some("file_ids"), message}]), ..None })` |
| `validation_error(msg)` (no attribute) | attribute defaults to literal `"message"` | `attribute: None` (divergence, §7) |
| `ActiveRecord::RecordNotFound` in CreateSubmission/CreateConversation | `"not found"` | `Err(Error::new("not found"))` |
| `raise GraphQL::ExecutionError, "invalid course: {id}"` | top-level error | `Err(Error::new(format!("invalid course: {id}")))` |
| `"assignment not found: {id}"`, `"insufficient permission"`, `"Unable to find Discussion Topic"`, `"Insufficient permissions"` | top-level errors | same strings via `Error::new` |
| `UpdateSubmissionGrade` per-submission authz | payload `errors: [{attribute: submissionId, message: "Not authorized to score Submission"}]` | payload error, `attribute: Some(submission_id.to_string())` |
| sqlx failure | (n/a) | `db_err` → `Error::new("database error")` |

Authz negatives for students must return the **same** `"not found"`/`"insufficient permission"`
strings as missing records wherever Canvas conflates them (CreateAssignment intentionally
conflates via `"invalid course: {id}"`).

## 4. Data model needs

Existing columns are complete unless listed. All changes are additive `ALTER TABLE … ADD COLUMN
IF NOT EXISTS` migrations following the star-db conventions (latest: 0107).

| Batch | Table | Gap | Migration needed |
| --- | --- | --- | --- |
| A | `assignments` (0001) | **no `published`/`workflow_state` column** — biggest gap in this spec; blocks `state` input, soft delete, restore | add `workflow_state TEXT NOT NULL DEFAULT 'unpublished' CHECK (… IN ('unpublished','published','deleted'))` (Canvas semantics: `published` bool ⇄ state; `deleted` for destroy) |
| B | `submissions` (0001 + 0060) | no `url` column — blocks `online_url` and `basic_lti_launch` submission types | add `url TEXT` when those types ship; `online_text_entry` + `online_upload` (via `submission_files`) + grading (`graded_at`, `grader_id`, `workflow_state`) already exist |
| C | `discussion_topics` (0017) | no `user_id` (author) — blocks student own-topic update/delete and anonymity; no anonymous columns | add `user_id BIGINT REFERENCES users(id) ON DELETE SET NULL` with the student-editing path; anonymous columns deferred with `anonymous_state` |
| D | `enrollments` (0001 + 0010) | role strings `student|teacher|ta` only, no Canvas `type` distinction (5 types) and no custom-role support | widen role whitelist / add `type` column when Designer/Observer ship; `workflow_state` (0010) already covers concluded/active |
| E | `conversations` (0020) | no `context_type`/`context_id` — blocks context-scoped sends and their error paths | add `context_type TEXT`, `context_id BIGINT` (nullable) when contextCode ships; participants/messages tables already sufficient |

## 5. Test plan

All tests are DB-backed integration tests in
`migration/crates/star-graphql/tests/` (extend `graphql_integration.rs` or add
`graphql_mutations_integration.rs` following it exactly):

- `setup()` returns `None` without `DATABASE_URL` → `println!("SKIP: DATABASE_URL not set")`;
- `run_prefix()` unique per run (pid + timestamp) so shared databases never collide;
- full `cleanup()` teardown of every seeded row;
- `seed_user` / `seed_course` / `seed_enrollment` helpers + `token_for`/`auth` bearer minting;
- `gql_post` asserts HTTP 200 and returns the JSON body (payload `errors` are 200-responses);
  execution errors are asserted as top-level `errors[]` with `data.<mutation>` null;
- unauthenticated mutation calls reuse the existing 401-pre-execution test pattern (assert the
  body is not a GraphQL error shape).

Per batch (each list = one `#[tokio::test]`):

- **A** `create_assignment_positive_teacher` (teacher creates; returns id/name/state);
  `create_assignment_student_gets_invalid_course` (assert exact `"invalid course: {id}"` in
  top-level errors — same for a nonexistent course id);
  `create_assignment_outsider_unauthorized`;
  `update_assignment_not_found` (`"assignment not found: {id}"`);
  `update_assignment_student_insufficient_permission` (`"insufficient permission"`);
  `update_assignment_delete_via_state_deleted` (teacher; row afterwards has
  workflow_state `deleted`, idempotent second call);
  `update_assignment_state_deleted_student_insufficient_permission`;
  `update_assignment_validation_name_required` (payload errors attribute `name`).
- **B** `create_submission_text_entry_positive` (student; body stored, upsert on repeat);
  `create_submission_not_enrolled_not_found`;
  `create_submission_lti_requires_url` (exact message, attribute null);
  `create_submission_upload_requires_file` (`"You must attach at least one file to this
  assignment"`, attribute `file_ids`);
  `create_submission_upload_unknown_file_ids` (`"No attachments found for the following ids:
  [..]"`, attribute `file_ids`);
  `create_submission_invalid_file_type` (assignment with allowed_extensions; `"Invalid file
  type"`, attribute `file_ids`);
  `update_submission_grade_positive_teacher`; `update_submission_grade_student_not_authorized`
  (payload errors `[{attribute: "<id>", message: "Not authorized to score Submission"}]`);
  `create_submission_comment_positive_student_own`; `create_submission_comment_outsider_not_found`;
  `create_submission_comment_unknown_submission_not_found`.
- **C** `create_topic_positive_teacher`; `create_topic_invalid_context` (payload error
  `"Invalid context"`); `create_topic_student_not_found`; `update_topic_insufficient_permission`
  (student; `"insufficient permission"`); `update_topic_cannot_lock_collapsed_threads` (exact
  message); `delete_topic_positive_returns_discussion_topic_id`;
  `delete_topic_not_found` (`"Unable to find Discussion Topic"`); `delete_topic_student`
  (`"Insufficient permissions"`); `unauthenticated_mutation_is_401` (one per batch is enough —
  the extractor is shared).
- **D** `create_enrollment_positive_teacher_adds_student`;
  `create_enrollment_student_insufficient_permission`;
  `create_enrollment_missing_user` (exact `"Can't create an enrollment without a user. Include
  enrollment[user_id] to create an enrollment"`); `create_enrollment_invalid_type`
  (`"Invalid type"`); `create_enrollment_invalid_role` (`"Invalid role"`);
  `create_enrollment_concluded_course` (`"Can't add an enrollment to a concluded course."` —
  seed course `workflow_state = 'completed'`).
- **E** `create_conversation_positive_creates_participants_and_message`;
  `create_conversation_empty_recipients` (`"Invalid recipients"`);
  `create_conversation_too_many_recipients` (101 recipients; attribute `recipients`);
  `create_conversation_context_cannot_be_blank` (attribute `context_code`);
  `create_conversation_bad_context_code` (exact interpolated message, attribute `context_code`);
  `create_conversation_concluded_course` (course completed; exact message).

Exact-string assertions use the messages in §2 verbatim (they are part of the parity surface).

## 6. Batching recommendation

Recommended order: **C → B → A → E → D**.

1. **C (discussions) first** — zero new migrations for the teacher/admin path: `create_topic`,
   `update_topic`, `delete_topic` REST handlers exist and the table already has
   `workflow_state` (soft delete + published mapping) and `discussion_type`. Only the student
   own-topic path waits on `user_id`. Best ratio of Canvas parity per unit of work.
2. **B (submissions) second** — handlers exist (`create_submission`, `grade_submission`,
   star-submission-comments); `online_text_entry`, `online_upload` (via `submission_files`,
   migration 0060) and grading need no schema change. Only `url`-typed submissions wait on one
   column.
3. **A (assignments) third** — highest-value surface and complete handlers, but **blocked on the
   `published`/`workflow_state` column** (§4): the `state` input, soft delete and restore all
   need it. One small migration unblocks the whole batch; land it early in parallel with C/B.
4. **E (conversations) fourth** — the context-less core (DM with recipients/body/subject, cap
   100) needs no migration; the Canvas context semantics (`read_roster` gate, contextCode
   resolution, concluded-course errors) carry the most design uncertainty (§7) and can land
   incrementally after the two context columns exist.
5. **D (enrollment create) last** — documented deviation with an open role-model decision
   (Canvas 5 types vs star-lms 3). Ship once §7 Q4 is decided; the REST handler already covers
   the happy path.

## 7. Open questions

1. **IDs**: Canvas mutations accept relay-or-legacy IDs (`GraphQLHelpers.parse_relay_or_legacy_id`);
   the star-graphql foundation uses plain stringified i64 `ID!`. Propose staying with plain IDs
   and rejecting non-numeric input as `"not found"` — confirm.
2. **Mutation name quirk**: Canvas's `graphql_name` is `"UpdateSubmissionsGrade"` (plural, the
   class is `UpdateSubmissionGrade`). Keep the exact Canvas name for client compatibility, or
   normalize? Propose keeping the Canvas name.
3. **`score: Int!` vs NUMERIC(10,2)**: Canvas types the grade input as `Int` while star-lms
   stores NUMERIC. Propose `Int` for byte-parity; decimals via a later mutation or REST.
4. **Enrollment types**: widen star-lms roles to Canvas's five enrollment types (add
   `designer`, `observer`) or introduce a `type` column distinct from `role` (Canvas's custom
   roles)? Affects REST `validate_enrollment_role` and this mutation's `"Invalid type"`/`"Invalid
   role"` split.
5. **Context-less conversations**: Canvas requires root-account `read_roster` to send without a
   context (`"Context cannot be blank"`); star-lms REST allows any authenticated user to DM.
   Keep the permissive REST rule for GraphQL (recommended — matches deployed behavior and
   tests) or adopt Canvas's gate via star-permissions role overrides?
6. **Recipient cap configurability**: Canvas's `max_group_conversation_size` is a
   Setting (default 100). Hardcode 100 in star-lms or make it configurable (env/internal
   settings)?
7. **Attribute-less validation errors**: Canvas defaults the `attribute` to the literal
   `"message"`; this spec uses `attribute: null`. Confirm the divergence is acceptable
   (client-visible but cosmetic).
8. **Proxy submissions**: `CreateSubmission` with `studentId` (teacher submitting on behalf,
   `:proxy_assignment_submission`). REST `create_submission` is own-submission only — implement
   in batch B or defer to a follow-up? Propose defer.
