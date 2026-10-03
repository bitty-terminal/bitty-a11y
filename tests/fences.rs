//! Contract fence tests: every negative path fails closed.
//!
//! Covers the verification plan's negative-path evidence: stale
//! generation, focus mismatch, unknown action, unknown label/handle,
//! fail-closed bounds, chrome tab-order exclusion, invalidation-driven
//! refresh, and the no-Event-Bus tripwire.

use bitty_a11y::{
    A11yError, ActionKind, Adapter, AnnouncementQueue, ChromeKind, CursorPosition, FocusOwner,
    Generation, HandleMap, InteractivePurpose, MAX_ANNOUNCEMENT_TEXT_LEN,
    MAX_PENDING_ANNOUNCEMENTS, MAX_SNAPSHOT_NODES, NodeView, Role, SceneKind, Snapshot,
    SnapshotBuilder, TextRun,
};
use bitty_a11y::{InvalidationSource, needs_refresh};

/// One terminal row of readable text.
fn run(row: u32, text: &str) -> TextRun {
    TextRun {
        row,
        text: text.to_string(),
    }
}

/// Standard snapshot: root, terminal leaf with two rows, one chrome
/// bar, one scene text node, one enabled button.
fn standard_snapshot(
    generation: Generation,
) -> (
    Snapshot,
    HandleMap,
    bitty_a11y::ElementHandle,
    bitty_a11y::ElementHandle,
) {
    let mut builder = SnapshotBuilder::new(generation);
    builder.set_root_name("terminal window").unwrap();
    builder
        .set_terminal(
            "shell",
            vec![run(0, "$ echo hi"), run(1, "hi")],
            CursorPosition { row: 1, col: 2 },
            true,
        )
        .unwrap();
    builder
        .add_chrome(ChromeKind::Bar, "status bar", Some("utf-8"))
        .unwrap();
    builder.add_scene(SceneKind::Text, 41);
    builder
        .add_interactive(InteractivePurpose::Button, "follow link", true)
        .unwrap();
    let (snapshot, map) = builder.finish().unwrap();
    let terminal = snapshot.lookup_label("shell").unwrap();
    let button = snapshot.lookup_label("follow link").unwrap();
    (snapshot, map, terminal, button)
}

