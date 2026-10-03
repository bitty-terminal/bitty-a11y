//! Controlled action adapter over the closed v1 action set.
//!
//! The adapter may interact with the terminal only through this
//! interface. It never receives a callback into the compositor,
//! renderer, or terminal, holds no raw PTY, GPU, or window handle, and
//! cannot write input or mutate grid, cursor, modes, scrollback,
//! attachment, focus, or policy.
//!
//! The v1 action class is activation of a declared interactive node. An
//! action request names the action, the element handle, and its
//! projection generation; the adapter revalidates the identity and
//! generation against the live projection, checks the target is
//! enabled, and asks the host sink to authorize before dispatching.
//! Unknown, expired, or stale elements fail closed with a typed error;
//! they never fall back to a default action or a silent dispatch.

use crate::A11yError;
use crate::handles::{Anchor, ElementHandle, Generation, HandleMap};
use crate::snapshot::{NodeView, Snapshot};

/// The closed v1 action set: activation of a declared interactive node.
///
/// Any other class requires its own reviewed extension and is not
/// authorized here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ActionKind {
    /// Activates an enabled interactive node.
    Activate,
}

impl ActionKind {
    /// Fixed v1 action label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Activate => "activate",
        }
    }

    /// Parses an action label against the closed set.
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::UnknownAction`] for any label outside the
    /// v1 set.
    pub fn parse(label: &str) -> Result<Self, A11yError> {
        match label {
            "activate" => Ok(Self::Activate),
            _ => Err(A11yError::UnknownAction {
                label: label.to_string(),
            }),
        }
    }
}

/// One action request from the platform side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ActionRequest {
    action: ActionKind,
    handle: ElementHandle,
    generation: Generation,
}

impl ActionRequest {
    /// Requests `action` on `handle` from `generation`.
    #[must_use]
    pub const fn new(action: ActionKind, handle: ElementHandle, generation: Generation) -> Self {
        Self {
            action,
            handle,
            generation,
        }
    }

    /// The requested action.
    #[must_use]
    pub const fn action(self) -> ActionKind {
        self.action
    }

    /// The targeted element handle.
    #[must_use]
    pub const fn handle(self) -> ElementHandle {
        self.handle
    }

    /// The projection generation the handle was derived from.
    #[must_use]
    pub const fn generation(self) -> Generation {
        self.generation
    }
}

/// Host side of controlled dispatch.
///
/// The host owns the accepted command registry and the target owner's
/// capability grants; the adapter forwards no authority. `authorize`
/// re-checks the grant for this exact request (an extracted adapter's
/// submissions are untrusted input at the host boundary), and
/// `dispatch` routes the authorized request through the registry. The
/// sink has no terminal handle to hand out: input still reaches the
/// terminal only through the public, capability-gated host path.
pub trait ActionSink {
    /// Revalidates the capability grant for `request` on `anchor`.
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::CapabilityDenied`] without the required
    /// grant; nothing is executed.
    fn authorize(&self, request: &ActionRequest, anchor: &Anchor) -> Result<(), A11yError>;

    /// Routes an authorized request through the host command registry.
    fn dispatch(&mut self, request: &ActionRequest, anchor: &Anchor);
}

/// Outcome of a controlled action request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionOutcome {
    /// Revalidated and routed through the host registry.
    Dispatched,
}

/// Revalidates `request` against the live projection and routes it
/// through `sink`.
///
/// The checks run in order and the first failure wins, fail-closed:
/// the handle must resolve in `map` at the live generation (forged,
/// foreign, or stale handles rejected), the presented generation must
/// equal the live generation (no action against a recycled identity),
/// the node must be an enabled interactive node of the requested
/// class, and the host must authorize the grant. Only then is the
/// request dispatched.
///
/// # Errors
///
/// Returns [`A11yError::UnknownHandle`] for forged or foreign handles,
/// [`A11yError::StaleGeneration`] for superseded generations (either
/// from the map or from the presented generation),
/// [`A11yError::NotActivatable`] when the target is not an activatable
/// interactive node, [`A11yError::NotEnabled`] for a disabled control,
/// and [`A11yError::CapabilityDenied`] when the host refuses the grant.
pub fn request_action(
    map: &HandleMap,
    snapshot: &Snapshot,
    live: Generation,
    sink: &mut dyn ActionSink,
    request: &ActionRequest,
) -> Result<ActionOutcome, A11yError> {
    if request.generation != live {
        return Err(A11yError::StaleGeneration {
            presented: request.generation.as_u64(),
            live: live.as_u64(),
        });
    }
    let anchor = map.resolve(request.handle, live)?;
    let view = snapshot
        .get(request.handle)
        .ok_or(A11yError::UnknownHandle)?;
    match (request.action, view) {
        (ActionKind::Activate, NodeView::Interactive { enabled, .. }) => {
            if !enabled {
                return Err(A11yError::NotEnabled);
            }
        }
        _ => return Err(A11yError::NotActivatable),
    }
    sink.authorize(request, anchor)?;
    sink.dispatch(request, anchor);
    Ok(ActionOutcome::Dispatched)
}
