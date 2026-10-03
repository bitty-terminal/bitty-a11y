//! Host-facing adapter: atomic snapshot swaps, focus checks, and dispatch.
//!
//! The adapter owns the live projection generation, the current
//! snapshot and handle map, the recorded focus association, and the
//! announcement queue. The host feeds snapshots in (built from Terminal
//! Truth, the scene model, and chrome state through
//! [`crate::SnapshotBuilder`]); platform backends observe out through
//! [`crate::PlatformBackend`]. Removal removes exposure only: dropping
//! the adapter (or running with zero backends, as in `bitty --safe`)
//! changes no terminal behavior and leaves the host baseline intact.

use crate::A11yError;
use crate::action::{ActionOutcome, ActionRequest, ActionSink, request_action};
use crate::announce::{Announcement, AnnouncementKind, AnnouncementQueue};
use crate::backend::PlatformBackend;
use crate::focus::{FocusAssociation, FocusOwner, resolve_focus};
use crate::handles::{ElementHandle, Generation, HandleMap};
use crate::snapshot::Snapshot;

/// What changed that may require rebuilding the projection.
///
/// The projection rebuilds only when the host names a source. Nothing
/// invalidated never rebuilds; a render, paint, or damage tick that
/// carries no content change does not rebuild. Rebuilds create no
/// periodic timer and execute no plugin code per keystroke or per PTY
/// read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InvalidationSource {
    /// Keyboard/pointer focus moved (pairs with one queued
    /// announcement).
    FocusChange,
    /// An async state transition completed (pairs with one queued
    /// announcement).
    AsyncStateChange,
    /// Scene-backed content changed.
    SceneUpdate,
    /// Terminal grid, title, or cursor changed.
    TerminalUpdate,
    /// Chrome surface name or active item changed.
    ChromeUpdate,
}

/// Whether the projection must rebuild for `source`.
///
/// [`None`] (nothing invalidated) never rebuilds; any named source
/// does.
#[must_use]
pub const fn needs_refresh(source: Option<InvalidationSource>) -> bool {
    source.is_some()
}

/// The adapter: live generation, current snapshot, focus record,
/// announcements, and controlled dispatch.
///
/// Snapshots swap atomically: [`Self::ingest`] validates the incoming
/// generation before replacing anything, so the adapter never presents
/// a partially built tree and a stale ingest is rejected outright.
pub struct Adapter {
    live: Generation,
    snapshot: Option<Snapshot>,
    map: Option<HandleMap>,
    focus: FocusAssociation,
    announcements: AnnouncementQueue,
}

impl Adapter {
    /// Starts an adapter with no snapshot: nothing is exposed until
    /// the first [`Self::ingest`], and focus resolves to no focused
    /// element.
    #[must_use]
    pub fn new() -> Self {
        Self {
            live: Generation::initial(),
            snapshot: None,
            map: None,
            focus: FocusAssociation::new(None, Generation::initial()),
            announcements: AnnouncementQueue::new(),
        }
    }

    /// Live projection generation.
    #[must_use]
    pub const fn generation(&self) -> Generation {
        self.live
    }

