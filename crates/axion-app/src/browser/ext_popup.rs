//! Extension popups (`action.default_popup`) as an overlay under the extensions button.
//!
//! Chromium cannot attach a toolbar popup to Axion's off-screen window (it wants a native one),
//! so the popup page is its own off-screen view (`chrome-extension://<id>/<popup>`, like any tab
//! but without one). Its frames are drawn into [`ExtensionPopupCard`]; pointer, wheel and
//! keyboard go to it while it is open. The card follows the page's size like Chrome's popup:
//! a script measures the page's natural size (`MEASURE`) and reports it as a console message,
//! the view is resized to it, within `extensions.popup_min_size` / `popup_max_*`.
//!
//! It closes on Escape, a click outside, a tab switch, `window.close()` and a link that opens
//! elsewhere (links with a target open as tabs in the window).

use super::*;
use crate::extension_popup::ExtensionPopupCard;

/// The console message the page script reports with: `…size:W,H` or `…close`.
const MARKER: &str = "\u{1}axion:extpopup:";

/// Measures the page like Chrome's popup auto-resize: the width it wants (its max-content
/// width) and the height that takes at that width, taking in content that sticks out of the
/// document (absolutely positioned, overflowing) unless a scrolling ancestor clips it. A
/// temporary style sheet takes the viewport out of the measurement (`min-height: 100vh`) and is
/// gone before the page can paint. Reports only when the size changed (every message wakes the
/// browser). Also makes `window.close()` report instead of doing nothing.
const MEASURE: &str = r#"(function(){
var d=document.documentElement,b=document.body;if(!d||!b)return;
if(!window.__axionPopup){window.__axionPopup=1;window.close=function(){console.log("\u0001axion:extpopup:close")}}
var st=document.createElement("style");
st.textContent="html{width:max-content!important;height:auto!important;min-width:0!important;min-height:0!important;max-width:none!important;max-height:none!important}body{min-height:0!important}";
d.appendChild(st);
var r=d.getBoundingClientRect(),w=r.width,h=r.height,sx=window.scrollX,sy=window.scrollY;
var all=b.getElementsByTagName("*"),n=Math.min(all.length,4000),clip=new Map();
function clipped(e){for(var a=e.parentElement;a&&a!==b&&a!==d;a=a.parentElement){
 var c=clip.get(a);if(c===undefined){var o=getComputedStyle(a).overflow;c=/hidden|auto|scroll|clip/.test(o);clip.set(a,c)}if(c)return true}return false}
for(var i=0;i<n;i++){var e=all[i],q=e.getBoundingClientRect();
 if(q.width===0&&q.height===0)continue;
 var right=q.right+sx,bottom=q.bottom+sy;
 if(right>w+.5||bottom>h+.5){var c=getComputedStyle(e);
  if(c.position==="fixed"||c.visibility==="hidden"||c.display==="none"||clipped(e))continue;
  if(right>w)w=right;if(bottom>h)h=bottom}}
st.remove();
w=Math.ceil(w);h=Math.ceil(h);
var key=w+","+h;if(window.__axionPopupSize===key)return;window.__axionPopupSize=key;
console.log("\u0001axion:extpopup:size:"+key)})()"#;

/// A classic scrollbar's width: a popup that scrolls gets this much wider, so the scrollbar does
/// not take the content's width.
const SCROLLBAR: f32 = 15.0;

/// An open extension popup.
pub(super) struct ExtPopup {
    pub view: ViewId,
    /// The extension.
    pub id: String,
    /// The popup page's address.
    url: String,
    /// The tab it was opened over: switching tabs closes it.
    tab: Option<ViewId>,
    /// The icon (or button) it flows out of.
    anchor: Rect,
    /// The page's size on screen (logical px) once measured; the card shows from then on.
    size: Option<Vec2>,
    started: Instant,
    loaded: bool,
    last_measure: Instant,
    /// The cursor the page asked for.
    cursor: CursorKind,
    /// The pointer is over the page (it was told so).
    hover: bool,
    /// A button went down on the page: its release goes there wherever the pointer is.
    pressed: bool,
    texture: Option<(wgpu::Texture, wgpu::TextureView)>,
}

impl Browser {
    pub(super) fn ext_popup_open(&self) -> bool {
        self.ext_popup.is_some()
    }

