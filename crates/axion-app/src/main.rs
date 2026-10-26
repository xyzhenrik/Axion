//! Axion — a Liquid Glass web browser.
//!
//! ```bash
//! axion --check-update   # asks the configured update source once, prints the answer and exits
//! axion [url…] [--account=ID] [--capture=out.png] [--size=1440x900] [--position=X,Y] [--maximized] [--overview] [--address=TEXT] [--open=menu|history|bookmarks|settings|private|private-locked|account-menu|…] [--lead=MS] [--onboarding]
//! ```
//!
//! Environment (they win over Settings › Hardware): `AXION_GPU=low|high|<name>` picks the
//! graphics card, `AXION_OSR=software|gpu` sets GPU frame sharing, `AXION_NO_SANDBOX=1` disables Chromium's sandbox, `AXION_CONFIG_DIR` and
//! `AXION_PROFILE_DIR` move the bases of settings and browsing data (each account has its own
//! directory in them, see `axion-accounts`), `AXION_DEVICE_KEY_DIR` moves the password vault's
//! device key (default `~/.local/share/axion`), `AXION_MONITOR=HDMI-A-1` opens the window on
//! that monitor (X11/XWayland only).

mod accounts;
mod app_icon;
mod avatars;
mod browser;
mod chrome;
mod context_menu;
mod desktop;
mod downloads;
mod downloads_popover;
mod drip;
mod engine_icons;
mod eye;
mod favicons;
mod filedrop;
mod find;
mod gpu;
mod hardware;
mod history;
mod home;
mod import;
mod input;
mod lifecycle;
mod locale;
mod lockscreen;
mod media_view;
mod menu;
mod morph;
mod mpris;
mod now_playing;
mod onboarding;
mod overview;
mod page_color;
mod page_menu;
mod passwords;
mod pip;
mod platform;
mod player;
mod playlist;
mod playlist_page;
mod playlist_popover;
mod popover;
mod private;
mod resize_bench;
mod settings;
mod shields;
mod shields_popover;
mod suggestions;
mod tabdrag;
mod tabs;
mod toast;
mod tooltip;
mod typing_probe;
mod update;
mod update_alert;
mod url;
mod window_resize;

use std::time::Duration;

use anyhow::Result;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

use browser::{Browser, CaptureDone, Options};

/// How long `--capture` waits for the page to load before saving.
const CAPTURE_AFTER: Duration = Duration::from_secs(8);

fn main() -> Result<()> {
    // Chromium re-runs this binary for its helper processes; they must exit right here.
    if let Some(code) = axion_engine::cef::run_subprocess_if_needed() {
        std::process::exit(code);
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu_core=warn,wgpu_hal=warn".into()),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--check-update") {
        std::process::exit(check_update_cli(&args));
    }
    let mut options = Options {
        urls: args.iter().filter(|a| !a.starts_with("--")).cloned().collect(),
        overview: args.iter().any(|a| a == "--overview"),
        address: args.iter().find_map(|a| a.strip_prefix("--address=")).map(str::to_owned),
        open: args.iter().find_map(|a| a.strip_prefix("--open=")).map(str::to_owned),
        lead: args.iter().find_map(|a| a.strip_prefix("--lead=")?.parse().ok()).map(Duration::from_millis),
        capture: args.iter().find_map(|a| a.strip_prefix("--capture=")).map(Into::into),
        capture_after: args
            .iter()
            .find_map(|a| a.strip_prefix("--after=")?.parse().ok())
            .map_or(CAPTURE_AFTER, Duration::from_secs_f32),
        size: args.iter().find_map(|a| a.strip_prefix("--size=")).and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        }),
        position: args.iter().find_map(|a| a.strip_prefix("--position=")).and_then(|s| {
            let (x, y) = s.split_once(',')?;
            Some((x.parse().ok()?, y.parse().ok()?))
        }),
        maximized: args.iter().any(|a| a == "--maximized"),
        account: args.iter().find_map(|a| a.strip_prefix("--account=")).map(str::to_owned),
        onboarding: args.iter().any(|a| a == "--onboarding"),
        bench_resize: args.iter().any(|a| a == "--bench-resize"),
        waker: None,
    };

    let mut builder = EventLoop::builder();
    // Test aid: `AXION_WINDOW_X11=1` puts the window on X11 (XWayland, where it can be placed
    // and resized by `--bench-resize`) while Chromium stays on Wayland, so pages keep sharing
    // GPU frames as in a normal Wayland session.
    #[cfg(target_os = "linux")]
    if std::env::var_os("AXION_WINDOW_X11").is_some() {
        use winit::platform::x11::EventLoopBuilderExtX11 as _;
        builder.with_x11();
    }
    let event_loop = builder.build()?;
    // Chromium's message loop is pumped from ours: the engine wakes it when it has work, and
    // says when it wants to run next (see `App::about_to_wait`).
    let proxy = event_loop.create_proxy();
    options.waker = Some(axion_engine::cef::Waker::new(move || {
        // Fails only once the event loop is gone.
        let _ = proxy.send_event(());
    }));
    let mut app = App { options: Some(options), browser: None, error: None };
    event_loop.run_app(&mut app)?;
    let relaunch = app.browser.as_mut().and_then(Browser::take_relaunch);
    // Dropping the browser saves the session and shuts Chromium down cleanly.
    drop(app.browser.take());
    // Switching accounts: Axion starts again in the other one.
    if let Some(relaunch) = relaunch {
        relaunch.spawn()?;
    }
    match app.error {
        Some(e) if e.downcast_ref::<CaptureDone>().is_none() => Err(e),
        _ => Ok(()),
    }
}