    /// Current snapshot, if one was ingested.
    #[must_use]
    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot.as_ref()
    }

    /// Recorded focus association.
    #[must_use]
    pub fn focus(&self) -> &FocusAssociation {
        &self.focus
    }

    /// Pending announcements in push order, without draining.
    #[must_use]
    pub fn pending_announcements(&self) -> Vec<Announcement> {
        self.announcements.pending()
    }

    /// Event-bus topics this adapter publishes to.
    ///
    /// Always empty: the snapshot, element identities, focus
    /// associations, and action outcomes are host-side and are never
    /// published on the Event Bus, so exposing one surface's content
    /// to assistive technology never becomes a cross-panel read path
    /// for plugins. This accessor is the single choke point — any
    /// future topic must be registered here, which the no-bus test
    /// would catch.
    #[must_use]
    pub fn event_bus_topics(&self) -> &'static [&'static str] {
        &[]
    }

    /// Atomically swaps in `snapshot` with its handle map.
    ///
    /// The incoming generation must be at or past the live generation:
    /// a structural change advances the generation (and the previous
    /// map is retired to `retired_to`), while an identical rebuild
    /// re-presents the same generation. `focus_owner` records the
    /// surface the host reports as focused (or [`None`]), and
    /// `announcements` carries at most one notice per transition that
    /// this ingest reports.
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::StaleGeneration`] when `snapshot` belongs
    /// to a superseded generation (nothing is swapped), and forwards
    /// announcement bound failures.
    pub fn ingest(
        &mut self,
        snapshot: Snapshot,
        map: HandleMap,
        focus_owner: Option<FocusOwner>,
        announcements: &[(AnnouncementKind, &str)],
        retired_to: Option<&mut dyn PlatformBackend>,
    ) -> Result<(), A11yError> {
        let incoming = snapshot.generation();
        if incoming < self.live {
            return Err(A11yError::StaleGeneration {
                presented: incoming.as_u64(),
                live: self.live.as_u64(),
            });
        }
        debug_assert_eq!(map.generation(), incoming);
        if map.generation() != incoming {
            return Err(A11yError::StaleGeneration {
                presented: map.generation().as_u64(),
                live: incoming.as_u64(),
            });
        }
        for (kind, text) in announcements {
            self.announcements.push(*kind, text)?;
        }
        let previous = self.live;
        self.live = incoming;
        self.snapshot = Some(snapshot);
        self.map = Some(map);
        self.focus = FocusAssociation::new(focus_owner, incoming);
        if incoming != previous {
            if let Some(backend) = retired_to {
                backend.retire(previous);
            }
        }
        Ok(())
    }

    /// Re-checks the recorded focus against the live focus.
    ///
    /// `live_owner` is the surface the host reports as focused right
    /// now. See [`resolve_focus`] for the fail-closed behavior.
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::StaleGeneration`] or
    /// [`A11yError::FocusMismatch`]; neither transfers focus, routes
    /// input, nor mutates state.
    pub fn synchronize_focus(
        &self,
        live_owner: Option<&FocusOwner>,
    ) -> Result<Option<FocusOwner>, A11yError> {
        resolve_focus(&self.focus, live_owner, self.live)
    }

    /// Revalidates `request` against the live projection and routes it
    /// through `sink`.
    ///
    /// # Errors
    ///
    /// Fails closed per [`request_action`]; nothing is executed on
    /// refusal.
    pub fn request_action(
        &mut self,
        sink: &mut dyn ActionSink,
        request: &ActionRequest,
    ) -> Result<ActionOutcome, A11yError> {
        let (map, snapshot) = self.live_projection()?;
        request_action(map, snapshot, self.live, sink, request)
    }

    /// Resolves `handle` against the live generation.
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::StaleGeneration`] when no snapshot is live
    /// yet or the map belongs to a superseded generation, and
    /// [`A11yError::UnknownHandle`] for forged or foreign handles.
    pub fn resolve(&self, handle: ElementHandle) -> Result<&crate::handles::Anchor, A11yError> {
        let (map, _) = self.live_projection()?;
        map.resolve(handle, self.live)
    }

    /// Materializes the live snapshot in `backend`.
    ///
    /// Refuses with [`A11yError::PermissionDenied`] when the
    /// platform's permission is not granted or when no snapshot is
    /// live yet (there is nothing truthful to expose, so nothing is
    /// exposed).
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::PermissionDenied`] or
    /// [`A11yError::InvalidStructure`] (no live snapshot).
    pub fn expose_to(&self, backend: &mut dyn PlatformBackend) -> Result<(), A11yError> {
        if !backend.permission_granted() {
            return Err(A11yError::PermissionDenied);
        }
        let (_, snapshot) = self.live_projection()?;
        backend.expose(snapshot)
    }

    /// Drains pending announcements into `backend`, in push order.
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::PermissionDenied`] when the platform's
    /// permission is not granted; the queue is left intact so no
    /// notice is lost to a refused drain.
    pub fn announce_to(&mut self, backend: &mut dyn PlatformBackend) -> Result<usize, A11yError> {
        if !backend.permission_granted() {
            return Err(A11yError::PermissionDenied);
        }
        let drained = self.announcements.drain();
        for announcement in &drained {
            backend.announce(announcement)?;
        }
        Ok(drained.len())
    }

    fn live_projection(&self) -> Result<(&HandleMap, &Snapshot), A11yError> {
        let map = self.map.as_ref().ok_or(A11yError::InvalidStructure)?;
        let snapshot = self.snapshot.as_ref().ok_or(A11yError::InvalidStructure)?;
        if map.generation() != self.live {
            return Err(A11yError::StaleGeneration {
                presented: map.generation().as_u64(),
                live: self.live.as_u64(),
            });
        }
        Ok((map, snapshot))
    }
}

impl Default for Adapter {
    fn default() -> Self {
        Self::new()
    }
}