    /// The view that gets the keyboard: the popup while it is open.
    pub(super) fn key_view(&self) -> Option<ViewId> {
        self.ext_popup.as_ref().filter(|p| p.size.is_some()).map(|p| p.view).or_else(|| self.active_view())
    }

    /// A click on extension `id`'s row: opens its popup under `anchor` (the extensions button);
    /// another extension's popup goes first, so there is never more than one. Without a popup
    /// page, or one that cannot run yet, it does what it can.
    pub(super) fn extension_clicked(&mut self, id: &str, anchor: Rect) {
        if self.ext_popup.as_ref().is_some_and(|p| p.id == id) {
            return;
        }
        self.close_extension_popup();
        let Some(ext) = self.extensions.get(id).cloned() else { return };
        if ext.broken || !ext.enabled {
            return;
        }
        let Some(popup) = ext.popup.clone() else {
            // No popup: its options page, else its details.
            if let Some(page) = ext.options_page.as_deref() {
                let url = crate::extensions::page_url(&ext.id, page);
                if let Err(e) = self.open_tab(&url, true) {
                    tracing::warn!("opening the extension page failed: {e}");
                }
            } else {
                self.extensions_details = Some(ext.id.clone());
                self.toggle_extensions_popover();
            }
            return;
        };
        if !ext.running {
            self.show_toast(Icon::Extensions, tr("Relaunch Axion to use this extension."));
            return;
        }
        // The popups of other overlays give way.
        self.extensions_popover.close(&mut self.ui, &self.config);
        self.shields_popover.close(&mut self.ui, &self.config);
        self.downloads_popover.close(&mut self.ui, &self.config);
        self.menu.close(&mut self.ui, &self.config);
        // The page starts as large as a popup may get (so what depends on the window lays out
        // fully); it is measured and shrunk to its content from there.
        let config = &self.config.extensions;
        let max = Vec2::new(config.popup_max_width as f32, config.popup_max_height as f32);
        let room =
            ExtensionPopupCard::room(anchor, self.logical_size(), ExtensionPopupCard::gap(&self.config));
        let start = max.min(room).max(Vec2::splat(config.popup_min_size.max(1) as f32)).round();
        let url = crate::extensions::page_url(&ext.id, &popup);
        let size = ViewSize { logical: start, scale: self.scale() };
        match self.engine.create_view(&url, size, false) {
            Ok(view) => {
                // Keys and the pointer go to the popup from now on.
                if let Some(active) = self.active_view() {
                    self.engine.set_focus(active, false);
                }
                let _ = self.ui.blur();
                self.engine.set_focus(view, true);
                self.ext_popup = Some(ExtPopup {
                    view,
                    id: ext.id,
                    url: url.clone(),
                    tab: self.active_view(),
                    anchor,
                    size: None,
                    started: Instant::now(),
                    loaded: false,
                    last_measure: Instant::now(),
                    cursor: CursorKind::Default,
                    hover: false,
                    pressed: false,
                    texture: None,
                });
            }
            Err(e) => {
                tracing::warn!("opening the extension popup failed: {e}");
                // The page in a tab still works.
                if let Err(e) = self.open_tab(&url, true) {
                    tracing::warn!("opening the extension page failed: {e}");
                }
            }
        }
    }

    /// Closes the popup (its view goes, the card flows back into its icon).
    pub(super) fn close_extension_popup(&mut self) {
        let Some(popup) = self.ext_popup.take() else { return };
        self.engine.set_focus(popup.view, false);
        self.engine.close_view(popup.view);
        self.ext_popup_card.close(&mut self.ui, &self.config);
        if popup.hover {
            self.page_cursor = CursorKind::Default;
        }
        if let Some(view) = self.active_view()
            && self.ui.focused().is_none()
            && !self.is_locked()
        {
            self.engine.set_focus(view, true);
        }
    }

