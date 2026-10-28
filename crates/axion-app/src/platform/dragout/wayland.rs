//! A drag out of the window on Wayland.
//!
//! winit owns the connection and has no drag source, but a drag only needs three things: a
//! `wl_data_device` of the seat, the `wl_surface` the drag starts from and the serial of the
//! button press. So this joins winit's connection (`from_foreign_display`) with an event queue
//! of its own, binds the data device manager and a second `wl_pointer` (the compositor sends
//! every pointer of the client the same button events, so its last press serial is winit's),
//! and starts the drag from winit's surface. winit's loop reads the socket for both queues; a
//! short-lived thread serves the data requests while a drag is on.

use std::ffi::c_void;
use std::io::Write;
use std::os::fd::OwnedFd;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::WlDataOffer;
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_pointer::{self, ButtonState, WlPointer};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::{self, Capability, WlSeat};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, event_created_child};

use super::{MIME_PLAIN, MIME_PLAIN_UTF8, MIME_URI_LIST, Payload, data_for};

/// How long the serving thread waits for a drag to end (a drop that was never finished).
const DRAG_TIMEOUT: Duration = Duration::from_secs(120);
/// How often the serving thread looks for requests while a drag is on.
const SERVE_INTERVAL: Duration = Duration::from_millis(4);

#[derive(Default)]
struct State {
    pointer: Option<WlPointer>,
    /// The serial of the latest button press.
    press_serial: Option<u32>,
    /// The drag that is on: its source and what it carries.
    drag: Option<(WlDataSource, Payload)>,
}

struct Inner {
    queue: EventQueue<State>,
    state: State,
}

pub struct Source {
    conn: Connection,
    qh: QueueHandle<State>,
    inner: Arc<Mutex<Inner>>,
    manager: WlDataDeviceManager,
    device: WlDataDevice,
    surface: WlSurface,
}

fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    // A poisoned lock only means the serving thread panicked; its state is still usable.
    inner.lock().unwrap_or_else(|e| e.into_inner())
}

impl Source {
    /// Joins winit's connection; `None` without a seat, pointer or data device manager.
    pub fn connect(display: NonNull<c_void>, surface: NonNull<c_void>) -> Option<Self> {
        // SAFETY: `display` is the `wl_display` of the window's connection (from its raw display
        // handle). winit keeps it open for as long as the window exists, and this `Source` is
        // owned by the browser that owns the window.
        let backend = unsafe { Backend::from_foreign_display(display.as_ptr().cast()) };
        let conn = Connection::from_backend(backend);
        let (globals, mut queue) = registry_queue_init::<State>(&conn)
            .inspect_err(|e| tracing::debug!("no Wayland registry for tab drags: {e}"))
            .ok()?;
        let qh = queue.handle();
        let manager: WlDataDeviceManager = globals.bind(&qh, 1..=3, ()).ok()?;
        let seat: WlSeat = globals.bind(&qh, 1..=5, ()).ok()?;
        let device = manager.get_data_device(&seat, &qh, ());
        let mut state = State::default();
        // The seat tells its capabilities, which makes it hand out the pointer.
        queue.roundtrip(&mut state).ok()?;
        state.pointer.as_ref()?;
        // SAFETY: `surface` is the window's `wl_surface` (raw window handle), alive as long as
        // the window; `from_ptr` checks the object's interface.
        let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.as_ptr().cast()) }.ok()?;
        let surface = WlSurface::from_id(&conn, id).ok()?;
        Some(Self { conn, qh, inner: Arc::new(Mutex::new(Inner { queue, state })), manager, device, surface })
    }

    /// Starts the drag from the press that is held now.
    pub fn start(&mut self, payload: Payload) -> bool {
        let mut guard = lock(&self.inner);
        let Inner { queue, state } = &mut *guard;
        // The press arrived with winit's read of the socket: take it from the queue.
        if let Err(e) = queue.dispatch_pending(state) {
            tracing::warn!("Wayland events for the tab drag failed: {e}");
            return false;
        }
        if state.drag.is_some() {
            return false;
        }
        let Some(serial) = state.press_serial else { return false };
        let source = self.manager.create_data_source(&self.qh, ());
        for mime in [MIME_URI_LIST, MIME_PLAIN_UTF8, MIME_PLAIN] {
            source.offer(mime.to_owned());
        }
        if self.manager.version() >= 3 {
            source.set_actions(DndAction::Copy);
        }
        self.device.start_drag(Some(&source), &self.surface, None, serial);
        if let Err(e) = self.conn.flush() {
            tracing::warn!("starting the tab drag failed: {e}");
            source.destroy();
            return false;
        }
        tracing::debug!(url = %payload.url, serial, "tab drag out started (Wayland)");
        state.drag = Some((source, payload));
        drop(guard);
        let (inner, conn) = (Arc::clone(&self.inner), self.conn.clone());
        let spawned =
            std::thread::Builder::new().name("axion-dragout".into()).spawn(move || serve(&inner, &conn));
        if let Err(e) = spawned {
            tracing::warn!("serving the tab drag failed: {e}");
        }
        true
    }
}

/// Answers the data requests of the drag until it ends.
fn serve(inner: &Mutex<Inner>, conn: &Connection) {
    let deadline = Instant::now() + DRAG_TIMEOUT;
    loop {
        std::thread::sleep(SERVE_INTERVAL);
        let mut guard = lock(inner);
        let Inner { queue, state } = &mut *guard;
        if queue.dispatch_pending(state).is_err() {
            state.drag = None;
        }
        let _ = conn.flush();
        if state.drag.is_none() {
            return;
        }
        if Instant::now() > deadline {
            if let Some((source, _)) = state.drag.take() {
                source.destroy();
            }
            let _ = conn.flush();
            return;
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = event
            && caps.contains(Capability::Pointer)
            && state.pointer.is_none()
        {
            state.pointer = Some(seat.get_pointer(qh, ()));
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_pointer::Event::Button { serial, state: WEnum::Value(ButtonState::Pressed), .. } = event {
            state.press_serial = Some(serial);
        }
    }
}

impl Dispatch<WlDataDeviceManager, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlDataDeviceManager,
        _: <WlDataDeviceManager as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlDataDevice, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlDataDevice,
        _: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }

    event_created_child!(State, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
    ]);
}

impl Dispatch<WlDataOffer, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlDataOffer,
        _: <WlDataOffer as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlDataSource, ()> for State {
    fn event(
        state: &mut Self,
        source: &WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                if let Some((_, payload)) = &state.drag
                    && let Some(bytes) = data_for(payload, &mime_type)
                {
                    write_all(fd, &bytes);
                }
            }
            // Dropped on nothing, cancelled, or taken and finished: the drag is over.
            wl_data_source::Event::Cancelled | wl_data_source::Event::DndFinished => {
                state.drag = None;
                source.destroy();
            }
            _ => {}
        }
    }
}

fn write_all(fd: OwnedFd, bytes: &[u8]) {
    let mut file = std::fs::File::from(fd);
    if let Err(e) = file.write_all(bytes) {
        tracing::debug!("writing the dragged link failed: {e}");
    }
}
