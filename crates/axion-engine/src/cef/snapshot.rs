//! PDF printing and screenshots (DevTools `Page.captureScreenshot`). Everything runs on the
//! main thread; results come back as events.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use base64::Engine as _;
use cef::*;

use super::client::ViewContext;
use crate::{EngineEvent, PdfOptions, ShotArea, ShotRequest};

const HEADER: &str = "<div style=\"font-size:8px;width:100%;padding:0 24px;display:flex;\
justify-content:space-between;color:#555\"><span class=\"title\"></span><span class=\"date\"></span></div>";
const FOOTER: &str = "<div style=\"font-size:8px;width:100%;padding:0 24px;display:flex;\
justify-content:space-between;color:#555\"><span class=\"url\"></span>\
<span><span class=\"pageNumber\"></span> / <span class=\"totalPages\"></span></span></div>";

pub(super) fn pdf_settings(options: &PdfOptions) -> PdfPrintSettings {
    use cef::sys::cef_pdf_print_margin_type_t;
    PdfPrintSettings {
        landscape: options.landscape as i32,
        print_background: options.print_background as i32,
        scale: options.scale,
        paper_width: options.paper_width,
        paper_height: options.paper_height,
        margin_type: cef_pdf_print_margin_type_t::PDF_PRINT_MARGIN_CUSTOM.into(),
        margin_top: options.margin_top,
        margin_right: options.margin_right,
        margin_bottom: options.margin_bottom,
        margin_left: options.margin_left,
        page_ranges: options.page_ranges.as_str().into(),
        display_header_footer: options.header_footer as i32,
        header_template: HEADER.into(),
        footer_template: FOOTER.into(),
        ..PdfPrintSettings::default()
    }
}

#[derive(Clone)]
pub(super) struct PdfJob {
    pub ctx: ViewContext,
    pub browser_id: i32,
}

wrap_pdf_print_callback! {
    pub(super) struct PdfCallbackBuilder {
        job: PdfJob,
    }

    impl PdfPrintCallback {
        fn on_pdf_print_finished(&self, path: Option<&CefString>, ok: ::std::os::raw::c_int) {
            let path = PathBuf::from(path.map(|p| p.to_string()).unwrap_or_default());
            let event = EngineEvent::PdfSaved { view: self.job.ctx.view, path, ok: ok != 0 };
            self.job.ctx.shared.events.borrow_mut().push((self.job.browser_id, event));
        }
    }
}

/// One screenshot in progress.
#[derive(Clone)]
pub(super) struct ShotJob {
    pub ctx: ViewContext,
    pub browser_id: i32,
    pub id: u32,
    pub request: ShotRequest,
    /// The DevTools message whose answer is awaited.
    pub message: Rc<Cell<i32>>,
    /// Keeps the observer registered until the screenshot is done.
    pub registration: Rc<RefCell<Option<Registration>>>,
    pub host: BrowserHost,
}

impl ShotJob {
    fn report(&self, event: EngineEvent) {
        self.ctx.shared.events.borrow_mut().push((self.browser_id, event));
        self.registration.borrow_mut().take();
    }

    fn fail(&self) {
        self.report(EngineEvent::ScreenshotFailed { view: self.ctx.view, id: self.id });
    }

    fn call(&self, method: &str, params: &serde_json::Value) {
        let Some(mut dict) = dictionary_value_create() else { return self.fail() };
        fill(&mut dict, params);
        let id = self.host.execute_dev_tools_method(0, Some(&method.into()), Some(&mut dict));
        if id == 0 {
            return self.fail();
        }
        self.message.set(id);
    }

    fn capture(&self, clip: Option<(f64, f64, f64, f64)>, beyond: bool) {
        let mut params = serde_json::json!({ "format": "png", "captureBeyondViewport": beyond });
        if let Some((x, y, width, height)) = clip {
            params["clip"] = serde_json::json!({
                "x": x, "y": y, "width": width, "height": height, "scale": self.request.scale
            });
        }
        self.call("Page.captureScreenshot", &params);
    }

    fn start(&self) {
        match self.request.area {
            ShotArea::Visible => self.capture(None, false),
            ShotArea::Rect { x, y, width, height } => self.capture(Some((x, y, width, height)), false),
            ShotArea::FullPage { .. } => self.call("Page.getLayoutMetrics", &serde_json::json!({})),
        }
    }

    fn result(&self, success: bool, result: &[u8]) {
        let value: Option<serde_json::Value> = serde_json::from_slice(result).ok();
        let Some(value) = value.filter(|_| success) else { return self.fail() };
        if let Some(data) = value.get("data").and_then(|d| d.as_str()) {
            return match base64::engine::general_purpose::STANDARD.decode(data) {
                Ok(png) => self.report(EngineEvent::ScreenshotReady { view: self.ctx.view, id: self.id, png }),
                Err(_) => self.fail(),
            };
        }
        // The layout metrics: now the whole document.
        let ShotArea::FullPage { max_height } = self.request.area else { return self.fail() };
        let size = value.get("cssContentSize").or_else(|| value.get("contentSize"));
        let number = |key: &str| size.and_then(|s| s.get(key)).and_then(|n| n.as_f64()).unwrap_or(0.0);
        let (width, height) = (number("width").ceil(), number("height").ceil().min(f64::from(max_height)));
        if width < 1.0 || height < 1.0 {
            return self.fail();
        }
        self.capture(Some((0.0, 0.0, width, height)), true);
    }
}

/// Copies a JSON object into a CEF dictionary (strings, numbers, booleans, nested objects).
fn fill(dict: &mut DictionaryValue, value: &serde_json::Value) {
    let Some(object) = value.as_object() else { return };
    for (key, value) in object {
        let key = CefString::from(key.as_str());
        match value {
            serde_json::Value::Bool(b) => {
                dict.set_bool(Some(&key), *b as i32);
            }
            serde_json::Value::Number(n) => {
                dict.set_double(Some(&key), n.as_f64().unwrap_or(0.0));
            }
            serde_json::Value::String(s) => {
                dict.set_string(Some(&key), Some(&s.as_str().into()));
            }
            serde_json::Value::Object(_) => {
                if let Some(mut child) = dictionary_value_create() {
                    fill(&mut child, value);
                    dict.set_dictionary(Some(&key), Some(&mut child));
                }
            }
            _ => {}
        }
    }
}

wrap_dev_tools_message_observer! {
    struct ShotObserverBuilder {
        job: ShotJob,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self,
            _browser: Option<&mut Browser>,
            message_id: ::std::os::raw::c_int,
            success: ::std::os::raw::c_int,
            result: Option<&[u8]>,
        ) {
            if message_id == self.job.message.get() {
                self.job.result(success != 0, result.unwrap_or_default());
            }
        }
    }
}

/// Starts a screenshot of `host`'s page.
pub(super) fn capture(host: BrowserHost, ctx: ViewContext, browser_id: i32, id: u32, request: ShotRequest) {
    let job = ShotJob {
        ctx,
        browser_id,
        id,
        request,
        message: Rc::new(Cell::new(-1)),
        registration: Rc::default(),
        host: host.clone(),
    };
    let mut observer = ShotObserverBuilder::new(job.clone());
    let Some(registration) = host.add_dev_tools_message_observer(Some(&mut observer)) else {
        return job.fail();
    };
    *job.registration.borrow_mut() = Some(registration);
    job.start();
}
