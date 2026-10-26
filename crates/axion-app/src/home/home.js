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

  // The site's cached icon from the browser, or a monogram while it has none.
  const siteIcon = (node, host, title) => {
    monogram(node, host, title);
    if (!host) return;
    const img = new Image();
    img.alt = "";
    img.decoding = "async";
    img.onload = () => {
      node.textContent = "";
      node.style.background = "transparent";
      node.appendChild(img);
    };
    img.src = `/api/favicon?host=${encodeURIComponent(host)}&v=${state ? state.icons : 0}`;
  };

  const formatCount = (n) => Math.round(n).toLocaleString("en-US");

  const formatBytes = (b) => {
    const units = ["bytes", "KB", "MB", "GB", "TB"];
    let i = 0;
    while (b >= 1024 && i < units.length - 1) {
      b /= 1024;
      i += 1;
    }
    return i === 0 ? `${Math.round(b)} ${units[0]}` : `${b < 10 ? b.toFixed(1) : Math.round(b)} ${units[i]}`;
  };

  const formatSeconds = (s) => {
    if (s < 60) return `${Math.round(s)} s`;
    if (s < 3600) return `${Math.round(s / 60)} min`;
    if (s < 86400) {
      const h = s / 3600;
      return `${h < 10 ? h.toFixed(1) : Math.round(h)} h`;
    }
    const d = s / 86400;
    return `${d < 10 ? d.toFixed(1) : Math.round(d)} days`;
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
      clock.append(`${h}:${minutes}`, el("span", "ampm", now.getHours() < 12 ? "AM" : "PM"));
    }
    const hour = now.getHours();
    const part = hour >= 5 && hour < 12 ? "morning" : hour >= 12 && hour < 18 ? "afternoon" : "evening";
    const name = state.greeting.name;
    $("greeting").textContent = name ? `Good ${part}, ${name}` : `Good ${part}`;
  }

  function startClock() {
    tick();
    // Tick on the minute boundary, then every 1 s keeps it exact after sleep.
    setInterval(tick, 1000);
  }

  // ---- search ----

  function engineName() {
    const s = state.search;
    return s.current >= 0 ? s.engines[s.current].name : s.customHost || "your search engine";
  }

  function renderSearch() {
    const s = state.search;
    $("search").hidden = !s.show;
    $("query").placeholder = `Search with ${engineName()} or enter website`;
    const host = s.current >= 0 ? s.engines[s.current].host : s.customHost;
    const icon = $("engine-icon");
    icon.textContent = "";
    icon.style.background = "";
    siteIcon(icon, host, engineName());
    $("engine").setAttribute("aria-label", `Search engine: ${engineName()}`);
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
    action(site.pinned ? "Unpin" : "Pin", ICON.pin, () =>
      post(site.pinned ? { type: "unpinSite", url: site.url } : { type: "pinSite", url: site.url, title: site.title }),
    );
    action("Edit", ICON.edit, () => openEditor(site));
    action("Remove", ICON.close, () => post({ type: "removeSite", url: site.url }));
    a.appendChild(actions);
    return a;
  }

  function addTile() {
    const b = el("button", "tile add");
    b.type = "button";
    const face = el("span", "face glass");
    face.appendChild(svg(ICON.plus, 26));
    b.append(face, el("span", "name", "Add Site"));
    b.addEventListener("click", () => openEditor(null));
    return b;
  }

  function renderSites() {
    const t = state.topSites;
    $("sites").hidden = !t.show;
    const restore = $("restore-row");
    restore.hidden = !(t.hidden > 0);
    $("restore-label").textContent = `${t.hidden} removed site${t.hidden === 1 ? "" : "s"}`;
    if (!t.show) return;
    const cols = columns();
    const perPage = cols * t.rows;
    const items = t.sites.map(tile);
    items.push(addTile());
    const pages = Math.max(1, Math.ceil(items.length / perPage));
    page = Math.min(page, pages - 1);
    const container = $("pages");
    container.textContent = "";
    const track = el("div", "pages-track");
    for (let p = 0; p < pages; p++) {
      const grid = el("div", "page");
      grid.style.setProperty("--columns", cols);
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
      dot.setAttribute("aria-label", `Page ${p + 1}`);
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
    $("edit-title").textContent = site ? "Edit Site" : "Add Site";
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
    $("bandwidth").textContent = `≈ ${formatBytes(s.bytes)}`;
    $("time").textContent = `≈ ${formatSeconds(s.seconds)}`;
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
    const key = JSON.stringify([b.list, b.selected, b.canAdd]);
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
        const img = new Image();
        img.alt = "";
        img.decoding = "async";
        img.src = `/api/thumb?name=${encodeURIComponent(name)}`;
        button.appendChild(img);
      } else {
        button.appendChild(el("span", "none-mark"));
      }
      button.addEventListener("click", () => selectBackground(name));
      item.appendChild(button);
      if (name) {
        const remove = el("button", "bg-remove");
        remove.type = "button";
        remove.setAttribute("aria-label", "Remove background");
        remove.title = "Remove";
        remove.appendChild(svg(ICON.close, 10));
        remove.addEventListener("click", (e) => {
          e.stopPropagation();
          post({ type: "removeBackground", name });
        });
        item.appendChild(remove);
      }
      return item;
    };
    picker.appendChild(swatch("", "No background"));
    b.list.forEach((name, i) => picker.appendChild(swatch(name, `Background ${i + 1}`)));
    if (b.canAdd) {
      const add = el("button", "bg-swatch add");
      add.type = "button";
      add.setAttribute("aria-label", "Add background image");
      add.title = "Add Image…";
      add.appendChild(svg(ICON.plus, 18));
      add.addEventListener("click", () => post({ type: "chooseBackground" }));
      picker.appendChild(add);
    }
  }

  function renderBackground() {
    const b = state.background;
    const backdrop = $("backdrop");
    document.body.classList.toggle("has-image", b.show);
    if (b.show && !state.private) {
      // The window around the page takes the image's colour (measured), not plain white/black.
      document.querySelectorAll('meta[name="theme-color"]').forEach((meta) => meta.remove());
    }
    if (b.show) {
      // File names are unique and never reused, so the name is the cache key.
      const url = `url("/api/background?v=${encodeURIComponent(b.selected)}")`;
      if (backdrop.style.backgroundImage !== url) backdrop.style.backgroundImage = url;
      // While a slider is held, its live preview wins over the saved look.
      if (!lookHeld) {
        backdrop.style.filter = b.blur > 0 ? `blur(${b.blur}px)` : "";
        backdrop.style.setProperty("--dim", b.dim);
      }
    } else {
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
  }

  // ---- settings panel ----

  function renderPanel() {
    const widgets = {
      clock: state.clock.show,
      greeting: state.greeting.show,
      search: state.search.show,
      topSites: state.topSites.show,
      stats: state.stats.show,
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
    // Axion's accent colour, live (controls, focus rings, selection).
    if (state.accent) document.documentElement.style.setProperty("--accent", state.accent);
    document.body.classList.toggle("private", state.private);
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
