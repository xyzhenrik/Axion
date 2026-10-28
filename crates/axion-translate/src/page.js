// Translation, page side: tells which language the page is in, hands the browser the page's
// text in batches, puts the translations back, and restores the original on request.
// Injected into top-level documents only.
//
// Page -> browser: console.debug of "\u0001axion:tr:" + JSON:
//   {t:"lang", lang, declared}              after load: the page's language ("" unknown)
//   {t:"batch", id, texts:[...]}            text to translate
//   {t:"progress", done, total}             pieces translated / found so far
//   {t:"state", active}                     translation started / restored
// Browser -> page: window.__axionTr.start(options) / .apply(id, texts) / .fail(id) / .stop().
(() => {
  if (window.top !== window || window.__axionTr) return;
  const MARKER = "\u0001axion:tr:";
  const MIN_DETECT = __AXION_DETECT_MIN__;
  const SKIP = new Set([
    "SCRIPT", "STYLE", "NOSCRIPT", "CODE", "PRE", "TEXTAREA", "SVG", "KBD", "SAMP", "VAR", "INPUT",
    "SELECT", "OPTION", "MATH", "CANVAS", "IFRAME", "TEMPLATE",
  ]);
  const post = (m) => { try { console.debug(MARKER + JSON.stringify(m)); } catch (_) {} };

  // ---- the page's language ----
  const STOP = {
    en: "the and of to in is that it for with as was on are this be by at from or have not but they you which",
    de: "der die das und ist nicht ein eine mit den von zu für auf dem sich auch als wie aber nach bei sind wird oder noch nur",
    fr: "le la les des est une un et en du que pour dans qui pas sur au avec par plus sont ce il elle ne se mais ou",
    es: "el la los las de que y en un una es por con para no se su al lo como más pero sus le ya o este si porque",
    it: "il lo la gli le di che e un una è per con non si su al come più ma sono anche del della nel nella ho",
    pt: "o a os as de que e um uma é para com não se na no por mais como mas foi ao ele das tem à seu sua ou",
    nl: "de het een en van is dat op te in niet zijn met voor ook als maar aan er om dit bij nog wordt",
  };
  const stopSets = Object.fromEntries(Object.entries(STOP).map(([k, v]) => [k, new Set(v.split(" "))]));
  const guess = () => {
    const sample = (document.body ? document.body.innerText : "").slice(0, 6000).toLowerCase();
    if (sample.length < MIN_DETECT) return "";
    const words = sample.match(/[\p{L}']+/gu) || [];
    if (words.length < 25) return "";
    const scores = {};
    for (const [lang, set] of Object.entries(stopSets)) scores[lang] = words.filter((w) => set.has(w)).length;
    const ranked = Object.entries(scores).sort((a, b) => b[1] - a[1]);
    const [best, second] = ranked;
    if (best[1] < words.length * 0.08 || best[1] < (second[1] || 0) * 1.3) return "";
    return best[0];
  };
  const declared = () => {
    const attr = (document.documentElement.getAttribute("lang") || "").trim();
    if (attr) return attr;
    const meta = document.querySelector('meta[http-equiv="content-language" i]');
    return meta ? (meta.getAttribute("content") || "").trim() : "";
  };
  const report = () => {
    const lang = declared();
    post({ t: "lang", lang: lang || guess(), declared: !!lang });
  };

  // ---- the text ----
  // Work is proportional to what the user sees: the text in (and `margin` screens around) the
  // viewport is requested first, in small batches; with scope "visible" the rest follows as it
  // is scrolled into reach, with scope "page" in the background. At most `inflight` requests
  // wait for the engine at once, so what scrolls into view is never queued behind the page's
  // far end. Progress counts what has been requested so far.
  const originals = new Map(); // text node -> {orig, shown}
  const memo = new Map(); // text -> translation (this page, this language pair)
  let titleOriginal = null;
  let titleShown = null;
  let options = {
    chars: 4000, pieces: 60, first: 12, inflight: 2, observe: true, target: "", scope: "visible", margin: 1,
  };
  let active = false;
  let nextId = 1;
  let total = 0;
  let done = 0;
  let observer = null;
  let sweep = 0;
  let pumpTimer = 0;
  let began = 0;
  let sentBatches = 0;
  let firstApplied = false;
  let visibleReported = false;
  const waiting = new Map(); // batch id -> [[entry...] per unique text]
  const todo = new Set(); // items not requested yet
  let langBefore = null;
  const writes = []; // [node, value, orig] waiting for the next frame
  let frame = 0;

  const wanted = (node) => {
    const text = node.nodeValue;
    if (!text || text.length < 2 || !/\p{L}/u.test(text)) return false;
    for (let p = node.parentElement; p; p = p.parentElement) {
      if (SKIP.has(p.tagName.toUpperCase())) return false;
      if (p.isContentEditable || p.getAttribute("translate") === "no" || p.classList.contains("notranslate")) return false;
    }
    return true;
  };

  // Distance of a node from the viewport in screens: 0 inside it, null when it has no box.
  const distance = (node) => {
    const el = node ? node.parentElement : null;
    if (!node) return 0; // the title
    if (!el) return null;
    const r = el.getBoundingClientRect();
    if (!r.width && !r.height) return null;
    const vh = Math.max(window.innerHeight, 1);
    if (r.bottom >= 0 && r.top <= vh) return 0;
    return (r.top > vh ? r.top - vh : -r.bottom) / vh;
  };

  const item = (node, text) => {
    const m = /^(\s*)([\s\S]*?)(\s*)$/.exec(text);
    return { node, orig: text, lead: m[1], core: m[2], trail: m[3] };
  };

  const scan = () => {
    if (!document.body) return [];
    const found = [];
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      const known = originals.get(n);
      if (known && known.shown === n.nodeValue) continue; // already translated
      if (!wanted(n)) continue;
      found.push(item(n, n.nodeValue));
    }
    return found;
  };

  const flushWrites = () => {
    frame = 0;
    for (const [node, value, orig] of writes.splice(0)) {
      // The page changed the text meanwhile: leave it.
      if (!node.isConnected || node.nodeValue !== orig) continue;
      const known = originals.get(node);
      originals.set(node, { orig: known ? known.orig : orig, shown: value });
      node.nodeValue = value;
    }
  };

  const finishItem = (entry, translated) => {
    done++;
    if (typeof translated !== "string") return;
    const value = entry.lead + translated + entry.trail;
    if (!entry.node) {
      if (document.title === entry.orig) { titleShown = value; document.title = value; }
      return;
    }
    writes.push([entry.node, value, entry.orig]);
    if (!frame) frame = requestAnimationFrame(flushWrites);
  };

  const progress = () => {
    post({ t: "progress", done: Math.min(done, total), total });
    if (!visibleReported && done >= total) {
      visibleReported = true;
      post({ t: "log", what: "visible part translated", ms: Math.round(performance.now() - began) });
    }
  };

  // Chooses the next items to request: those within reach first, nearest first.
  const reach = () => (options.scope === "page" ? Infinity : Math.max(options.margin, 0));

  const pump = () => {
    pumpTimer = 0;
    if (!active) return;
    if (!todo.size) { progress(); return; }
    // Distances are read once per pump (one layout), then sorted.
    const ranked = [];
    for (const it of todo) {
      const d = distance(it.node);
      ranked.push([d === null ? 1e9 : d, it]);
    }
    ranked.sort((a, b) => a[0] - b[0]);
    const limit = reach();
    while (waiting.size < Math.max(options.inflight, 1) && ranked.length && ranked[0][0] <= limit) {
      const first = sentBatches === 0;
      const maxPieces = first ? Math.max(options.first, 1) : options.pieces;
      const group = [];
      let chars = 0;
      while (ranked.length && ranked[0][0] <= limit && group.length < maxPieces) {
        const next = ranked[0][1];
        if (group.length && chars + next.core.length > options.chars) break;
        ranked.shift();
        todo.delete(next);
        group.push(next);
        chars += next.core.length;
      }
      if (!group.length) break;
      request(group);
    }
    progress();
  };

  // Sends the unique texts of `group`; the memoised ones are applied at once.
  const request = (group) => {
    total += group.length;
    const unique = new Map(); // core -> [entry]
    let fresh = 0;
    for (const entry of group) {
      if (memo.has(entry.core)) { finishItem(entry, memo.get(entry.core)); continue; }
      if (!unique.has(entry.core)) { unique.set(entry.core, []); fresh++; }
      unique.get(entry.core).push(entry);
    }
    if (fresh) {
      const id = nextId++;
      sentBatches++;
      const texts = [...unique.keys()];
      waiting.set(id, texts.map((t) => unique.get(t)));
      post({ t: "batch", id, texts });
    }
  };

  const add = (items) => {
    for (const it of items) todo.add(it);
    pump();
  };

  const start = (opts) => {
    options = Object.assign(options, opts || {});
    if (active) return;
    active = true;
    began = performance.now();
    langBefore = document.documentElement.getAttribute("lang");
    const items = scan();
    titleOriginal = document.title;
    if (titleOriginal && /\p{L}/u.test(titleOriginal)) items.unshift(item(null, titleOriginal));
    total = 0;
    done = 0;
    post({ t: "state", active: true });
    post({ t: "log", what: "text pieces found", ms: Math.round(performance.now() - began), n: items.length });
    // total counts what has been requested: it grows as the page is scrolled.
    for (const it of items) todo.add(it);
    pump();
    watch();
  };

  const apply = (id, texts) => {
    const groups = waiting.get(id);
    waiting.delete(id);
    if (!groups || !active) return;
    groups.forEach((entries, i) => {
      const translated = texts[i];
      if (typeof translated === "string") memo.set(entries[0].core, translated);
      for (const entry of entries) finishItem(entry, translated);
    });
    if (!firstApplied) {
      firstApplied = true;
      post({ t: "log", what: "first translation applied", ms: Math.round(performance.now() - began) });
    }
    if (options.target) document.documentElement.setAttribute("lang", options.target);
    pump();
  };

  const fail = (id) => {
    const groups = waiting.get(id);
    waiting.delete(id);
    if (groups) for (const entries of groups) done += entries.length;
    pump();
  };

  const rescan = () => {
    if (!active) return;
    const known = new Set([...todo].map((i) => i.node));
    const items = scan().filter((it) => !known.has(it.node) && !isWaiting(it.node));
    if (items.length) add(items);
  };

  const isWaiting = (node) => {
    for (const groups of waiting.values()) {
      for (const entries of groups) if (entries.some((e) => e.node === node)) return true;
    }
    return false;
  };

  const watch = () => {
    if (!options.observe || observer || !document.body) return;
    observer = new MutationObserver(() => {
      clearTimeout(sweep);
      sweep = setTimeout(rescan, 400);
    });
    observer.observe(document.body, { childList: true, subtree: true, characterData: true });
  };

  // Scrolling (and resizing) brings more of the page within reach.
  let scrollTimer = 0;
  const moved = () => {
    if (!active || !todo.size || scrollTimer) return;
    scrollTimer = setTimeout(() => { scrollTimer = 0; pump(); }, 120);
  };
  window.addEventListener("scroll", moved, { passive: true, capture: true });
  window.addEventListener("resize", moved, { passive: true });

  const stop = () => {
    if (!active) return;
    active = false;
    if (observer) { observer.disconnect(); observer = null; }
    clearTimeout(sweep);
    clearTimeout(pumpTimer);
    pumpTimer = 0;
    if (frame) { cancelAnimationFrame(frame); frame = 0; }
    writes.length = 0;
    waiting.clear();
    todo.clear();
    memo.clear();
    sentBatches = 0;
    firstApplied = visibleReported = false;
    for (const [node, state] of originals) {
      if (node.isConnected && node.nodeValue === state.shown) node.nodeValue = state.orig;
    }
    originals.clear();
    if (titleOriginal !== null && document.title === titleShown) document.title = titleOriginal;
    titleOriginal = titleShown = null;
    if (langBefore === null) document.documentElement.removeAttribute("lang");
    else document.documentElement.setAttribute("lang", langBefore);
    post({ t: "state", active: false });
  };

  Object.defineProperty(window, "__axionTr", {
    value: Object.freeze({ start, apply, fail, stop, report }),
    enumerable: false,
  });

  const begin = () => setTimeout(report, 300);
  if (document.readyState === "complete") begin();
  else window.addEventListener("load", begin, { once: true });
})();