    /// Per frame: asks the page for its size, shows its latest frame in the card and moves the
    /// card along. Call before the layout.
    pub(super) fn ext_popup_tick(&mut self, dt: f32) {
        self.ext_popup_card.animate(&mut self.ui, dt);
        let window = self.logical_size();
        let gap = ExtensionPopupCard::gap(&self.config);
        let Some(popup) = self.ext_popup.as_mut() else { return };
        let config = &self.config.extensions;
        // A tab switch leaves the popup behind.
        let gone_tab = popup.tab != self.tabs.active().map(|t| t.view);
        let measure_every = Duration::from_millis(u64::from(config.popup_measure_ms.max(16)));
        if popup.loaded && popup.last_measure.elapsed() >= measure_every {
            popup.last_measure = Instant::now();
            self.engine.run_script(popup.view, MEASURE, false);
        }
        let timeout = Duration::from_millis(u64::from(config.popup_load_timeout_ms));
        let stuck = popup.size.is_none() && popup.started.elapsed() >= timeout;
        if gone_tab {
            self.close_extension_popup();
            return;
        }
        if stuck {
            // It never reported a size: show the page in a tab instead.
            let (id, url) = (popup.id.clone(), popup.url.clone());
            tracing::warn!(%id, "the extension popup did not start; opening it in a tab");
            self.close_extension_popup();
            if let Err(e) = self.open_tab(&url, true) {
                tracing::warn!("opening the extension page failed: {e}");
            }
            return;
        }
        let Some(size) = popup.size else { return };
        // Beside the extensions list when that is open, never under it.
        let natural = ExtensionPopupCard::natural(popup.anchor, window, size, gap);
        let list = self.extensions_popover.target_frame();
        let placed = crate::extension_popup::place_clear(natural, list, window);
        self.ext_popup_card.follow(&mut self.ui, placed);
        self.engine.request_frame(popup.view);
        if let Some(frame) = self.engine.frame(popup.view) {
            let view = match &popup.texture {
                Some((texture, view)) if *texture == frame.texture => view.clone(),
                _ => {
                    let view = frame.texture.create_view(&Default::default());
                    popup.texture = Some((frame.texture.clone(), view.clone()));
                    view
                }
            };
            let shown = popup.size.unwrap_or_default() * self.scale();
            let image = Some((view, (frame.texture.width(), frame.texture.height())));
            self.ext_popup_card.set_picture(&mut self.ui, image, shown);
        }
    }

    /// A message of the popup page: its size, or that it wants to close.
    fn ext_popup_message(&mut self, message: &str) {
        let Some(rest) = message.strip_prefix(MARKER) else { return };
        if rest == "close" {
            self.close_extension_popup();
            return;
        }
        let Some((w, h)) = rest.strip_prefix("size:").and_then(|s| s.split_once(',')) else { return };
        let (Ok(w), Ok(h)) = (w.parse::<f32>(), h.parse::<f32>()) else { return };
        let window = self.logical_size();
        let gap = ExtensionPopupCard::gap(&self.config);
        let scale = self.scale();
        let Some(popup) = self.ext_popup.as_mut() else { return };
        let config = &self.config.extensions;
        let room = ExtensionPopupCard::room(popup.anchor, window, gap);
        let min = config.popup_min_size.max(1) as f32;
        let max = Vec2::new(config.popup_max_width as f32, config.popup_max_height as f32).min(room);
        let max = max.max(Vec2::splat(min));
        // Taller than it may be: it scrolls, and the scrollbar needs room of its own.
        let w = if h > max.y { w + SCROLLBAR } else { w };
        let size = Vec2::new(w, h).max(Vec2::splat(min)).min(max).round();
        if popup.size == Some(size) {
            return;
        }
        popup.size = Some(size);
        // The page lays out again at its new size: look once more at once.
        popup.last_measure = Instant::now() - Duration::from_secs(1);
        let (view, anchor) = (popup.view, popup.anchor);
        self.engine.resize(view, ViewSize { logical: size, scale });
        self.ext_popup_card.show(&mut self.ui, &self.config, anchor, window, size);
    }

