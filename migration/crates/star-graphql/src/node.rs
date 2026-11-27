//! Canvas GraphQL `Node` — the interface behind `Query.node` and
//! `Query.legacyNode` — plus Canvas' id parsing.
//!
//! Canvas (`app/graphql/types/query_type.rb`):
//!
//! - `node(id: ID!)` takes a **global** relay id: `base64("Type-id")`, the
//!   form `CanvasSchema.id_from_object` produces and `object_from_id`
//!   decodes (`GraphQL::Schema::UniqueWithinType`).
//! - `legacyNode(_id: ID!, type: NodeType!)` takes the plain legacy id plus
//!   the type enum. This is the form the Canvas frontend actually sends
//!   (`ui/features/inbox/graphql/Queries.js`,
//!   `ui/features/speed_grader/react/CommentLibraryV2/graphql/queries.ts`).
//!
//! Every other `id` argument in the schema accepts "a graphql or legacy id":
//! `GraphQLHelpers.parse_relay_or_legacy_id` takes the plain numeric Canvas
//! id, or a type-scoped relay id, and rejects a relay id of the wrong type
//! with `expected an id for <Type>`. `parse_relay_or_legacy_id` below is
//! that function, ported.
//!
//! **Deviation (ids).** The port exposes plain numeric ids on every object
//! type (that is what the REST surface and the existing GraphQL tests
//! expect), where Canvas returns the type-scoped relay form. `node(id:)`
//! therefore accepts the Canvas global relay id, and `legacyNode(_id:, type:)`
//! accepts the plain legacy id — the same pair Canvas accepts. The `id`
//! returned by the `Node` interface is the port's plain numeric id.

use async_graphql::{Context, Enum, ID};

use crate::{
    assignment::Assignment,
    conversations::Conversation,
    course_user_fields::Account,
    discussion_topic::DiscussionTopic,
    files::{File, Folder},
    groups::{Group, GroupSet},
    modules::{Module, ModuleItem},
    pages::{Page, Progress},
    root_types::{
        AssignmentGroup, InstitutionalTag, InstitutionalTagCategory, InternalSetting,
        LearningOutcome, LearningOutcomeGroup, OutcomeCalculationMethod, OutcomeProficiency,
        PeerReviewSubAssignment, Rubric,
    },
    submission::Submission,
    terms_sections::{CourseSection, EnrollmentTerm},
    Course, Enrollment, User,
};

// ---------------------------------------------------------------------------
// NodeType
// ---------------------------------------------------------------------------

/// Canvas `NodeType` enum (`app/graphql/types/legacy_node_type.rb`; the 37
/// members of `enum NodeType` in Canvas' `schema.graphql`). The members are
/// PascalCase in Canvas, so every variant carries an explicit GraphQL name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum NodeType {
    #[graphql(name = "Account")]
    Account,
    #[graphql(name = "AllocationRule")]
    AllocationRule,
    #[graphql(name = "Assignment")]
    Assignment,
    #[graphql(name = "AssignmentGroup")]
    AssignmentGroup,
    #[graphql(name = "Conversation")]
    Conversation,
    #[graphql(name = "Course")]
    Course,
    #[graphql(name = "Discussion")]
    Discussion,
    #[graphql(name = "DiscussionEntry")]
    DiscussionEntry,
    #[graphql(name = "Enrollment")]
    Enrollment,
    #[graphql(name = "File")]
    File,
    #[graphql(name = "Folder")]
    Folder,
    #[graphql(name = "GradingPeriod")]
    GradingPeriod,
    #[graphql(name = "GradingPeriodGroup")]
    GradingPeriodGroup,
    #[graphql(name = "Group")]
    Group,
    #[graphql(name = "GroupSet")]
    GroupSet,
    #[graphql(name = "InstitutionalTag")]
    InstitutionalTag,
    #[graphql(name = "InstitutionalTagAssociation")]
    InstitutionalTagAssociation,
    #[graphql(name = "InstitutionalTagCategory")]
    InstitutionalTagCategory,
    #[graphql(name = "InternalSetting")]
    InternalSetting,
    #[graphql(name = "LearningOutcomeGroup")]
    LearningOutcomeGroup,
    #[graphql(name = "MediaObject")]
    MediaObject,
    #[graphql(name = "Module")]
    Module,
    #[graphql(name = "ModuleItem")]
    ModuleItem,
    #[graphql(name = "ModuleProgression")]
    ModuleProgression,
    #[graphql(name = "OutcomeCalculationMethod")]
    OutcomeCalculationMethod,
    #[graphql(name = "OutcomeProficiency")]
    OutcomeProficiency,
    #[graphql(name = "Page")]
    Page,
    #[graphql(name = "PeerReviewSubAssignment")]
    PeerReviewSubAssignment,
    #[graphql(name = "PostPolicy")]
    PostPolicy,
    #[graphql(name = "Progress")]
    Progress,
    #[graphql(name = "Rubric")]
    Rubric,
    #[graphql(name = "Section")]
    Section,
    #[graphql(name = "SubAssignment")]
    SubAssignment,
    #[graphql(name = "Submission")]
    Submission,
    #[graphql(name = "Term")]
    Term,
    #[graphql(name = "UsageRights")]
    UsageRights,
    #[graphql(name = "User")]
    User,
}

