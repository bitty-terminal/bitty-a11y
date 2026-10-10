//! Stable host-side contract for [`SnapshotBuilder`] ingest plus the expose path.
//!
//! This is the blocker the crate root names: Core publishes its
//! projection generation, focus owner, and invalidation source through
//! this trait instead of a private bypass. The host pushes scene and
//! semantic state in through [`SnapshotBuilder`]; platform backends
//! observe out through [`PlatformBackend`]. Removal removes exposure
//! only.
//!
//! # Ingest cycle
//!
//! One ingest is `build` then `ingest` then `expose`:
//!
//! 1. The caller creates a [`SnapshotBuilder`] for `generation` and
//!    hands it to [`SnapshotHost::build_snapshot`]. The host pushes
//!    root, terminal, chrome, scene, and interactive state in caller
//!    order. The builder takes owned values; building cannot mutate
//!    grid, cursor, modes, scrollback, attachment, focus, or policy.
//! 2. [`SnapshotBuilder::finish`] validates before anything attaches:
//!    overlong names, unmapped scene kinds, node-cap overflow, and
//!    structural violations abort with a typed error and no partial
//!    snapshot escapes.
//! 3. [`SnapshotHost::focus_for`] binds the focus owner against the
//!    finished [`Snapshot`] and [`HandleMap`] transiently.
//!    [`SnapshotHost::announcements`] carries at most one notice per
//!    transition reported by this ingest.
//! 4. [`Adapter::ingest`] swaps the snapshot atomically when the
//!    incoming generation is at or past the live generation; a stale
//!    ingest is rejected and nothing swaps.
//! 5. [`Adapter::expose_to`] materializes the live snapshot in a
//!    [`PlatformBackend`] when the platform permission is granted;
//!    [`Adapter::announce_to`] drains the queued notices in push order.
//!    A generation advance retires the previous generation through
//!    [`PlatformBackend::retire`].
//!
//! [`Adapter::ingest_host`] drives steps 1-4 for any [`SnapshotHost`]
//! so Core and the conformance test share one path.
//!
//! # Bounded inputs
//!
//! Every bound is finite and fail-closed; silent truncation is
//! non-conforming. The builder and the adapter enforce:
//!
//! - Names, labels, and chrome state: at most
//!   [`MAX_ACCESSIBLE_NAME_LEN`] characters, else
//!   [`A11yError::NameTooLong`]. Row text is content, not a name, and
//!   is never length-checked.
//! - Nodes per snapshot: at most [`MAX_SNAPSHOT_NODES`], else
//!   [`A11yError::TooManyNodes`] with no partial tree.
//! - Announcement text: at most [`MAX_ANNOUNCEMENT_TEXT_LEN`]
//!   characters, else [`A11yError::AnnouncementTooLong`].
//! - Pending announcements: at most [`MAX_PENDING_ANNOUNCEMENTS`];
//!   a full queue drops the oldest so the newest state wins.
//!
//! # Error mapping
//!
//! The host surfaces failures as [`A11yError`] only: no panics, no
//! silent truncation, no fallback role, no default action.
//!
//! - Unmapped [`SceneKind`] or interactive purpose:
//!   [`A11yError::UnmappedSceneKind`]; the whole build fails before
//!   anything attaches.
//! - Missing root or terminal data, or a modal overlay without an
//!   overlay surface: [`A11yError::InvalidStructure`].
//! - Superseded generation presented against the live generation:
//!   [`A11yError::StaleGeneration`]; nothing swaps.
//! - Recorded focus disagrees with the live focus:
//!   [`A11yError::FocusMismatch`]; nothing transfers, routes, or
//!   mutates.
//! - Platform permission not granted on expose or announce:
//!   [`A11yError::PermissionDenied`]; nothing is exposed and a refused
//!   drain leaves the queue intact.
//!
//! # No retained handles
//!
//! Handles are snapshot-scoped, opaque, and ephemeral. The host must
//! not store [`Snapshot`], [`HandleMap`], [`ElementHandle`], or
//! [`FocusOwner`] beyond the ingest call that created them. Only
//! persistent identity may persist across ingests: row ordinals,
//! producer-assigned scene identities (`u64`), kind and purpose
//! spellings, accessible names, and the [`Generation`] itself. Each
//! ingest re-binds focus fresh through [`SnapshotHost::focus_for`];
//! presenting a handle from a superseded generation fails with
//! [`A11yError::StaleGeneration`].

