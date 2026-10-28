//! Extension popups (`action.default_popup`) as an overlay under the icon that was clicked.
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
use crate::extensions::ExtensionView;

/// The console message the page script reports with: `…size:W,H` or `…close`.
const MARKER: &str = "\u{1}axion:extpopup:";

/// Measures the page as Chrome's popup auto-resize does: the width it wants (its max-content
/// width) and the height that takes at that width. Styles it changes for the measurement are
/// put back before the page can paint. Reports only when the size changed (every message
/// wakes the browser). Also makes `window.close()` report instead of doing nothing.
const MEASURE: &str = r#"(function(){
var d=document.documentElement;if(!d||!document.body)return;
if(!window.__axionPopup){window.__axionPopup=1;window.close=function(){console.log("\u0001axion:extpopup:close")}}
var s=d.style,k=["width","height","min-width","min-height","max-width"],o={};
k.forEach(function(p){o[p]=[s.getPropertyValue(p),s.getPropertyPriority(p)]});
s.setProperty("width","max-content","important");s.setProperty("height","auto","important");
s.setProperty("min-width","0","important");s.setProperty("min-height","0","important");
s.setProperty("max-width","none","important");
var r=d.getBoundingClientRect(),w=Math.ceil(r.width),h=Math.ceil(r.height);
k.forEach(function(p){if(o[p][0])s.setProperty(p,o[p][0],o[p][1]);else s.removeProperty(p)});
var key=w+","+h;if(window.__axionPopupSize===key)return;window.__axionPopupSize=key;
console.log("\u0001axion:extpopup:size:"+key)})()"#;

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

    /// A click on extension `id`'s icon or row: opens its popup under `anchor` (a second click
    /// closes it). Without a popup page, or one that cannot run yet, it does what it can.
    pub(super) fn extension_clicked(&mut self, id: &str, anchor: Rect) {
        if self.ext_popup.as_ref().is_some_and(|p| p.id == id) {
            self.close_extension_popup();
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
        let config = &self.config.extensions;
        let min = Vec2::splat(config.popup_min_size.max(1) as f32);
        let remembered = self.ext_popup_sizes.get(id).copied().unwrap_or(min);
        let url = crate::extensions::page_url(&ext.id, &popup);
        let size = ViewSize { logical: remembered, scale: self.scale() };
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
        if popup.size.is_none() {
            return;
        }
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
        let scale = self.scale();
        let Some(popup) = self.ext_popup.as_mut() else { return };
        let config = &self.config.extensions;
        let room = ExtensionPopupCard::room(popup.anchor, window);
        let min = config.popup_min_size.max(1) as f32;
        let max = Vec2::new(config.popup_max_width as f32, config.popup_max_height as f32).min(room);
        let size = Vec2::new(w, h).max(Vec2::splat(min)).min(max.max(Vec2::splat(min))).round();
        if popup.size == Some(size) {
            return;
        }
        popup.size = Some(size);
        let (view, id, anchor) = (popup.view, popup.id.clone(), popup.anchor);
        self.ext_popup_sizes.insert(id, size);
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

    /// Whether `p` is on the popup's card or the icon it belongs to (a click there is not
    /// "outside").
    pub(super) fn ext_popup_owns(&self, p: Vec2) -> bool {
        self.ext_popup
            .as_ref()
            .is_some_and(|popup| popup.anchor.contains(p) || self.ext_popup_card.contains(&self.ui, p))
    }

    /// The extension icons of the address field, for the page that is shown: only when they
    /// changed, the chrome is told.
    pub(super) fn sync_extension_icons(&mut self) {
        let mode = self.config.extensions.show_in_address_bar;
        let url = self.tabs.active().map(|t| t.url.clone()).unwrap_or_default();
        let wanted: Vec<&ExtensionView> = if self.config.extensions.enabled {
            self.extensions.views().iter().filter(|e| e.shows_icon(mode, &url)).collect()
        } else {
            Vec::new()
        };
        let open = self.ext_popup.as_ref().map(|p| p.id.as_str());
        let icons: Vec<(String, String, Option<crate::home::Image>, bool)> = wanted
            .iter()
            .map(|e| {
                let path = e.icon.clone();
                let picture = self.avatars.get(&format!("extension:{}", e.id), || path);
                (e.id.clone(), e.name.clone(), picture, open == Some(e.id.as_str()))
            })
            .collect();
        let signature: Vec<(String, bool, bool)> =
            icons.iter().map(|(id, _, picture, open)| (id.clone(), picture.is_some(), *open)).collect();
        if signature == self.address_extension_signature {
            return;
        }
        self.address_extension_signature = signature;
        self.address_extensions = icons.iter().map(|(id, ..)| id.clone()).collect();
        let items: Vec<crate::chrome::ExtensionIcon> = icons
            .iter()
            .map(|(_, name, picture, open)| crate::chrome::ExtensionIcon {
                name,
                picture: picture.clone(),
                open: *open,
            })
            .collect();
        self.chrome.set_extension_icons(&mut self.ui, &items);
    }

    /// A click on the `i`th extension icon of the address field.
    pub(super) fn extension_icon_clicked(&mut self, i: usize) {
        let Some(id) = self.address_extensions.get(i).cloned() else { return };
        let Some(anchor) = self.chrome.extension_icon_frame(&self.ui, i) else { return };
        self.extension_clicked(&id, anchor);
    }

    /// Test aids for `--open=`: `extension-popup` clicks the first extension icon (else the
    /// puzzle piece), `ext-click:XxY` presses and releases the pointer at (X, Y) of the popup's
    /// page, `ext-type:TEXT` types into it, `ext-wait` only pauses between steps.
    pub(super) fn ext_popup_aid(&mut self, aid: &str) {
        tracing::info!(aid, open = self.ext_popup.is_some(), "extension popup test aid");
        if aid == "extension-popup" {
            self.sync_extension_icons();
            self.ui.layout();
            if self.address_extensions.is_empty() {
                let anchor = self.chrome.extensions_frame(&self.ui);
                if let Some(id) = self.extensions.views().iter().find(|e| e.has_popup()).map(|e| e.id.clone())
                {
                    self.extension_clicked(&id, anchor);
                }
            } else {
                self.extension_icon_clicked(0);
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
