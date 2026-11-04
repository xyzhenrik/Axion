//! Desktop notifications through the freedesktop `org.freedesktop.Notifications` D-Bus service.
//!
//! Used for "Axion X is available" when the window is not in front. Everything degrades to
//! nothing: without a session bus, a notification daemon or action support the in-app toast
//! (which always shows) is all there is. One thread per notification waits for its
//! `ActionInvoked` / `NotificationClosed` signals and reports them through a channel that the
//! browser drains every frame.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};

use axion_config::Updates;
use axion_update::{Clock, Release};
use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::Value;

const SERVICE: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
/// The action key a click on the notification's body invokes (freedesktop spec).
const DEFAULT_ACTION: &str = "default";
const UPDATE_ACTION: &str = "update";
const LATER_ACTION: &str = "later";
/// Normal urgency (low 0, normal 1, critical 2).
const URGENCY_NORMAL: u8 = 1;
/// The daemon's own default timeout.
const TIMEOUT_DEFAULT: i32 = -1;
/// The body is the release notes' first line, cut to this many characters.
const BODY_CHARS: usize = 140;

/// What the user did with a notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotifyEvent {
    /// "Update" (or a click on the notification): open Info & Updates and ask to install.
    Update,
    /// "Later": remind again after `updates.remind_later_hours`.
    Later,
}

/// Everything `Notify` is called with, apart from the replaced id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payload {
    pub app_name: String,
    pub icon: String,
    pub summary: String,
    pub body: String,
    /// Pairs of action key and label, flattened like the D-Bus call wants them.
    pub actions: Vec<String>,
    pub urgency: u8,
    pub timeout_ms: i32,
}