use crate::A11yError;
use crate::adapter::Adapter;
use crate::announce::AnnouncementKind;
use crate::backend::PlatformBackend;
use crate::focus::FocusOwner;
use crate::handles::{Generation, HandleMap};
use crate::snapshot::{Snapshot, SnapshotBuilder};

/// Stable host-side source of snapshot content.
///
/// Core implements this trait to push scene and semantic state into the
/// adapter. The trait is object-safe so the adapter drives any host
/// through `&dyn SnapshotHost` on the single [`Adapter::ingest_host`]
/// path. Test hosts implement the same trait; there is no private
/// bypass.
pub trait SnapshotHost {
    /// Pushes root, terminal, chrome, scene, and interactive state into
    /// `builder` in caller order.
    ///
    /// The builder enforces the bounded-input ceilings fail-closed;
    /// this method propagates those errors unchanged and maps any
    /// host-side unmappable kind to [`A11yError::UnmappedSceneKind`].
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::NameTooLong`] for an overlong name, label,
    /// or state, [`A11yError::UnmappedSceneKind`] for an unmappable
    /// scene kind or purpose, and [`A11yError::InvalidStructure`] for
    /// a structural violation detected early. [`SnapshotBuilder::finish`]
    /// may additionally return [`A11yError::TooManyNodes`] or
    /// [`A11yError::InvalidStructure`].
    fn build_snapshot(&self, builder: &mut SnapshotBuilder) -> Result<(), A11yError>;

    /// Binds the focus owner against the finished `snapshot` and `map`.
    ///
    /// Both references are transient: the host inspects them, binds at
    /// most one focusable handle through [`FocusOwner::bind`], and
    /// returns the owner without retaining `snapshot`, `map`, or any
    /// handle. Returns [`None`] when nothing is focused (unfocused
    /// window, empty workspace, or just-detached surface).
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::UnknownHandle`] for a handle the snapshot
    /// does not know and [`A11yError::FocusMismatch`] when the anchor
    /// behind the handle can never own focus.
    fn focus_for(
        &self,
        snapshot: &Snapshot,
        map: &HandleMap,
    ) -> Result<Option<FocusOwner>, A11yError>;

    /// Notices reported by this ingest, at most one per transition.
    ///
    /// Each entry is a transition kind plus the bounded text the
    /// backend speaks. Overlong text fails the ingest with
    /// [`A11yError::AnnouncementTooLong`], never truncated. A full
    /// queue drops the oldest so the newest state wins; the host
    /// normally provides zero to two entries per ingest.
    fn announcements(&self) -> Vec<(AnnouncementKind, String)>;
}

impl Adapter {
    /// Drives one host ingest at `generation` through `host`.
    ///
    /// Builds a [`SnapshotBuilder`] for `generation`, fills it through
    /// [`SnapshotHost::build_snapshot`], finishes it, binds focus
    /// through [`SnapshotHost::focus_for`], and swaps the result
    /// atomically through [`Self::ingest`]. The expose path stays
    /// separate: call [`Self::expose_to`] and [`Self::announce_to`]
    /// afterwards. `retired_to` retires the previous generation when
    /// `generation` advances past it.
    ///
    /// The host retains nothing: `snapshot`, `map`, and every
    /// [`ElementHandle`] live only inside this call except for the
    /// swapped-in live projection owned by the adapter.
    ///
    /// # Errors
    ///
    /// Forwards [`SnapshotHost::build_snapshot`] failures,
    /// [`SnapshotBuilder::finish`] failures ([`A11yError::TooManyNodes`],
    /// [`A11yError::UnmappedSceneKind`],
    /// [`A11yError::InvalidStructure`]), [`SnapshotHost::focus_for`]
    /// failures, announcement bound failures
    /// ([`A11yError::AnnouncementTooLong`]), and [`Self::ingest`]
    /// staleness ([`A11yError::StaleGeneration`]). Nothing swaps on
    /// error.
    pub fn ingest_host(
        &mut self,
        host: &dyn SnapshotHost,
        generation: Generation,
        retired_to: Option<&mut dyn PlatformBackend>,
    ) -> Result<(), A11yError> {
        let mut builder = SnapshotBuilder::new(generation);
        host.build_snapshot(&mut builder)?;
        let (snapshot, map) = builder.finish()?;
        let focus = host.focus_for(&snapshot, &map)?;
        let notices = host.announcements();
        let refs: Vec<(AnnouncementKind, &str)> = notices
            .iter()
            .map(|(kind, text)| (*kind, text.as_str()))
            .collect();
        self.ingest(snapshot, map, focus, &refs, retired_to)
    }
}