struct App {
    options: Option<Options>,
    browser: Option<Browser>,
    error: Option<anyhow::Error>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let Some(options) = self.options.take() else { return };
        match Browser::new(event_loop, options) {
            Ok(browser) => self.browser = Some(browser),
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(browser) = &mut self.browser else { return };
        if browser.is_pip_window(id) {
            browser.pip_window_event(event);
            return;
        }
        // Late events of a closed PiP window: never the browser window's (its CloseRequested
        // would quit).
        if !browser.is_main_window(id) {
            return;
        }
        match browser.window_event(event) {
            Ok(true) => {}
            Ok(false) => event_loop.exit(),
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(browser) = &mut self.browser {
            if browser.wants_exit() {
                event_loop.exit();
                return;
            }
            browser.tick_probe();
            browser.sync_windows(event_loop);
            browser.pump();
            // Sleep until the next frame (requested below; the compositor's frame callback
            // wakes us), input, or Chromium's next work, whichever comes first. After a frame
            // that was skipped (nothing changed, nothing presented) the next one is due a
            // display refresh later instead of at once.
            let wakeup = browser.next_wakeup();
            match browser.redraw_at().filter(|at| *at > std::time::Instant::now()) {
                Some(at) => {
                    event_loop.set_control_flow(ControlFlow::WaitUntil(wakeup.map_or(at, |w| w.min(at))));
                }
                None => {
                    event_loop.set_control_flow(match wakeup {
                        Some(at) => ControlFlow::WaitUntil(at),
                        None => ControlFlow::Poll,
                    });
                    browser.request_redraw();
                }
            }
        }
    }
}

/// `--check-update`: the account's `[updates]` settings, one check, the answer on stdout.
fn check_update_cli(args: &[String]) -> i32 {
    let load = || -> Result<axion_config::Config> {
        let mut accounts = axion_accounts::Accounts::load(
            axion_accounts::Bases::discover()?,
            i64::try_from(update::now()).unwrap_or(0),
        )?;
        let wanted = args.iter().find_map(|a| a.strip_prefix("--account=")).map(str::to_owned);
        let id = accounts.select(wanted.as_deref())?;
        let paths = axion_config::ConfigPaths::in_dir(accounts.paths(&id)?.config);
        Ok(axion_config::load_config(&paths)?)
    };
    match load() {
        Ok(config) => update::check_cli(&config),
        Err(e) => {
            tracing::error!("could not read the settings: {e:#}");
            1
        }
    }
}