    /// Engine events of the popup's view. Returns whether the event was the popup's.
    pub(super) fn ext_popup_event(&mut self, event: &EngineEvent) -> bool {
        let Some(view) = self.ext_popup.as_ref().map(|p| p.view) else { return false };
        match event {
            EngineEvent::PageMessage { view: v, message } if *v == view => self.ext_popup_message(message),
            EngineEvent::LoadingStateChanged { view: v, loading, .. } if *v == view => {
                if !*loading && let Some(popup) = self.ext_popup.as_mut() {
                    popup.loaded = true;
                    // Measure at once.
                    popup.last_measure = Instant::now() - Duration::from_secs(1);
                }
            }
            EngineEvent::AddressChanged { view: v, url } if *v == view => {
                // Away from the extension's own page (a link without a target): that page
                // belongs in a tab.
                let own = url.starts_with("chrome-extension://")
                    || url.starts_with("about:")
                    || url.starts_with("chrome-error:")
                    || url.is_empty();
                if !own {
                    let url = url.clone();
                    self.close_extension_popup();
                    if !url.starts_with("axion:")
                        && let Err(e) = self.open_tab(&url, true)
                    {
                        tracing::warn!("opening {url} failed: {e:#}");
                    }
                }
            }
            EngineEvent::NewViewRequested { opener, url, .. } if *opener == view => {
                let url = url.clone();
                self.close_extension_popup();
                // Axion's own pages open only from the browser.
                if !url.starts_with("axion:")
                    && let Err(e) = self.open_tab(&url, true)
                {
                    tracing::warn!("opening {url} failed: {e:#}");
                }
            }
            EngineEvent::CursorChanged { view: v, cursor } if *v == view => {
                if let Some(popup) = self.ext_popup.as_mut() {
                    popup.cursor = *cursor;
                }
            }
            EngineEvent::PermissionRequest { view: v, id, .. } if *v == view => {
                self.engine.answer_permission(view, *id, axion_engine::PermissionAnswer::Deny);
            }
            EngineEvent::ScreenPickRequested { view: v, token, .. } if *v == view => {
                self.engine.answer_screen_pick(view, *token, None);
            }
            EngineEvent::RendererGone { view: v, .. } | EngineEvent::Closed { view: v } if *v == view => {
                // Closed by the page itself (`window.close()`), or gone.
                if let Some(popup) = self.ext_popup.take() {
                    self.engine.set_focus(popup.view, false);
                    self.ext_popup_card.close(&mut self.ui, &self.config);
                    if let Some(active) = self.active_view()
                        && self.ui.focused().is_none()
                    {
                        self.engine.set_focus(active, true);
                    }
                }
            }
            EngineEvent::ContextMenu { view: v, .. }
            | EngineEvent::FullscreenRequested { view: v, .. }
            | EngineEvent::TitleChanged { view: v, .. }
            | EngineEvent::LoadProgress { view: v, .. }
            | EngineEvent::FaviconUrlsChanged { view: v, .. }
            | EngineEvent::PageMetaChanged { view: v, .. }
            | EngineEvent::RequestBlocked { view: v, .. }
            | EngineEvent::NavigationBlocked { view: v, .. }
            | EngineEvent::CaptureChanged { view: v, .. }
            | EngineEvent::MediaSniffed { view: v, .. }
            | EngineEvent::FindResult { view: v, .. }
            | EngineEvent::FaviconReady { view: v, .. }
            | EngineEvent::FaviconFailed { view: v, .. }
            | EngineEvent::ImageReady { view: v, .. }
            | EngineEvent::ImageFailed { view: v, .. }
            | EngineEvent::PermissionCancelled { view: v, .. }
            | EngineEvent::InternalMessage { view: v, .. }
                if *v == view => {}
            _ => return false,
        }
        true
    }

    /// The page area of the card, while it shows one.
    fn ext_popup_page(&self) -> Option<Rect> {
        self.ext_popup.as_ref().filter(|p| p.size.is_some())?;
        Some(self.ext_popup_card.page_frame(&self.ui))
    }

    /// The pointer moved to `p`: the popup's page hears it when it is over it.
    pub(super) fn ext_popup_pointer(&mut self, p: Vec2) {
        let page = self.ext_popup_page();
        let modifiers = input::engine_modifiers(self.modifiers);
        let Some(popup) = self.ext_popup.as_mut() else { return };
        match page.filter(|f| f.contains(p) || popup.pressed) {
            Some(frame) => {
                popup.hover = true;
                self.engine
                    .send_input(popup.view, &InputEvent::MouseMove { pos: p - frame.origin, modifiers });
            }
            None if popup.hover => {
                popup.hover = false;
                self.engine.send_input(popup.view, &InputEvent::MouseLeave { modifiers });
            }
            None => {}
        }
    }

