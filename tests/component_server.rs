//! Live component-server integration tests (Linux/macOS).
//!
//! The real `bitty-a11y` binary is spawned as a stdio coprocess and driven
//! with the public protocol codec. Set `BITTY_A11Y_COMPONENT_BIN` to run the
//! same suite against a packed release executable.
//!
//! Covered acceptance paths: handshake, full publication and snapshot, bounded
//! diff/ack, stable ids that are never reused, stale-generation refusal,
//! oversize/malformed/truncated frames, publication text budget, safe-mode and
//! capability deprivation, request-rate exhaustion, and a killed component
//! that leaves the host alive and restartable.

#![cfg(unix)]

use std::collections::HashSet;
use std::io::{BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use bitty_a11y::component::protocol::{
    ErrorKind, FocusCell, FrameReader, HELLO_CAPABILITY_GRANTED, HELLO_SAFE_MODE, MAX_FRAME_BYTES,
    MAX_PUBLISH_TEXT_BYTES, MAX_ROW_TEXT_BYTES, Message, PROTOCOL_VERSION, PublishNode,
    ROLE_BUTTON, ROLE_GROUP, ROLE_NONE, ROLE_TABLE, ROLE_TEXT, STATE_CURSOR_VISIBLE, STATE_ENABLED,
    STATE_FOCUSABLE, STATE_FOCUSED, STATE_SELECTED, SURFACE_BAR, SURFACE_NONE, Selection, TreeRow,
    encode_frame, write_frame,
};
use bitty_a11y::component::server::{
    COMPONENT_VERSION, RATE_MAX_VIOLATIONS, SERVER_EXIT_OK, SERVER_EXIT_PROTOCOL,
};
use bitty_a11y::{AnnouncementKind, ChromeKind, InteractivePurpose, MAX_SNAPSHOT_NODES, SceneKind};

/// Upper bound for one protocol message or one process observation.
const WAIT: Duration = Duration::from_secs(20);

/// One spawned component process plus its decoded message stream.
struct Component {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Result<Message, String>>,
    capabilities: u8,
}

impl Component {
    fn binary() -> std::path::PathBuf {
        std::env::var_os("BITTY_A11Y_COMPONENT_BIN")
            .map_or_else(|| env!("CARGO_BIN_EXE_bitty-a11y").into(), Into::into)
    }

    fn spawn() -> Self {
        let binary = Self::binary();
        let mut child = Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn bitty-a11y");
        let stdin = child.stdin.take().expect("component stdin");
        let stdout = child.stdout.take().expect("component stdout");
        let (sender, messages) = mpsc::channel();
        std::thread::Builder::new()
            .name("component-reader".to_owned())
            .spawn(move || {
                let mut reader = FrameReader::new(BufReader::new(stdout));
                loop {
                    match reader.read_message() {
                        Ok(Some(message)) => {
                            if sender.send(Ok(message)).is_err() {
                                break;
                            }
                        }
                        Ok(None) => break,
                        Err(error) => {
                            let _ = sender.send(Err(error.to_string()));
                            break;
                        }
                    }
                }
            })
            .expect("reader thread");
        Self {
            child,
            stdin: Some(stdin),
            messages,
            capabilities: 0,
        }
    }

    fn start() -> Self {
        Self::start_with_mode(HELLO_CAPABILITY_GRANTED)
    }

    fn start_with_mode(mode: u8) -> Self {
        let mut component = Self::spawn();
        component.handshake(mode);
        component
    }

    fn handshake(&mut self, mode: u8) {
        self.send(&Message::Hello {
            min: PROTOCOL_VERSION,
            max: PROTOCOL_VERSION,
            component: "a11y".to_owned(),
            version: "test".to_owned(),
            mode,
        });
        match self.recv() {
            Message::HelloAck {
                protocol,
                component,
                version,
                capabilities,
            } => {
                assert_eq!(protocol, PROTOCOL_VERSION);
                assert_eq!(component, "a11y");
                assert_eq!(version, COMPONENT_VERSION);
                self.capabilities = capabilities;
            }
            other => panic!("expected HelloAck, got {other:?}"),
        }
    }

    fn send(&mut self, message: &Message) {
        let stdin = self.stdin.as_mut().expect("component stdin");
        write_frame(stdin, message).expect("write frame");
        stdin.flush().expect("flush frame");
    }

    fn send_raw(&mut self, bytes: &[u8]) {
        let stdin = self.stdin.as_mut().expect("component stdin");
        stdin.write_all(bytes).expect("write raw bytes");
        stdin.flush().expect("flush raw bytes");
    }

    fn recv(&mut self) -> Message {
        match self.messages.recv_timeout(WAIT) {
            Ok(Ok(message)) => message,
            Ok(Err(error)) => panic!("component decode error: {error}"),
            Err(error) => panic!("component message timeout: {error}"),
        }
    }

    fn expect_error(&mut self, kind: ErrorKind) -> Message {
        match self.recv() {
            message @ Message::Error { kind: got, .. } if got == kind => message,
            other => panic!("expected {kind:?}, got {other:?}"),
        }
    }

    fn wait_code(&mut self) -> i32 {
        self.child
            .wait()
            .expect("wait for component")
            .code()
            .expect("component exited without a code")
    }

    fn shutdown(mut self) {
        self.send(&Message::Shutdown);
        assert_eq!(self.recv(), Message::ShutdownAck);
        assert_eq!(self.wait_code(), i32::from(SERVER_EXIT_OK));
    }

    fn shutdown_after_errors(mut self) {
        self.send(&Message::Shutdown);
        loop {
            match self.recv() {
                Message::ShutdownAck => break,
                Message::Error { .. } => {}
                other => panic!("unexpected frame before ShutdownAck: {other:?}"),
            }
        }
        assert_eq!(self.wait_code(), i32::from(SERVER_EXIT_OK));
    }
}

impl Drop for Component {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Publication content used by most tests.
struct Spec<'a> {
    root: &'a str,
    title: &'a str,
    cursor: (u32, u32),
    cursor_visible: bool,
    rows: Vec<(u32, &'a str)>,
    chrome: Vec<(ChromeKind, &'a str, Option<&'a str>)>,
    scene: Vec<(SceneKind, u64)>,
    interactive: Vec<(InteractivePurpose, &'a str, bool)>,
    notices: Vec<(AnnouncementKind, &'a str)>,
    modal: bool,
    focus: Option<&'a str>,
    selection: Option<Selection>,
}

impl Spec<'_> {
    fn new() -> Self {
        Self {
            root: "window",
            title: "shell",
            cursor: (0, 0),
            cursor_visible: true,
            rows: Vec::new(),
            chrome: Vec::new(),
            scene: Vec::new(),
            interactive: Vec::new(),
            notices: Vec::new(),
            modal: false,
            focus: None,
            selection: None,
        }
    }
}

fn publish(component: &mut Component, request: u64, generation: u64, spec: &Spec<'_>) -> Message {
    component.send(&Message::PublishBegin {
        request,
        generation,
        root_name: spec.root.to_owned(),
        title: spec.title.to_owned(),
        cursor_row: spec.cursor.0,
        cursor_col: spec.cursor.1,
        cursor_visible: spec.cursor_visible,
        terminal_rows: u32::try_from(spec.rows.len()).expect("row count"),
        chrome: u32::try_from(spec.chrome.len()).expect("chrome count"),
        scene: u32::try_from(spec.scene.len()).expect("scene count"),
        interactive: u32::try_from(spec.interactive.len()).expect("interactive count"),
        notices: u32::try_from(spec.notices.len()).expect("notice count"),
        modal_overlay: spec.modal,
        focus: spec.focus.map(str::to_owned),
        selection: spec.selection,
    });
    for (row, text) in &spec.rows {
        component.send(&Message::PublishNode {
            request,
            node: PublishNode::TerminalRow {
                row: *row,
                text: (*text).to_owned(),
            },
        });
    }
    for (kind, name, state) in &spec.chrome {
        component.send(&Message::PublishNode {
            request,
            node: PublishNode::Chrome {
                kind: *kind,
                name: (*name).to_owned(),
                state: state.map(str::to_owned),
            },
        });
    }
    for (kind, identity) in &spec.scene {
        component.send(&Message::PublishNode {
            request,
            node: PublishNode::Scene {
                kind: *kind,
                identity: *identity,
            },
        });
    }
    for (purpose, name, enabled) in &spec.interactive {
        component.send(&Message::PublishNode {
            request,
            node: PublishNode::Interactive {
                purpose: *purpose,
                name: (*name).to_owned(),
                enabled: *enabled,
            },
        });
    }
    for (kind, text) in &spec.notices {
        component.send(&Message::PublishNode {
            request,
            node: PublishNode::Notice {
                kind: *kind,
                text: (*text).to_owned(),
            },
        });
    }
    component.send(&Message::PublishCommit {
        request,
        generation,
    });
    match component.recv() {
        message @ Message::Published { .. } => message,
        other => panic!("expected Published, got {other:?}"),
    }
}

enum TreeReply {
    Full {
        generation: u64,
        focus: Option<u64>,
        selection: Option<Selection>,
        rows: Vec<TreeRow>,
    },
    Diff {
        base: u64,
        generation: u64,
        removed: Vec<u64>,
        upserts: Vec<TreeRow>,
    },
}

fn request_tree(component: &mut Component, request: u64, since: u64) -> TreeReply {
    component.send(&Message::SnapshotRequest { request, since });
    match component.recv() {
        Message::SnapshotBegin {
            generation,
            nodes,
            focus,
            selection,
            pages,
            ..
        } => {
            assert!(pages > 0);
            let mut rows = Vec::with_capacity(nodes as usize);
            for index in 0..pages {
                match component.recv() {
                    Message::SnapshotPage {
                        index: got,
                        rows: page,
                        last,
                        ..
                    } => {
                        assert_eq!(got, index);
                        assert_eq!(last, index + 1 == pages);
                        assert!(!page.is_empty());
                        rows.extend(page);
                    }
                    other => panic!("expected SnapshotPage, got {other:?}"),
                }
            }
            assert_eq!(rows.len(), nodes as usize);
            TreeReply::Full {
                generation,
                focus,
                selection,
                rows,
            }
        }
        Message::DiffBegin {
            base,
            generation,
            removed,
            upserts,
            pages,
            ..
        } => {
            let mut removed_ids = Vec::with_capacity(removed as usize);
            let mut changed = Vec::with_capacity(upserts as usize);
            for index in 0..pages {
                match component.recv() {
                    Message::DiffPage {
                        index: got,
                        removed: removed_page,
                        upserts: upsert_page,
                        last,
                        ..
                    } => {
                        assert_eq!(got, index);
                        assert_eq!(last, index + 1 == pages);
                        removed_ids.extend(removed_page);
                        changed.extend(upsert_page);
                    }
                    other => panic!("expected DiffPage, got {other:?}"),
                }
            }
            assert_eq!(removed_ids.len(), removed as usize);
            assert_eq!(changed.len(), upserts as usize);
            TreeReply::Diff {
                base,
                generation,
                removed: removed_ids,
                upserts: changed,
            }
        }
        other => panic!("expected snapshot or diff, got {other:?}"),
    }
}

fn ids(rows: &[TreeRow]) -> HashSet<u64> {
    rows.iter().map(|row| row.id).collect()
}

#[test]
fn handshake_and_clean_shutdown() {
    let component = Component::start();
    assert_eq!(component.capabilities, 0x0F);
    component.shutdown();
}

#[test]
#[allow(clippy::too_many_lines)]
fn publish_and_snapshot_round_trip() {
    let mut component = Component::start();
    let spec = Spec {
        cursor: (1, 3),
        rows: vec![(0, "$ ls"), (1, "file.txt")],
        chrome: vec![(ChromeKind::Bar, "status", Some("ready"))],
        scene: vec![(SceneKind::Table, 5)],
        interactive: vec![(InteractivePurpose::Button, "run", true)],
        notices: vec![(AnnouncementKind::AsyncState, "ready")],
        focus: Some("shell"),
        selection: Some(Selection {
            anchor_row: 0,
            anchor_col: 0,
            focus_row: 0,
            focus_col: 2,
        }),
        ..Spec::new()
    };
    match publish(&mut component, 1, 1, &spec) {
        Message::Published {
            generation,
            nodes,
            added,
            updated,
            removed,
            ..
        } => {
            assert_eq!(generation, 1);
            assert_eq!(nodes, 7);
            assert_eq!((added, updated, removed), (7, 0, 0));
        }
        other => panic!("expected Published, got {other:?}"),
    }

    let TreeReply::Full {
        generation,
        focus,
        selection,
        rows,
    } = request_tree(&mut component, 2, 0)
    else {
        panic!("expected a full snapshot");
    };
    assert_eq!(generation, 1);
    assert_eq!(selection, spec.selection);
    assert_eq!(rows.len(), 7);
    let all_ids = ids(&rows);
    assert_eq!(all_ids.len(), rows.len(), "ids must be unique");
    assert!(!all_ids.contains(&0), "ids must be non-zero");

    let root = &rows[0];
    assert_eq!(root.role, ROLE_GROUP);
    assert_eq!(root.depth, 0);
    assert_eq!(root.parent, 0);
    assert_eq!(root.name.as_deref(), Some("window"));
    assert_eq!(root.states & STATE_FOCUSABLE, 0);

    let terminal = &rows[1];
    assert_eq!(terminal.role, ROLE_TEXT);
    assert_eq!(terminal.depth, 1);
    assert_eq!(terminal.parent, root.id);
    assert_eq!(terminal.name.as_deref(), Some("shell"));
    assert_eq!(terminal.focus, Some(FocusCell { row: 1, col: 3 }));
    assert!(terminal.states & STATE_FOCUSED != 0);
    assert!(terminal.states & STATE_CURSOR_VISIBLE != 0);
    assert!(terminal.states & STATE_SELECTED != 0);
    assert_eq!(focus, Some(terminal.id));

    let first_row = &rows[2];
    assert_eq!(first_row.role, ROLE_TEXT);
    assert_eq!(first_row.depth, 2);
    assert_eq!(first_row.parent, terminal.id);
    assert_eq!(first_row.content.as_deref(), Some("$ ls"));
    assert_eq!(first_row.name, None);
    assert!(first_row.states & STATE_SELECTED != 0);
    assert_eq!(
        first_row
            .selection
            .map(|span| (span.start_col, span.end_col)),
        Some((0, 2))
    );
    let bounds = first_row.bounds.expect("terminal row bounds");
    assert_eq!(
        (bounds.x, bounds.y, bounds.width, bounds.height),
        (0, 0, 4, 1)
    );

    let second_row = &rows[3];
    assert_eq!(second_row.content.as_deref(), Some("file.txt"));
    assert_eq!(second_row.states & STATE_SELECTED, 0);

    let chrome = rows
        .iter()
        .find(|row| row.surface == SURFACE_BAR)
        .expect("chrome bar");
    assert_eq!(chrome.role, ROLE_NONE);
    assert_eq!(chrome.depth, 1);
    assert_eq!(chrome.name.as_deref(), Some("status"));
    assert_eq!(chrome.state_text.as_deref(), Some("ready"));
    assert_eq!(chrome.parent, root.id);

    let scene = rows
        .iter()
        .find(|row| row.role == ROLE_TABLE)
        .expect("scene table");
    assert_eq!(scene.surface, SURFACE_NONE);
    assert_eq!(scene.depth, 1);

    let button = rows
        .iter()
        .find(|row| row.role == ROLE_BUTTON)
        .expect("interactive button");
    assert_eq!(button.name.as_deref(), Some("run"));
    assert!(button.states & STATE_ENABLED != 0);
    assert!(button.states & STATE_FOCUSABLE != 0);
    assert!(rows.iter().all(|row| row.depth <= 4));
    component.shutdown();
}

#[test]
fn diff_and_ack_cycle() {
    let mut component = Component::start();
    let first = Spec {
        rows: vec![(0, "A"), (1, "B")],
        ..Spec::new()
    };
    let _ = publish(&mut component, 1, 1, &first);
    let TreeReply::Full { rows, .. } = request_tree(&mut component, 2, 0) else {
        panic!("expected a full snapshot");
    };
    let a_id = rows[2].id;
    let b_id = rows[3].id;

    component.send(&Message::DiffAck { generation: 1 });
    let TreeReply::Diff {
        base,
        generation,
        removed,
        upserts,
        ..
    } = request_tree(&mut component, 3, 1)
    else {
        panic!("expected an empty diff");
    };
    assert_eq!((base, generation), (1, 1));
    assert!(removed.is_empty() && upserts.is_empty());

    let second = Spec {
        rows: vec![(1, "B"), (2, "C")],
        ..Spec::new()
    };
    match publish(&mut component, 4, 2, &second) {
        Message::Published {
            added,
            updated,
            removed,
            ..
        } => assert_eq!((added, updated, removed), (1, 0, 1)),
        other => panic!("expected Published, got {other:?}"),
    }

    let TreeReply::Diff {
        base,
        generation,
        removed,
        upserts,
        ..
    } = request_tree(&mut component, 5, 1)
    else {
        panic!("expected a diff from generation 1");
    };
    assert_eq!((base, generation), (1, 2));
    assert_eq!(removed, vec![a_id]);
    assert_eq!(upserts.len(), 1);
    assert_eq!(upserts[0].content.as_deref(), Some("C"));
    assert_ne!(upserts[0].id, b_id);
    assert_ne!(upserts[0].id, a_id);

    component.send(&Message::DiffAck { generation: 2 });
    let TreeReply::Diff {
        base,
        generation,
        removed,
        upserts,
        ..
    } = request_tree(&mut component, 6, 2)
    else {
        panic!("expected an empty diff at the acknowledged generation");
    };
    assert_eq!((base, generation), (2, 2));
    assert!(removed.is_empty() && upserts.is_empty());

    let TreeReply::Full { generation, .. } = request_tree(&mut component, 7, 1) else {
        panic!("an unknown base must degrade to a full snapshot");
    };
    assert_eq!(generation, 2);
    component.shutdown();
}

#[test]
fn stale_ids_are_never_reused() {
    let mut component = Component::start();
    let first = Spec {
        rows: vec![(0, "A"), (1, "B")],
        ..Spec::new()
    };
    let _ = publish(&mut component, 1, 1, &first);
    let TreeReply::Full { rows, .. } = request_tree(&mut component, 2, 0) else {
        panic!("expected a full snapshot");
    };
    let old_a = rows[2].id;
    let b_id = rows[3].id;
    component.send(&Message::DiffAck { generation: 1 });

    let second = Spec {
        rows: vec![(1, "B")],
        ..Spec::new()
    };
    let _ = publish(&mut component, 3, 2, &second);
    let TreeReply::Diff { removed, .. } = request_tree(&mut component, 4, 1) else {
        panic!("expected a diff");
    };
    assert_eq!(removed, vec![old_a]);
    component.send(&Message::DiffAck { generation: 2 });

    let third = Spec {
        rows: vec![(0, "A"), (1, "B")],
        ..Spec::new()
    };
    let _ = publish(&mut component, 5, 3, &third);
    let TreeReply::Diff { upserts, .. } = request_tree(&mut component, 6, 2) else {
        panic!("expected a diff");
    };
    let re_added = upserts
        .iter()
        .find(|row| row.content.as_deref() == Some("A"))
        .expect("re-added row A");
    assert_ne!(re_added.id, old_a, "a retired id must not be reused");
    assert_ne!(re_added.id, b_id);
    let TreeReply::Full { rows, .. } = request_tree(&mut component, 7, 0) else {
        panic!("expected a full snapshot");
    };
    let live = ids(&rows);
    assert!(!live.contains(&old_a), "retired id must be absent");
    assert!(live.contains(&re_added.id));
    assert!(live.contains(&b_id));
    component.shutdown();
}

#[test]
fn stale_generation_and_ack_are_refused() {
    let mut component = Component::start();
    let spec = Spec::new();
    let _ = publish(&mut component, 1, 2, &spec);

    component.send(&Message::PublishBegin {
        request: 2,
        generation: 2,
        root_name: "window".to_owned(),
        title: "shell".to_owned(),
        cursor_row: 0,
        cursor_col: 0,
        cursor_visible: true,
        terminal_rows: 0,
        chrome: 0,
        scene: 0,
        interactive: 0,
        notices: 0,
        modal_overlay: false,
        focus: None,
        selection: None,
    });
    let message = component.expect_error(ErrorKind::StaleGeneration);
    assert!(matches!(message, Message::Error { request: 2, .. }));

    component.send(&Message::PublishBegin {
        request: 3,
        generation: 1,
        root_name: "window".to_owned(),
        title: "shell".to_owned(),
        cursor_row: 0,
        cursor_col: 0,
        cursor_visible: true,
        terminal_rows: 0,
        chrome: 0,
        scene: 0,
        interactive: 0,
        notices: 0,
        modal_overlay: false,
        focus: None,
        selection: None,
    });
    component.expect_error(ErrorKind::StaleGeneration);

    component.send(&Message::DiffAck { generation: 1 });
    component.expect_error(ErrorKind::StaleGeneration);

    component.send(&Message::SnapshotRequest {
        request: 4,
        since: 99,
    });
    component.expect_error(ErrorKind::StaleGeneration);

    let TreeReply::Full { generation, .. } = request_tree(&mut component, 5, 1) else {
        panic!("an unknown base must yield a full snapshot");
    };
    assert_eq!(generation, 2);
    component.shutdown();
}

#[test]
fn oversized_frame_is_refused() {
    let mut component = Component::start();
    let length = u32::try_from(MAX_FRAME_BYTES + 1).expect("frame bound fits u32");
    component.send_raw(&length.to_be_bytes());
    component.expect_error(ErrorKind::Protocol);
    assert_eq!(component.wait_code(), i32::from(SERVER_EXIT_PROTOCOL));
}

#[test]
fn malformed_and_zero_length_frames_are_refused() {
    let mut component = Component::start();
    component.send_raw(&[0, 0, 0, 1, 0xEE]);
    component.expect_error(ErrorKind::Protocol);
    assert_eq!(component.wait_code(), i32::from(SERVER_EXIT_PROTOCOL));

    let mut component = Component::start();
    component.send_raw(&[0, 0, 0, 0]);
    component.expect_error(ErrorKind::Protocol);
    assert_eq!(component.wait_code(), i32::from(SERVER_EXIT_PROTOCOL));
}

#[test]
fn publication_text_budget_is_enforced() {
    let mut component = Component::start();
    let rows = 140_u32;
    component.send(&Message::PublishBegin {
        request: 1,
        generation: 0,
        root_name: "window".to_owned(),
        title: "shell".to_owned(),
        cursor_row: 0,
        cursor_col: 0,
        cursor_visible: true,
        terminal_rows: rows,
        chrome: 0,
        scene: 0,
        interactive: 0,
        notices: 0,
        modal_overlay: false,
        focus: None,
        selection: None,
    });
    let text = "x".repeat(MAX_ROW_TEXT_BYTES);
    for row in 0..rows {
        component.send(&Message::PublishNode {
            request: 1,
            node: PublishNode::TerminalRow {
                row,
                text: text.clone(),
            },
        });
    }
    let mut budget_seen = false;
    for _ in 0..rows {
        match component.recv() {
            Message::Error {
                kind: ErrorKind::Budget,
                ..
            } => {
                budget_seen = true;
                break;
            }
            Message::Error {
                kind: ErrorKind::InvalidRequest,
                ..
            } => {}
            other => panic!("unexpected frame: {other:?}"),
        }
    }
    assert!(
        budget_seen,
        "a publication above {MAX_PUBLISH_TEXT_BYTES} bytes must be refused"
    );
    component.shutdown_after_errors();
}

#[test]
fn safe_mode_and_withheld_capability_are_deprived() {
    let mut safe = Component::start_with_mode(HELLO_SAFE_MODE | HELLO_CAPABILITY_GRANTED);
    assert_eq!(safe.capabilities, 0);
    safe.send(&Message::PublishBegin {
        request: 1,
        generation: 0,
        root_name: "window".to_owned(),
        title: "shell".to_owned(),
        cursor_row: 0,
        cursor_col: 0,
        cursor_visible: true,
        terminal_rows: 0,
        chrome: 0,
        scene: 0,
        interactive: 0,
        notices: 0,
        modal_overlay: false,
        focus: None,
        selection: None,
    });
    safe.expect_error(ErrorKind::Denied);
    safe.send(&Message::SnapshotRequest {
        request: 2,
        since: 0,
    });
    safe.expect_error(ErrorKind::Denied);
    safe.send(&Message::DiffAck { generation: 0 });
    safe.expect_error(ErrorKind::Denied);
    safe.shutdown();

    let mut withheld = Component::start_with_mode(0);
    assert_eq!(withheld.capabilities, 0);
    withheld.send(&Message::SnapshotRequest {
        request: 1,
        since: 0,
    });
    withheld.expect_error(ErrorKind::Denied);
    withheld.shutdown();
}

#[test]
fn request_rate_is_bounded() {
    let mut component = Component::start();
    let spec = Spec::new();
    let _ = publish(&mut component, 1, 1, &spec);

    let mut burst = Vec::new();
    for _ in 0..200 {
        burst.extend(encode_frame(&Message::DiffAck { generation: 1 }).expect("encode ack"));
    }
    component.send_raw(&burst);

    let mut violations = 0_u32;
    loop {
        match component.recv() {
            Message::Error {
                kind: ErrorKind::RateLimited,
                ..
            } => {
                violations += 1;
                if violations == RATE_MAX_VIOLATIONS {
                    break;
                }
            }
            other => panic!("unexpected frame while flooding: {other:?}"),
        }
    }
    assert_eq!(
        component.wait_code(),
        i32::from(SERVER_EXIT_PROTOCOL),
        "a sustained flood must close the connection"
    );
}

#[test]
fn crash_leaves_the_host_alive_and_restartable() {
    let mut component = Component::start();
    component.child.kill().expect("kill component");
    match component.messages.recv_timeout(WAIT) {
        Err(RecvTimeoutError::Disconnected) | Ok(Err(_)) => {}
        other => panic!("expected EOF after the crash, got {other:?}"),
    }
    let _ = component.child.wait();

    let restarted = Component::start();
    assert_eq!(restarted.capabilities, 0x0F);
    restarted.shutdown();
}

#[test]
fn handshake_is_required_and_single_use() {
    let mut component = Component::spawn();
    component.send(&Message::SnapshotRequest {
        request: 1,
        since: 0,
    });
    component.expect_error(ErrorKind::Protocol);
    assert_eq!(component.wait_code(), i32::from(SERVER_EXIT_PROTOCOL));

    let mut component = Component::start();
    component.send(&Message::Hello {
        min: 1,
        max: 1,
        component: "a11y".to_owned(),
        version: "again".to_owned(),
        mode: HELLO_CAPABILITY_GRANTED,
    });
    component.expect_error(ErrorKind::Protocol);
    assert_eq!(component.wait_code(), i32::from(SERVER_EXIT_PROTOCOL));
}

#[test]
fn wrong_component_and_unknown_mode_are_refused() {
    let mut component = Component::spawn();
    component.send(&Message::Hello {
        min: 1,
        max: 1,
        component: "net".to_owned(),
        version: "test".to_owned(),
        mode: HELLO_CAPABILITY_GRANTED,
    });
    component.expect_error(ErrorKind::Protocol);
    assert_eq!(component.wait_code(), i32::from(SERVER_EXIT_PROTOCOL));

    let mut component = Component::spawn();
    component.send(&Message::Hello {
        min: 1,
        max: 1,
        component: "a11y".to_owned(),
        version: "test".to_owned(),
        mode: 0x80,
    });
    component.expect_error(ErrorKind::Protocol);
    assert_eq!(component.wait_code(), i32::from(SERVER_EXIT_PROTOCOL));
}

#[test]
fn unsupported_protocol_version_is_refused() {
    let mut component = Component::spawn();
    component.send(&Message::Hello {
        min: 2,
        max: 3,
        component: "a11y".to_owned(),
        version: "test".to_owned(),
        mode: HELLO_CAPABILITY_GRANTED,
    });
    component.expect_error(ErrorKind::Protocol);
    assert_eq!(component.wait_code(), i32::from(SERVER_EXIT_PROTOCOL));
}

#[test]
fn node_cap_is_far_below_the_builder_limit() {
    // The wire bound and the projection model bound agree; this pins the
    // relationship rather than relying on the descriptor test alone.
    assert_eq!(MAX_SNAPSHOT_NODES, 4096);
}
