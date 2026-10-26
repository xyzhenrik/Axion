// Axion's start page. Data comes from the browser (`/api/state`), what the user does goes back
// as messages (`/api/message`); both endpoints only answer this page (see home.rs). Everything
// from history is shown with textContent, never parsed as HTML.
"use strict";

(() => {
  const $ = (id) => document.getElementById(id);
  let state = null;
  let page = 0;
  let editing = null; // the site being edited, or {} for a new one
  let refreshTimer = 0;

  // ---- talking to the browser ----

  const post = (message) =>
    fetch("/api/message", { method: "POST", body: JSON.stringify(message) })
      .catch(() => {})
      .finally(() => {
        // The browser also asks us to refresh; this is the fallback.
        clearTimeout(refreshTimer);
        refreshTimer = setTimeout(refresh, 250);
      });

  async function refresh() {
    try {
      const response = await fetch("/api/state", { cache: "no-store" });
      if (!response.ok) {
        // Refused while the page was still being committed: ask again shortly.
        if (!state) setTimeout(refresh, 500);
        return;
      }
      state = await response.json();
      render();
    } catch (_) {
      // Not served (e.g. opened outside Axion): stay as we are.
    }
  }

  window.axionHome = { refresh };

  // ---- translation ----

  // The browser sends the current language's translations of this page's texts with its state
  // (`state.i18n`, keyed by the English text). Untranslated text stays English.
  const t = (text, args) => {
    const known = state && state.i18n && state.i18n[text];
    let out = typeof known === "string" ? known : text;
    if (args) out = out.replace(/\{(\w+)\}/g, (m, name) => (name in args ? String(args[name]) : m));
    return out;
  };

  const lang = () => (state && state.lang) || "en";

  // Numbers in the page language's format.
  const num = (value, digits = 0) =>
    value.toLocaleString(lang(), { minimumFractionDigits: digits, maximumFractionDigits: digits });

  // Static text: elements marked `data-t` (their text) and `data-t-attrs` (attributes whose
  // values are the English texts).
  let domTranslated = false;
  function translateDom() {
    if (domTranslated || !state) return;
    domTranslated = true;
    document.documentElement.lang = lang();
    if (!state.i18n) return;
    document.querySelectorAll("[data-t]").forEach((node) => {
      node.textContent = t(node.textContent.trim());
    });
    document.querySelectorAll("[data-t-attrs]").forEach((node) => {
      for (const attr of node.dataset.tAttrs.split(",")) {
        const name = attr.trim();
        if (node.hasAttribute(name)) node.setAttribute(name, t(node.getAttribute(name)));
      }
    });
  }

  // ---- helpers ----

  const el = (tag, className, text) => {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  };

  const svg = (paths, size = 14) => {
    const ns = "http://www.w3.org/2000/svg";
    const root = document.createElementNS(ns, "svg");
    root.setAttribute("viewBox", "0 0 24 24");
    root.setAttribute("width", size);
    root.setAttribute("height", size);
    root.setAttribute("aria-hidden", "true");
    for (const d of paths) {
      const path = document.createElementNS(ns, "path");
      path.setAttribute("class", "i");
      path.setAttribute("d", d);
      root.appendChild(path);
    }
    return root;
  };

  const ICON = {
    close: ["M7 7l10 10M17 7L7 17"],
    pin: ["M9 4h6l-1 6 3 3H7l3-3zM12 13v7"],
    edit: ["M4 20h4L19 9l-4-4L4 16zM14 6l4 4"],
    plus: ["M12 5v14M5 12h14"],
    play: ["M8 5l11 7-11 7z"],
  };

  // A range input's filled part (the accent-coloured track left of the thumb).
  const paintRange = (input) => {
    const min = Number(input.min) || 0;
    const max = Number(input.max) || 1;
    const p = ((Number(input.value) - min) / (max - min || 1)) * 100;
    input.style.setProperty("--p", `${Math.max(0, Math.min(100, p))}%`);
  };

  // A stable, pleasant colour per host for monograms.
  const hue = (text) => {
    let h = 0;
    for (const c of text) h = (h * 31 + c.codePointAt(0)) % 360;
    return h;
  };

  const monogram = (node, host, title) => {
    const name = (title || host || "?").replace(/^www\./, "").trim();
    node.textContent = (name[0] || "?").toUpperCase();
    node.style.background = `hsl(${hue(host || name)} 62% 52%)`;
  };

  // The site's cached icon from the browser, or a monogram while it has none. Every render
  // rebuilds the tiles, so loaded icons are kept per host: a rebuilt tile shows its icon at
  // once (never the monogram in between), and a newer icon replaces the old one only once it
  // has loaded.
  const iconCache = new Map(); // host -> { img, version, pending }
  const iconNodes = new Map(); // host -> Set of nodes showing it

  const showIcon = (node, img) => {
    const copy = img.cloneNode(false);
    copy.decoding = "sync";
    node.textContent = "";
    node.style.background = "transparent";
    node.appendChild(copy);
  };

  const siteIcon = (node, host, title) => {
    monogram(node, host, title);
    if (!host) return;
    if (!iconNodes.has(host)) iconNodes.set(host, new Set());
    const nodes = iconNodes.get(host);
    for (const n of nodes) if (!n.isConnected) nodes.delete(n);
    nodes.add(node);
    const version = state ? state.icons : 0;
    let entry = iconCache.get(host);
    if (entry && entry.img) showIcon(node, entry.img);
    if (entry && (entry.version === version || entry.pending === version)) return;
    if (!entry) {
      entry = { img: null, version: -1, pending: null };
      iconCache.set(host, entry);
    }
    entry.pending = version;
    const img = new Image();
    img.alt = "";
    img.onload = () => {
      entry.img = img;
      entry.version = version;
      entry.pending = null;
      for (const n of nodes) if (n.isConnected) showIcon(n, img);
    };
    img.onerror = () => {
      // No icon (yet): ask again only when icons changed.
      entry.version = version;
      entry.pending = null;
    };
    img.src = `/api/favicon?host=${encodeURIComponent(host)}&v=${version}`;
  };

  const formatCount = (n) => num(Math.round(n));

  const formatBytes = (b) => {
    const units = [t("bytes"), "KB", "MB", "GB", "TB"];
    let i = 0;
    while (b >= 1024 && i < units.length - 1) {
      b /= 1024;
      i += 1;
    }
    return i === 0 ? `${num(Math.round(b))} ${units[0]}` : `${b < 10 ? num(b, 1) : num(Math.round(b))} ${units[i]}`;
  };

  const formatSeconds = (s) => {
    if (s < 60) return t("{n} s", { n: num(Math.round(s)) });
    if (s < 3600) return t("{n} min", { n: num(Math.round(s / 60)) });
    if (s < 86400) {
      const h = s / 3600;
      return t("{n} h", { n: h < 10 ? num(h, 1) : num(Math.round(h)) });
    }
    const d = s / 86400;
    return t("{n} days", { n: d < 10 ? num(d, 1) : num(Math.round(d)) });
  };

  // ---- clock and greeting ----

  function tick() {
    if (!state) return;
    const now = new Date();
    const clock = $("clock");
    const minutes = String(now.getMinutes()).padStart(2, "0");
    clock.textContent = "";
    if (state.clock.h24) {
      clock.textContent = `${String(now.getHours()).padStart(2, "0")}:${minutes}`;
    } else {
      const h = now.getHours() % 12 || 12;
      clock.append(`${h}:${minutes}`, el("span", "ampm", now.getHours() < 12 ? t("AM") : t("PM")));
    }
    const hour = now.getHours();
    const name = state.greeting.name;
    let greeting;
    if (hour >= 5 && hour < 12) greeting = name ? t("Good morning, {name}", { name }) : t("Good morning");
    else if (hour >= 12 && hour < 18) greeting = name ? t("Good afternoon, {name}", { name }) : t("Good afternoon");
    else greeting = name ? t("Good evening, {name}", { name }) : t("Good evening");
    $("greeting").textContent = greeting;
  }

  function startClock() {
    tick();
    // Tick on the minute boundary, then every 1 s keeps it exact after sleep.
    setInterval(tick, 1000);
  }

  // ---- search ----

  function engineName() {
    const s = state.search;
    return s.current >= 0 ? s.engines[s.current].name : s.customHost || t("your search engine");
  }

  function renderSearch() {
    const s = state.search;
    $("search").hidden = !s.show;
    $("query").placeholder = t("Search with {engine} or enter website", { engine: engineName() });
    const host = s.current >= 0 ? s.engines[s.current].host : s.customHost;
    const icon = $("engine-icon");
    icon.textContent = "";
    icon.style.background = "";
    siteIcon(icon, host, engineName());
    $("engine").setAttribute("aria-label", t("Search engine: {name}", { name: engineName() }));
    const menu = $("engine-menu");
    menu.textContent = "";
    s.engines.forEach((engine, index) => {
      const item = el("button");
      item.type = "button";
      item.setAttribute("role", "menuitemradio");
      item.setAttribute("aria-checked", String(index === s.current));
      const mark = el("span", "mark");
      siteIcon(mark, engine.host, engine.name);
      item.append(mark, el("span", "", engine.name));
      if (index === s.current) item.append(el("span", "check", "✓"));
      item.addEventListener("click", () => {
        closeMenu();
        state.search.current = index;
        renderSearch();
        post({ type: "engine", index });
        $("query").focus();
      });
      menu.appendChild(item);
    });
  }

  function closeMenu() {
    $("engine-menu").hidden = true;
    $("engine").setAttribute("aria-expanded", "false");
  }

  // ---- top sites ----

  function columns() {
    const width = Math.min(880, window.innerWidth - 48);
    return Math.max(2, Math.min(state.topSites.columns, Math.floor(width / 118)));
  }

  function tile(site) {
    const a = el("a", "tile");
    a.href = site.url;
    a.title = site.title ? `${site.title} - ${site.host}` : site.host;
    const face = el("span", "face glass");
    const icon = el("span", "icon");
    siteIcon(icon, site.host, site.title);
    face.appendChild(icon);
    a.append(face, el("span", "name", site.title || site.host));
    if (site.pinned) a.appendChild(el("span", "pin-dot"));
    const actions = el("span", "actions");
    const action = (label, paths, handler) => {
      const b = el("button");
      b.type = "button";
      b.setAttribute("aria-label", label);
      b.title = label;
      b.appendChild(svg(paths, 12));
      b.addEventListener("click", (e) => {
        e.preventDefault();
        e.stopPropagation();
        handler();
      });
      actions.appendChild(b);
    };
    action(site.pinned ? t("Unpin") : t("Pin"), ICON.pin, () =>
      post(site.pinned ? { type: "unpinSite", url: site.url } : { type: "pinSite", url: site.url, title: site.title }),
    );
    action(t("Edit"), ICON.edit, () => openEditor(site));
    action(t("Remove"), ICON.close, () => post({ type: "removeSite", url: site.url }));
    a.appendChild(actions);
    return a;
  }

  function addTile() {
    const b = el("button", "tile add");
    b.type = "button";
    const face = el("span", "face glass");
    face.appendChild(svg(ICON.plus, 26));
    b.append(face, el("span", "name", t("Add Site")));
    b.addEventListener("click", () => openEditor(null));
    return b;
  }

  function renderSites() {
    const top = state.topSites;
    $("sites").hidden = !top.show;
    const restore = $("restore-row");
    restore.hidden = !(top.hidden > 0);
    $("restore-label").textContent =
      top.hidden === 1 ? t("{n} removed site", { n: top.hidden }) : t("{n} removed sites", { n: top.hidden });
    if (!top.show) return;
    const cols = columns();
    const perPage = cols * top.rows;
    const items = top.sites.map(tile);
    // Room left in the count: a tile to add a site.
    if (top.sites.length < (top.count || top.sites.length + 1)) items.push(addTile());
    const pages = Math.max(1, Math.ceil(items.length / perPage));
    page = Math.min(page, pages - 1);
    const container = $("pages");
    container.textContent = "";
    const track = el("div", "pages-track");
    for (let p = 0; p < pages; p++) {
      const grid = el("div", "page");
      grid.style.setProperty("--columns", pages === 1 ? Math.max(1, Math.min(cols, items.length)) : cols);
      items.slice(p * perPage, (p + 1) * perPage).forEach((item) => grid.appendChild(item));
      track.appendChild(grid);
    }
    track.style.transform = `translateX(${-100 * page}%)`;
    container.appendChild(track);
    const dots = $("dots");
    dots.textContent = "";
    dots.hidden = pages < 2;
    for (let p = 0; p < pages; p++) {
      const dot = el("button");
      dot.type = "button";
      dot.setAttribute("role", "tab");
      dot.setAttribute("aria-label", t("Page {n}", { n: p + 1 }));
      dot.setAttribute("aria-selected", String(p === page));
      dot.addEventListener("click", () => showPage(p));
      dots.appendChild(dot);
    }
  }

  function showPage(p) {
    const track = document.querySelector(".pages-track");
    const count = track ? track.children.length : 1;
    page = Math.max(0, Math.min(count - 1, p));
    if (track) track.style.transform = `translateX(${-100 * page}%)`;
    document.querySelectorAll("#dots button").forEach((d, i) => d.setAttribute("aria-selected", String(i === page)));
  }

  // Horizontal swipes (touchpad) flip pages.
  let swipe = 0;
  let swipeReset = 0;
  $("pages").addEventListener(
    "wheel",
    (e) => {
      if (Math.abs(e.deltaX) <= Math.abs(e.deltaY)) return;
      e.preventDefault();
      swipe += e.deltaX;
      clearTimeout(swipeReset);
      swipeReset = setTimeout(() => (swipe = 0), 200);
      if (Math.abs(swipe) > 80) {
        showPage(page + Math.sign(swipe));
        swipe = 0;
        clearTimeout(swipeReset);
        swipeReset = setTimeout(() => (swipe = 0), 400);
      }
    },
    { passive: false },
  );

  function openEditor(site) {
    editing = site || {};
    $("edit-title").textContent = site ? t("Edit Site") : t("Add Site");
    $("edit-name").value = site ? site.title || "" : "";
    $("edit-url").value = site ? site.url : "";
    $("edit").showModal();
    $(site ? "edit-name" : "edit-url").focus();
  }

  $("edit-form").addEventListener("submit", (e) => {
    e.preventDefault();
    const url = $("edit-url").value.trim();
    if (!url) return;
    post({ type: "editSite", old: editing && editing.url ? editing.url : "", url, title: $("edit-name").value.trim() });
    $("edit").close();
  });
  $("edit-cancel").addEventListener("click", () => $("edit").close());

  // ---- Shields ----

  function renderStats() {
    const s = state.stats;
    $("stats").hidden = !s.show;
    $("blocked").textContent = formatCount(s.blocked);
    $("bandwidth").textContent = t("≈ {value}", { value: formatBytes(s.bytes) });
    $("time").textContent = t("≈ {value}", { value: formatSeconds(s.seconds) });
    fitStats();
  }

  // The stats row is pinned to the viewport bottom; hide it if the content reaches into it.
  function fitStats() {
    const el = $("stats");
    el.classList.remove("crowded");
    if (el.hidden) return;
    const top = el.getBoundingClientRect().top - 12;
    const last = [...document.querySelectorAll("main > *")]
      .filter((n) => n !== el && !n.hidden && n.offsetParent !== null)
      .reduce((m, n) => Math.max(m, n.getBoundingClientRect().bottom), 0);
    el.classList.toggle("crowded", last > top);
  }

  // ---- background ----

  // A look slider (dim, blur) is being dragged.
  let lookHeld = false;

  // The mini picker: "none", a thumbnail per background, "add". Rebuilt only when the list
  // or the selection changed, so the thumbnails don't reload.
  let pickerKey = "";

  function selectBackground(name) {
    if (state.background.selected === name) return;
    state.background.selected = name;
    state.background.show = !!name;
    renderBackground();
    post({ type: "selectBackground", name });
  }

  function renderPicker() {
    const b = state.background;
    const key = JSON.stringify([b.list, b.kinds, b.needsThumb, b.selected, b.canAdd]);
    if (key === pickerKey) return;
    pickerKey = key;
    const picker = $("bg-picker");
    picker.textContent = "";
    const swatch = (name, label) => {
      const item = el("div", "bg-item");
      const button = el("button", name ? "bg-swatch" : "bg-swatch none");
      button.type = "button";
      button.setAttribute("role", "radio");
      button.setAttribute("aria-checked", String(b.selected === name));
      button.setAttribute("aria-label", label);
      button.title = label;
      if (name) {
        const kind = b.kinds[b.list.indexOf(name)] || "image";
        // A video's thumbnail is its first frame, made by this page (see makeVideoThumbs).
        if (b.needsThumb.includes(name)) {
          button.classList.add("pending");
        } else {
          const img = new Image();
          img.alt = "";
          img.decoding = "async";
          img.src = `/api/thumb?name=${encodeURIComponent(name)}`;
          button.appendChild(img);
        }
        if (kind !== "image") {
          const badge = el("span", "live-badge");
          badge.appendChild(svg(ICON.play, 8));
          button.appendChild(badge);
          button.title = kind === "video" ? t("{label} (video)", { label }) : t("{label} (animated)", { label });
        }
      } else {
        button.appendChild(el("span", "none-mark"));
      }
      button.addEventListener("click", () => selectBackground(name));
      item.appendChild(button);
      if (name) {
        const remove = el("button", "bg-remove");
        remove.type = "button";
        remove.setAttribute("aria-label", t("Remove background"));
        remove.title = t("Remove");
        remove.appendChild(svg(ICON.close, 10));
        remove.addEventListener("click", (e) => {
          e.stopPropagation();
          post({ type: "removeBackground", name });
        });
        item.appendChild(remove);
      }
      return item;
    };
    picker.appendChild(swatch("", t("No background")));
    b.list.forEach((name, i) => picker.appendChild(swatch(name, t("Background {n}", { n: i + 1 }))));
    if (b.canAdd) {
      const add = el("button", "bg-swatch add");
      add.type = "button";
      add.setAttribute("aria-label", t("Add background image"));
      add.title = t("Add Image…");
      add.appendChild(svg(ICON.plus, 18));
      add.addEventListener("click", () => post({ type: "chooseBackground" }));
      picker.appendChild(add);
    }
  }

  // ---- live wallpapers ----

  // The wallpaper video, and which background it was made for.
  let wallpaper = null;
  let wallpaperName = "";
  // Blob URLs of first frames of animated images (shown while live wallpapers are off).
  const stills = new Map();
  let frozen = false;

  const mediaUrl = (name) => `/api/background?name=${encodeURIComponent(name)}`;

  function fetchBlobUrl(name) {
    return fetch(mediaUrl(name))
      .then((r) => (r.ok ? r.blob() : Promise.reject(new Error("missing"))))
      .then((blob) => URL.createObjectURL(blob));
  }

  function setNote(text) {
    $("bg-note").textContent = text;
    $("bg-note").hidden = !text;
  }

  // A short message on the page itself: the panel with the note is usually closed, and a
  // wallpaper that silently stays still looks broken.
  let hintTimer = 0;
  const hinted = new Set();
  function showHint(text) {
    const hint = $("bg-hint");
    hint.textContent = text;
    hint.hidden = false;
    clearTimeout(hintTimer);
    hintTimer = setTimeout(() => { hint.hidden = true; }, 9000);
  }
  // Once per page load and message.
  function hintOnce(text) {
    if (hinted.has(text)) return;
    hinted.add(text);
    showHint(text);
  }

  function removeWallpaper() {
    if (!wallpaper) return;
    wallpaper.pause();
    URL.revokeObjectURL(wallpaper.src);
    wallpaper.removeAttribute("src");
    wallpaper.load();
    wallpaper.remove();
    wallpaper = null;
    wallpaperName = "";
  }

  // Plays the wallpaper video only while it can be seen and may move.
  function syncPlayback() {
    if (!wallpaper) return;
    const run = state.background.live && document.visibilityState === "visible" && !frozen;
    if (run && wallpaper.paused) wallpaper.play().catch(() => {});
    else if (!run && !wallpaper.paused) wallpaper.pause();
  }
  document.addEventListener("visibilitychange", () => syncPlayback());
  document.addEventListener("freeze", () => { frozen = true; syncPlayback(); });
  document.addEventListener("resume", () => { frozen = false; syncPlayback(); });

  // The video could not be shown (e.g. mp4 with H.264 in a build without proprietary codecs):
  // its thumbnail stands in, and a note says why.
  function wallpaperFailed(name, why) {
    removeWallpaper();
    const backdrop = $("backdrop");
    backdrop.style.backgroundImage = `url("/api/thumb?name=${encodeURIComponent(name)}")`;
    setNote(why);
    hintOnce(why);
  }

  function showVideo(name) {
    if (wallpaperName === name && wallpaper) {
      syncPlayback();
      return;
    }
    removeWallpaper();
    wallpaperName = name;
    const mime = state.background.mimes[state.background.list.indexOf(name)] || "video/mp4";
    const video = document.createElement("video");
    video.id = "wallpaper";
    // The media bridge ignores it: a wallpaper is not media (no audio icon, no Now Playing).
    video.setAttribute("data-axion-wallpaper", "");
    video.muted = true;
    video.loop = true;
    video.playsInline = true;
    video.disablePictureInPicture = true;
    video.controls = false;
    video.preload = "auto";
    wallpaper = video;
    if (!video.canPlayType(mime)) {
      wallpaperFailed(name, mime === "video/mp4"
        ? t("This build of Axion can't play H.264 video. WebM videos always play.")
        : t("This video format can't be played here."));
      return;
    }
    fetchBlobUrl(name)
      .then((url) => {
        if (wallpaper !== video) return URL.revokeObjectURL(url);
        video.addEventListener("error", () =>
          wallpaperFailed(name, t("This video can't be played here (its codec, e.g. HEVC or AV1 in MP4, isn't supported). Use H.264 MP4 or WebM.")));
        video.addEventListener("loadeddata", () => {
          $("backdrop").style.backgroundImage = "";
          setNote("");
        });
        video.src = url;
        $("backdrop").insertBefore(video, $("backdrop").firstChild);
        syncPlayback();
      })
      .catch(() => wallpaperFailed(name, t("The video file could not be read.")));
  }

  // The first frame of an animated image, as a still (a CSS background would keep playing).
  function showStill(name) {
    const apply = (url) => {
      if (state.background.selected === name) $("backdrop").style.backgroundImage = `url("${url}")`;
    };
    if (stills.has(name)) return apply(stills.get(name));
    const img = new Image();
    img.onload = () => {
      const canvas = document.createElement("canvas");
      canvas.width = img.naturalWidth;
      canvas.height = img.naturalHeight;
      canvas.getContext("2d").drawImage(img, 0, 0);
      canvas.toBlob((blob) => {
        if (!blob) return;
        const url = URL.createObjectURL(blob);
        stills.set(name, url);
        apply(url);
      });
    };
    img.src = `/api/background?v=${encodeURIComponent(name)}`;
  }

  // Videos without a thumbnail get one from their first frame, posted back (a small JPEG).
  const thumbTried = new Set();
  let thumbBusy = false;
  function makeVideoThumbs() {
    const b = state.background;
    if (thumbBusy || state.private) return;
    const name = b.needsThumb.find((n) => !thumbTried.has(n));
    if (!name) return;
    thumbBusy = true;
    thumbTried.add(name);
    const done = () => { thumbBusy = false; makeVideoThumbs(); };
    fetchBlobUrl(name).then((url) => {
      const video = document.createElement("video");
      video.muted = true;
      video.preload = "auto";
      video.setAttribute("data-axion-wallpaper", "");
      const finish = () => { URL.revokeObjectURL(url); video.removeAttribute("src"); video.load(); done(); };
      video.addEventListener("error", finish);
      video.addEventListener("loadeddata", () => {
        video.currentTime = Math.min(0.5, (video.duration || 1) / 4);
      });
      video.addEventListener("seeked", () => {
        const w = 320;
        const h = Math.max(1, Math.round((w * video.videoHeight) / Math.max(1, video.videoWidth)));
        const canvas = document.createElement("canvas");
        canvas.width = w;
        canvas.height = h;
        canvas.getContext("2d").drawImage(video, 0, 0, w, h);
        const data = canvas.toDataURL("image/jpeg", 0.7);
        if (data.length < 60000) post({ type: "backgroundThumb", name, data });
        finish();
      }, { once: true });
      video.src = url;
    }).catch(done);
  }

  // The last "could not add" message, shown when it changes (not for one from before this load).
  let shownError;
  let shownSelected;
  function reportAddError(error) {
    if (shownError !== undefined && error && error !== shownError) showHint(error);
    shownError = error;
  }

  function renderBackground() {
    const b = state.background;
    reportAddError(b.error || "");
    const backdrop = $("backdrop");
    const kind = b.show ? b.kinds[b.list.indexOf(b.selected)] || "image" : "image";
    $("live-row").hidden = !b.show || kind === "image";
    $("live-row").querySelector("input").checked = !!b.liveSetting;
    document.body.classList.toggle("has-image", b.show);
    if (b.show && !state.private) {
      // The window around the page takes the image's colour (measured), not plain white/black.
      document.querySelectorAll('meta[name="theme-color"]').forEach((meta) => meta.remove());
    }
    if (b.show) {
      // File names are unique and never reused, so the name is the cache key.
      if (kind === "video") {
        showVideo(b.selected);
      } else {
        removeWallpaper();
        setNote("");
        if (kind === "animated" && !b.live) {
          showStill(b.selected);
        } else {
          const url = `url("/api/background?v=${encodeURIComponent(b.selected)}")`;
          if (backdrop.style.backgroundImage !== url) backdrop.style.backgroundImage = url;
        }
      }
      if (kind !== "image" && !b.live) {
        const text = b.liveSetting ? t("Reduce Motion is on, so the wallpaper shows a still frame.") : "";
        setNote(text);
        // Said when the user picks it, not on every new tab (Reduce Motion is a choice).
        if (text && shownSelected !== undefined && shownSelected !== b.selected) showHint(text);
      }
      shownSelected = b.selected;
      // While a slider is held, its live preview wins over the saved look.
      if (!lookHeld) {
        backdrop.style.filter = b.blur > 0 ? `blur(${b.blur}px)` : "";
        backdrop.style.setProperty("--dim", b.dim);
      }
    } else {
      removeWallpaper();
      setNote("");
      backdrop.style.backgroundImage = "";
    }
    if (!lookHeld) {
      $("dim").value = b.dim;
      $("blur").value = b.blur;
    }
    paintRange($("dim"));
    paintRange($("blur"));
    $("look-rows").hidden = !b.show;
    renderPicker();
    makeVideoThumbs();
  }

  // ---- settings panel ----

  function renderPanel() {
    const widgets = {
      clock: state.clock.show,
      greeting: state.greeting.show,
      search: state.search.show,
      topSites: state.topSites.show,
      stats: state.stats.show,
      liveWallpapers: state.background.liveSetting,
    };
    document.querySelectorAll("[data-widget]").forEach((input) => {
      input.checked = !!widgets[input.dataset.widget];
    });
    document.querySelectorAll("[data-format]").forEach((b) => {
      b.setAttribute("role", "radio");
      b.setAttribute("aria-checked", String(b.dataset.format === state.clock.format));
    });
    $("clock-format-row").hidden = !state.clock.show;
  }

  function togglePanel(open) {
    const panel = $("panel");
    const show = open === undefined ? panel.hidden : open;
    panel.hidden = !show;
    $("gear").setAttribute("aria-expanded", String(show));
  }

  $("gear").addEventListener("click", (e) => {
    e.stopPropagation();
    togglePanel();
  });
  document.querySelectorAll("[data-widget]").forEach((input) =>
    input.addEventListener("change", () => post({ type: "widget", name: input.dataset.widget, on: input.checked })),
  );
  document.querySelectorAll("[data-format]").forEach((b) =>
    b.addEventListener("click", () => post({ type: "clockFormat", format: b.dataset.format })),
  );
  $("restore").addEventListener("click", () => post({ type: "restoreSites" }));
  $("path-form").addEventListener("submit", (e) => {
    e.preventDefault();
    const path = $("path").value.trim();
    if (path) {
      post({ type: "backgroundPath", path });
      $("path").value = "";
    }
  });
  // Live preview while dragging (one style change per frame), saved once on release, so the
  // page never re-renders under the thumb.
  let lookFrame = 0;
  const previewLook = (e) => {
    if (e && e.target) paintRange(e.target);
    if (lookFrame) return;
    lookFrame = requestAnimationFrame(() => {
      lookFrame = 0;
      const backdrop = $("backdrop");
      const blur = Number($("blur").value);
      backdrop.style.setProperty("--dim", $("dim").value);
      backdrop.style.filter = blur > 0 ? `blur(${blur}px)` : "";
    });
  };
  const saveLook = () => {
    lookHeld = false;
    post({ type: "backgroundLook", dim: Number($("dim").value), blur: Number($("blur").value) });
  };
  for (const id of ["dim", "blur"]) {
    $(id).addEventListener("pointerdown", () => (lookHeld = true));
    $(id).addEventListener("input", previewLook);
    $(id).addEventListener("change", saveLook);
  }
  $("more-settings").addEventListener("click", () => {
    togglePanel(false);
    post({ type: "openSettings" });
  });

  // ---- the whole page ----

  function render() {
    if (!state) return;
    translateDom();
    // Axion's accent colour, live (controls, focus rings, selection).
    if (state.accent) document.documentElement.style.setProperty("--accent", state.accent);
    document.body.classList.toggle("private", state.private);
    document.body.classList.toggle("flat", !!state.flat);
    if (state.private) {
      // The window around the page takes the page's theme colour: black, like the page.
      document.querySelectorAll('meta[name="theme-color"]').forEach((meta) => {
        meta.setAttribute("content", "#000000");
        meta.removeAttribute("media");
      });
    }
    $("gear").hidden = state.private;
    if (state.private) togglePanel(false);
    $("private-card").hidden = !state.private;
    $("clock-block").hidden = !state.clock.show && !state.greeting.show;
    $("clock").hidden = !state.clock.show;
    $("greeting").hidden = !state.greeting.show;
    tick();
    renderSearch();
    renderSites();
    renderStats();
    renderBackground();
    renderPanel();
  }

  $("search").addEventListener("submit", (e) => {
    e.preventDefault();
    const text = $("query").value.trim();
    if (text) post({ type: "go", text });
  });
  $("engine").addEventListener("click", (e) => {
    e.stopPropagation();
    const menu = $("engine-menu");
    menu.hidden = !menu.hidden;
    $("engine").setAttribute("aria-expanded", String(!menu.hidden));
    if (!menu.hidden) {
      const current = menu.querySelector('[aria-checked="true"]') || menu.firstElementChild;
      if (current) current.focus();
    }
  });

  document.addEventListener("click", (e) => {
    // The path as dispatched: a click may have rebuilt (detached) its own target meanwhile.
    const path = e.composedPath();
    if (!$("engine-menu").hidden && !path.includes($("engine-menu"))) closeMenu();
    if (!$("panel").hidden && !path.includes($("panel"))) togglePanel(false);
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      closeMenu();
      togglePanel(false);
    }
    if (document.activeElement === document.body && !e.ctrlKey && !e.metaKey && !e.altKey && e.key.length === 1) {
      // Typing anywhere goes into the search field.
      if (state && state.search.show) $("query").focus();
    }
  });
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") refresh();
  });
  let resizeTimer = 0;
  window.addEventListener("resize", () => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => { if (state) { renderSites(); fitStats(); } }, 120);
  });

  refresh().then(startClock);
})();
