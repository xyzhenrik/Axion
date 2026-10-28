// Chrome Web Store pages: "Add to Chrome" must not open Chromium's own install dialog (it needs
// a native window and crashes in off-screen mode). The store's page calls
// chrome.webstorePrivate.beginInstallWithManifest3 when the button is pressed; this script
// replaces that call by a message to the browser, which downloads, checks and asks in its own
// dialog, and answers the page "user_cancelled" so its button goes back to normal.
//
// getExtensionStatus is answered "enabled" for installed extensions, so the store shows its
// installed state, and its "Remove" (chrome.management.uninstall) opens Axion's own details
// for that extension. If the store has no webstorePrivate (it was redesigned), a small button of
// our own is shown instead.
(function () {
  'use strict';
  if (window.__axionStore || window !== window.top) return;
  var host = location.hostname;
  var legacy = (host === 'chrome.google.com' || host === 'www.chrome.google.com') && location.pathname.indexOf('/webstore') === 0;
  if (location.protocol !== 'https:' || (host !== 'chromewebstore.google.com' && !legacy)) return;

  var MARK = '\u0001axion:ws:';
  var log = console.log.bind(console);
  var init = __AXION_INIT__;
  var state = { installed: {}, add: init.add || 'Add to Axion', remove: init.remove || 'Remove from Axion' };
  (init.installed || []).forEach(function (id) { state.installed[id] = true; });
  var lastHref = '';
  var pill = null;

  function send(message) {
    try { log(MARK + JSON.stringify(message)); } catch (e) { /* nothing to do */ }
  }

  function pageId() {
    var m = location.pathname.match(/\/detail\/(?:[^\/]+\/)?([a-p]{32})(?:\/|$)/);
    return m ? m[1] : '';
  }

  function request(id) {
    send({ e: state.installed[id] ? 'remove' : 'install', id: id });
  }

  // "Remove from Chrome" asks chrome.management.uninstall (with Chromium's own dialog).
  function shimManagement() {
    var mg = window.chrome && window.chrome.management;
    if (!mg || typeof mg.uninstall !== 'function' || mg.__axion) return;
    try {
      Object.defineProperty(mg, 'uninstall', { configurable: true, writable: true, value: function (id, options, callback) {
        if (typeof options === 'function') callback = options;
        request(id);
        if (typeof callback === 'function') setTimeout(function () { try { callback(); } catch (e) { /* the page's */ } }, 0);
      } });
      Object.defineProperty(mg, '__axion', { value: true });
    } catch (e) { /* read-only */ }
  }

  // The page's own install call.
  function shim() {
    try {
      var wp = window.chrome && window.chrome.webstorePrivate;
      if (!wp) return false;
      if (wp.__axion) { shimManagement(); return true; }
      var begin = function (details, callback) {
        var id = (details && details.id) || pageId();
        request(id);
        if (typeof callback === 'function') setTimeout(function () { try { callback('user_cancelled'); } catch (e) { /* the page's */ } }, 0);
      };
      var original = wp.getExtensionStatus;
      var status = function () {
        var args = Array.prototype.slice.call(arguments);
        var callback = args[args.length - 1];
        if (state.installed[args[0]] && typeof callback === 'function') {
          setTimeout(function () { try { callback('enabled'); } catch (e) { /* the page's */ } }, 0);
          return;
        }
        return original.apply(wp, args);
      };
      var replace = function (name, fn) {
        try { Object.defineProperty(wp, name, { value: fn, configurable: true, writable: true }); } catch (e) { try { wp[name] = fn; } catch (e2) { /* read-only */ } }
      };
      ['beginInstallWithManifest3', 'beginInstallWithManifest2', 'beginInstallWithManifest'].forEach(function (name) {
        if (typeof wp[name] === 'function') replace(name, begin);
      });
      if (typeof original === 'function') replace('getExtensionStatus', status);
      shimManagement();
      try { Object.defineProperty(wp, '__axion', { value: true }); } catch (e) { /* ignore */ }
      return true;
    } catch (e) {
      return false;
    }
  }

  // Our own button, only when the store has no install API we know.
  function showPill() {
    var id = pageId();
    if (!id || (window.chrome && window.chrome.webstorePrivate)) {
      if (pill) { pill.remove(); pill = null; }
      return;
    }
    if (!pill) {
      pill = document.createElement('button');
      pill.id = '__axion-store-pill';
      pill.type = 'button';
      pill.style.cssText = 'position:fixed;right:20px;bottom:20px;z-index:2147483647;padding:10px 18px;border:0;border-radius:999px;background:#1a73e8;color:#fff;font:500 14px system-ui,sans-serif;cursor:pointer;box-shadow:0 2px 10px rgba(0,0,0,.35)';
      pill.addEventListener('click', function () { var now = pageId(); if (now) request(now); });
      (document.body || document.documentElement).appendChild(pill);
    }
    pill.textContent = state.installed[id] ? state.remove : state.add;
  }

  function tick() {
    shim();
    if (location.href !== lastHref) {
      lastHref = location.href;
      send({ e: 'ready', id: pageId() });
    }
    showPill();
  }

  window.__axionStore = {
    configure: function (config) {
      state.installed = {};
      (config.installed || []).forEach(function (id) { state.installed[id] = true; });
      if (config.add) state.add = config.add;
      if (config.remove) state.remove = config.remove;
      showPill();
    },
    // Debugging aid: what the script sees (reported to the browser's log).
    probe: function () {
      var wp = window.chrome && window.chrome.webstorePrivate;
      send({ e: 'probe', id: pageId(), hasPrivate: !!wp, shim: !!(wp && wp.__axion), installed: Object.keys(state.installed) });
    }
  };

  shim();
  tick();
  setInterval(tick, 1000);
  setTimeout(function () { window.__axionStore.probe(); }, 4000);
})();
