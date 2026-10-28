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
use std::os::fd::{AsFd, OwnedFd};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::WlDataOffer;
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_pointer::{self, ButtonState, WlPointer};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::{self, Capability, WlSeat};
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, event_created_child};

use super::{Image, MIME_MOZ_URL, MIME_PLAIN, MIME_PLAIN_UTF8, MIME_URI_LIST, Payload, data_for};

/// How long the serving thread waits for a drag to end (a drop that was never finished).
const DRAG_TIMEOUT: Duration = Duration::from_secs(120);
/// How often the serving thread looks for requests while a drag is on.
const SERVE_INTERVAL: Duration = Duration::from_millis(4);

#[derive(Default)]
struct State {
    pointer: Option<WlPointer>,
    /// The serial of the latest button press.
    press_serial: Option<u32>,
    /// The surface with pointer focus (our own pointer's view of it).
    focus: Option<ObjectId>,
    /// The drag that is on: its source and what it carries.
    drag: Option<(WlDataSource, Payload)>,
    /// The picture beside the pointer during that drag.
    icon: Option<DragIcon>,
}

impl State {
    /// The drag is over: its drag image goes. Returns the source, for the caller to destroy
    /// unless an event already hands it that duty.
    fn end_drag(&mut self) -> Option<WlDataSource> {
        if let Some(icon) = self.icon.take() {
            icon.destroy();
        }
        self.drag.take().map(|(source, _)| source)
    }
}

/// The drag icon: a `wl_surface` (the compositor gives it the drag-icon role in `start_drag`)
/// showing a shared-memory buffer.
struct DragIcon {
    surface: WlSurface,
    buffer: WlBuffer,
}

