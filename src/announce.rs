//! Coalesced announcements with no timer.
//!
//! One bounded announcement per focus transition and per async state
//! transition; never on render and never on a repeating cadence. The
//! queue holds no clock, takes no clock, and advances only on explicit
//! push/drain calls — there is deliberately no coalescing time window:
//! the contract's no-periodic-timer rule forbids one, so coalescing is
//! consecutive-duplicate collapse plus drop-oldest overflow.

use std::collections::VecDeque;

use crate::A11yError;

/// Accepted ceiling in characters for one announcement text.
///
/// Longer input is rejected with [`A11yError::AnnouncementTooLong`],
/// never silently truncated.
pub const MAX_ANNOUNCEMENT_TEXT_LEN: usize = 256;

/// Accepted ceiling on queued announcements.
///
/// When full, the oldest pending notice is discarded so the newest
/// state always wins and memory stays bounded.
pub const MAX_PENDING_ANNOUNCEMENTS: usize = 8;

/// What transition an announcement reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AnnouncementKind {
    /// A keyboard/pointer focus transition (one per transition).
    Focus,
    /// An async state transition: completion, notice arrival, guard
    /// resolution (one per transition).
    AsyncState,
}

impl AnnouncementKind {
    /// Fixed v1 spelling for this kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Focus => "focus",
            Self::AsyncState => "state",
        }
    }
}

/// One queued announcement: its transition kind plus the bounded text
/// the backend speaks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Announcement {
    kind: AnnouncementKind,
    text: String,
}

impl Announcement {
    /// The transition this notice reports.
    #[must_use]
    pub const fn kind(&self) -> AnnouncementKind {
        self.kind
    }

    /// The bounded text the backend speaks.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Bounded, coalesced announcement queue.
///
/// Push exactly once per focus transition and once per async state
/// transition; drain into the platform backend. Consecutive duplicates
/// (same kind and text as the tail) coalesce to a single notice, and a
/// full queue drops the oldest notice so the newest state always wins.
/// Nothing here touches render, paint, timers, or plugin code.
#[derive(Clone, Debug, Default)]
pub struct AnnouncementQueue {
    pending: VecDeque<Announcement>,
}

impl AnnouncementQueue {
    /// Starts an empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending: VecDeque::new(),
        }
    }

    /// Queues one notice for `kind` with `text`.
    ///
    /// Returns `Ok(true)` when queued and `Ok(false)` when coalesced
    /// with an identical tail notice (no duplicate is stored).
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::AnnouncementTooLong`] when `text` exceeds
    /// [`MAX_ANNOUNCEMENT_TEXT_LEN`] characters.
    pub fn push(&mut self, kind: AnnouncementKind, text: &str) -> Result<bool, A11yError> {
        let len = text.chars().count();
        if len > MAX_ANNOUNCEMENT_TEXT_LEN {
            return Err(A11yError::AnnouncementTooLong {
                len,
                cap: MAX_ANNOUNCEMENT_TEXT_LEN,
            });
        }
        if let Some(tail) = self.pending.back() {
            if tail.kind == kind && tail.text == text {
                return Ok(false);
            }
        }
        if self.pending.len() >= MAX_PENDING_ANNOUNCEMENTS {
            self.pending.pop_front();
        }
        self.pending.push_back(Announcement {
            kind,
            text: text.to_string(),
        });
        Ok(true)
    }

    /// Queues one notice for a focus transition.
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::AnnouncementTooLong`] when `text` exceeds
    /// [`MAX_ANNOUNCEMENT_TEXT_LEN`] characters.
    pub fn push_focus(&mut self, text: &str) -> Result<bool, A11yError> {
        self.push(AnnouncementKind::Focus, text)
    }

    /// Queues one notice for an async state transition.
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::AnnouncementTooLong`] when `text` exceeds
    /// [`MAX_ANNOUNCEMENT_TEXT_LEN`] characters.
    pub fn push_state(&mut self, text: &str) -> Result<bool, A11yError> {
        self.push(AnnouncementKind::AsyncState, text)
    }

    /// Pending notice count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Whether no notice is pending.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Pending notices in push order, without draining.
    #[must_use]
    pub fn pending(&self) -> Vec<Announcement> {
        self.pending.iter().cloned().collect()
    }

    /// Drains every pending notice in push order, leaving the queue
    /// empty.
    pub fn drain(&mut self) -> Vec<Announcement> {
        self.pending.drain(..).collect()
    }
}
