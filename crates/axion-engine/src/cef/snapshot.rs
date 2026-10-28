//! PDF printing and screenshots (DevTools `Page.captureScreenshot`). Everything runs on the
//! main thread; results come back as events.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use base64::Engine as _;
use cef::*;

use super::client::ViewContext;
use crate::{EngineEvent, PdfOptions};

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

    fn start(&self) {
        let Some(mut dict) = dictionary_value_create() else { return self.fail() };
        dict.set_string(Some(&"format".into()), Some(&"png".into()));
        let id =
            self.host.execute_dev_tools_method(0, Some(&"Page.captureScreenshot".into()), Some(&mut dict));
        if id == 0 {
            return self.fail();
        }
        self.message.set(id);
    }

    fn result(&self, success: bool, result: &[u8]) {
        let value: Option<serde_json::Value> = serde_json::from_slice(result).ok();
        let data = value.filter(|_| success).and_then(|v| v.get("data")?.as_str().map(str::to_owned));
        let png = data.and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok());
        match png {
            Some(png) => self.report(EngineEvent::ScreenshotReady { view: self.ctx.view, id: self.id, png }),
            None => self.fail(),
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
pub(super) fn capture(host: BrowserHost, ctx: ViewContext, browser_id: i32, id: u32) {
    let job = ShotJob {
        ctx,
        browser_id,
        id,
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
