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
  const originals = new Map(); // text node -> {orig, shown}
  let titleOriginal = null;
  let titleShown = null;
  let options = { chars: 4000, pieces: 60, observe: true, target: "" };
  let active = false;
  let nextId = 1;
  let total = 0;
  let done = 0;
  let observer = null;
  let sweep = 0;
  const waiting = new Map(); // batch id -> [{node|null, lead, trail}]
  let langBefore = null;

  const wanted = (node) => {
    const text = node.nodeValue;
    if (!text || text.length < 2 || !/\p{L}/u.test(text)) return false;
    for (let p = node.parentElement; p; p = p.parentElement) {
      if (SKIP.has(p.tagName.toUpperCase())) return false;
      if (p.isContentEditable || p.getAttribute("translate") === "no" || p.classList.contains("notranslate")) return false;
    }
    return true;
  };

  const rect = (node) => {
    const el = node.parentElement;
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return r.width || r.height ? r : null;
  };

  const collect = () => {
    const pieces = [];
    if (!document.body) return pieces;
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      const known = originals.get(n);
      if (known && known.shown === n.nodeValue) continue; // already translated
      if (!wanted(n)) continue;
      pieces.push(n);
    }
    // What is on screen first.
    const vh = window.innerHeight;
    const score = (n) => {
      const r = rect(n);
      if (!r) return 2e9; // hidden: last
      if (r.bottom >= 0 && r.top <= vh) return 0;
      return r.top > vh ? r.top : -r.bottom + vh;
    };
    return pieces.map((n) => [score(n), n]).sort((a, b) => a[0] - b[0]).map((p) => p[1]);
  };

  const send = (items) => {
    // items: [{node|null, text}]
    let batch = [];
    let chars = 0;
    const flush = () => {
      if (!batch.length) return;
      const id = nextId++;
      waiting.set(id, batch.map((b) => ({ node: b.node, lead: b.lead, trail: b.trail, orig: b.orig })));
      post({ t: "batch", id, texts: batch.map((b) => b.core) });
      batch = [];
      chars = 0;
    };
    for (const item of items) {
      if (batch.length >= options.pieces || (batch.length && chars + item.core.length > options.chars)) flush();
      batch.push(item);
      chars += item.core.length;
    }
    flush();
  };

  const item = (node, text) => {
    const m = /^(\s*)([\s\S]*?)(\s*)$/.exec(text);
    return { node, orig: text, lead: m[1], core: m[2], trail: m[3] };
  };

  const start = (opts) => {
    options = Object.assign(options, opts || {});
    if (active) return;
    active = true;
    langBefore = document.documentElement.getAttribute("lang");
    const nodes = collect();
    const items = nodes.map((n) => item(n, n.nodeValue));
    titleOriginal = document.title;
    if (titleOriginal && /\p{L}/u.test(titleOriginal)) items.unshift(item(null, titleOriginal));
    total = items.length;
    done = 0;
    post({ t: "state", active: true });
    post({ t: "progress", done, total });
    send(items);
    watch();
    if (!items.length) finish();
  };

  const finish = () => post({ t: "progress", done: total, total });

  const apply = (id, texts) => {
    const entries = waiting.get(id);
    waiting.delete(id);
    if (!entries || !active) return;
    entries.forEach((entry, i) => {
      const translated = texts[i];
      done++;
      if (typeof translated !== "string") return;
      const value = entry.lead + translated + entry.trail;
      if (!entry.node) {
        if (document.title === entry.orig) { titleShown = value; document.title = value; }
        return;
      }
      // The page changed the text meanwhile: leave it.
      if (!entry.node.isConnected || entry.node.nodeValue !== entry.orig) return;
      const known = originals.get(entry.node);
      originals.set(entry.node, { orig: known ? known.orig : entry.orig, shown: value });
      entry.node.nodeValue = value;
    });
    if (options.target) document.documentElement.setAttribute("lang", options.target);
    post({ t: "progress", done: Math.min(done, total), total });
  };

  const fail = (id) => {
    const entries = waiting.get(id);
    waiting.delete(id);
    if (entries) done += entries.length;
  };

  const watch = () => {
    if (!options.observe || observer || !document.body) return;
    observer = new MutationObserver(() => {
      clearTimeout(sweep);
      sweep = setTimeout(() => {
        if (!active) return;
        const items = collect().map((n) => item(n, n.nodeValue));
        if (!items.length) return;
        total += items.length;
        post({ t: "progress", done, total });
        send(items);
      }, 400);
    });
    observer.observe(document.body, { childList: true, subtree: true, characterData: true });
  };

  const stop = () => {
    if (!active) return;
    active = false;
    if (observer) { observer.disconnect(); observer = null; }
    clearTimeout(sweep);
    waiting.clear();
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
