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
  // Sites whose media elements are calls and chats (Discord voice/video...), not media.
  // How long a paused element that left the document is still reported (ms).
  var KEEP = __AXION_KEEP__;
  var IGNORED = (function (host, sites) {
    return sites.some(function (s) { return host === s || host.slice(-s.length - 1) === '.' + s; });
  })(location.hostname, __AXION_IGNORED__);
  // Hover previews and tiles (see `isPreview`): whether they are told apart at all, and how
  // narrow (fraction of the viewport width) a controls-less, looping/autoplaying video must
  // be to count as one (0: no size rule).
  var DETECT_PREVIEWS = __AXION_DETECT_PREVIEWS__;
  var PREVIEW_MAX_WIDTH = __AXION_PREVIEW_MAX_WIDTH__;
  var HOST_IS = function (s) { return location.hostname === s || location.hostname.slice(-s.length - 1) === '.' + s; };
  var MARK = '\u0001axion:';
  var log = console.debug.bind(console);
  var isTop = window === window.top;
  var FRAME = Math.random().toString(36).slice(2, 8);
  var EVENTS = ['play', 'playing', 'pause', 'ended', 'timeupdate', 'volumechange', 'ratechange',
    'durationchange', 'seeked', 'loadedmetadata', 'emptied', 'progress', 'resize', 'waiting', 'stalled',
    'canplay', 'seeking', 'loadstart', 'error'];
  var seq = 0;
  var media = new Set();
  // Shadow roots are held weakly: sites like YouTube create thousands of them while the page
  // lives, and a strong list would keep every removed component's whole DOM alive (a leak that
  // ends in the renderer running out of memory after hours of watching).
  var shadowRefs = [];
  var WEAK = typeof WeakRef === 'function';
  var children = new Map();   // child window -> its players
  var childOf = new Map();    // player id -> child window
  var childSession = null;
  var takeover = null;        // {el, style, rootOverflow}
  var handlers = {};          // mediaSession action -> the page's handler
  var lastSent = '';
  var timer = 0;

  // Media inside shadow DOM (also closed roots) and media never put in the DOM (new Audio()).
  var attachShadow = Element.prototype.attachShadow;
  Element.prototype.attachShadow = function () {
    var root = attachShadow.apply(this, arguments);
    shadowRefs.push(WEAK ? new WeakRef(root) : root);
    return root;
  };
  var play = HTMLMediaElement.prototype.play;
  // After Axion paused the page's media on the user's behalf, a `play()` the page itself calls
  // within this long is not carried out: sites that start their element again as soon as it
  // pauses (to "keep playing") would otherwise undo the user's pause.
  var HOLD_MS = 1500;
  var holdUntil = 0;
  var ownPlay = false;
  HTMLMediaElement.prototype.play = function () {
    register(this);
    if (!ownPlay && Date.now() < holdUntil) return Promise.resolve();
    return play.apply(this, arguments);
  };

  // The page's track handlers (previoustrack / nexttrack) tell the browser whether Previous
  // and Next work, and are what they call.
  if (window.MediaSession && MediaSession.prototype.setActionHandler) {
    var setHandler = MediaSession.prototype.setActionHandler;
    MediaSession.prototype.setActionHandler = function (action, handler) {
      handlers[action] = handler || null;
      schedule();
      return setHandler.apply(this, arguments);
    };
  }

  // A live stream's position state (`setPositionState({duration: Infinity})`), and the media
  // sources the page attached to elements (`blob:` URL -> MediaSource), for `isLive`.
  var positionLive = false;
  if (window.MediaSession && MediaSession.prototype.setPositionState) {
    var setPosition = MediaSession.prototype.setPositionState;
    MediaSession.prototype.setPositionState = function (state) {
      positionLive = !!state && state.duration === Infinity;
      return setPosition.apply(this, arguments);
    };
  }
  var sources = new Map();
  if (window.MediaSource && URL.createObjectURL) {
    var createURL = URL.createObjectURL, revokeURL = URL.revokeObjectURL;
    URL.createObjectURL = function (obj) {
      var url = createURL.apply(this, arguments);
      try {
        if (obj instanceof MediaSource) {
          if (sources.size > 64) sources.delete(sources.keys().next().value);
          sources.set(url, WEAK ? new WeakRef(obj) : obj);
        }
      } catch (e) {}
      return url;
    };
    URL.revokeObjectURL = function (url) {
      sources.delete(url);
      return revokeURL.apply(this, arguments);
    };
  }

  var style = document.createElement('style');
  // The video that covers the page in a takeover (stylesheet rules with !important: a site's
  // own resize logic keeps setting plain inline `width`/`top`..., which replace our inline
  // declarations but never an !important rule), and the ancestors that must not trap a
  // `position:fixed` child in their own box (transform, contain, filter... make it relative).
  var COVER =
    'position:fixed!important;inset:0!important;top:0!important;left:0!important;right:0!important;bottom:0!important;' +
    'width:100vw!important;height:100vh!important;min-width:0!important;min-height:0!important;' +
    'max-width:none!important;max-height:none!important;margin:0!important;padding:0!important;border:0!important;' +
    'transform:none!important;translate:none!important;rotate:none!important;scale:none!important;' +
    'z-index:2147483647!important;background:#000!important;object-fit:contain!important;opacity:1!important';
  var FREE =
    'transform:none!important;translate:none!important;rotate:none!important;scale:none!important;' +
    'contain:none!important;content-visibility:visible!important;container-type:normal!important;' +
    'filter:none!important;backdrop-filter:none!important;perspective:none!important;will-change:auto!important;' +
    'clip-path:none!important;mask:none!important;opacity:1!important;offset-path:none!important';
  function takeoverRules(prefix) {
    return prefix + '[data-axion-takeover]{' + COVER + '}' + prefix + '[data-axion-anc]{' + FREE + '}';
  }
  // Takeover hides everything but the video: `visibility` (unlike z-index) is not trapped by
  // the site's stacking contexts, and hidden elements take no clicks.
  style.textContent =
    'html.axion-takeover,html.axion-takeover body{background:#000!important}' +
    'html.axion-takeover *{visibility:hidden!important}' +
    'html.axion-takeover [data-axion-takeover]{visibility:visible!important}' +
    takeoverRules('html.axion-takeover ') +
    'html.axion-takeover video::-webkit-media-controls{display:none!important}' +
    // A fullscreen <video> shows Axion's player, not the built-in controls.
    (OWN_FULLSCREEN ? 'video:fullscreen::-webkit-media-controls{display:none!important}' : '');
  function addStyle() {
    var root = document.head || document.documentElement;
    if (!root) return false;
    root.appendChild(style);
    return true;
  }
  if (!addStyle()) document.addEventListener('DOMContentLoaded', addStyle);

  // The live shadow roots; the ones the page let go of are dropped from the list.
  function shadowRoots() {
    var live = [], keep = [];
    for (var i = 0; i < shadowRefs.length; i++) {
      var root = WEAK ? shadowRefs[i].deref() : shadowRefs[i];
      if (!root) continue;
      keep.push(shadowRefs[i]);
      live.push(root);
    }
    // Without WeakRef, roots whose host left the document are dropped (media in them is found
    // again by `play()`).
    if (!WEAK) {
      keep = []; live = live.filter(function (r) { var ok = r.host && r.host.isConnected; if (ok) keep.push(r); return ok; });
    }
    shadowRefs = keep;
    return live;
  }

  // A call's or camera's live stream (WebRTC, getUserMedia) is not something to play, list or
  // control as media: such elements are left alone, wherever they appear.
  function isCall(el) {
    if (IGNORED) return true;
    try {
      return !!el.srcObject && typeof MediaStream !== 'undefined' && el.srcObject instanceof MediaStream;
    } catch (e) { return false; }
  }

  function register(el) {
    if (IGNORED) return;
    // The start page's live wallpaper is decoration, not media: no player, no "playing" tab,
    // no Now Playing entry, and it never keeps the tab from freezing.
    if (el.hasAttribute && el.hasAttribute('data-axion-wallpaper')) return;
    if (media.has(el)) return;
    media.add(el);
    el.__axId = FRAME + ':' + (++seq);
    // A `new Audio()` never put in the document: it is the player, and stays reported (paused
    // too) for as long as it has a source.
    el.__axDetached = !el.isConnected;
    if (el.tagName === 'VIDEO' && el.hasAttribute('controls')) el.__axNative = true;
    EVENTS.forEach(function (t) { el.addEventListener(t, schedule, true); });
    schedule();
  }

  function scan() {
    [document].concat(shadowRoots()).forEach(function (root) {
      if (root.querySelectorAll) root.querySelectorAll('video,audio').forEach(register);
    });
  }

  function frames() {
    var out = [];
    [document].concat(shadowRoots()).forEach(function (root) {
      if (root.querySelectorAll) root.querySelectorAll('iframe,frame').forEach(function (f) { out.push(f); });
    });
    return out;
  }
  function frameOf(win) {
    var all = frames();
    for (var i = 0; i < all.length; i++) if (all[i].contentWindow === win) return all[i];
    return null;
  }

  // A live stream: an endless duration, or (in Axion's own media view) a live HLS playlist,
  // whose duration hls.js otherwise reports as the length of the window it holds.
  function isLive(el) {
    if (el.duration === Infinity) return true;
    try {
      var h = window.__axionHls;
      if (h && h.media === el && h.levels && h.levels[h.currentLevel] && h.levels[h.currentLevel].details) {
        return !!h.levels[h.currentLevel].details.live;
      }
    } catch (e) {}
    try { return siteLive(el); } catch (e) { return false; }
  }

  // Live streams whose element reports a finite duration: YouTube's DVR streams, Twitch's
  // live indicator, MSE sources whose duration is endless, a position state without an end,
  // and a seekable window that slides along while playing.
  function visible(node) { return !!node && getComputedStyle(node).display !== 'none'; }
  function siteLive(el) {
    if (el.tagName !== 'VIDEO' && el.tagName !== 'AUDIO') return false;
    if (HOST_IS('youtube.com')) {
      var yt = el.closest('.html5-video-player');
      if (yt) {
        // An ad in front of a stream (finite, and nothing to go live on) is not the stream.
        if (yt.classList.contains('ad-showing')) return false;
        var data = typeof yt.getVideoData === 'function' ? yt.getVideoData() : null;
        if (data && data.isLive) return true;
        if (yt.classList.contains('ytp-live') || visible(yt.querySelector('.ytp-live-badge'))) return true;
        if (data) return false;
      }
    }
    if (HOST_IS('twitch.tv')) {
      var tw = el.closest('[data-a-target="video-player"],.video-player');
      if (tw && visible(tw.querySelector('.live-indicator-container,[data-a-target="player-overlay-live-indicator"]'))) return true;
    }
    if (positionLive && !el.paused) return true;
    var src = el.currentSrc || el.src;
    var ref = src && sources.get(src);
    var ms = ref && (WEAK ? ref.deref() : ref);
    if (ms && ms.duration === Infinity) return true;
    // The seekable window of a finite-duration stream moves while it plays: its start
    // creeps up (a recording's stays put).
    if (isFinite(el.duration) && el.seekable && el.seekable.length) {
      var start = el.seekable.start(0), w = el.__axWindow;
      if (!w || w.src !== src) w = el.__axWindow = { src: src, start: start, at: Date.now(), live: false };
      else if (!w.live && !el.paused && start - w.start > 5 && Date.now() - w.at > 3000 &&
        el.seekable.end(el.seekable.length - 1) > el.duration - 30) w.live = true;
      return w.live;
    }
    return false;
  }

  // Hover previews and autoplaying tiles (YouTube's home page and result grids, Twitch's
  // directory): they are not what the Now Playing card, the desktop's media controls or the
  // "playing" mark of a tab are about. Told apart by where they sit (known preview
  // containers, YouTube's pages other than the player's), by being small controls-less
  // looping/autoplaying tiles, and (without a media session) by being muted without controls.
  // Never one that Axion's own view plays, one the user touched through Axion's controls,
  // and for the loose rules one that was heard (audible and playing). Real playback is kept:
  // the player of a watch page, Shorts, YouTube's miniplayer.
  var PREVIEW_PLACES = 'ytd-video-preview,#inline-preview-player,ytd-thumbnail,ytd-rich-item-renderer,' +
    'ytd-grid-video-renderer,ytd-compact-video-renderer,ytd-playlist-thumbnail,ytd-moving-thumbnail-renderer,' +
    'ytd-playlist-video-renderer,ytd-video-renderer,[data-preview],.ytp-preview,' +
    '[data-a-target="video-preview"],[data-test-selector="video-preview"],[data-a-target="preview-card-image-link"]';
  var REAL_PLAYERS = 'ytd-reel-video-renderer,ytd-shorts,ytd-miniplayer,ytd-watch-flexy,ytd-watch-grid,[data-axion-takeover]';
  // `closest` across shadow roots.
  function closestDeep(el, selector) {
    for (var n = el; n; ) {
      var found = n.closest && n.closest(selector);
      if (found) return found;
      var root = n.getRootNode && n.getRootNode();
      n = root && root.host;
    }
    return null;
  }
  function isPreview(el) {
    if (!DETECT_PREVIEWS || location.protocol === 'axion:' || el.__axTouched) return false;
    // The page's main player never sits inside a thumbnail or list item, so this holds even
    // for a preview that was unmuted or that the page gave a media session.
    if (closestDeep(el, PREVIEW_PLACES)) return true;
    if ((HOST_IS('youtube.com') && !HOST_IS('music.youtube.com')) && !closestDeep(el, REAL_PLAYERS) &&
      !/^\/(watch|shorts|live|embed|clip)(\/|$)/.test(location.pathname)) return true;
    if (!el.muted && !el.paused && el.volume > 0) el.__axOwn = true;
    if (el.__axOwn) return false;
    var meta = navigator.mediaSession && navigator.mediaSession.metadata;
    if (!!el.muted && !meta && !el.controls) return true;
    if (el.controls) return false;
    // A muted, looping or autoplaying inline tile inside a link (a card).
    if (el.muted && (el.loop || el.autoplay) && el.hasAttribute('playsinline') && el.closest('a[href]')) return true;
    // A small controls-less video that loops or autoplays, whatever the page's media session.
    if (PREVIEW_MAX_WIDTH > 0 && el.tagName === 'VIDEO' && !meta && (el.loop || el.autoplay) && !document.fullscreenElement) {
      var r = el.getBoundingClientRect();
      if (r.width > 0 && r.width < PREVIEW_MAX_WIDTH * window.innerWidth) return true;
    }
    return false;
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
      duration: isFinite(el.duration) ? el.duration : 0, live: isLive(el),
      preview: isPreview(el),
      loading: !el.ended && !el.error && el.readyState < 3 && (!el.paused || el.networkState === 2),
      volume: el.volume, muted: el.muted, rate: el.playbackRate, buffered: buffered,
      captions: captions, caption: caption, native: !!el.__axNative,
      trusted: TRUSTED,
      src: el.currentSrc || el.src || '', poster: el.poster || '', frame: location.href,
      fullscreen: !!document.fullscreenElement &&
        (document.fullscreenElement === el || document.fullscreenElement.contains(el)) || (takeover && takeover.el === el) || false
    };
  }

  function session() {
    var m = navigator.mediaSession && navigator.mediaSession.metadata;
    var prev = !!handlers.previoustrack, next = !!handlers.nexttrack;
    if (childSession) { prev = prev || childSession.prev; next = next || childSession.next; }
    if (!m) {
      if (!childSession && !prev && !next) return null;
      var c = childSession || { title: '', artist: '', album: '', artwork: '' };
      return { title: c.title, artist: c.artist, album: c.album, artwork: c.artwork, prev: prev, next: next };
    }
    var art = '';
    if (m.artwork && m.artwork.length) {
      try { art = new URL(m.artwork[m.artwork.length - 1].src, document.baseURI).href; } catch (e) {}
    }
    return { title: m.title || '', artist: m.artist || '', album: m.album || '', artwork: art, prev: prev, next: next };
  }

  // The page's own preview picture (Open Graph / Twitter card), for the Playlist's thumbnail.
  function pageImage() {
    var m = document.querySelector('meta[property="og:image"],meta[name="og:image"],meta[name="twitter:image"],meta[property="twitter:image"]');
    var url = m && m.getAttribute('content');
    if (!url) return '';
    try { return new URL(url, document.baseURI).href; } catch (e) { return ''; }
  }

  function schedule() { if (!timer) timer = setTimeout(report, 120); }
  function report() {
    timer = 0;
    var list = [];
    media.forEach(function (el) {
      if (isCall(el)) return;
      // Nothing loaded (a page that cleared its element when the track or the page changed):
      // there is nothing to show or control.
      if (el.paused && !el.currentSrc && !el.src && !el.srcObject) return;
      if (el.isConnected) { el.__axGoneAt = 0; el.__axDetached = false; list.push(describe(el)); return; }
      if (!el.paused) { list.push(describe(el)); return; }
      var hasSource = !!(el.currentSrc || el.src || el.srcObject);
      if (el.__axDetached && hasSource) { list.push(describe(el)); return; }
      // Left the document while paused (a site swapping its element): kept for a while, so
      // the card stays on its play button until the page's new element takes over.
      var now = Date.now();
      if (!el.__axGoneAt) el.__axGoneAt = now;
      if (now - el.__axGoneAt < KEEP) { list.push(describe(el)); return; }
      // Really gone: forgotten (a later play() registers it again).
      media.delete(el);
    });
    children.forEach(function (players, win) {
      var f = frameOf(win);
      if (!f) {
        players.forEach(function (p) { childOf.delete(p.id); });
        children.delete(win);
        return;
      }
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

  // Tells the site its viewport is the whole window now (or again), so it lays out for it.
  function relayout() {
    var fire = function () { try { window.dispatchEvent(new Event('resize')); } catch (e) {} };
    setTimeout(fire, 0);
    setTimeout(fire, 300);
  }

  // The video covers the whole window: by stylesheet rules (!important, so the site's own
  // inline `width`/`top`... from its resize logic cannot undo them), with the ancestors that
  // would trap `position:fixed` freed of transform/contain/filter, in every shadow root on the
  // way up. Everything is restored on exit.
  function setTakeover(el, on) {
    if (on && !takeover) {
      takeover = { el: el, style: el.getAttribute('style'), overflow: document.documentElement.style.overflow,
        ancestors: [], styles: [] };
      var roots = new Set();
      var addRootStyle = function (root) {
        if (root === document || roots.has(root)) return;
        roots.add(root);
        var s = document.createElement('style');
        s.textContent = takeoverRules('');
        root.appendChild(s);
        takeover.styles.push(s);
      };
      el.setAttribute('data-axion-takeover', '');
      var node = el;
      while (node) {
        var root = node.getRootNode && node.getRootNode();
        if (root && root !== document && root.host) addRootStyle(root);
        var up = node.parentNode;
        if (!up || up.nodeType === 9) break;
        if (up.nodeType === 11) up = up.host;
        if (!up || up === document.documentElement) break;
        up.setAttribute('data-axion-anc', '');
        takeover.ancestors.push(up);
        node = up;
      }
      // Inline copy of the cover as a fallback (a page's CSP may refuse the stylesheet).
      el.style.cssText += ';' + COVER;
      document.documentElement.classList.add('axion-takeover');
      document.documentElement.style.overflow = 'hidden';
      relayout();
    } else if (!on && takeover) {
      if (takeover.style === null) takeover.el.removeAttribute('style'); else takeover.el.setAttribute('style', takeover.style);
      takeover.el.removeAttribute('data-axion-takeover');
      takeover.ancestors.forEach(function (a) { a.removeAttribute('data-axion-anc'); });
      takeover.styles.forEach(function (s) { if (s.parentNode) s.parentNode.removeChild(s); });
      document.documentElement.classList.remove('axion-takeover');
      document.documentElement.style.overflow = takeover.overflow;
      takeover = null;
      relayout();
    }
  }

  function apply(el, c) {
    var v = c.value;
    // The user's volume controls: this is their video now, not a hover preview.
    if (c.action === 'volume' || c.action === 'mute') el.__axTouched = true;
    switch (c.action) {
      case 'play': holdUntil = 0; ownPlay = true; try { el.play(); } finally { ownPlay = false; } break;
      case 'pause': el.pause(); break;
      case 'stop':
        // Stop: paused, back at the start. A live stream has no start: it just pauses.
        el.pause();
        if (!isLive(el) && isFinite(el.duration)) { try { el.currentTime = 0; } catch (e) {} }
        break;
      case 'toggle':
        if (el.paused || el.ended) { holdUntil = 0; ownPlay = true; try { el.play(); } finally { ownPlay = false; } }
        else pauseAll(1);
        break;
      case 'seek': el.currentTime = Math.max(0, v); break;
      case 'skip': el.currentTime = Math.max(0, el.currentTime + v); break;
      case 'volume': el.volume = Math.min(1, Math.max(0, v)); if (v > 0) el.muted = false; break;
      case 'mute': el.muted = !!v; break;
      case 'rate': el.playbackRate = v; break;
      case 'caption':
        for (var i = 0; i < el.textTracks.length; i++) el.textTracks[i].mode = (i === v) ? 'showing' : 'disabled';
        break;
      case 'takeover': setTakeover(el, !!v); break;
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

  // Pauses what plays in this frame and, by message, in the child frames; then looks again
  // after a moment: sites start their element again, or swap it for a new one, and a pause
  // that did not stick is repeated, then asked of the page's own `pause` handler.
  function pauseAll(depth) {
    var n = 0;
    holdUntil = Date.now() + HOLD_MS;
    media.forEach(function (el) { if (!el.paused) { n++; try { el.pause(); } catch (e) {} } });
    children.forEach(function (players, win) {
      try { win.postMessage({ s: SECRET, k: 'cmd', cmd: { id: '*', action: 'pauseall' } }, '*'); } catch (e) {}
    });
    if (depth > 1) return n;
    setTimeout(function () {
      scan();
      var still = 0;
      media.forEach(function (el) { if (!el.paused && !isCall(el)) { still++; try { el.pause(); } catch (e) {} } });
      if (still && handlers.pause) { try { handlers.pause({ action: 'pause' }); } catch (e) {} }
      if (isTop) log(MARK + JSON.stringify({ t: 'probe', event: 'pauseall', frame: location.href, paused: n, still: still,
        handler: !!handlers.pause }));
      schedule();
    }, 200);
    return n;
  }

  function cmd(c) {
    if (c.action === 'pauseall') { pauseAll(1); schedule(); return; }
    if (c.action === 'prev' || c.action === 'next') {
      var name = c.action === 'prev' ? 'previoustrack' : 'nexttrack';
      var h = handlers[name];
      if (h) { try { h({ action: name }); } catch (e) {} schedule(); return; }
      var w = childOf.get(c.id);
      if (w) w.postMessage({ s: SECRET, k: 'cmd', cmd: c }, '*');
      return;
    }
    var el = find(c.id);
    if (el) {
      apply(el, c);
      if (isTop) {
        setTimeout(function () {
          log(MARK + JSON.stringify({ t: 'probe', event: 'cmd-result', action: c.action, id: c.id, found: true,
            paused: el.paused, connected: el.isConnected, time: el.currentTime }));
        }, 250);
      }
      return;
    }
    var win = childOf.get(c.id);
    if (!win) {
      if (isTop) log(MARK + JSON.stringify({ t: 'probe', event: 'cmd-result', action: c.action, id: c.id, found: false }));
      return;
    }
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
      var before = children.get(e.source) || [];
      var now = {};
      d.players.forEach(function (p) { now[p.id] = true; childOf.set(p.id, e.source); });
      before.forEach(function (p) { if (!now[p.id]) childOf.delete(p.id); });
      children.set(e.source, d.players);
      if (d.session) childSession = d.session;
      schedule();
    } else if (d.k === 'event') {
      event(d.event);
    }
  });

  // Single-page sites change the page without loading one: what the old page had removed
  // (and left paused) is gone for good then, with no grace period.
  function navigated() {
    media.forEach(function (el) { if (!el.isConnected && !el.__axDetached) media.delete(el); });
    schedule();
  }
  ['pushState', 'replaceState'].forEach(function (name) {
    var original = history[name];
    history[name] = function () {
      var result = original.apply(this, arguments);
      setTimeout(navigated, 0);
      return result;
    };
  });
  window.addEventListener('popstate', navigated);
  window.addEventListener('hashchange', navigated);
  window.addEventListener('yt-navigate-finish', navigated);

  document.addEventListener('fullscreenchange', function () {
    if (!document.fullscreenElement) setTakeover(null, false);
    schedule();
  });
  window.addEventListener('scroll', schedule, { passive: true, capture: true });
  window.addEventListener('resize', schedule);
  window.addEventListener('pagehide', function () {
    media.clear(); children.clear(); childOf.clear();
    if (timer) clearTimeout(timer);
    report();
  });
  document.addEventListener('DOMContentLoaded', scan);
  setInterval(function () { scan(); schedule(); }, 2000);
  scan();

  Object.defineProperty(window, '__axionMedia', { value: Object.freeze({ cmd: cmd }) });
})();
