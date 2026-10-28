//! A drag out of the window on X11 (and XWayland): the XDND protocol, version 5, as the source.
//!
//! The press that starts the drag stays with Axion's window (X's implicit grab), so the drag runs
//! beside it on a connection and hidden window of its own, in a thread: it follows the pointer
//! by asking the server, tells the window under it (`XdndEnter`, `XdndPosition`), owns the
//! `XdndSelection` that the target converts, and drops (`XdndDrop`) when the button goes up.
//!
//! XDND has no drag image, so the picture is a window of ours that follows the pointer: an
//! override-redirect window with a 32-bit ARGB visual (translucent under a compositing manager;
//! without one the transparent parts show black) whose input region is empty, so the pointer
//! still "is" over the window beneath it and the target lookup sees through it.

use std::time::{Duration, Instant};

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ConnectionExt, CreateGCAux, CreateWindowAux, EventMask, ImageFormat,
    KeyButMask, PropMode, SelectionNotifyEvent, StackMode, VisualClass, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, CURRENT_TIME};

use super::{Image, MIME_PLAIN, MIME_PLAIN_UTF8, MIME_URI_LIST, Payload, data_for};

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

    pub fn start(&mut self, payload: Payload, image: Option<Image>) -> bool {
        use std::sync::atomic::Ordering;
        if self.busy.swap(true, Ordering::SeqCst) {
            return false;
        }
        let (own, busy) = (self.own, self.busy.clone());
        let spawned = std::thread::Builder::new().name("axion-dragout".into()).spawn(move || {
            if let Err(e) = run(own, &payload, image) {
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

/// The drag image's window.
struct Icon {
    window: Window,
    hotspot: (i32, i32),
}

impl Icon {
    /// Makes and shows the window with `image`; `Err` when the screen has no 32-bit visual or
    /// the picture cannot be sent (the drag then goes on without one).
    fn show(conn: &RustConnection, root: Window, image: &Image) -> Result<Self, Box<dyn std::error::Error>> {
        let (w, h) = (u16::try_from(image.width)?, u16::try_from(image.height)?);
        let screen = conn.setup().roots.iter().find(|s| s.root == root).ok_or("no X screen")?;
        let visual = screen
            .allowed_depths
            .iter()
            .filter(|d| d.depth == 32)
            .flat_map(|d| d.visuals.iter())
            .find(|v| v.class == VisualClass::TRUE_COLOR)
            .ok_or("no 32-bit visual")?
            .visual_id;
        let colormap = conn.generate_id()?;
        conn.create_colormap(x11rb::protocol::xproto::ColormapAlloc::NONE, colormap, root, visual)?;
        let window = conn.generate_id()?;
        conn.create_window(
            32,
            window,
            root,
            -i16::try_from(w)?,
            -i16::try_from(h)?,
            w,
            h,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &CreateWindowAux::new()
                .background_pixel(0)
                .border_pixel(0)
                .override_redirect(1)
                .colormap(colormap),
        )?;
        // No input region: the pointer and the target lookup go through the picture.
        conn.shape_rectangles(
            shape::SO::SET,
            shape::SK::INPUT,
            x11rb::protocol::xproto::ClipOrdering::UNSORTED,
            window,
            0,
            0,
            &[],
        )?;
        // Mapped first: what is drawn on an unmapped window is lost.
        conn.map_window(window)?;
        conn.configure_window(
            window,
            &x11rb::protocol::xproto::ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        )?;
        let gc = conn.generate_id()?;
        conn.create_gc(gc, window, &CreateGCAux::new())?;
        // The request size is limited: send the rows in strips.
        let row = image.width as usize * 4;
        let max_rows =
            ((conn.maximum_request_bytes().saturating_sub(64)) / row).clamp(1, image.height as usize);
        for (n, strip) in image.bgra.chunks(row * max_rows).enumerate() {
            let rows = u16::try_from(strip.len() / row)?;
            conn.put_image(
                ImageFormat::Z_PIXMAP,
                window,
                gc,
                w,
                rows,
                0,
                i16::try_from(n * max_rows)?,
                0,
                32,
                strip,
            )?;
        }
        conn.free_gc(gc)?;
        conn.free_colormap(colormap)?;
        conn.flush()?;
        Ok(Self { window, hotspot: image.hotspot })
    }

    fn move_to(&self, conn: &RustConnection, x: i16, y: i16) -> Result<(), Box<dyn std::error::Error>> {
        let aux = x11rb::protocol::xproto::ConfigureWindowAux::new()
            .x(i32::from(x) - self.hotspot.0)
            .y(i32::from(y) - self.hotspot.1);
        conn.configure_window(self.window, &aux)?;
        Ok(())
    }

    fn destroy(self, conn: &RustConnection) {
        let _ = conn.destroy_window(self.window);
        let _ = conn.flush();
    }
}

fn run(own: u32, payload: &Payload, image: Option<Image>) -> Result<(), Box<dyn std::error::Error>> {
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
    let mut icon = image.and_then(|image| {
        Icon::show(&conn, root, &image)
            .inspect_err(|e| tracing::debug!("drag-out XDND: no drag image: {e}"))
            .ok()
    });
    tracing::debug!(has_image = icon.is_some(), "drag-out XDND: drag image");
    tracing::debug!(url = %payload.url, "tab drag out started (XDND)");

    let started = Instant::now();
    // The window the drag is over (with its XDND version), and whether it accepts a drop.
    let mut over: Option<(Window, u32)> = None;
    let mut accepted = false;
    let mut last_sent = (i16::MIN, i16::MIN);
    let mut dropped: Option<(Window, Instant)> = None;

    let result = loop {
        // Answers first: status of the target, conversion requests, finish.
        let mut finished = false;
        while let Some(event) = conn.poll_for_event()? {
            match event {
                Event::ClientMessage(m) if m.type_ == atoms.status => {
                    if over.is_some_and(|(w, _)| w == m.data.as_data32()[0]) {
                        accepted = m.data.as_data32()[1] & 1 != 0;
                        tracing::trace!(accepted, "drag-out XDND: status");
                    }
                }
                Event::ClientMessage(m) if m.type_ == atoms.finished => {
                    tracing::debug!("drag-out XDND: finished");
                    finished |= dropped.is_some();
                }
                Event::SelectionRequest(request) => {
                    tracing::debug!(target = request.target, "drag-out XDND: selection request");
                    answer(&conn, &atoms, payload, &request)?
                }
                _ => {}
            }
        }

        if finished {
            break Ok(());
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
        if released {
            if let Some(icon) = icon.take() {
                icon.destroy(&conn);
            }
        } else if let Some(icon) = &icon {
            let _ = icon.move_to(&conn, pointer.root_x, pointer.root_y);
            conn.flush()?;
        }
        let now = target_at(&conn, root, &atoms, own)?;

        if now.map(|n| n.0) != over.map(|o| o.0) {
            if let Some((old, _)) = over {
                send(&conn, old, source, atoms.leave, [source, 0, 0, 0, 0])?;
            }
            accepted = false;
            last_sent = (i16::MIN, i16::MIN);
            if let Some((new, version)) = now {
                tracing::debug!(window = new, version, "drag-out XDND: enter");
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
                    tracing::debug!(window = target, "drag-out XDND: drop");
                    send(&conn, target, source, atoms.drop, [source, 0, CURRENT_TIME, 0, 0])?;
                    dropped = Some((target, Instant::now()));
                    continue;
                }
                Some((target, _)) => {
                    tracing::debug!(
                        window = target,
                        "drag-out XDND: released over a target that did not accept"
                    );
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
    if let Some(icon) = icon.take() {
        icon.destroy(&conn);
    }
    conn.destroy_window(source)?;
    conn.flush()?;
    result
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
