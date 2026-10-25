//! Both directions of autofill. Page → browser: the injected script reports sign-in forms
//! and, after the user submitted one, the user name and password, as console messages
//! starting with [`PREFIX`]. Browser → page: [`fill_script`], run in the top frame only after
//! the user picked a login.
//!
//! Messages can be forged by any script in the page, so they only ever lead to an offer the
//! user has to accept; which logins may be filled is decided from the tab's own URL.

use serde::Deserialize;
use zeroize::Zeroizing;

/// Console messages starting with this come from the autofill script.
pub const PREFIX: &str = "\u{1}axion:vault:";

const SCRIPT: &str = include_str!("autofill.js");

/// Finding the fields, shared by the page script and the fill script.
const FINDERS: &str = r#"
  function visible(el) {
    if (el.disabled || el.readOnly) return false;
    var r = el.getBoundingClientRect();
    if (r.width < 2 || r.height < 2) return false;
    var s = getComputedStyle(el);
    return s.visibility !== 'hidden' && s.display !== 'none' && s.opacity !== '0';
  }
  function passwords(scope) {
    var all = scope.querySelectorAll('input[type=password]'), out = [];
    for (var i = 0; i < all.length; i++) if (visible(all[i])) out.push(all[i]);
    return out;
  }
  function passwordField() { return passwords(document)[0] || null; }
  // On sign-up and change-password forms the last filled one is the new password.
  function submittedPassword(scope) {
    var list = passwords(scope).filter(function (p) { return p.value; });
    return list.length ? list[list.length - 1] : null;
  }
  // The last visible text field before the password (or one marked as the user name).
  function userField(pw) {
    var scope = pw.form || document;
    var inputs = scope.querySelectorAll('input'), user = null;
    for (var i = 0; i < inputs.length && inputs[i] !== pw; i++) {
      var t = (inputs[i].type || 'text').toLowerCase();
      if ((t === 'text' || t === 'email' || t === 'tel') && visible(inputs[i])) user = inputs[i];
    }
    return user || scope.querySelector('input[autocomplete~=username]');
  }
"#;

const FILL: &str = r#"
(function () {
  if (window !== window.top || location.hostname !== __HOST__) return;
  /*FINDERS*/
  var data = __DATA__;
  var setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
  function set(el, value) {
    el.focus();
    setter.call(el, value);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
  }
  var pw = passwordField();
  if (!pw) return;
  var user = userField(pw);
  if (user && data.user) set(user, data.user);
  set(pw, data.pass);
})();
"#;

/// The script injected into every document.
pub fn page_script() -> String {
    SCRIPT.replace("/*FINDERS*/", FINDERS)
}

/// What the page reported.
pub enum PageEvent {
    /// A sign-in form is on the page.
    Form,
    /// No (more) sign-in form.
    NoForm,
    /// The user submitted a form with these credentials.
    Submit { username: String, password: Zeroizing<String> },
}

/// Never shows the password.
impl std::fmt::Debug for PageEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PageEvent::Form => f.write_str("Form"),
            PageEvent::NoForm => f.write_str("NoForm"),
            PageEvent::Submit { .. } => f.write_str("Submit"),
        }
    }
}

#[derive(Deserialize)]
struct Wire {
    e: String,
    #[serde(default)]
    user: String,
    #[serde(default)]
    pass: String,
}

/// Whether a console message belongs to autofill (it may hold a password: never log it).
pub fn is_message(console_message: &str) -> bool {
    console_message.starts_with(PREFIX)
}

/// Parses an autofill message (`None` for other or malformed messages).
pub fn parse(console_message: &str) -> Option<PageEvent> {
    let json = Zeroizing::new(console_message.strip_prefix(PREFIX)?.to_owned());
    let wire: Wire = serde_json::from_str(&json).ok()?;
    let password = Zeroizing::new(wire.pass);
    match wire.e.as_str() {
        "form" => Some(PageEvent::Form),
        "noform" => Some(PageEvent::NoForm),
        "submit" if !password.is_empty() => {
            Some(PageEvent::Submit { username: wire.user.trim().to_owned(), password })
        }
        _ => None,
    }
}

/// The script that fills a login into the page at `page_url`. It does nothing if the top
/// frame has navigated to another host meanwhile. `None` if the URL has no host.
pub fn fill_script(page_url: &str, username: &str, password: &str) -> Option<Zeroizing<String>> {
    let host = url::Url::parse(page_url).ok()?.host_str()?.to_owned();
    let host = serde_json::to_string(&host).ok()?;
    let data = Zeroizing::new(
        serde_json::to_string(&serde_json::json!({ "user": username, "pass": password })).ok()?,
    );
    Some(Zeroizing::new(
        FILL.replace("/*FINDERS*/", FINDERS).replace("__HOST__", &host).replace("__DATA__", &data),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages() {
        assert!(matches!(parse("\u{1}axion:vault:{\"e\":\"form\"}"), Some(PageEvent::Form)));
        let submit =
            parse("\u{1}axion:vault:{\"e\":\"submit\",\"user\":\" me \",\"pass\":\"test-only-pass\"}");
        let Some(PageEvent::Submit { username, password }) = &submit else { panic!() };
        assert_eq!((username.as_str(), password.as_str()), ("me", "test-only-pass"));
        assert!(!format!("{submit:?}").contains("test-only-pass"));
        assert!(parse("\u{1}axion:vault:{\"e\":\"submit\",\"pass\":\"\"}").is_none());
        assert!(parse("\u{1}axion:{\"e\":\"form\"}").is_none() && !is_message("\u{1}axion:{}"));
    }

    #[test]
    fn scripts() {
        assert!(page_script().contains("function passwordField") && !page_script().contains("/*FINDERS*/"));
        let script = fill_script("https://login.example.com/x", "me", "a\"b'</script>").unwrap();
        assert!(script.contains(r#"location.hostname !== "login.example.com""#));
        assert!(script.contains(r#""pass":"a\"b'</script>""#), "values are JSON-escaped");
        assert!(fill_script("about:blank", "", "").is_none());
    }
}
