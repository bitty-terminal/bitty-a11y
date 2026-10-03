//! Headless backend round-trip: ingest, expose, announce, act, retire.
//!
//! Proves the [`bitty_a11y::PlatformBackend`] shape end to end without
//! any real AT-SPI/UIA/AX backend: the host ingests a snapshot, the
//! headless backend materializes it, focus synchronizes, announcements
//! drain, a platform-initiated activation routes through the
//! controlled interface, and generation advance retires the old
//! exposure. A dropped backend removes exposure only; the adapter
//! keeps resolving.

use bitty_a11y::{
    A11yError, ActionKind, ActionOutcome, ActionRequest, ActionSink, Adapter, Anchor,
    AnnouncementKind, ChromeKind, CursorPosition, ElementHandle, FocusOwner, Generation,
    HeadlessBackend, InteractivePurpose, SceneKind, SnapshotBuilder, TextRun,
};

/// Host command-registry stand-in: grants or denies the capability and
/// records every dispatched request.
struct TestSink {
    granted: bool,
    dispatched: Vec<ActionRequest>,
}

impl ActionSink for TestSink {
    fn authorize(&self, _: &ActionRequest, _: &Anchor) -> Result<(), A11yError> {
        if self.granted {
            Ok(())
        } else {
            Err(A11yError::CapabilityDenied)
        }
    }

    fn dispatch(&mut self, request: &ActionRequest, _: &Anchor) {
        self.dispatched.push(*request);
    }
}

/// Snapshot with an enabled `open` button and a disabled `save` button.
fn build_generation(generation: Generation) -> (bitty_a11y::Snapshot, bitty_a11y::HandleMap) {
    let mut builder = SnapshotBuilder::new(generation);
    builder.set_root_name("terminal window").unwrap();
    builder
        .set_terminal(
            "shell",
            vec![
                TextRun {
                    row: 0,
                    text: "$ open report".to_string(),
                },
                TextRun {
                    row: 1,
                    text: "opened".to_string(),
                },
            ],
            CursorPosition { row: 1, col: 6 },
            true,
        )
        .unwrap();
    builder
        .add_chrome(ChromeKind::TabStrip, "tabs", Some("shell"))
        .unwrap();
    builder.add_scene(SceneKind::Text, 7);
    builder
        .add_interactive(InteractivePurpose::Button, "open", true)
        .unwrap();
    builder
        .add_interactive(InteractivePurpose::Button, "save", false)
        .unwrap();
    builder.finish().unwrap()
}

