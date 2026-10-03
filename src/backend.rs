//! Platform-backend trait plus the headless test backend.
//!
//! The adapter is a consumer of the projection, never a producer of
//! terminal state. Materializing the projection in a platform
//! accessibility model (AT-SPI over D-Bus on Linux, Windows UI
//! Automation on Windows, the macOS Accessibility bridge on macOS) is a
//! per-platform follow-up behind this trait; this crate ships only the
//! trait and an in-memory headless backend that proves the shape.
//!
//! Every backend is capability-gated twice: the host grants the
//! capability, and the platform's own accessibility permission is
//! surfaced through [`PlatformBackend::permission_granted`]. The adapter
//! refuses to expose or act when the permission is not granted. An
//! absent, denied, or crashed backend removes platform exposure only;
//! it never disables the Core baseline or forces a degraded state.

use crate::A11yError;
use crate::announce::Announcement;
use crate::handles::{ElementHandle, Generation};
use crate::snapshot::Snapshot;

/// What the adapter asks a platform backend to materialize.
///
/// Backends observe these calls; they never receive a callback into the
/// compositor, renderer, or terminal, and they hold no raw PTY, GPU, or
/// window handle.
pub trait PlatformBackend {
    /// Whether the platform's own accessibility permission is granted.
    ///
    /// Exposure and action calls must refuse while this is false; the
    /// adapter checks first and fails with
    /// [`A11yError::PermissionDenied`].
    fn permission_granted(&self) -> bool;

    /// Materializes `snapshot` in the platform accessibility model.
    ///
    /// Reconciliation across generations is by identity; the backend
    /// must not retain handles past [`Self::retire`].
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::PermissionDenied`] when the platform
    /// permission is not granted.
    fn expose(&mut self, snapshot: &Snapshot) -> Result<(), A11yError>;

    /// Drops all state for `generation` after the adapter advanced past
    /// it.
    fn retire(&mut self, generation: Generation);

    /// Delivers one bounded announcement through the platform's
    /// announcement mechanism.
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::PermissionDenied`] when the platform
    /// permission is not granted.
    fn announce(&mut self, announcement: &Announcement) -> Result<(), A11yError>;

    /// Reports the platform-side focused element, if any.
    ///
    /// Used only for diagnostics: focus authority stays with the host,
    /// and any disagreement fails closed on the adapter side.
    fn platform_focus(&self) -> Option<ElementHandle>;
}

/// In-memory headless backend for tests and host integration drills.
///
/// Records every exposure, announcement, and retirement without
/// touching any platform accessibility API. The platform permission is
/// a plain flag the test flips to prove the permission gate. There is
/// deliberately no event-bus publisher anywhere in this backend: the
/// projection stays host-side.
#[derive(Clone, Debug, Default)]
pub struct HeadlessBackend {
    permission: bool,
    exposures: Vec<ExposedSnapshot>,
    announcements: Vec<Announcement>,
    retired: Vec<Generation>,
    focus: Option<ElementHandle>,
}

/// Summary of one exposure call, for test assertions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExposedSnapshot {
    /// Generation that was materialized.
    pub generation: Generation,
    /// Node count that was materialized.
    pub nodes: usize,
    /// Whether a modal overlay confined focus.
    pub modal: bool,
}

impl HeadlessBackend {
    /// Starts a backend with the platform permission denied.
    ///
    /// Tests opt into exposure with [`Self::set_permission`], proving
    /// the gate instead of assuming it.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the platform's accessibility permission state.
    pub fn set_permission(&mut self, granted: bool) {
        self.permission = granted;
    }

    /// Sets the platform-side focused element (diagnostics only).
    pub fn set_platform_focus(&mut self, focus: Option<ElementHandle>) {
        self.focus = focus;
    }

    /// Exposures recorded so far, in call order.
    #[must_use]
    pub fn exposures(&self) -> &[ExposedSnapshot] {
        &self.exposures
    }

    /// Announcements delivered so far, in call order.
    #[must_use]
    pub fn announcements(&self) -> &[Announcement] {
        &self.announcements
    }

    /// Generations retired so far, in call order.
    #[must_use]
    pub fn retired(&self) -> &[Generation] {
        &self.retired
    }
}

impl PlatformBackend for HeadlessBackend {
    fn permission_granted(&self) -> bool {
        self.permission
    }

    fn expose(&mut self, snapshot: &Snapshot) -> Result<(), A11yError> {
        if !self.permission {
            return Err(A11yError::PermissionDenied);
        }
        self.exposures.push(ExposedSnapshot {
            generation: snapshot.generation(),
            nodes: snapshot.len(),
            modal: !matches!(snapshot.focus_scope(), crate::snapshot::FocusScope::Full),
        });
        Ok(())
    }

    fn retire(&mut self, generation: Generation) {
        self.retired.push(generation);
    }

    fn announce(&mut self, announcement: &Announcement) -> Result<(), A11yError> {
        if !self.permission {
            return Err(A11yError::PermissionDenied);
        }
        self.announcements.push(announcement.clone());
        Ok(())
    }

    fn platform_focus(&self) -> Option<ElementHandle> {
        self.focus
    }
}
