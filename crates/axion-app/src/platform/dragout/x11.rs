//! A drag out of the window on X11 (and XWayland): the XDND protocol, version 5, as the source.
//!
//! The press that starts the drag stays with Axion's window (X's implicit grab), so the drag runs
//! beside it on a connection and hidden window of its own, in a thread: it follows the pointer
//! by asking the server, tells the window under it (`XdndEnter`, `XdndPosition`), owns the
//! `XdndSelection` that the target converts, and drops (`XdndDrop`) when the button goes up.

use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ConnectionExt, CreateWindowAux, EventMask, KeyButMask, PropMode,
    SelectionNotifyEvent, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, CURRENT_TIME};

use super::{MIME_PLAIN, MIME_PLAIN_UTF8, MIME_URI_LIST, Payload, data_for};

/// The XDND version spoken.
const VERSION: u32 = 5;
/// How often the pointer is asked for.
const TICK: Duration = Duration::from_millis(8);
/// A drag that is still on after this long is dropped.
const MAX_DRAG: Duration = Duration::from_secs(120);
/// How long a drop waits for the target to finish converting the selection.
const FINISH_WAIT: Duration = Duration::from_secs(3);
/// The deepest window chain looked through below the root.
const MAX_DEPTH: usize = 32;

pub struct Source {
    /// Axion's own window, which never counts as a target (in-app moves are the tab drag's).
    own: u32,
    busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Source {
    pub fn connect(own: u32) -> Option<Self> {
        Some(Self { own, busy: Default::default() })
    }

    pub fn start(&mut self, payload: Payload) -> bool {
        use std::sync::atomic::Ordering;
        if self.busy.swap(true, Ordering::SeqCst) {
            return false;
        }
        let (own, busy) = (self.own, self.busy.clone());
        let spawned = std::thread::Builder::new().name("axion-dragout".into()).spawn(move || {
            if let Err(e) = run(own, &payload) {
                tracing::warn!("tab drag out (XDND) failed: {e}");
            }
            busy.store(false, Ordering::SeqCst);
        });
        match spawned {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!("starting the tab drag failed: {e}");
                self.busy.store(false, Ordering::SeqCst);
                false
            }
        }
    }
}

struct Atoms {
    aware: Atom,
    enter: Atom,
    position: Atom,
    status: Atom,
    leave: Atom,
    drop: Atom,
    finished: Atom,
    selection: Atom,
    copy: Atom,
    targets: Atom,
    uri_list: Atom,
    plain_utf8: Atom,
    plain: Atom,
    utf8_string: Atom,
    string: Atom,
}

fn intern(conn: &RustConnection, name: &str) -> Result<Atom, Box<dyn std::error::Error>> {
    Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
}

impl Atoms {
    fn new(conn: &RustConnection) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            aware: intern(conn, "XdndAware")?,
            enter: intern(conn, "XdndEnter")?,
            position: intern(conn, "XdndPosition")?,
            status: intern(conn, "XdndStatus")?,
            leave: intern(conn, "XdndLeave")?,
            drop: intern(conn, "XdndDrop")?,
            finished: intern(conn, "XdndFinished")?,
            selection: intern(conn, "XdndSelection")?,
            copy: intern(conn, "XdndActionCopy")?,
            targets: intern(conn, "TARGETS")?,
            uri_list: intern(conn, MIME_URI_LIST)?,
            plain_utf8: intern(conn, MIME_PLAIN_UTF8)?,
            plain: intern(conn, MIME_PLAIN)?,
            utf8_string: intern(conn, "UTF8_STRING")?,
            string: intern(conn, "STRING")?,
        })
    }

    /// The MIME name of a target atom we serve.
    fn mime(&self, target: Atom) -> Option<&'static str> {
        if target == self.uri_list {
            Some(MIME_URI_LIST)
        } else if [self.plain_utf8, self.utf8_string, self.plain, self.string].contains(&target) {
            Some(MIME_PLAIN)
        } else {
            None
        }
    }
}

/// The window under the pointer that speaks XDND (and its version), other than `own`.
fn target_at(
    conn: &RustConnection,
    root: Window,
    atoms: &Atoms,
    own: u32,
) -> Result<Option<(Window, u32)>, Box<dyn std::error::Error>> {
    let mut found = None;
    let mut at = root;
    for _ in 0..MAX_DEPTH {
        let child = conn.query_pointer(at)?.reply()?.child;
        if child == 0 {
            break;
        }
        if child == own {
            return Ok(None);
        }
        at = child;
        let reply = conn.get_property(false, at, atoms.aware, AtomEnum::ATOM, 0, 1)?.reply()?;
        if let Some(version) = reply.value32().and_then(|mut v| v.next()) {
            found = Some((at, version.min(VERSION)));
        }
    }
    Ok(found)
}

fn send(
    conn: &RustConnection,
    to: Window,
    from: Window,
    kind: Atom,
    data: [u32; 5],
) -> Result<(), Box<dyn std::error::Error>> {
    let event = ClientMessageEvent::new(32, to, kind, data);
    conn.send_event(false, to, EventMask::NO_EVENT, event)?;
    conn.flush()?;
    let _ = from;
    Ok(())
}

