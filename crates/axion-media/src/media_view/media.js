// Axion's media view: plays the one source the browser captured for it, and nothing else.
// The browser serves the source to this page only (`/api/source?s=<token>`); the page has no
// links, no navigation and no site scripts. Playback state and commands go through the bridge
// (bridge.js, loaded before this script); failures are reported as marked console messages.
(function () {
  'use strict';
  var MARK = '\u0001axion:';
  var v = document.getElementById('v');
  var token = new URLSearchParams(location.search).get('s') || '';
  var failed = false;

  function fail(code, message) {
    if (failed) return;
    failed = true;
    console.debug(MARK + JSON.stringify({ t: 'failure', code: code, message: String(message || '') }));
  }

  v.addEventListener('error', function () {
    var e = v.error;
    fail('media', (e ? (e.code + ' ' + (e.message || '')) : 'unknown media error') + ' [src=' + v.currentSrc +
      ' net=' + v.networkState + ' ready=' + v.readyState + ' webm=' + v.canPlayType('video/webm; codecs="vp9, opus"') + ']');
  });
  // Nothing may take the page elsewhere or open anything.
  document.addEventListener('click', function (e) { e.preventDefault(); }, true);
  window.open = function () { return null; };

  function start(src) {
    if (src.kind === 'hls') {
      if (window.Hls && Hls.isSupported()) {
        var hls = new Hls({ enableWorker: true, lowLatencyMode: false, maxBufferLength: 60 });
        window.__axionHls = hls;
        var retried = false, recovered = false;
        hls.on(Hls.Events.ERROR, function (_, data) {
          if (!data.fatal) return;
          if (data.type === Hls.ErrorTypes.NETWORK_ERROR && !retried) {
            retried = true;
            hls.startLoad();
            return;
          }
          if (data.type === Hls.ErrorTypes.MEDIA_ERROR && !recovered) {
            recovered = true;
            hls.recoverMediaError();
            return;
          }
          fail('hls', data.type + ' ' + data.details);
        });
        hls.loadSource(src.url);
        hls.attachMedia(v);
      } else {
        fail('hls', 'HLS is not supported here');
      }
    } else {
      v.src = src.url;
    }
  }

  // A new view's page may ask before the browser has committed it (refused with 403): ask again.
  function load(tries) {
    fetch('/api/source?s=' + encodeURIComponent(token), { cache: 'no-store' })
      .then(function (r) {
        if (r.status === 403 && tries > 0) { setTimeout(function () { load(tries - 1); }, 150); return null; }
        if (!r.ok) throw new Error('source ' + r.status);
        return r.json();
      })
      .then(function (src) { if (src) start(src); })
      .catch(function (e) { fail('source', e); });
  }
  load(20);
})();
