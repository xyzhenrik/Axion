// Axion Passwords: finds sign-in forms and reports them to the browser. Runs in the top
// frame only, so cross-site iframes are never looked at. It never reads a password before the
// user submits the form, and it never fills anything: filling is a separate script the browser
// runs in the top frame after the user picked a login.
(function () {
  if (window !== window.top || window.__axionPasswords) return;
  window.__axionPasswords = true;
  var PREFIX = '\u0001axion:vault:';
  // Taken now, before the page's own scripts could replace it.
  var log = console.log.bind(console);
  function post(message) {
    try { log(PREFIX + JSON.stringify(message)); } catch (e) {}
  }

  /*FINDERS*/

  var reported = 'noform';
  function scan() {
    var found = passwordField() ? 'form' : identifierField() ? 'idform' : 'noform';
    if (found !== reported) {
      reported = found;
      post({ e: found });
    }
  }

  var last = '';
  // The user submitted: report the user name and password for "Save password?".
  function submitted(scope) {
    var pw = submittedPassword(scope);
    if (!pw || !pw.value) return;
    var user = userField(pw);
    var name = user ? user.value : '';
    var key = name + '\n' + pw.value;
    if (key === last) return;
    last = key;
    post({ e: 'submit', user: name, pass: pw.value });
  }

  // The first step of a two-step sign-in: report the user name so the browser can pair it with
  // the password asked on the next page.
  var lastId = '';
  function identified() {
    var id = identifierField();
    if (!id || !id.value.trim() || id.value === lastId) return;
    lastId = id.value;
    post({ e: 'user', user: id.value });
  }

  document.addEventListener('submit', function (ev) {
    if (!ev.isTrusted || !ev.target || !ev.target.querySelector) return;
    if (passwordField()) submitted(ev.target); else identified();
  }, true);
  // Sites that sign in from script: Enter in the password field, or a click on a button.
  document.addEventListener('keydown', function (ev) {
    var t = ev.target;
    if (!ev.isTrusted || ev.key !== 'Enter' || !t) return;
    if (t.type === 'password') submitted(t.form || document);
    else if (t === identifierField()) identified();
  }, true);
  document.addEventListener('click', function (ev) {
    if (!ev.isTrusted || !ev.target || !ev.target.closest) return;
    var button = ev.target.closest('button, input[type=submit], input[type=button], [role=button]');
    var pw = button && passwordField();
    if (pw && (!pw.form || !button.form || pw.form === button.form)) submitted(pw.form || document);
    else if (button && !pw) {
      var id = identifierField();
      if (id && (!id.form || !button.form || id.form === button.form)) identified();
    }
  }, true);

  var timer = 0;
  function later() {
    if (!timer) timer = setTimeout(function () { timer = 0; scan(); }, 400);
  }
  function start() {
    scan();
    try {
      new MutationObserver(later).observe(document.documentElement, { childList: true, subtree: true, attributes: true, attributeFilter: ['type', 'style', 'class', 'hidden'] });
    } catch (e) {}
  }
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start);
  else start();
  window.addEventListener('pageshow', scan);
})();
