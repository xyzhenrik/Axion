// Axion media bridge. Injected into every frame as it starts loading. It finds the frame's
// media elements, reports their state to the browser and carries out the browser's commands.
// Child frames report to their parent with postMessage; the top frame adds the iframe's
// offset and reports everything as one marked console message. Placeholders are filled in
// by axion-media.
(function () {
  'use strict';
  if (window.__axionMedia) return;
  var SECRET = '__AXION_SECRET__';
  var REPLACE_NATIVE = __AXION_REPLACE__;
  var REPLACE_SITE = __AXION_REPLACE_SITE__;
  // Site players (YouTube…) smaller than this keep their own UI.
  var MIN_W = 220, MIN_H = 124;
  // A player's container is the ancestors up to this much bigger than its video.
  var CONTAINER_SLACK = 1.6;
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
  style.textContent = 'video[data-axion-controls]::-webkit-media-controls{display:none!important}' +
    'html.axion-takeover,html.axion-takeover body{background:#000!important}' +
    'html.axion-takeover *{visibility:hidden!important}' +
    'html.axion-takeover [data-axion-takeover]{visibility:visible!important}' +
    // A site player whose UI ours replaces: everything in its container but the video.
    '[data-axion-player] *{visibility:hidden!important}' +
    '[data-axion-player] [data-axion-own]{visibility:visible!important}';
  function addStyle() {
    var root = document.head || document.documentElement;
    if (!root) return false;
    root.appendChild(style);
    return true;
  }
  if (!addStyle()) document.addEventListener('DOMContentLoaded', addStyle);

  function register(el) {
    if (media.has(el)) return;
    media.add(el);
    el.__axId = FRAME + ':' + (++seq);
    if (el.tagName === 'VIDEO' && el.hasAttribute('controls')) {
      el.__axNative = true;
      if (REPLACE_NATIVE) replaceControls(el);
    }
    EVENTS.forEach(function (t) { el.addEventListener(t, schedule, true); });
    schedule();
  }

  function replaceControls(el) {
    el.controls = false;
    el.setAttribute('data-axion-controls', '');
    new MutationObserver(function () { if (el.controls) el.controls = false; })
      .observe(el, { attributes: true, attributeFilter: ['controls'] });
    el.addEventListener('click', function () { if (el.paused) el.play(); else el.pause(); });
    el.addEventListener('dblclick', function () { event({ e: 'dblclick', id: el.__axId }); });
  }

  // Gives a site player's video our controls: hides the site's player UI around it. Muted
  // looping videos are decoration (animated backgrounds) and stay as they are.
  function replaceSitePlayer(el) {
    if (el.__axOwn) return true;
    if (!REPLACE_SITE || el.__axNative || el.tagName !== 'VIDEO' || (el.loop && el.muted)) return false;
    var r = el.getBoundingClientRect();
    if (r.width < MIN_W || r.height < MIN_H) return false;
    var area = r.width * r.height, container = null;
    for (var p = el.parentElement; p && p !== document.body && p !== document.documentElement; p = p.parentElement) {
      var pr = p.getBoundingClientRect();
      if (pr.width * pr.height > area * CONTAINER_SLACK) break;
      container = p;
    }
    if (container) container.setAttribute('data-axion-player', '');
    el.setAttribute('data-axion-own', '');
    el.__axOwn = true;
    return true;
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
      x: r.left, y: r.top, w: r.width, h: r.height,
      paused: el.paused, ended: el.ended, time: el.currentTime || 0,
      duration: isFinite(el.duration) ? el.duration : 0, live: el.duration === Infinity,
      volume: el.volume, muted: el.muted, rate: el.playbackRate, buffered: buffered,
      captions: captions, caption: caption, native: !!el.__axNative,
      own: (!!el.__axNative && REPLACE_NATIVE) || replaceSitePlayer(el),
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
      var msg = JSON.stringify({ t: 'state', players: list, dpr: window.devicePixelRatio, session: s });
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
