//! Focus association: the projection records focus, Core owns it.
//!
//! Exactly zero or one surface per active workspace is focused; zero
//! occurs only when the window is unfocused, the workspace is empty, or
//! the focused surface was just hidden or detached. The projection
//! records the focused surface's owner for announcement; it never defines
//! focus. If the recorded owner disagrees with the live focus — stale,
//! cleared, or reassigned — the adapter must not assert focus: it
//! reports no focused element, transfers nothing, routes nothing, and
//! mutates nothing.

use crate::A11yError;
use crate::handles::{Anchor, ElementHandle};
use crate::snapshot::Snapshot;

/// Who the projection records as focused.
///
/// Only focusable anchors qualify: the terminal leaf (and its rows) and
/// interactive nodes. Chrome surfaces, scene nodes, and the root can
/// never own focus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusOwner {
    handle: ElementHandle,
    anchor: Anchor,
}

impl FocusOwner {
    /// Binds `handle` to its anchor as a focus owner.
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::UnknownHandle`] for a handle the snapshot
    /// does not know, and [`A11yError::FocusMismatch`] when the anchor
    /// behind the handle can never own focus (chrome, scene, root).
    pub fn bind(
        snapshot: &Snapshot,
        map: &crate::HandleMap,
        handle: ElementHandle,
    ) -> Result<Self, A11yError> {
        let anchor = map
            .anchor(handle)
            .filter(|_| snapshot.get(handle).is_some())
            .ok_or(A11yError::UnknownHandle)?;
        if !anchor.is_focusable() {
            return Err(A11yError::FocusMismatch);
        }
        Ok(Self {
            handle,
            anchor: anchor.clone(),
        })
    }

    /// Handle of the focused element.
    #[must_use]
    pub fn handle(&self) -> ElementHandle {
        self.handle
    }

    /// Anchor of the focused element.
    #[must_use]
    pub fn anchor(&self) -> &Anchor {
        &self.anchor
    }
}

/// The projection's recorded focus owner plus its generation.
///
/// Recorded at ingest time from the surface the host reports as
/// focused; re-checked against the live focus before every focus
/// assertion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusAssociation {
    owner: Option<FocusOwner>,
    generation: crate::Generation,
}

impl FocusAssociation {
    /// Records `owner` (or no focused element) at `generation`.
    #[must_use]
    pub const fn new(owner: Option<FocusOwner>, generation: crate::Generation) -> Self {
        Self { owner, generation }
    }

    /// The recorded owner, if any.
    #[must_use]
    pub fn owner(&self) -> Option<&FocusOwner> {
        self.owner.as_ref()
    }

    /// Generation the recording belongs to.
    #[must_use]
    pub const fn generation(&self) -> crate::Generation {
        self.generation
    }
}

/// Re-checks a recorded association against the live focus.
///
/// `live_owner` is the surface the host reports as focused right now
/// (or [`None`] for an unfocused window, an empty workspace, or a
/// just-detached surface), and `live` is the live projection
/// generation.
///
/// Returns the asserted owner — [`None`] when nothing is focused — or
/// fails closed: [`A11yError::StaleGeneration`] when the recording
/// belongs to a superseded generation, [`A11yError::FocusMismatch`]
/// when the recorded owner disagrees with the live focus. A mismatch
/// never transfers focus, never routes input, and never mutates state:
/// the caller must re-synchronize to the current generation or report
/// no focused element.
///
/// # Errors
///
/// Returns [`A11yError::StaleGeneration`] or
/// [`A11yError::FocusMismatch`] as described above.
pub fn resolve_focus(
    recorded: &FocusAssociation,
    live_owner: Option<&FocusOwner>,
    live: crate::Generation,
) -> Result<Option<FocusOwner>, A11yError> {
    if recorded.generation != live {
        return Err(A11yError::StaleGeneration {
            presented: recorded.generation.as_u64(),
            live: live.as_u64(),
        });
    }
    if recorded.owner.as_ref() == live_owner {
        return Ok(recorded.owner.clone());
    }
    Err(A11yError::FocusMismatch)
}
