// Axion capture helper. Injected into every frame as it starts loading: it keeps the tracks
// the page gets from getUserMedia / getDisplayMedia, so the browser's "Stop" can end them the
// way the page's own button would (the tracks stop and report `ended`). It reports nothing.
(function () {
  'use strict';
  var KEY = '__axionStopCapture';
  if (window[KEY] || !window.MediaDevices) return;
  var proto = MediaDevices.prototype;
  var kept = [];
  var rawGetUserMedia = proto.getUserMedia;

  function keep(stream, screen) {
    try {
      kept = kept.filter(function (e) { return e.track.readyState === 'live'; });
      stream.getTracks().forEach(function (track) { kept.push({ track: track, screen: screen }); });
    } catch (e) { /* not a stream: nothing to keep */ }
    return stream;
  }

  function wrap(name, screen) {
    var original = proto[name];
    if (typeof original !== 'function') return;
    proto[name] = function () {
      return original.apply(this, arguments).then(function (stream) { return keep(stream, screen); });
    };
  }

  wrap('getUserMedia', false);
  wrap('getDisplayMedia', true);

  // Axion's own screen picker (`window.__axionPicker`, set by the engine before this script
  // runs): getDisplayMedia() asks the browser which screen or window to share instead of
  // leaving it to Chromium, then captures that source through the desktop-capture constraints.
  // The report goes out as a console message the engine swallows; the browser answers through
  // `__axionPicked`.
  if (window.__axionPicker && typeof proto.getDisplayMedia === 'function') {
    var waiting = {};
    var seq = 0;
    proto.getDisplayMedia = function (constraints) {
      var self = this;
      if (!(navigator.userActivation && navigator.userActivation.isActive)) {
        return Promise.reject(new DOMException(
          'getDisplayMedia requires transient activation from a user gesture.', 'NotAllowedError'));
      }
      var audio = !!(constraints && constraints.audio);
      return new Promise(function (resolve, reject) {
        var token = ++seq;
        waiting[token] = { self: self, audio: audio, resolve: resolve, reject: reject };
        console.debug('\u0001axion-screen-pick:' + JSON.stringify({ token: token, audio: audio }));
      });
    };
    Object.defineProperty(window, '__axionPicked', {
      value: function (token, id, audio) {
        var entry = waiting[token];
        delete waiting[token];
        if (!entry) return;
        if (!id) {
          entry.reject(new DOMException('Permission denied', 'NotAllowedError'));
          return;
        }
        var source = { mandatory: { chromeMediaSource: 'desktop', chromeMediaSourceId: id } };
        rawGetUserMedia.call(entry.self, { video: source, audio: audio ? source : false })
          .then(function (stream) { entry.resolve(keep(stream, true)); }, entry.reject);
      },
    });
  }

  // kinds: 'microphone', 'camera', 'screen'.
  Object.defineProperty(window, KEY, {
    value: function (kinds) {
      kept.forEach(function (e) {
        var kind = e.screen ? 'screen' : (e.track.kind === 'audio' ? 'microphone' : 'camera');
        if (kinds.indexOf(kind) < 0 || e.track.readyState !== 'live') return;
        e.track.stop();
        try { e.track.dispatchEvent(new Event('ended')); } catch (err) { /* stopped anyway */ }
      });
    },
  });
})();
