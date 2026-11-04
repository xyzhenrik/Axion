// Axion clipboard helper. Injected into every frame as it starts loading, only where Chromium
// cannot put text on the desktop clipboard itself (off-screen Chromium on Wayland has no surface
// with keyboard focus, and Wayland lets only the focused client set the clipboard). The page's
// own copy still runs as usual; when Chromium accepts it (or refuses it only for lack of window
// focus, after a click), the text is reported to the
// browser (a console message the engine swallows), which sets the desktop clipboard.
//
// `__AXION_NONCE__` is a secret of this run, so a page cannot fake a report, and
// `__AXION_MAX__` the longest text reported (UTF-16 units).
(function () {
  'use strict';
  var KEY = '__axionClipboard';
  if (window[KEY]) return;
  Object.defineProperty(window, KEY, { value: true });
  var PREFIX = '\u0001axion-clipboard:__AXION_NONCE__:';
  var MAX = __AXION_MAX__;
  var report = console.debug.bind(console);
  var activation = function () {
    return !!(navigator.userActivation && navigator.userActivation.isActive);
  };

  function send(text, html) {
    try {
      var message = {};
      if (typeof text === 'string' && text) message.text = text.slice(0, MAX);
      if (typeof html === 'string' && html) message.html = html.slice(0, MAX);
      if (message.text !== undefined || message.html !== undefined) report(PREFIX + JSON.stringify(message));
    } catch (e) { /* nothing to report */ }
  }

  // The selection as `copy` would take it: text of a field, else the document's selection.
  function selectedText() {
    var el = document.activeElement;
    try {
      if (el && typeof el.selectionStart === 'number' && el.selectionEnd > el.selectionStart) {
        return el.value.slice(el.selectionStart, el.selectionEnd);
      }
    } catch (e) { /* a field type without a selection */ }
    var selection = window.getSelection && window.getSelection();
    return selection ? selection.toString() : '';
  }

  function selectedHtml() {
    var selection = window.getSelection && window.getSelection();
    if (!selection || selection.rangeCount === 0 || selection.isCollapsed) return '';
    var box = document.createElement('div');
    for (var i = 0; i < selection.rangeCount; i++) box.appendChild(selection.getRangeAt(i).cloneContents());
    return box.innerHTML;
  }

  // navigator.clipboard.writeText(text)
  var proto = window.Clipboard && Clipboard.prototype;
  if (proto && typeof proto.writeText === 'function') {
    var rawWriteText = proto.writeText;
    proto.writeText = function (text) {
      var active = activation();
      var done = function () { send(String(text)); };
      var result = rawWriteText.apply(this, arguments);
      // What Chromium accepts is copied; what it refuses only because its window has no focus
      // on the desktop is copied too when a click or key press asked for it.
      return result.then(function (value) { done(); return value; }, function (error) {
        if (active && error && error.name === 'NotAllowedError') { done(); return undefined; }
        throw error;
      });
    };
  }

  // navigator.clipboard.write([new ClipboardItem({'text/plain': ..., 'text/html': ...})])
  if (proto && typeof proto.write === 'function') {
    var rawWrite = proto.write;
    proto.write = function (items) {
      var active = activation();
      var done = function () {
        var list = Array.prototype.slice.call(items || []);
        Promise.all(list.map(function (item) {
          var read = function (type) {
            return item.types.indexOf(type) < 0 ? Promise.resolve('')
              : item.getType(type).then(function (blob) { return blob.text(); });
          };
          return Promise.all([read('text/plain'), read('text/html')]);
        })).then(function (all) {
          for (var i = 0; i < all.length; i++) {
            if (all[i][0] || all[i][1]) { send(all[i][0], all[i][1]); return; }
          }
        }, function () { /* an item that cannot be read as text: nothing to hand over */ });
      };
      var result = rawWrite.apply(this, arguments);
      return result.then(function (value) { done(); return value; }, function (error) {
        if (active && error && error.name === 'NotAllowedError') { done(); return undefined; }
        throw error;
      });
    };
  }

  // document.execCommand('copy' | 'cut'), the way of every "copy" button before the async API.
  // The copy event runs first (the page may fill `clipboardData` itself); what stays is reported.
  var inExec = false;
  var copied = null;
  window.addEventListener('copy', function (event) { collect(event); }, false);
  window.addEventListener('cut', function (event) { collect(event); }, false);
  function collect(event) {
    if (!inExec) return;
    var data = event.clipboardData;
    var text = data ? data.getData('text/plain') : '';
    var html = data ? data.getData('text/html') : '';
    if (!text && !html && !event.defaultPrevented) {
      text = selectedText();
      html = selectedHtml();
    }
    copied = { text: text, html: html };
  }
  var rawExec = Document.prototype.execCommand;
  Document.prototype.execCommand = function (command) {
    var name = String(command).toLowerCase();
    if (name !== 'copy' && name !== 'cut') return rawExec.apply(this, arguments);
    var active = activation();
    inExec = true;
    copied = null;
    var ok;
    try {
      ok = rawExec.apply(this, arguments);
    } finally {
      inExec = false;
    }
    var taken = copied;
    copied = null;
    if (!ok && active && name === 'copy' && !document.hasFocus()) {
      // Chromium refuses a copy while its window has no focus on the desktop. The copy event
      // is the page's chance to fill the clipboard itself, so it still gets to run.
      inExec = true;
      copied = null;
      try {
        var event = new ClipboardEvent('copy', {
          clipboardData: new DataTransfer(), bubbles: true, cancelable: true, composed: true,
        });
        (document.activeElement || document.body).dispatchEvent(event);
      } catch (e) { /* no copy event to run */ }
      inExec = false;
      taken = copied;
      copied = null;
      if (taken && (taken.text || taken.html)) {
        send(taken.text, taken.html);
        return true;
      }
      return ok;
    }
    if (ok && taken) send(taken.text, taken.html);
    return ok;
  };
})();
