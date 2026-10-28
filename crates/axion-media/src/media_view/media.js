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

  // One log line per step of the video's loading, so a log tells what happened.
  function diag(event) {
    var q = v.getVideoPlaybackQuality && v.getVideoPlaybackQuality();
    console.debug(MARK + JSON.stringify({ t: 'probe', event: event, src: v.currentSrc, ready: v.readyState,
      network: v.networkState, error: v.error && v.error.code, time: v.currentTime, duration: v.duration,
      vw: v.videoWidth, vh: v.videoHeight, frames: q ? q.totalVideoFrames : -1, dropped: q ? q.droppedVideoFrames : -1 }));
  }
  ['loadedmetadata', 'playing', 'waiting', 'stalled', 'canplay', 'ended', 'seeked'].forEach(function (e) {
    v.addEventListener(e, function () { diag(e); });
  });
  // The picture can't be decoded (HEVC and other formats Chromium lacks): the sound plays,
  // the screen stays black. Say so instead of leaving a silent black picture.
  var AUDIO_ONLY = /\.(mp3|m4a|aac|ogg|oga|opus|flac|wav)(\?|#|$)/i;
  var sourceUrl = '';
  var audioMime = false;   // the host calls the source audio (audio/mp4, audio/mpeg…)
  function probeMime(url) {
    // The start of the file, through the browser: its content type, and (for an mp4 whose
    // index sits up front) whether it has a video track at all.
    fetch(url, { headers: { Range: 'bytes=0-131071' }, cache: 'no-store' }).then(function (r) {
      if (/^audio\//i.test(r.headers.get('content-type') || '')) { audioMime = true; return null; }
      return r.arrayBuffer();
    }).then(function (buf) {
      if (!buf) return;
      var text = '';
      var bytes = new Uint8Array(buf);
      for (var i = 0; i < bytes.length; i++) text += String.fromCharCode(bytes[i]);
      // An index (`moov`) listing sound tracks only.
      if (text.indexOf('moov') >= 0 && text.indexOf('vide') < 0 && text.indexOf('soun') >= 0) audioMime = true;
    }).catch(function () {});
  }
  // A stream whose playlist names no video codec is sound only.
  function hlsAudioOnly() {
    try {
      var h = window.__axionHls, l = h && h.levels && h.levels[h.currentLevel];
      return !!l && !l.videoCodec && !l.width;
    } catch (e) { return false; }
  }
  v.addEventListener('playing', function () {
    setTimeout(function () {
      // Only a source that should have a picture can fail to show one: never sound-only ones
      // (music, podcasts), however they are named.
      if (failed || v.paused || v.ended || AUDIO_ONLY.test(sourceUrl) || audioMime || hlsAudioOnly()) return;
      var q = v.getVideoPlaybackQuality && v.getVideoPlaybackQuality();
      if (v.videoWidth === 0 || (q && q.totalVideoFrames === 0 && v.currentTime > 1)) {
        diag('no-picture');
        fail('codec', 'no decodable video track (videoWidth=' + v.videoWidth + ')');
      }
    }, 2500);
  });
  v.addEventListener('error', function () {
    var e = v.error;
    fail('media', (e ? (e.code + ' ' + (e.message || '')) : 'unknown media error') + ' [src=' + v.currentSrc +
      ' net=' + v.networkState + ' ready=' + v.readyState + ' webm=' + v.canPlayType('video/webm; codecs="vp9, opus"') + ']');
  });
  // Nothing may take the page elsewhere or open anything.
  document.addEventListener('click', function (e) { e.preventDefault(); }, true);
  window.open = function () { return null; };

  function start(src) {
    sourceUrl = src.url || '';
    if (src.kind !== 'hls') probeMime(sourceUrl);
    if (src.kind === 'hls') {
      if (window.Hls && Hls.isSupported()) {
        var hls = new Hls({ enableWorker: true, lowLatencyMode: false, maxBufferLength: 60, backBufferLength: 90, liveDurationInfinity: true });
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