#[test]
fn headless_backend_round_trip() {
    let gen1 = Generation::initial().advance();
    let (snapshot, map) = build_generation(gen1);
    let node_count = snapshot.len();

    let terminal = snapshot.lookup_label("shell").unwrap();
    let owner = FocusOwner::bind(&snapshot, &map, terminal).unwrap();

    let mut adapter = Adapter::new();
    adapter
        .ingest(
            snapshot,
            map,
            Some(owner.clone()),
            &[(AnnouncementKind::Focus, "shell focused")],
            None,
        )
        .unwrap();

    let mut backend = HeadlessBackend::new();

    // Permission gate: denied backends expose and announce nothing.
    assert_eq!(
        adapter.expose_to(&mut backend),
        Err(A11yError::PermissionDenied)
    );
    assert_eq!(
        adapter.announce_to(&mut backend),
        Err(A11yError::PermissionDenied)
    );
    assert!(backend.exposures().is_empty());
    assert!(backend.announcements().is_empty());

    // Granted: the snapshot materializes and the notice drains.
    backend.set_permission(true);
    adapter.expose_to(&mut backend).unwrap();
    assert_eq!(backend.exposures().len(), 1);
    assert_eq!(backend.exposures()[0].generation, gen1);
    assert_eq!(backend.exposures()[0].nodes, node_count);
    assert!(!backend.exposures()[0].modal);

    assert_eq!(adapter.announce_to(&mut backend), Ok(1));
    assert_eq!(backend.announcements().len(), 1);
    assert_eq!(backend.announcements()[0].text(), "shell focused");
    assert!(adapter.pending_announcements().is_empty());

    // Focus still agrees with the live host state.
    assert_eq!(
        adapter.synchronize_focus(Some(&owner)),
        Ok(Some(owner.clone()))
    );

    // Platform-initiated activation routes through the controlled
    // interface: label to handle, revalidation, grant, dispatch.
    // Handles are copied out first so no snapshot borrow crosses the
    // mutable dispatch calls below.
    let live = adapter.snapshot().unwrap();
    let open: ElementHandle = live.lookup_label("open").unwrap();
    let save: ElementHandle = live.lookup_label("save").unwrap();
    let shell: ElementHandle = live.lookup_label("shell").unwrap();
    let request = ActionRequest::new(ActionKind::Activate, open, gen1);
    let mut sink = TestSink {
        granted: true,
        dispatched: Vec::new(),
    };
    assert_eq!(
        adapter.request_action(&mut sink, &request),
        Ok(ActionOutcome::Dispatched)
    );
    assert_eq!(sink.dispatched, vec![request]);

    // Disabled controls expose state with no activation...
    let disabled = ActionRequest::new(ActionKind::Activate, save, gen1);
    assert_eq!(
        adapter.request_action(&mut sink, &disabled),
        Err(A11yError::NotEnabled)
    );

    // ...and a denied grant executes nothing.
    let mut denied = TestSink {
        granted: false,
        dispatched: Vec::new(),
    };
    assert_eq!(
        adapter.request_action(&mut denied, &request),
        Err(A11yError::CapabilityDenied)
    );
    assert!(denied.dispatched.is_empty());

    // Non-activatable targets (terminal leaf, not an interactive node)
    // fail closed.
    let wrong_class = ActionRequest::new(ActionKind::Activate, shell, gen1);
    assert_eq!(
        adapter.request_action(&mut sink, &wrong_class),
        Err(A11yError::NotActivatable)
    );
    assert_eq!(sink.dispatched.len(), 1);

    // Structural change advances the generation; the old exposure
    // retires to the backend while the new one materializes.
    let gen2 = gen1.advance();
    let (snapshot2, map2) = build_generation(gen2);
    let terminal2 = snapshot2.lookup_label("shell").unwrap();
    let owner2 = FocusOwner::bind(&snapshot2, &map2, terminal2).unwrap();
    adapter
        .ingest(
            snapshot2,
            map2,
            Some(owner2.clone()),
            &[],
            Some(&mut backend),
        )
        .unwrap();
    assert_eq!(backend.retired(), &[gen1]);
    adapter.expose_to(&mut backend).unwrap();
    assert_eq!(backend.exposures().len(), 2);
    assert_eq!(adapter.synchronize_focus(Some(&owner2)), Ok(Some(owner2)));

    // No Event-Bus publication anywhere on the path.
    assert!(adapter.event_bus_topics().is_empty());
}

#[test]
fn crashed_backend_removes_exposure_only() {
    let generation = Generation::initial().advance();
    let (snapshot, map) = build_generation(generation);
    let terminal = snapshot.lookup_label("shell").unwrap();
    let owner = FocusOwner::bind(&snapshot, &map, terminal).unwrap();

    let mut adapter = Adapter::new();
    adapter
        .ingest(snapshot, map, Some(owner.clone()), &[], None)
        .unwrap();

    {
        let mut backend = HeadlessBackend::new();
        backend.set_permission(true);
        adapter.expose_to(&mut backend).unwrap();
        // Backend crashes here: dropped with its exposure state.
    }

    // The adapter keeps resolving focus and handles; the Core-side
    // baseline is unaffected by the lost backend.
    assert_eq!(
        adapter.synchronize_focus(Some(&owner)),
        Ok(Some(owner.clone()))
    );
    assert!(adapter.resolve(terminal).is_ok());
    assert_eq!(adapter.generation(), generation);
}

#[test]
fn removal_is_safe_mode_still_functional() {
    // Zero backends (as in `bitty --safe`): the mechanism works, there
    // is simply nothing to expose to.
    let mut adapter = Adapter::new();
    let mut backend = HeadlessBackend::new();
    backend.set_permission(true);
    assert_eq!(
        adapter.expose_to(&mut backend),
        Err(A11yError::InvalidStructure)
    );

    let generation = Generation::initial();
    let (snapshot, map) = build_generation(generation);
    adapter.ingest(snapshot, map, None, &[], None).unwrap();
    assert_eq!(adapter.synchronize_focus(None), Ok(None));
    adapter.expose_to(&mut backend).unwrap();
    assert_eq!(backend.exposures().len(), 1);
}