    /// A mouse button at `p`. Returns whether the popup took it: a press on its card (the page
    /// or its glass), and the release of such a press.
    pub(super) fn ext_popup_button(
        &mut self,
        p: Vec2,
        button: winit::event::MouseButton,
        pressed: bool,
    ) -> bool {
        let page_button = match button {
            winit::event::MouseButton::Left => MouseButton::Left,
            winit::event::MouseButton::Middle => MouseButton::Middle,
            winit::event::MouseButton::Right => MouseButton::Right,
            _ => return false,
        };
        let page = self.ext_popup_page();
        let on_card = self.ext_popup_card.contains(&self.ui, p);
        let modifiers = input::engine_modifiers(self.modifiers);
        let click_count = if pressed { self.clicks.press(button, p) } else { self.clicks.current() };
        let Some(popup) = self.ext_popup.as_mut() else { return false };
        let Some(frame) = page else { return false };
        if pressed && !on_card {
            return false;
        }
        if !pressed && !popup.pressed {
            return on_card;
        }
        if pressed {
            popup.pressed = true;
            self.engine.set_focus(popup.view, true);
        } else {
            popup.pressed = false;
        }
        self.engine.send_input(
            popup.view,
            &InputEvent::MouseButton {
                pos: p - frame.origin,
                button: page_button,
                pressed,
                click_count,
                modifiers,
            },
        );
        true
    }

    /// Wheel at `p`: the popup's page scrolls when the pointer is over it. Returns whether it did.
    pub(super) fn ext_popup_wheel(&mut self, p: Vec2, delta: Vec2) -> bool {
        let Some(frame) = self.ext_popup_page().filter(|f| f.contains(p)) else { return false };
        let modifiers = input::engine_modifiers(self.modifiers);
        if let Some(popup) = &self.ext_popup {
            self.engine
                .send_input(popup.view, &InputEvent::Wheel { pos: p - frame.origin, delta, modifiers });
        }
        true
    }

    /// The page's cursor while the pointer is over it.
    pub(super) fn ext_popup_cursor(&self) -> Option<CursorKind> {
        self.ext_popup.as_ref().filter(|p| p.hover).map(|p| p.cursor)
    }

    /// Whether `p` is on the popup's card, the extensions list beside it or the button they
    /// belong to (a click there is not "outside").
    pub(super) fn ext_popup_owns(&self, p: Vec2) -> bool {
        self.ext_popup.as_ref().is_some_and(|popup| {
            popup.anchor.contains(p)
                || self.ext_popup_card.contains(&self.ui, p)
                || self.extensions_popover.contains(&self.ui, p)
        })
    }

    /// Test aids for `--open=`: `extension-popup` opens the first extension's popup (as a click on
    /// its row does), `ext-click:XxY` presses and releases the pointer at (X, Y) of the popup's
    /// page, `ext-type:TEXT` types into it, `ext-wait` only pauses between steps.
    pub(super) fn ext_popup_aid(&mut self, aid: &str) {
        tracing::info!(aid, open = self.ext_popup.is_some(), "extension popup test aid");
        if aid == "extension-popup" {
            self.ui.layout();
            let anchor = self.chrome.extensions_frame(&self.ui);
            if let Some(id) = self.extensions.views().iter().find(|e| e.has_popup()).map(|e| e.id.clone()) {
                self.extension_clicked(&id, anchor);
            }
        } else if let Some(at) = aid.strip_prefix("ext-click:") {
            let Some((x, y)) = at.split_once('x').and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?)))
            else {
                return;
            };
            let Some(page) = self.ext_popup_page() else { return };
            let p = page.origin + Vec2::new(x, y);
            tracing::info!(?p, ?page, "test click into the extension popup");
            self.pointer_moved(p);
            self.mouse_button(ElementState::Pressed, winit::event::MouseButton::Left);
            self.mouse_button(ElementState::Released, winit::event::MouseButton::Left);
        } else if let Some(text) = aid.strip_prefix("ext-type:") {
            let modifiers = input::engine_modifiers(self.modifiers);
            if let Some(view) = self.key_view() {
                let text = text.to_owned();
                self.engine
                    .send_input(view, &InputEvent::Text { text, key_code: 0, native_code: 0, modifiers });
            }
        }
    }
}
