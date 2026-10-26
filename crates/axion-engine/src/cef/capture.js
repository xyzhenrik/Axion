// Axion capture helper. Injected into every frame as it starts loading: it keeps the tracks
// the page gets from getUserMedia / getDisplayMedia, so the browser's "Stop" can end them the
// way the page's own button would (the tracks stop and report `ended`). It reports nothing.
(function () {
  'use strict';
  var KEY = '__axionStopCapture';
  if (window[KEY] || !window.MediaDevices) return;
  var proto = MediaDevices.prototype;
  var kept = [];

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
