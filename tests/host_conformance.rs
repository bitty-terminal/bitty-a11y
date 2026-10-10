//! Host conformance: [`bitty_a11y::SnapshotHost`] ingest and expose pinned
//! against a fake host.
//!
//! The fake host implements the stable host trait with owned values
//! only: it holds row ordinals, producer identities, kind spellings,
//! names, and notices, never a [`bitty_a11y::Snapshot`],
//! [`bitty_a11y::HandleMap`], [`bitty_a11y::ElementHandle`], or
//! [`bitty_a11y::FocusOwner`]. Each ingest re-binds focus fresh, so a
//! superseded generation fails closed and nothing swaps.

use bitty_a11y::{
    A11yError, Adapter, AnnouncementKind, ChromeKind, CursorPosition, FocusOwner, Generation,
    HandleMap, HeadlessBackend, InteractivePurpose, SceneKind, Snapshot, SnapshotBuilder,
    SnapshotHost, TextRun,
};

/// Fake host: owned host state only, no retained handles.
struct FakeHost {
    root: String,
    title: String,
    rows: Vec<(u32, String)>,
    chrome: Vec<(ChromeKind, String, Option<String>)>,
    scene: Vec<(SceneKind, u64)>,
    interactive: Vec<(InteractivePurpose, String, bool)>,
    modal: bool,
    focus_label: Option<String>,
    notices: Vec<(AnnouncementKind, String)>,
}

/// Standard fake host: root, terminal leaf with two rows, one chrome
/// bar, one scene text node, one enabled button, focus on the terminal
/// title.
fn standard_host() -> FakeHost {
    FakeHost {
        root: "terminal window".to_string(),
        title: "shell".to_string(),
        rows: vec![(0, "$ echo hi".to_string()), (1, "hi".to_string())],
        chrome: vec![(
            ChromeKind::Bar,
            "status bar".to_string(),
            Some("utf-8".to_string()),
        )],
        scene: vec![(SceneKind::Text, 41)],
        interactive: vec![(InteractivePurpose::Button, "follow link".to_string(), true)],
        modal: false,
        focus_label: Some("shell".to_string()),
        notices: vec![(AnnouncementKind::Focus, "shell focused".to_string())],
    }
}

impl SnapshotHost for FakeHost {
    fn build_snapshot(&self, builder: &mut SnapshotBuilder) -> Result<(), A11yError> {
        builder.set_root_name(&self.root)?;
        let runs: Vec<TextRun> = self
            .rows
            .iter()
            .map(|(row, text)| TextRun {
                row: *row,
                text: text.clone(),
            })
            .collect();
        builder.set_terminal(&self.title, runs, CursorPosition { row: 1, col: 2 }, true)?;
        for (kind, name, state) in &self.chrome {
            builder.add_chrome(*kind, name, state.as_deref())?;
        }
        for (kind, identity) in &self.scene {
            builder.add_scene(*kind, *identity);
        }
        for (purpose, name, enabled) in &self.interactive {
            builder.add_interactive(*purpose, name, *enabled)?;
        }
        builder.set_modal_overlay(self.modal);
        Ok(())
    }

    fn focus_for(
        &self,
        snapshot: &Snapshot,
        map: &HandleMap,
    ) -> Result<Option<FocusOwner>, A11yError> {
        let Some(label) = self.focus_label.as_deref() else {
            return Ok(None);
        };
        let Some(handle) = snapshot.lookup_label(label) else {
            return Err(A11yError::UnknownHandle);
        };
        FocusOwner::bind(snapshot, map, handle).map(Some)
    }

    fn announcements(&self) -> Vec<(AnnouncementKind, String)> {
        self.notices.clone()
    }
}

#[test]
fn host_ingest_expose_round_trip() {
    let host = standard_host();
    let generation = Generation::initial();
    let mut adapter = Adapter::new();
    adapter.ingest_host(&host, generation, None).unwrap();
    assert_eq!(adapter.generation(), generation);

    let mut backend = HeadlessBackend::new();
    assert_eq!(
        adapter.expose_to(&mut backend),
        Err(A11yError::PermissionDenied)
    );
    assert_eq!(
        adapter.announce_to(&mut backend),
        Err(A11yError::PermissionDenied)
    );

    backend.set_permission(true);
    adapter.expose_to(&mut backend).unwrap();
    assert_eq!(backend.exposures().len(), 1);
    assert_eq!(backend.exposures()[0].generation, generation);
    assert!(!backend.exposures()[0].modal);

    assert_eq!(adapter.announce_to(&mut backend), Ok(1));
    assert_eq!(backend.announcements().len(), 1);
    assert_eq!(backend.announcements()[0].text(), "shell focused");
    assert!(adapter.pending_announcements().is_empty());

    // Focus was recorded at ingest: clearing disagrees fail-closed,
    // and nothing is published on the event bus.
    assert!(adapter.focus().owner().is_some());
    assert_eq!(
        adapter.synchronize_focus(None),
        Err(A11yError::FocusMismatch)
    );
    assert!(adapter.event_bus_topics().is_empty());
}

#[test]
fn host_focus_none_synchronizes_to_none() {
    let mut host = standard_host();
    host.focus_label = None;
    host.notices = vec![];
    let mut adapter = Adapter::new();
    adapter
        .ingest_host(&host, Generation::initial(), None)
        .unwrap();
    assert!(adapter.focus().owner().is_none());
    assert_eq!(adapter.synchronize_focus(None), Ok(None));
}

