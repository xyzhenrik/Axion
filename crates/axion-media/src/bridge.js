// Axion media bridge. Injected into every frame as it starts loading. It finds the frame's
// media elements, reports their state to the browser and carries out the browser's commands.
// Child frames report to their parent with postMessage; the top frame adds the iframe's
// offset and reports everything as one marked console message. Placeholders are filled in
// by axion-media.
(function () {
  'use strict';
  if (window.__axionMedia) return;
  var SECRET = '__AXION_SECRET__';
  // Axion's player takes over videos in fullscreen (pages keep their own controls inline).
  var OWN_FULLSCREEN = __AXION_OWN_FULLSCREEN__;
  // Sites whose own player is trusted and kept, also in fullscreen (YouTube, Twitch…).
  var TRUSTED = (function (host, sites) {
    return sites.some(function (s) { return host === s || host.slice(-s.length - 1) === '.' + s; });
  })(location.hostname, __AXION_TRUSTED__);
  if (TRUSTED) OWN_FULLSCREEN = false;
  var MARK = '\u0001axion:';
  var log = console.debug.bind(console);
  var isTop = window === window.top;
  var FRAME = Math.random().toString(36).slice(2, 8);
  var EVENTS = ['play', 'playing', 'pause', 'ended', 'timeupdate', 'volumechange', 'ratechange',
    'durationchange', 'seeked', 'loadedmetadata', 'emptied', 'progress', 'resize'];
  var seq = 0;
  var media = new Set();
  var shadowRoots = [];
  var children = new Map();   // child window -> its players
  var childOf = new Map();    // player id -> child window
  var childSession = null;
  var takeover = null;        // {el, style, rootOverflow}
  var lastSent = '';
  var timer = 0;

  // Media inside shadow DOM (also closed roots) and media never put in the DOM (new Audio()).
  var attachShadow = Element.prototype.attachShadow;
  Element.prototype.attachShadow = function () {
    var root = attachShadow.apply(this, arguments);
    shadowRoots.push(root);
    return root;
  };
  var play = HTMLMediaElement.prototype.play;
  HTMLMediaElement.prototype.play = function () {
    register(this);
    return play.apply(this, arguments);
  };

  var style = document.createElement('style');
  // Takeover hides everything but the video: `visibility` (unlike z-index) is not trapped by
  // the site's stacking contexts, and hidden elements take no clicks.
  style.textContent =
    'html.axion-takeover,html.axion-takeover body{background:#000!important}' +
    'html.axion-takeover *{visibility:hidden!important}' +
    'html.axion-takeover [data-axion-takeover]{visibility:visible!important}' +
    // A fullscreen <video> shows Axion's player, not the built-in controls.
    (OWN_FULLSCREEN ? 'video:fullscreen::-webkit-media-controls{display:none!important}' : '');
  function addStyle() {
    var root = document.head || document.documentElement;
    if (!root) return false;
    root.appendChild(style);
    return true;
  }
  if (!addStyle()) document.addEventListener('DOMContentLoaded', addStyle);

  function register(el) {
    // The start page's live wallpaper is decoration, not media: no player, no "playing" tab,
    // no Now Playing entry, and it never keeps the tab from freezing.
    if (el.hasAttribute && el.hasAttribute('data-axion-wallpaper')) return;
    if (media.has(el)) return;
    media.add(el);
    el.__axId = FRAME + ':' + (++seq);
    if (el.tagName === 'VIDEO' && el.hasAttribute('controls')) el.__axNative = true;
    EVENTS.forEach(function (t) { el.addEventListener(t, schedule, true); });
    schedule();
  }

  function scan() {
    [document].concat(shadowRoots).forEach(function (root) {
      if (root.querySelectorAll) root.querySelectorAll('video,audio').forEach(register);
    });
  }

  function frames() {
    var out = [];
    [document].concat(shadowRoots).forEach(function (root) {
      if (root.querySelectorAll) root.querySelectorAll('iframe,frame').forEach(function (f) { out.push(f); });
    });
    return out;
  }
  function frameOf(win) {
    var all = frames();
    for (var i = 0; i < all.length; i++) if (all[i].contentWindow === win) return all[i];
    return null;
  }

  function describe(el) {
    var r = el.getBoundingClientRect();
    var buffered = 0;
    try {
      for (var i = 0; i < el.buffered.length; i++) {
        if (el.buffered.start(i) <= el.currentTime + 0.5) buffered = Math.max(buffered, el.buffered.end(i));
      }
    } catch (e) {}
    var captions = [], caption = -1;
    if (el.textTracks) {
      for (var j = 0; j < el.textTracks.length; j++) {
        var t = el.textTracks[j];
        if (t.kind !== 'subtitles' && t.kind !== 'captions') continue;
        captions.push({ index: j, label: t.label || t.language || ('Track ' + (captions.length + 1)) });
        if (t.mode === 'showing') caption = j;
      }
    }
    return {
      id: el.__axId, kind: el.tagName === 'VIDEO' ? 'video' : 'audio',
      x: r.left, y: r.top, w: r.width, h: r.height, vw: el.videoWidth || 0, vh: el.videoHeight || 0,
      fit: el.tagName === 'VIDEO' ? (getComputedStyle(el).objectFit || '') : '',
      paused: el.paused, ended: el.ended, time: el.currentTime || 0,
      duration: isFinite(el.duration) ? el.duration : 0, live: el.duration === Infinity,
      volume: el.volume, muted: el.muted, rate: el.playbackRate, buffered: buffered,
      captions: captions, caption: caption, native: !!el.__axNative,
      trusted: TRUSTED,
      src: el.currentSrc || el.src || '', poster: el.poster || '',
      fullscreen: !!document.fullscreenElement &&
        (document.fullscreenElement === el || document.fullscreenElement.contains(el)) || (takeover && takeover.el === el) || false
    };
  }

  function session() {
    var m = navigator.mediaSession && navigator.mediaSession.metadata;
    if (!m) return childSession;
    var art = (m.artwork && m.artwork.length) ? m.artwork[m.artwork.length - 1].src : '';
    return { title: m.title || '', artist: m.artist || '', album: m.album || '', artwork: art };
  }

  // The page's own preview picture (Open Graph / Twitter card), for the Playlist's thumbnail.
  function pageImage() {
    var m = document.querySelector('meta[property="og:image"],meta[name="og:image"],meta[name="twitter:image"],meta[property="twitter:image"]');
    var url = m && m.getAttribute('content');
    if (!url) return '';
    try { return new URL(url, document.baseURI).href; } catch (e) { return ''; }
  }

  // A small JPEG of the video's current frame; empty when the page's CORS rules forbid it.
  function snapshot(el, max) {
    var data = '';
    try {
      var w = el.videoWidth, h = el.videoHeight;
      if (w && h) {
        var k = Math.min(1, max / Math.max(w, h));
        var c = document.createElement('canvas');
        c.width = Math.max(1, Math.round(w * k));
        c.height = Math.max(1, Math.round(h * k));
        c.getContext('2d').drawImage(el, 0, 0, c.width, c.height);
        data = c.toDataURL('image/jpeg', 0.8);
      }
    } catch (e) {}
    event({ e: 'snapshot', id: el.__axId, data: data });
  }

  function schedule() { if (!timer) timer = setTimeout(report, 120); }
  function report() {
    timer = 0;
    var list = [];
    media.forEach(function (el) { if (el.isConnected || !el.paused) list.push(describe(el)); });
    children.forEach(function (players, win) {
      var f = frameOf(win);
      if (!f) { children.delete(win); return; }
      var r = f.getBoundingClientRect();
      var ox = r.left + f.clientLeft, oy = r.top + f.clientTop;
      var inFullscreen = !!document.fullscreenElement && (document.fullscreenElement === f || document.fullscreenElement.contains(f));
      players.forEach(function (p) {
        var q = Object.assign({}, p);
        q.x += ox; q.y += oy;
        q.fullscreen = q.fullscreen || inFullscreen;
        list.push(q);
      });
    });
    var s = session();
    if (isTop) {
      var msg = JSON.stringify({ t: 'state', players: list, dpr: window.devicePixelRatio, session: s, image: list.length ? pageImage() : '' });
      if (msg !== lastSent) { lastSent = msg; log(MARK + msg); }
    } else {
      var key = JSON.stringify([list, s]);
      if (key !== lastSent) { lastSent = key; parent.postMessage({ s: SECRET, k: 'state', players: list, session: s }, '*'); }
    }
  }

  function event(e) {
    if (isTop) log(MARK + JSON.stringify({ t: 'event', event: e }));
    else parent.postMessage({ s: SECRET, k: 'event', event: e }, '*');
  }

  function find(id) {
    var found = null;
    media.forEach(function (el) { if (el.__axId === id) found = el; });
    return found;
  }

  function setTakeover(el, on) {
    if (on && !takeover) {
      takeover = { el: el, style: el.getAttribute('style'), overflow: document.documentElement.style.overflow };
      el.style.cssText += ';position:fixed!important;inset:0!important;width:100vw!important;height:100vh!important;' +
        'max-width:none!important;max-height:none!important;margin:0!important;transform:none!important;' +
        'z-index:2147483647!important;background:#000!important;object-fit:contain!important';
      el.setAttribute('data-axion-takeover', '');
      document.documentElement.classList.add('axion-takeover');
      document.documentElement.style.overflow = 'hidden';
    } else if (!on && takeover) {
      if (takeover.style === null) takeover.el.removeAttribute('style'); else takeover.el.setAttribute('style', takeover.style);
      takeover.el.removeAttribute('data-axion-takeover');
      document.documentElement.classList.remove('axion-takeover');
      document.documentElement.style.overflow = takeover.overflow;
      takeover = null;
    }
  }

  function apply(el, c) {
    var v = c.value;
    switch (c.action) {
      case 'play': el.play(); break;
      case 'pause': el.pause(); break;
      case 'toggle': if (el.paused || el.ended) el.play(); else el.pause(); break;
      case 'seek': el.currentTime = Math.max(0, v); break;
      case 'skip': el.currentTime = Math.max(0, el.currentTime + v); break;
      case 'volume': el.volume = Math.min(1, Math.max(0, v)); if (v > 0) el.muted = false; break;
      case 'mute': el.muted = !!v; break;
      case 'rate': el.playbackRate = v; break;
      case 'caption':
        for (var i = 0; i < el.textTracks.length; i++) el.textTracks[i].mode = (i === v) ? 'showing' : 'disabled';
        break;
      case 'takeover': setTakeover(el, !!v); break;
      case 'snapshot': snapshot(el, v || 480); break;
      case 'probe':
        log(MARK + JSON.stringify({ t: 'probe', frame: location.href, src: el.currentSrc, ready: el.readyState,
          network: el.networkState, error: el.error && el.error.code, paused: el.paused,
          jw: window.jwplayer && window.jwplayer().getState && window.jwplayer().getState(),
          jwError: (document.querySelector('.jw-error-msg, .jw-error-text') || {}).textContent,
          h264: window.MediaSource && MediaSource.isTypeSupported('video/mp4; codecs="avc1.42E01E"'),
          aac: window.MediaSource && MediaSource.isTypeSupported('audio/mp4; codecs="mp4a.40.2"'),
          mp4: el.canPlayType('video/mp4; codecs="avc1.42E01E, mp4a.40.2"') }));
        break;
      case 'fullscreen':
        if (v) { if (el.requestFullscreen) el.requestFullscreen().catch(function () { setTakeover(el, true); }); else setTakeover(el, true); }
        else { setTakeover(el, false); if (document.fullscreenElement) document.exitFullscreen(); }
        break;
    }
    schedule();
  }

  function cmd(c) {
    var el = find(c.id);
    if (el) { apply(el, c); return; }
    var win = childOf.get(c.id);
    if (!win) return;
    // A player in a child frame: fullscreen goes to its iframe here (only this frame has
    // the user activation), the child fills the iframe with the video.
    if (c.action === 'fullscreen') {
      var f = frameOf(win);
      if (c.value && f && f.requestFullscreen) {
        f.requestFullscreen().catch(function () {});
        win.postMessage({ s: SECRET, k: 'cmd', cmd: { id: c.id, action: 'takeover', value: true } }, '*');
        return;
      }
      if (!c.value) {
        if (document.fullscreenElement) document.exitFullscreen();
        win.postMessage({ s: SECRET, k: 'cmd', cmd: { id: c.id, action: 'takeover', value: false } }, '*');
        return;
      }
    }
    win.postMessage({ s: SECRET, k: 'cmd', cmd: c }, '*');
  }

  window.addEventListener('message', function (e) {
    var d = e.data;
    if (!d || d.s !== SECRET) return;
    if (d.k === 'cmd' && e.source === parent && !isTop) { cmd(d.cmd); return; }
    if (!frameOf(e.source)) return;
    if (d.k === 'state') {
      children.set(e.source, d.players);
      d.players.forEach(function (p) { childOf.set(p.id, e.source); });
      if (d.session) childSession = d.session;
      schedule();
    } else if (d.k === 'event') {
      event(d.event);
    }
  });

  document.addEventListener('fullscreenchange', function () {
    if (!document.fullscreenElement) setTakeover(null, false);
    schedule();
  });
  window.addEventListener('scroll', schedule, { passive: true, capture: true });
  window.addEventListener('resize', schedule);
  window.addEventListener('pagehide', function () {
    media.clear(); children.clear();
    if (timer) clearTimeout(timer);
    report();
  });
  document.addEventListener('DOMContentLoaded', scan);
  setInterval(function () { scan(); schedule(); }, 2000);
  scan();

  Object.defineProperty(window, '__axionMedia', { value: Object.freeze({ cmd: cmd }) });
})();
