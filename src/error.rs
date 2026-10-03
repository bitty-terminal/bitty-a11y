//! Typed fail-closed errors for every contract fence.
//!
//! Each variant maps to one required control in the failure-case table of
//! `bitty-terminal-docs/specifications/accessibility-extraction-contract.md`.
//! No fence falls back to a default action, a stale focus owner, a partial
//! snapshot, a truncated name, or a privileged path: every failure surfaces
//! here and the caller must re-synchronize or refuse.

use std::fmt;

/// Every way the adapter refuses work, typed so callers cannot mistake a
/// refusal for a success.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum A11yError {
    /// A scene kind (or interactive purpose) has no mapped role.
    ///
    /// Fail-closed: the whole snapshot build aborts before anything is
    /// exposed; no partial snapshot is ever presented.
    UnmappedSceneKind,
    /// An accessible name, label, or state exceeds
    /// [`crate::MAX_ACCESSIBLE_NAME_LEN`] characters.
    ///
    /// Fail-closed: rejected, never silently truncated.
    NameTooLong {
        /// Length in characters of the rejected value.
        len: usize,
        /// The cap that was exceeded.
        cap: usize,
    },
    /// A snapshot build would exceed [`crate::MAX_SNAPSHOT_NODES`] nodes.
    ///
    /// Fail-closed: no partial tree is returned.
    TooManyNodes {
        /// Nodes the build attempted to hold.
        count: usize,
        /// The cap that was exceeded.
        cap: usize,
    },
    /// A handle is forged, foreign (issued by another builder), or out of
    /// range.
    ///
    /// Handles are snapshot-scoped and cannot be forged; the request is
    /// rejected without effect.
    UnknownHandle,
    /// A handle from an earlier projection generation was presented
    /// against the live generation.
    ///
    /// The snapshot (or action) is rejected against the live generation
    /// and the adapter must re-synchronize; nothing is applied.
    StaleGeneration {
        /// Generation the presented handle or snapshot belongs to.
        presented: u64,
        /// The live projection generation.
        live: u64,
    },
    /// The recorded focus owner disagrees with the live focus.
    ///
    /// The adapter asserts no focus, transfers nothing, routes nothing,
    /// and mutates nothing.
    FocusMismatch,
    /// An action label outside the closed v1 action set.
    ///
    /// The v1 class is activation of a declared interactive node; any
    /// other class requires its own reviewed extension and is rejected
    /// here.
    UnknownAction {
        /// The rejected label, echoed for diagnosis.
        label: String,
    },
    /// An action without the required capability or owner grant.
    ///
    /// Fails closed with no default action and no silent execution.
    CapabilityDenied,
    /// The platform's own accessibility permission is not granted.
    ///
    /// The adapter refuses to expose or act; the Core baseline is
    /// unaffected.
    PermissionDenied,
    /// Activation of an interactive node that is currently disabled.
    ///
    /// A disabled control exposes state with no activation.
    NotEnabled,
    /// An action targeted a node outside its action class (for example
    /// activation of a terminal row or a chrome surface).
    ///
    /// The v1 class is activation of a declared interactive node only;
    /// anything else fails closed rather than falling back to a default
    /// action.
    NotActivatable,
    /// A structural invariant was violated (for example a modal overlay
    /// was declared without an overlay surface).
    ///
    /// Fail-closed: the snapshot is not built.
    InvalidStructure,
    /// An announcement text exceeds [`crate::MAX_ANNOUNCEMENT_TEXT_LEN`]
    /// characters.
    ///
    /// Fail-closed: rejected, never silently truncated.
    AnnouncementTooLong {
        /// Length in characters of the rejected text.
        len: usize,
        /// The cap that was exceeded.
        cap: usize,
    },
}

impl fmt::Display for A11yError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnmappedSceneKind => {
                f.write_str("unmapped scene kind: no role mapping; snapshot fails closed")
            }
            Self::NameTooLong { len, cap } => {
                write!(
                    f,
                    "accessible name too long: {len} chars exceeds cap {cap}; rejected, never truncated"
                )
            }
            Self::TooManyNodes { count, cap } => {
                write!(
                    f,
                    "accessibility snapshot too large: {count} nodes exceeds cap {cap}; no partial snapshot"
                )
            }
            Self::UnknownHandle => {
                f.write_str("unknown element handle: forged, foreign, or out of range")
            }
            Self::StaleGeneration { presented, live } => {
                write!(
                    f,
                    "stale generation: presented {presented} against live {live}; re-synchronize"
                )
            }
            Self::FocusMismatch => f.write_str(
                "focus mismatch: recorded owner disagrees with live focus; assert no focus",
            ),
            Self::UnknownAction { label } => {
                write!(
                    f,
                    "unknown action: {label:?} is outside the closed v1 action set"
                )
            }
            Self::CapabilityDenied => {
                f.write_str("capability denied: action lacks the required grant")
            }
            Self::PermissionDenied => f.write_str(
                "platform accessibility permission not granted: refuse to expose or act",
            ),
            Self::NotEnabled => {
                f.write_str("control is disabled: activation not available while disabled")
            }
            Self::NotActivatable => f.write_str(
                "target is not activatable: v1 activation applies to interactive nodes only",
            ),
            Self::InvalidStructure => {
                f.write_str("invalid snapshot structure: violates a structural invariant")
            }
            Self::AnnouncementTooLong { len, cap } => {
                write!(
                    f,
                    "announcement too long: {len} chars exceeds cap {cap}; rejected, never truncated"
                )
            }
        }
    }
}

impl std::error::Error for A11yError {}