impl NodeType {
    /// The Canvas type name this enum member stands for — the same string
    /// Canvas uses to build the relay id (`CanvasSchema.id_from_object`
    /// encodes the *GraphQL* type name, not the model class name).
    pub fn type_name(self) -> &'static str {
        match self {
            NodeType::Account => "Account",
            NodeType::AllocationRule => "AllocationRule",
            NodeType::Assignment => "Assignment",
            NodeType::AssignmentGroup => "AssignmentGroup",
            NodeType::Conversation => "Conversation",
            NodeType::Course => "Course",
            NodeType::Discussion => "Discussion",
            NodeType::DiscussionEntry => "DiscussionEntry",
            NodeType::Enrollment => "Enrollment",
            NodeType::File => "File",
            NodeType::Folder => "Folder",
            NodeType::GradingPeriod => "GradingPeriod",
            NodeType::GradingPeriodGroup => "GradingPeriodGroup",
            NodeType::Group => "Group",
            NodeType::GroupSet => "GroupSet",
            NodeType::InstitutionalTag => "InstitutionalTag",
            NodeType::InstitutionalTagAssociation => "InstitutionalTagAssociation",
            NodeType::InstitutionalTagCategory => "InstitutionalTagCategory",
            NodeType::InternalSetting => "InternalSetting",
            NodeType::LearningOutcomeGroup => "LearningOutcomeGroup",
            NodeType::MediaObject => "MediaObject",
            NodeType::Module => "Module",
            NodeType::ModuleItem => "ModuleItem",
            NodeType::ModuleProgression => "ModuleProgression",
            NodeType::OutcomeCalculationMethod => "OutcomeCalculationMethod",
            NodeType::OutcomeProficiency => "OutcomeProficiency",
            NodeType::Page => "Page",
            NodeType::PeerReviewSubAssignment => "PeerReviewSubAssignment",
            NodeType::PostPolicy => "PostPolicy",
            NodeType::Progress => "Progress",
            NodeType::Rubric => "Rubric",
            NodeType::Section => "Section",
            NodeType::SubAssignment => "SubAssignment",
            NodeType::Submission => "Submission",
            NodeType::Term => "Term",
            NodeType::UsageRights => "UsageRights",
            NodeType::User => "User",
        }
    }

    /// The [`NodeType`] member with this Canvas type name.
    ///
    /// Used by `Query.node(id:)`, whose argument is the global relay id
    /// `base64("Type-id")` — the type name is the part before the dash.
    pub fn from_type_name(name: &str) -> Option<NodeType> {
        ALL_NODE_TYPES
            .iter()
            .copied()
            .find(|node_type| node_type.type_name() == name)
    }
}

/// Every Canvas `NodeType` member, in Canvas' declaration order.
const ALL_NODE_TYPES: [NodeType; 37] = [
    NodeType::Account,
    NodeType::AllocationRule,
    NodeType::Assignment,
    NodeType::AssignmentGroup,
    NodeType::Conversation,
    NodeType::Course,
    NodeType::Discussion,
    NodeType::DiscussionEntry,
    NodeType::Enrollment,
    NodeType::File,
    NodeType::Folder,
    NodeType::GradingPeriod,
    NodeType::GradingPeriodGroup,
    NodeType::Group,
    NodeType::GroupSet,
    NodeType::InstitutionalTag,
    NodeType::InstitutionalTagAssociation,
    NodeType::InstitutionalTagCategory,
    NodeType::InternalSetting,
    NodeType::LearningOutcomeGroup,
    NodeType::MediaObject,
    NodeType::Module,
    NodeType::ModuleItem,
    NodeType::ModuleProgression,
    NodeType::OutcomeCalculationMethod,
    NodeType::OutcomeProficiency,
    NodeType::Page,
    NodeType::PeerReviewSubAssignment,
    NodeType::PostPolicy,
    NodeType::Progress,
    NodeType::Rubric,
    NodeType::Section,
    NodeType::SubAssignment,
    NodeType::Submission,
    NodeType::Term,
    NodeType::UsageRights,
    NodeType::User,
];

// ---------------------------------------------------------------------------
// Node interface
// ---------------------------------------------------------------------------