impl DragIcon {
    fn destroy(self) {
        self.surface.destroy();
        self.buffer.destroy();
    }
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
    /// For the drag image (`None` without them: the drag then has no picture).
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
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
        let manager: WlDataDeviceManager = globals
            .bind(&qh, 1..=3, ())
            .inspect_err(|e| tracing::debug!("tab drags: no wl_data_device_manager: {e}"))
            .ok()?;
        let seat: WlSeat = globals
            .bind(&qh, 1..=5, ())
            .inspect_err(|e| tracing::debug!("tab drags: no wl_seat: {e}"))
            .ok()?;
        let device = manager.get_data_device(&seat, &qh, ());
        let compositor: Option<WlCompositor> = globals.bind(&qh, 1..=6, ()).ok();
        let shm: Option<WlShm> = globals.bind(&qh, 1..=1, ()).ok();
        let mut state = State::default();
        // The seat tells its capabilities, which makes it hand out the pointer.
        queue
            .roundtrip(&mut state)
            .inspect_err(|e| tracing::debug!("tab drags: the seat roundtrip failed: {e}"))
            .ok()?;
        // A seat without a pointer yet (none plugged in) is fine: the capability can arrive
        // later and `pump` then creates it.
        tracing::debug!(has_pointer = state.pointer.is_some(), "tab drags: seat bound");
        // SAFETY: `surface` is the window's `wl_surface` (raw window handle), alive as long as
        // the window; `from_ptr` checks the object's interface.
        let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.as_ptr().cast()) }
            .inspect_err(|e| tracing::debug!("tab drags: the window surface is not usable: {e}"))
            .ok()?;
        let surface = WlSurface::from_id(&conn, id).ok()?;
        tracing::debug!(surface = ?surface.id(), "tab drags: connected to the Wayland seat");
        Some(Self {
            conn,
            qh,
            inner: Arc::new(Mutex::new(Inner { queue, state })),
            manager,
            device,
            surface,
            compositor,
            shm,
        })
    }

    /// Takes the events that winit's reads of the socket queued for this connection's own queue
    /// (pointer buttons with their serials, seat changes). Cheap; called from the window's
    /// event handlers so the queue neither grows without bound nor lags behind a press.
    pub fn pump(&mut self) {
        let Ok(mut guard) = self.inner.try_lock() else { return };
        let Inner { queue, state } = &mut *guard;
        if let Err(e) = queue.dispatch_pending(state) {
            tracing::warn!("Wayland events for the tab drag failed: {e}");
        }
    }

    /// Starts the drag from the press that is held now.
    pub fn start(&mut self, payload: Payload, image: Option<Image>) -> bool {
        let icon = image.and_then(|image| self.make_icon(&image));
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
        let Some(serial) = state.press_serial else {
            tracing::debug!("tab drag out (Wayland): no button press seen on the drag pointer");
            return false;
        };
        tracing::debug!(
            serial,
            focus = ?state.focus,
            surface = ?self.surface.id(),
            manager_version = self.manager.version(),
            "tab drag out (Wayland): starting wl_data_device.start_drag"
        );
        let source = self.manager.create_data_source(&self.qh, ());
        for mime in super::offered_mimes(&[MIME_PLAIN_UTF8, MIME_PLAIN, MIME_URI_LIST, MIME_MOZ_URL]) {
            tracing::debug!(%mime, "drag-out source: offer");
            source.offer(mime);
        }
        if self.manager.version() >= 3 {
            source.set_actions(DndAction::Copy);
        }
        self.device.start_drag(Some(&source), &self.surface, icon.as_ref().map(|i| &i.surface), serial);
        if let Some(icon) = &icon {
            // Now that the surface has its role, show what it holds.
            icon.surface.commit();
        }
        if let Err(e) = self.conn.flush() {
            tracing::warn!("starting the tab drag failed: {e}");
            source.destroy();
            if let Some(icon) = icon {
                icon.destroy();
            }
            return false;
        }
        tracing::debug!(has_image = icon.is_some(), "tab drag out (Wayland): drag image");
        state.icon = icon;
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

    /// A surface showing `image`, hot-spotted so the pointer sits where the image says. `None`
    /// if the compositor lacks `wl_compositor`/`wl_shm` or the buffer cannot be made.
    fn make_icon(&self, image: &Image) -> Option<DragIcon> {
        let (compositor, shm) = (self.compositor.as_ref()?, self.shm.as_ref()?);
        let (w, h) = (i32::try_from(image.width).ok()?, i32::try_from(image.height).ok()?);
        let size = image.bgra.len();
        if size == 0 || size != image.width as usize * image.height as usize * 4 {
            return None;
        }
        let fd = rustix::fs::memfd_create("axion-drag-image", rustix::fs::MemfdFlags::CLOEXEC)
            .inspect_err(|e| tracing::debug!("drag image: no memfd: {e}"))
            .ok()?;
        rustix::fs::ftruncate(&fd, size as u64).inspect_err(|e| tracing::debug!("drag image: {e}")).ok()?;
        let mut file = std::fs::File::from(fd);
        file.write_all(&image.bgra)
            .inspect_err(|e| tracing::debug!("drag image: writing it failed: {e}"))
            .ok()?;
        let pool = shm.create_pool(file.as_fd(), i32::try_from(size).ok()?, &self.qh, ());
        let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, &self.qh, ());
        // The buffer keeps the memory; the pool object is not needed any more.
        pool.destroy();
        let surface = compositor.create_surface(&self.qh, ());
        let scale = image.scale.max(1);
        if surface.version() >= 3 {
            surface.set_buffer_scale(scale);
        }
        // The icon's origin is the pointer position plus the surface offset: move it up and left
        // by the hotspot (in surface-local units, i.e. logical px).
        let (dx, dy) = (-image.hotspot.0 / scale, -image.hotspot.1 / scale);
        if surface.version() >= 5 {
            surface.attach(Some(&buffer), 0, 0);
            surface.offset(dx, dy);
        } else {
            surface.attach(Some(&buffer), dx, dy);
        }
        if surface.version() >= 4 {
            surface.damage_buffer(0, 0, w, h);
        } else {
            surface.damage(0, 0, w / scale, h / scale);
        }
        surface.commit();
        tracing::debug!(w, h, scale, dx, dy, "drag image buffer made");
        Some(DragIcon { surface, buffer })
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
            let _ = state.end_drag();
        }
        let _ = conn.flush();
        if state.drag.is_none() {
            return;
        }
        if Instant::now() > deadline {
            if let Some(source) = state.end_drag() {
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
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = event {
            tracing::debug!(?caps, "drag-out seat capabilities");
            if caps.contains(Capability::Pointer) {
                if state.pointer.is_none() {
                    state.pointer = Some(seat.get_pointer(qh, ()));
                }
            } else if let Some(pointer) = state.pointer.take() {
                // The last pointer went away (it may come back as a new object).
                if pointer.version() >= 3 {
                    pointer.release();
                }
                state.press_serial = None;
                state.focus = None;
            }
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
        match event {
            wl_pointer::Event::Enter { serial, surface, .. } => {
                tracing::trace!(serial, surface = ?surface.id(), "drag-out pointer: enter");
                state.focus = Some(surface.id());
            }
            wl_pointer::Event::Leave { serial, .. } => {
                tracing::trace!(serial, "drag-out pointer: leave");
                state.focus = None;
            }
            wl_pointer::Event::Button { serial, button, state: bstate, .. } => {
                tracing::trace!(serial, button, state = ?bstate, "drag-out pointer: button");
                if matches!(bstate, WEnum::Value(ButtonState::Pressed)) {
                    state.press_serial = Some(serial);
                }
            }
            _ => {}
        }
    }
}

/// Objects of the drag image that never need to be listened to.
macro_rules! ignore_events {
    ($($proxy:ty),+) => {$(
        impl Dispatch<$proxy, ()> for State {
            fn event(
                _: &mut Self,
                _: &$proxy,
                _: <$proxy as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    )+};
}

ignore_events!(WlCompositor, WlShm, WlShmPool, WlBuffer, WlSurface);

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
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        tracing::trace!(?event, "drag-out data device event");
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
                let bytes = state.drag.as_ref().and_then(|(_, payload)| data_for(payload, &mime_type));
                tracing::debug!(%mime_type, len = bytes.as_ref().map(Vec::len), "drag-out source: send");
                if let Some(bytes) = bytes {
                    write_all(fd, &bytes);
                }
                // else: `fd` drops here, which closes it (the target sees EOF).
            }
            // Dropped on nothing, or the compositor refused/ended it: the drag is over.
            wl_data_source::Event::Cancelled => {
                tracing::debug!("drag-out source: cancelled");
                let _ = state.end_drag();
                source.destroy();
            }
            // The target took the data and is done with it.
            wl_data_source::Event::DndFinished => {
                tracing::debug!("drag-out source: dnd_finished (the drop was received)");
                let _ = state.end_drag();
                source.destroy();
            }
            wl_data_source::Event::DndDropPerformed => {
                // The data may still be requested: the source stays until `dnd_finished`.
                tracing::debug!("drag-out source: dnd_drop_performed");
                // The drag is over for the eye: the picture goes now.
                if let Some(icon) = state.icon.take() {
                    icon.destroy();
                }
            }
            wl_data_source::Event::Target { mime_type } => {
                tracing::debug!(?mime_type, "drag-out source: target accepts");
            }
            wl_data_source::Event::Action { dnd_action } => {
                tracing::debug!(?dnd_action, "drag-out source: action");
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