#[test]
fn stale_generation_fails_closed() {
    let gen0 = Generation::initial();
    let (snap0, map0, _, _) = standard_snapshot(gen0);
    let mut adapter = Adapter::new();
    adapter.ingest(snap0, map0, None, &[], None).unwrap();

    // Advance on a structural change.
    let gen1 = gen0.advance();
    let (snap1, map1, _, _) = standard_snapshot(gen1);
    adapter.ingest(snap1, map1, None, &[], None).unwrap();
    assert_eq!(adapter.generation(), gen1);

    // The old map rejects its own handles against the live generation
    // (proven in `stale_handle_rejected_against_live_generation`).

    // Re-ingesting a superseded snapshot is rejected; nothing swaps.
    let (stale_snap, stale_map, _, _) = standard_snapshot(gen0);
    let err = adapter
        .ingest(stale_snap, stale_map, None, &[], None)
        .unwrap_err();
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
fn stale_handle_rejected_against_live_generation() {
    let gen0 = Generation::initial();
    let (snap0, map0, terminal0, _) = standard_snapshot(gen0);
    let gen0_map = map0.clone();
    let mut adapter = Adapter::new();
    adapter.ingest(snap0, map0, None, &[], None).unwrap();

    let gen1 = gen0.advance();
    let (snap1, map1, _, _) = standard_snapshot(gen1);
    adapter.ingest(snap1, map1, None, &[], None).unwrap();

    // The previous generation's map reports staleness, not a resolved node.
    assert_eq!(
        gen0_map.resolve(terminal0, adapter.generation()),
        Err(A11yError::StaleGeneration {
            presented: gen0.as_u64(),
            live: gen1.as_u64(),
        })
    );

    // The adapter's live map does not know the old epoch at all.
    assert_eq!(adapter.resolve(terminal0), Err(A11yError::UnknownHandle));
}

#[test]
fn stale_action_generation_rejected_before_dispatch() {
    use bitty_a11y::{ActionRequest, ActionSink, Anchor};

    struct RefusingSink;
    impl ActionSink for RefusingSink {
        fn authorize(&self, _: &ActionRequest, _: &Anchor) -> Result<(), A11yError> {
            Ok(())
        }
        fn dispatch(&mut self, _: &ActionRequest, _: &Anchor) {}
    }

    let gen0 = Generation::initial();
    let (snap0, map0, _, button0) = standard_snapshot(gen0);
    let mut adapter = Adapter::new();
    adapter.ingest(snap0, map0, None, &[], None).unwrap();

    let gen1 = gen0.advance();
    let (snap1, map1, _, _) = standard_snapshot(gen1);
    adapter.ingest(snap1, map1, None, &[], None).unwrap();

    // An action from the superseded generation never reaches the sink.
    let request = ActionRequest::new(ActionKind::Activate, button0, gen0);
    let mut sink = RefusingSink;
    assert_eq!(
        adapter.request_action(&mut sink, &request),
        Err(A11yError::StaleGeneration {
            presented: gen0.as_u64(),
            live: gen1.as_u64(),
        })
    );
}

#[test]
fn focus_mismatch_fails_closed() {
    let generation = Generation::initial();
    let (snapshot, map, terminal, button) = standard_snapshot(generation);
    let terminal_owner = FocusOwner::bind(&snapshot, &map, terminal).unwrap();
    let button_owner = FocusOwner::bind(&snapshot, &map, button).unwrap();

    let mut adapter = Adapter::new();
    adapter
        .ingest(snapshot, map, Some(terminal_owner.clone()), &[], None)
        .unwrap();

    // Agreement asserts the recorded owner.
    assert_eq!(
        adapter.synchronize_focus(Some(&terminal_owner)),
        Ok(Some(terminal_owner.clone()))
    );

    // Disagreement asserts nothing and mutates nothing.
    assert_eq!(
        adapter.synchronize_focus(Some(&button_owner)),
        Err(A11yError::FocusMismatch)
    );
    assert_eq!(
        adapter.synchronize_focus(None),
        Err(A11yError::FocusMismatch)
    );
    assert_eq!(adapter.focus().owner(), Some(&terminal_owner));
    assert_eq!(adapter.generation(), generation);
}

#[test]
fn focus_cleared_reports_no_focus() {
    let generation = Generation::initial();
    let (snapshot, map, _, _) = standard_snapshot(generation);
    let mut adapter = Adapter::new();
    adapter.ingest(snapshot, map, None, &[], None).unwrap();

    // Empty workspace / unfocused window: no owner recorded, none live.
    assert_eq!(adapter.synchronize_focus(None), Ok(None));
}

#[test]
fn chrome_handle_cannot_own_focus() {
    let generation = Generation::initial();
    let (snapshot, map, _, _) = standard_snapshot(generation);
    let chrome = snapshot.lookup_label("status bar").unwrap();
    assert_eq!(
        FocusOwner::bind(&snapshot, &map, chrome),
        Err(A11yError::FocusMismatch)
    );
}

#[test]
fn unknown_action_rejected() {
    assert_eq!(
        ActionKind::parse("launch-missiles"),
        Err(A11yError::UnknownAction {
            label: "launch-missiles".to_string(),
        })
    );
    assert_eq!(
        ActionKind::parse(""),
        Err(A11yError::UnknownAction {
            label: String::new(),
        })
    );
    // The closed set is exact: case variants are unknown.
    assert!(ActionKind::parse("ACTIVATE").is_err());
    assert_eq!(ActionKind::parse("activate"), Ok(ActionKind::Activate));
}

#[test]
fn unknown_label_and_handle_rejected() {
    let generation = Generation::initial();
    let (snapshot, map, _, _) = standard_snapshot(generation);

    // Unknown labels resolve to nothing; row content is not a label.
    assert_eq!(snapshot.lookup_label("no such surface"), None);
    assert_eq!(snapshot.lookup_label("$ echo hi"), None);

    // A handle from another map is foreign, even at the same generation:
    // resolving the live handle through the foreign map fails by epoch.
    let live_handle = snapshot.lookup_label("shell").unwrap();
    let (_, foreign_map, _, _) = standard_snapshot(generation);
    assert_eq!(
        foreign_map.resolve(live_handle, generation).map(|_| ()),
        Err(A11yError::UnknownHandle)
    );
    let _ = map;
}

#[test]
fn snapshot_get_rejects_foreign_handle() {
    let generation = Generation::initial();
    let (snap_a, _, _, _) = standard_snapshot(generation);
    let (snap_b, _, _, _) = standard_snapshot(generation);
    let handle_b = snap_b.lookup_label("shell").unwrap();
    assert!(snap_a.get(handle_b).is_none());
}

#[test]
fn unmapped_scene_kind_fails_whole_snapshot() {
    let mut builder = SnapshotBuilder::new(Generation::initial());
    builder.set_root_name("window").unwrap();
    builder
        .set_terminal("shell", vec![], CursorPosition { row: 0, col: 0 }, true)
        .unwrap();
    builder.add_scene(SceneKind::Text, 1);
    builder.add_scene(SceneKind::Unknown, 2);
    assert_eq!(builder.finish().unwrap_err(), A11yError::UnmappedSceneKind);
}

#[test]
fn overlong_names_rejected_never_truncated() {
    let long = "n".repeat(300);
    assert!(long.chars().count() > 256);

    let mut builder = SnapshotBuilder::new(Generation::initial());
    let err = builder.set_root_name(&long).unwrap_err();
    assert_eq!(err, A11yError::NameTooLong { len: 300, cap: 256 });

    let mut builder = SnapshotBuilder::new(Generation::initial());
    builder.set_root_name("window").unwrap();
    let err = builder
        .add_chrome(ChromeKind::Bar, &long, None)
        .unwrap_err();
    assert_eq!(err, A11yError::NameTooLong { len: 300, cap: 256 });

    let mut builder = SnapshotBuilder::new(Generation::initial());
    builder.set_root_name("window").unwrap();
    let err = builder
        .add_interactive(InteractivePurpose::Input, &long, true)
        .unwrap_err();
    assert_eq!(err, A11yError::NameTooLong { len: 300, cap: 256 });
}

#[test]
fn oversized_snapshot_fails_closed() {
    let mut builder = SnapshotBuilder::new(Generation::initial());
    builder.set_root_name("window").unwrap();
    let runs: Vec<TextRun> = (0..MAX_SNAPSHOT_NODES)
        .map(|row| {
            run(
                u32::try_from(row).expect("cap test stays below u32::MAX"),
                "x",
            )
        })
        .collect();
    builder
        .set_terminal("shell", runs, CursorPosition { row: 0, col: 0 }, true)
        .unwrap();
    let err = builder.finish().unwrap_err();
    assert!(matches!(err, A11yError::TooManyNodes { .. }));
}

#[test]
fn announcements_coalesce_and_drop_oldest() {
    let mut queue = AnnouncementQueue::new();

    // Consecutive duplicates collapse to one notice.
    assert_eq!(queue.push_focus("link list updated"), Ok(true));
    assert_eq!(queue.push_focus("link list updated"), Ok(false));
    assert_eq!(queue.len(), 1);

    // Distinct kinds do not coalesce.
    assert_eq!(queue.push_state("link list updated"), Ok(true));
    assert_eq!(queue.len(), 2);

    // Overlong text is rejected, never truncated.
    let long = "a".repeat(MAX_ANNOUNCEMENT_TEXT_LEN + 1);
    assert_eq!(
        queue.push_focus(&long),
        Err(A11yError::AnnouncementTooLong {
            len: MAX_ANNOUNCEMENT_TEXT_LEN + 1,
            cap: MAX_ANNOUNCEMENT_TEXT_LEN,
        })
    );

    // A full queue drops the oldest so the newest state always wins.
    let mut full = AnnouncementQueue::new();
    for i in 0..MAX_PENDING_ANNOUNCEMENTS {
        full.push_state(&format!("notice {i}")).unwrap();
    }
    full.push_state("newest").unwrap();
    assert_eq!(full.len(), MAX_PENDING_ANNOUNCEMENTS);
    let drained = full.drain();
    assert_eq!(drained.len(), MAX_PENDING_ANNOUNCEMENTS);
    assert_eq!(drained.first().unwrap().text(), "notice 1");
    assert_eq!(drained.last().unwrap().text(), "newest");
    assert!(full.is_empty());
}

#[test]
fn chrome_excluded_from_tab_order() {
    let generation = Generation::initial();
    let (snapshot, _, _, _) = standard_snapshot(generation);

    // Names and states stay exposed...
    let chrome = snapshot.lookup_label("status bar").unwrap();
    assert!(matches!(
        snapshot.get(chrome),
        Some(NodeView::Chrome { .. })
    ));
    assert_eq!(snapshot.get(chrome).map(NodeView::role), Some(None));

    // ...while the tab order holds no chrome surface.
    for handle in snapshot.tab_order() {
        assert!(!matches!(
            snapshot.get(handle),
            Some(NodeView::Chrome { .. })
        ));
    }
    // Terminal leaf plus the button participate.
    assert_eq!(snapshot.tab_order().len(), 2);
}

#[test]
fn modal_overlay_confines_focus_scope() {
    let mut builder = SnapshotBuilder::new(Generation::initial());
    builder.set_root_name("window").unwrap();
    builder
        .set_terminal("shell", vec![], CursorPosition { row: 0, col: 0 }, true)
        .unwrap();
    builder.set_modal_overlay(true);
    // Declared modal without an overlay surface fails closed.
    assert_eq!(builder.finish().unwrap_err(), A11yError::InvalidStructure);

    let mut builder = SnapshotBuilder::new(Generation::initial());
    builder.set_root_name("window").unwrap();
    builder
        .set_terminal("shell", vec![], CursorPosition { row: 0, col: 0 }, true)
        .unwrap();
    builder
        .add_chrome(ChromeKind::Overlay, "confirm dialog", Some("modal"))
        .unwrap();
    builder.set_modal_overlay(true);
    let (snapshot, _) = builder.finish().unwrap();
    let overlay = snapshot.lookup_label("confirm dialog").unwrap();
    assert_eq!(
        snapshot.focus_scope(),
        bitty_a11y::FocusScope::OverlayOnly { overlay }
    );
}

#[test]
fn refresh_is_invalidation_driven_no_timer() {
    assert!(!needs_refresh(None));
    assert!(needs_refresh(Some(InvalidationSource::FocusChange)));
    assert!(needs_refresh(Some(InvalidationSource::AsyncStateChange)));
    assert!(needs_refresh(Some(InvalidationSource::SceneUpdate)));
    assert!(needs_refresh(Some(InvalidationSource::TerminalUpdate)));
    assert!(needs_refresh(Some(InvalidationSource::ChromeUpdate)));
}

#[test]
fn no_event_bus_exposure() {
    let adapter = Adapter::new();
    assert!(adapter.event_bus_topics().is_empty());
}

#[test]
fn role_vocabulary_is_fixed_v1() {
    assert_eq!(bitty_a11y::role_of(SceneKind::Text), Ok(Role::Text));
    assert_eq!(
        bitty_a11y::role_of(SceneKind::Unknown),
        Err(A11yError::UnmappedSceneKind)
    );
    assert_eq!(InteractivePurpose::Button.role(), Role::Button);
    assert_eq!(
        InteractivePurpose::parse("slider"),
        Err(A11yError::UnmappedSceneKind)
    );
}