#[test]
fn host_overlong_name_fails_closed() {
    let mut host = standard_host();
    host.root = "n".repeat(300);
    host.notices = vec![];
    host.focus_label = None;
    let mut adapter = Adapter::new();
    let err = adapter
        .ingest_host(&host, Generation::initial(), None)
        .unwrap_err();
    assert_eq!(err, A11yError::NameTooLong { len: 300, cap: 256 });
    assert!(adapter.snapshot().is_none());
}

#[test]
fn host_unmapped_scene_fails_whole_ingest() {
    let mut host = standard_host();
    host.scene.push((SceneKind::Unknown, 99));
    host.notices = vec![];
    host.focus_label = None;
    let mut adapter = Adapter::new();
    assert_eq!(
        adapter.ingest_host(&host, Generation::initial(), None),
        Err(A11yError::UnmappedSceneKind)
    );
    assert!(adapter.snapshot().is_none());
}

#[test]
fn host_oversized_snapshot_fails_closed() {
    let mut host = standard_host();
    host.rows = (0..5000u32).map(|row| (row, "x".to_string())).collect();
    host.notices = vec![];
    host.focus_label = None;
    let mut adapter = Adapter::new();
    let err = adapter
        .ingest_host(&host, Generation::initial(), None)
        .unwrap_err();
    assert!(matches!(err, A11yError::TooManyNodes { .. }));
    assert!(adapter.snapshot().is_none());
}

#[test]
fn host_overlong_announcement_fails_closed() {
    let mut host = standard_host();
    host.notices = vec![(AnnouncementKind::Focus, "a".repeat(300))];
    let mut adapter = Adapter::new();
    let err = adapter
        .ingest_host(&host, Generation::initial(), None)
        .unwrap_err();
    assert_eq!(err, A11yError::AnnouncementTooLong { len: 300, cap: 256 });
}

#[test]
fn host_non_focusable_focus_fails_closed() {
    let mut host = standard_host();
    host.focus_label = Some("status bar".to_string());
    host.notices = vec![];
    let mut adapter = Adapter::new();
    assert_eq!(
        adapter.ingest_host(&host, Generation::initial(), None),
        Err(A11yError::FocusMismatch)
    );
    assert!(adapter.snapshot().is_none());
}

#[test]
fn host_modal_without_overlay_fails_closed() {
    let mut host = standard_host();
    host.modal = true;
    host.notices = vec![];
    host.focus_label = None;
    let mut adapter = Adapter::new();
    assert_eq!(
        adapter.ingest_host(&host, Generation::initial(), None),
        Err(A11yError::InvalidStructure)
    );
    assert!(adapter.snapshot().is_none());
}

#[test]
fn host_stale_generation_rejected_nothing_swaps() {
    let mut host = standard_host();
    host.notices = vec![];
    host.focus_label = None;
    let mut adapter = Adapter::new();
    let gen0 = Generation::initial();
    adapter.ingest_host(&host, gen0, None).unwrap();
    let gen1 = gen0.advance();
    adapter.ingest_host(&host, gen1, None).unwrap();
    assert_eq!(adapter.generation(), gen1);

    let err = adapter.ingest_host(&host, gen0, None).unwrap_err();
    assert_eq!(
        err,
        A11yError::StaleGeneration {
            presented: gen0.as_u64(),
            live: gen1.as_u64(),
        }
    );
    assert_eq!(adapter.generation(), gen1);
}

#[test]
fn host_advance_retires_previous_exposure() {
    let mut host = standard_host();
    host.notices = vec![];
    host.focus_label = None;
    let mut adapter = Adapter::new();
    let mut backend = HeadlessBackend::new();
    backend.set_permission(true);

    let gen0 = Generation::initial();
    adapter
        .ingest_host(&host, gen0, Some(&mut backend))
        .unwrap();
    adapter.expose_to(&mut backend).unwrap();

    let gen1 = gen0.advance();
    adapter
        .ingest_host(&host, gen1, Some(&mut backend))
        .unwrap();
    assert_eq!(backend.retired(), &[gen0]);
    adapter.expose_to(&mut backend).unwrap();
    assert_eq!(backend.exposures().len(), 2);
    assert_eq!(backend.exposures()[1].generation, gen1);
}

#[test]
fn host_holds_no_handles_across_generations() {
    let mut host = standard_host();
    host.notices = vec![];
    host.focus_label = None;
    let mut adapter = Adapter::new();
    let gen0 = Generation::initial();
    adapter.ingest_host(&host, gen0, None).unwrap();
    let live0 = adapter.snapshot().unwrap();
    let terminal0 = live0.lookup_label("shell").unwrap();

    // Same host re-binds fresh at the next generation without retaining
    // the old handle: the re-ingest succeeds.
    let gen1 = gen0.advance();
    adapter.ingest_host(&host, gen1, None).unwrap();

    // The old handle is unknown to the live projection.
    assert_eq!(adapter.resolve(terminal0), Err(A11yError::UnknownHandle));
    // No focus was recorded, so synchronization reports no focus.
    assert_eq!(adapter.synchronize_focus(None), Ok(None));
}