fn run(own: u32, payload: &Payload) -> Result<(), Box<dyn std::error::Error>> {
    let (conn, screen_num) = x11rb::connect(None)?;
    let root = conn.setup().roots.get(screen_num).ok_or("no X screen")?.root;
    let atoms = Atoms::new(&conn)?;
    // The drag's own window: it owns the selection and receives the target's answers.
    let source = conn.generate_id()?;
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        source,
        root,
        -10,
        -10,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new(),
    )?;
    conn.set_selection_owner(source, atoms.selection, CURRENT_TIME)?;
    conn.flush()?;
    tracing::debug!(url = %payload.url, "tab drag out started (XDND)");

    let started = Instant::now();
    // The window the drag is over (with its XDND version), and whether it accepts a drop.
    let mut over: Option<(Window, u32)> = None;
    let mut accepted = false;
    let mut last_sent = (i16::MIN, i16::MIN);
    let mut dropped: Option<(Window, Instant)> = None;

    let result = loop {
        // Answers first: status of the target, conversion requests, finish.
        while let Some(event) = conn.poll_for_event()? {
            match event {
                Event::ClientMessage(m) if m.type_ == atoms.status => {
                    if over.is_some_and(|(w, _)| w == m.data.as_data32()[0]) {
                        accepted = m.data.as_data32()[1] & 1 != 0;
                    }
                }
                Event::ClientMessage(m) if m.type_ == atoms.finished => {
                    if dropped.is_some() {
                        return finish(&conn, source);
                    }
                }
                Event::SelectionRequest(request) => answer(&conn, &atoms, payload, &request)?,
                _ => {}
            }
        }

        if let Some((target, since)) = dropped {
            // The target converts the selection after the drop; stay to serve it.
            if since.elapsed() > FINISH_WAIT {
                let _ = target;
                break Ok(());
            }
            std::thread::sleep(TICK);
            continue;
        }

        if started.elapsed() > MAX_DRAG {
            if let Some((target, _)) = over {
                send(&conn, target, source, atoms.leave, [source, 0, 0, 0, 0])?;
            }
            break Ok(());
        }

        let pointer = conn.query_pointer(root)?.reply()?;
        let released = !pointer.mask.contains(KeyButMask::BUTTON1);
        let now = target_at(&conn, root, &atoms, own)?;

        if now.map(|n| n.0) != over.map(|o| o.0) {
            if let Some((old, _)) = over {
                send(&conn, old, source, atoms.leave, [source, 0, 0, 0, 0])?;
            }
            accepted = false;
            last_sent = (i16::MIN, i16::MIN);
            if let Some((new, version)) = now {
                let flags = version << 24;
                send(
                    &conn,
                    new,
                    source,
                    atoms.enter,
                    [source, flags, atoms.uri_list, atoms.plain_utf8, atoms.plain],
                )?;
            }
            over = now;
        }

        if released {
            match over {
                Some((target, _)) if accepted => {
                    send(&conn, target, source, atoms.drop, [source, 0, CURRENT_TIME, 0, 0])?;
                    dropped = Some((target, Instant::now()));
                    continue;
                }
                Some((target, _)) => {
                    send(&conn, target, source, atoms.leave, [source, 0, 0, 0, 0])?;
                    break Ok(());
                }
                None => break Ok(()),
            }
        }

        if let Some((target, _)) = over
            && last_sent != (pointer.root_x, pointer.root_y)
        {
            last_sent = (pointer.root_x, pointer.root_y);
            let position = ((pointer.root_x as u16 as u32) << 16) | pointer.root_y as u16 as u32;
            send(&conn, target, source, atoms.position, [source, 0, position, CURRENT_TIME, atoms.copy])?;
        }
        std::thread::sleep(TICK);
    };
    conn.destroy_window(source)?;
    conn.flush()?;
    result
}

fn finish(conn: &RustConnection, source: Window) -> Result<(), Box<dyn std::error::Error>> {
    conn.destroy_window(source)?;
    conn.flush()?;
    Ok(())
}

/// Hands the target the converted selection (or refuses with `property = None`).
fn answer(
    conn: &RustConnection,
    atoms: &Atoms,
    payload: &Payload,
    request: &x11rb::protocol::xproto::SelectionRequestEvent,
) -> Result<(), Box<dyn std::error::Error>> {
    let property = if request.property == 0 { request.target } else { request.property };
    let mut done = false;
    if request.target == atoms.targets {
        let list =
            [atoms.targets, atoms.uri_list, atoms.plain_utf8, atoms.plain, atoms.utf8_string, atoms.string];
        conn.change_property32(PropMode::REPLACE, request.requestor, property, AtomEnum::ATOM, &list)?;
        done = true;
    } else if let Some(bytes) = atoms.mime(request.target).and_then(|mime| data_for(payload, mime)) {
        conn.change_property8(PropMode::REPLACE, request.requestor, property, request.target, &bytes)?;
        done = true;
    }
    let notify = SelectionNotifyEvent {
        response_type: x11rb::protocol::xproto::SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: request.time,
        requestor: request.requestor,
        selection: request.selection,
        target: request.target,
        property: if done { property } else { 0 },
    };
    conn.send_event(false, request.requestor, EventMask::NO_EVENT, notify)?;
    conn.flush()?;
    Ok(())
}
