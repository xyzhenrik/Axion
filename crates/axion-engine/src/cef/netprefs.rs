//! Applies proxy settings and the leak-protection preferences to a request context.
//!
//! How routing works in CEF 156 (Chromium's network service, one per process): a request
//! context owns a profile, and the profile owns the `proxy` preference. So the proxy is
//! chosen **per context**: the account's tabs share the persistent global context, private
//! tabs the in-memory one. Per-site routing inside a context is done by a PAC script (see
//! `axion-net`'s `pac` module), which Chromium evaluates for every request. A per-tab
//! override would need a third context per route (and with it its own cookie jar); that is
//! not done (see docs/network.md).

use cef::*;

use crate::proxy::{LEAK_PROTECTION_DEFAULTS, LEAK_PROTECTION_PREFS, ProxySetting, proxy_pref_json};

/// Sets one preference from JSON. Preferences this Chromium does not know are skipped
/// quietly (they differ between versions).
fn set_pref(context: &RequestContext, name: &str, json: &str) -> bool {
    let Some(mut value) =
        parse_json(Some(&json.into()), cef::sys::cef_json_parser_options_t::JSON_PARSER_RFC.into())
    else {
        tracing::warn!(pref = name, "preference value is not valid JSON");
        return false;
    };
    if context.can_set_preference(Some(&name.into())) != 1 {
        tracing::debug!(pref = name, "this Chromium has no such preference");
        return false;
    }
    // CEF refuses a null error string, which an empty `CefString` is: give it a placeholder.
    let mut error = CefString::from("-");
    let ok = context.set_preference(Some(&name.into()), Some(&mut value), Some(&mut error)) == 1;
    if !ok {
        tracing::debug!(pref = name, error = %error.to_string(), "could not set the preference (yet)");
    }
    ok
}

/// Points a context at a proxy (or back at the default) and switches the leak protections.
/// Returns whether the proxy preference was accepted: a context that is not initialized yet
/// (Chromium sets it up while the message loop runs) refuses it, and then nothing is changed.
pub(super) fn apply(context: &RequestContext, setting: &ProxySetting, leak_protection: bool) -> bool {
    let ok = set_pref(context, "proxy", &proxy_pref_json(setting));
    if !ok {
        return false;
    }
    // The setting is logged without the PAC script (it names the sites the user routes).
    let kind = match setting {
        ProxySetting::Direct => "default",
        ProxySetting::Fixed(_) => "fixed server",
        ProxySetting::Pac(_) => "per-site script",
    };
    tracing::info!(mode = kind, applied = ok, leak_protection, "proxy set");
    let list = if leak_protection { LEAK_PROTECTION_PREFS } else { LEAK_PROTECTION_DEFAULTS };
    for (name, json) in list {
        set_pref(context, name, json);
    }
    true
}