/// Canvas GraphQL `Node` interface (`interface Node { id: ID! }`).
///
/// The variant list is the set of types the port can actually resolve; the
/// `NodeType` enum stays complete (Canvas' 37 members) so a client can
/// send any of them, and an unsupported type resolves to null the way
/// Canvas resolves a missing record.
#[derive(async_graphql::Interface)]
#[graphql(name = "Node", field(name = "id", ty = "ID", method = "node_id"))]
pub enum Node {
    Account(Account),
    Assignment(Assignment),
    AssignmentGroup(AssignmentGroup),
    Conversation(Conversation),
    Course(Course),
    Discussion(DiscussionTopic),
    Enrollment(Enrollment),
    File(File),
    Folder(Folder),
    Group(Group),
    GroupSet(GroupSet),
    InstitutionalTag(InstitutionalTag),
    InstitutionalTagCategory(InstitutionalTagCategory),
    InternalSetting(InternalSetting),
    LearningOutcome(LearningOutcome),
    LearningOutcomeGroup(LearningOutcomeGroup),
    Module(Module),
    ModuleItem(ModuleItem),
    OutcomeCalculationMethod(OutcomeCalculationMethod),
    OutcomeProficiency(OutcomeProficiency),
    Page(Page),
    PeerReviewSubAssignment(PeerReviewSubAssignment),
    Progress(Progress),
    Rubric(Rubric),
    Section(CourseSection),
    Submission(Submission),
    Term(EnrollmentTerm),
    User(User),
}

/// The `id` of a node, as the `Node` interface exposes it.
///
/// The async-graphql `Interface` derive calls `node_id(ctx)` on the concrete
/// type of each variant, so every node-able type implements this trait. It
/// is `pub(crate)` because it is an implementation detail of the interface,
/// not part of the public GraphQL surface.
pub(crate) trait NodeId {
    /// The node id of this object.
    async fn node_id(&self, ctx: &Context<'_>) -> Result<ID, async_graphql::Error>;
}

// ---------------------------------------------------------------------------
// Id parsing (Canvas `GraphQLHelpers`)
// ---------------------------------------------------------------------------

/// Canvas `GraphQLHelpers.parse_relay_or_legacy_id`.
///
/// Accepts the plain numeric Canvas id, or a type-scoped relay id
/// (`base64("Type-id")`). A relay id of a different type — or anything that
/// is neither — is rejected with Canvas' exact message
/// `expected an id for <Type>`.
pub(crate) fn parse_relay_or_legacy_id(
    raw: &str,
    expected_type: &str,
) -> Result<i64, async_graphql::Error> {
    let rejected = || async_graphql::Error::new(format!("expected an id for {expected_type}"));

    if raw.bytes().all(|b| b.is_ascii_digit()) && !raw.is_empty() {
        return raw.parse::<i64>().map_err(|_| rejected());
    }

    let (kind, id) = decode_relay_id(raw).ok_or_else(rejected)?;
    if kind != expected_type {
        return Err(rejected());
    }
    id.parse::<i64>().map_err(|_| rejected())
}

/// Decode a Canvas global relay id (`base64("Type-id")`) into its two parts.
///
/// `GraphQL::Schema::UniqueWithinType.encode` is
/// `Base64.strict_encode64("#{type_name}-#{id}")`, so the decoder is the
/// inverse: strict base64, then split on the first `-`.
pub(crate) fn decode_relay_id(raw: &str) -> Option<(String, String)> {
    let decoded = base64_decode(raw)?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (kind, id) = decoded.split_once('-')?;
    if kind.is_empty() || id.is_empty() {
        return None;
    }
    Some((kind.to_string(), id.to_string()))
}

/// Strict base64 decode (standard alphabet, `=` padding).
///
/// The port has no base64 dependency, and this is the only place one is
/// needed — Canvas' relay ids are the sole use.
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    fn value(b: u8) -> Option<u32> {
        match b {
            b'A'..=b'Z' => Some((b - b'A') as u32),
            b'a'..=b'z' => Some((b - b'a') as u32 + 26),
            b'0'..=b'9' => Some((b - b'0') as u32 + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let bytes = input.as_bytes();
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return None;
    }

    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let (blocks, _) = bytes.as_chunks::<4>();
    for block in blocks {
        let pad = block.iter().rev().take_while(|&&b| b == b'=').count();
        if pad > 2 {
            return None;
        }
        let mut acc = 0u32;
        for (i, &b) in block.iter().enumerate() {
            let v = if b == b'=' && i >= 4 - pad {
                0
            } else {
                value(b)?
            };
            acc = (acc << 6) | v;
        }
        out.push((acc >> 16) as u8);
        if pad < 2 {
            out.push((acc >> 8) as u8);
        }
        if pad < 1 {
            out.push(acc as u8);
        }
    }
    Some(out)
}