/// The first non-empty line of the release notes, cut to `max` characters.
pub fn first_line(notes: &str, max: usize) -> String {
    let line = notes.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if line.chars().count() <= max {
        return line.to_owned();
    }
    let cut: String = line.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// The notification for an available update. `summary` and the labels are already translated.
pub fn update_payload(summary: &str, notes: &str, update_label: &str, later_label: &str) -> Payload {
    Payload {
        app_name: "Axion".into(),
        // The icon theme name the desktop integration installs.
        icon: "axion".into(),
        summary: summary.to_owned(),
        body: first_line(notes, BODY_CHARS),
        actions: vec![
            DEFAULT_ACTION.into(),
            update_label.into(),
            UPDATE_ACTION.into(),
            update_label.into(),
            LATER_ACTION.into(),
            later_label.into(),
        ],
        urgency: URGENCY_NORMAL,
        timeout_ms: TIMEOUT_DEFAULT,
    }
}

/// Whether an automatically found `release` is announced on the desktop: the checks are on,
/// the user wants desktop notifications, it is not skipped or snoozed, it was not announced
/// before, and the window is not in front (unless the user asked for that too: then the toast
/// would be a second, redundant notice).
pub fn should_notify(updates: &Updates, release: &Release, window_focused: bool, clock: &dyn Clock) -> bool {
    updates.enabled
        && updates.desktop_notification
        && (!window_focused || updates.desktop_notification_when_focused)
        && updates.notified_version != release.version.to_string()
        && axion_update::may_notify(updates, release, clock)
}

/// Sends notifications and collects what the user did with them.
pub struct Notifier {
    events: Receiver<NotifyEvent>,
    sender: Sender<NotifyEvent>,
}

impl Default for Notifier {
    fn default() -> Self {
        let (sender, events) = channel();
        Self { events, sender }
    }
}

impl Notifier {
    /// Shows `payload` on a background thread (the bus may be slow or missing).
    pub fn send(&self, payload: Payload) {
        let sender = self.sender.clone();
        let spawned = std::thread::Builder::new().name("notify".into()).spawn(move || {
            if let Err(e) = deliver(&payload, &sender) {
                tracing::debug!("desktop notification not shown: {e}");
            }
        });
        if let Err(e) = spawned {
            tracing::debug!("desktop notification thread not started: {e}");
        }
    }

    /// What the user did since the last call.
    pub fn events(&self) -> Vec<NotifyEvent> {
        self.events.try_iter().collect()
    }
}

/// The `Notify` call itself; returns the notification's id.
fn notify_call(connection: &Connection, payload: &Payload) -> zbus::Result<u32> {
    let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
    hints.insert("urgency", Value::U8(payload.urgency));
    hints.insert("desktop-entry", Value::from("axion"));
    let actions: Vec<&str> = payload.actions.iter().map(String::as_str).collect();
    let reply = connection.call_method(
        Some(SERVICE),
        PATH,
        Some(SERVICE),
        "Notify",
        &(
            payload.app_name.as_str(),
            0u32,
            payload.icon.as_str(),
            payload.summary.as_str(),
            payload.body.as_str(),
            actions,
            hints,
            payload.timeout_ms,
        ),
    )?;
    reply.body().deserialize()
}

/// Calls `Notify` and waits for the user's answer (until the notification closes).
fn deliver(payload: &Payload, events: &Sender<NotifyEvent>) -> zbus::Result<()> {
    let connection = Connection::session()?;
    // Listening starts before the call, so a quick click is not missed.
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(SERVICE)?
        .path(PATH)?
        .build();
    let signals = MessageIterator::for_match_rule(rule, &connection, Some(16))?;
    let id = notify_call(&connection, payload)?;
    for message in signals {
        let message = message?;
        let header = message.header();
        let Some(member) = header.member() else { continue };
        match member.as_str() {
            "ActionInvoked" => {
                let (notification, key): (u32, String) = message.body().deserialize()?;
                if notification != id {
                    continue;
                }
                let event = if key == LATER_ACTION { NotifyEvent::Later } else { NotifyEvent::Update };
                // Fails only once the browser is gone.
                let _ = events.send(event);
            }
            "NotificationClosed" => {
                let (notification, _reason): (u32, u32) = message.body().deserialize()?;
                if notification == id {
                    break;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axion_update::{Release, Version};

    struct Fixed(u64);
    impl Clock for Fixed {
        fn now(&self) -> u64 {
            self.0
        }
    }

    fn release(version: &str) -> Release {
        Release {
            version: Version::parse(version).expect("version"),
            tag: format!("v{version}"),
            name: String::new(),
            notes: String::new(),
            page_url: String::new(),
            prerelease: false,
            assets: Vec::new(),
        }
    }

    #[test]
    fn the_body_is_the_first_line_cut_to_length() {
        assert_eq!(first_line("\n\n  What's new in 0.4  \nmore", 140), "What's new in 0.4");
        assert_eq!(first_line("", 140), "");
        let long = "word ".repeat(60);
        let cut = first_line(&long, 20);
        assert!(cut.chars().count() <= 20 && cut.ends_with('…'), "{cut}");
        // Multi-byte text is cut on characters.
        assert_eq!(first_line("ääääää", 4).chars().count(), 4);
    }

    #[test]
    fn the_payload_has_the_app_icon_actions_and_normal_urgency() {
        let p = update_payload("Axion 0.4.0 is available", "## Faster tabs\nmore", "Update", "Later");
        assert_eq!(p.icon, "axion");
        assert_eq!(p.app_name, "Axion");
        assert_eq!(p.summary, "Axion 0.4.0 is available");
        assert_eq!(p.body, "## Faster tabs");
        assert_eq!(p.urgency, 1);
        assert_eq!(p.timeout_ms, -1);
        assert_eq!(p.actions, ["default", "Update", "update", "Update", "later", "Later"]);
    }

    #[test]
    fn the_decision_follows_the_settings() {
        let clock = Fixed(1000);
        let r = release("0.4.0");
        let on = Updates::default();
        assert!(should_notify(&on, &r, false, &clock), "in the background it is announced");
        assert!(!should_notify(&on, &r, true, &clock), "in front only the toast shows");

        let when_focused = Updates { desktop_notification_when_focused: true, ..Updates::default() };
        assert!(should_notify(&when_focused, &r, true, &clock));

        let off = Updates { desktop_notification: false, ..Updates::default() };
        assert!(!should_notify(&off, &r, false, &clock));
        let disabled = Updates { enabled: false, ..Updates::default() };
        assert!(!should_notify(&disabled, &r, false, &clock));

        // Once per version.
        let told = Updates { notified_version: "0.4.0".into(), ..Updates::default() };
        assert!(!should_notify(&told, &r, false, &clock));
        assert!(should_notify(&told, &release("0.5.0"), false, &clock));

        // Skipped versions and "remind me later" stay silent.
        let skipped = Updates { skipped_version: "0.4.0".into(), ..Updates::default() };
        assert!(!should_notify(&skipped, &r, false, &clock));
        let snoozed = Updates { remind_until: 5000, ..Updates::default() };
        assert!(!should_notify(&snoozed, &r, false, &clock));
        assert!(should_notify(&snoozed, &r, false, &Fixed(6000)));
    }

    /// Sends one real notification (manual test: `cargo test -p axion-app real_notification --
    /// --ignored`), then closes it again. Needs a session bus with a notification daemon.
    #[test]
    #[ignore = "shows a real desktop notification"]
    fn real_notification() {
        let payload = update_payload(
            "Axion test notification",
            "Sent by Axion's test suite; ignore it.",
            "Update",
            "Later",
        );
        let connection = Connection::session().expect("session bus");
        let id = notify_call(&connection, &payload).expect("Notify");
        assert!(id > 0);
        std::thread::sleep(std::time::Duration::from_secs(4));
        connection
            .call_method(Some(SERVICE), PATH, Some(SERVICE), "CloseNotification", &(id,))
            .expect("CloseNotification");
    }

    #[test]
    fn without_a_bus_nothing_happens() {
        // Delivery on a missing bus is an error the caller swallows; the events stay empty.
        let notifier = Notifier::default();
        assert!(notifier.events().is_empty());
    }
}
