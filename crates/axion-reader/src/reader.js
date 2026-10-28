// Axion's Reader view: finds the article of a page, and swaps the page for a clean sheet with
// only that article in it (and back). Injected into top-level documents only.
//
// Page -> browser: console.debug of "\u0001axion:reader:" + JSON:
//   {t:"probe", readable:bool, active:bool}   after load, and when the page changes its address
//   {t:"state", active:bool}                   after enter / exit
// Browser -> page: window.__axionReader.enter(options) / .apply(options) / .exit() / .probe().
//
// The extraction is a compact take on the well-known "text density" approach: paragraphs score
// their parents (commas, length, class names), the best container wins, and its content is
// rebuilt from a whitelist of tags, so nothing the page ran or styled comes along.
(() => {
  if (window.top !== window || window.__axionReader) return;
  const MARKER = "\u0001axion:reader:";
  const MIN_CHARS = __AXION_MIN_CHARS__;
  const DELAY = __AXION_DELAY__;
  const POSITIVE = /article|body|content|entry|main|page|post|text|blog|story|prose|markdown|hentry/i;
  const NEGATIVE =
    /comment|meta|footer|footnote|foot|sidebar|side-bar|nav|menu|breadcrumb|banner|share|social|related|promo|sponsor|advert|\bads?\b|popup|modal|cookie|newsletter|subscribe|signup|sign-up|login|toolbar|widget|recommend|outbrain|taboola|disqus|pagination|pager|skip/i;
  const SKIP_TAGS = new Set([
    "SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "IFRAME", "FRAME", "OBJECT", "EMBED", "FORM", "INPUT",
    "BUTTON", "SELECT", "TEXTAREA", "NAV", "ASIDE", "FOOTER", "DIALOG", "CANVAS", "AUDIO", "VIDEO",
    "LINK", "META", "SVG", "MATH", "MENU", "LABEL",
  ]);
  const KEEP = new Set([
    "P", "H1", "H2", "H3", "H4", "H5", "H6", "UL", "OL", "LI", "BLOCKQUOTE", "PRE", "CODE", "EM", "STRONG",
    "B", "I", "U", "S", "SUB", "SUP", "BR", "HR", "FIGURE", "FIGCAPTION", "TABLE", "THEAD", "TBODY", "TFOOT",
    "TR", "TH", "TD", "CAPTION", "DL", "DT", "DD", "MARK", "SMALL", "ABBR", "CITE", "Q", "DEL", "INS", "KBD",
    "SAMP", "VAR", "TIME", "A", "IMG", "PICTURE",
  ]);
  const BLOCKS = new Set([
    "P", "DIV", "SECTION", "ARTICLE", "MAIN", "UL", "OL", "LI", "BLOCKQUOTE", "PRE", "H1", "H2", "H3", "H4",
    "H5", "H6", "FIGURE", "TABLE", "DL", "HEADER", "HR", "ADDRESS", "DETAILS", "SUMMARY", "TR",
  ]);

  let original = null; // the page's <html>, kept while Reader view shows
  let sheet = null;
  let scrollBefore = 0;
  let titleBefore = "";
  let root = null; // Reader view's <html>
  let lastOptions = null;

  const post = (message) => {
    try { console.debug(MARKER + JSON.stringify(message)); } catch (_) {}
  };
  const text = (el) => (el.textContent || "").replace(/\s+/g, " ").trim();
  const hidden = (el) => {
    if (el.hidden || el.getAttribute("aria-hidden") === "true") return true;
    const s = getComputedStyle(el);
    return s.display === "none" || s.visibility === "hidden";
  };
  const linkDensity = (el) => {
    const total = text(el).length;
    if (!total) return 0;
    let links = 0;
    for (const a of el.querySelectorAll("a")) links += text(a).length;
    return links / total;
  };
  const tagBonus = (el) => {
    let score = 0;
    switch (el.tagName) {
      case "ARTICLE": score += 25; break;
      case "MAIN": score += 12; break;
      case "DIV": case "SECTION": score += 5; break;
      case "PRE": case "TD": case "BLOCKQUOTE": score += 3; break;
      case "ADDRESS": case "OL": case "UL": case "DL": case "DD": case "DT": case "LI": case "FORM": score -= 3; break;
      case "H1": case "H2": case "H3": case "H4": case "H5": case "H6": case "TH": score -= 5; break;
    }
    const name = (el.className && el.className.baseVal === undefined ? el.className : "") + " " + (el.id || "");
    if (NEGATIVE.test(name)) score -= 25;
    if (POSITIVE.test(name)) score += 25;
    if (el.getAttribute("itemprop") === "articleBody") score += 40;
    if (el.getAttribute("role") === "main") score += 15;
    return score;
  };

  // The element that holds the article's text, or null.
  const findArticle = () => {
    const scores = new Map();
    const add = (el, n) => {
      if (!el || el === document.documentElement) return;
      if (!scores.has(el)) scores.set(el, tagBonus(el));
      scores.set(el, scores.get(el) + n);
    };
    const paragraphs = document.body ? document.body.querySelectorAll("p, pre, td, blockquote") : [];
    let seen = 0;
    for (const p of paragraphs) {
      if (++seen > 3000) break;
      const t = text(p);
      if (t.length < 25) continue;
      if (p.closest("nav, aside, footer, header, form, [role=navigation], [role=complementary]") &&
          !p.closest("article, main, [role=main]")) continue;
      if (hidden(p)) continue;
      const points = 1 + (t.match(/[,，、،]/g) || []).length + Math.min(Math.floor(t.length / 100), 4);
      add(p.parentElement, points);
      if (p.parentElement) add(p.parentElement.parentElement, points / 2);
      const up = p.parentElement && p.parentElement.parentElement && p.parentElement.parentElement.parentElement;
      if (up) add(up, points / 3);
    }
    let best = null;
    let bestScore = 0;
    for (const [el, score] of scores) {
      const adjusted = score * (1 - linkDensity(el));
      if (adjusted > bestScore) { bestScore = adjusted; best = el; }
    }
    if (!best) return null;
    // A single <article> or <main> inside the winner's family usually is the article itself.
    const inner = best.querySelector("article");
    if (inner && text(inner).length > text(best).length * 0.6 && !hidden(inner)) best = inner;
    // Climb while the parent adds hardly anything but headings (the title often sits beside).
    for (let i = 0; i < 2; i++) {
      const parent = best.parentElement;
      if (!parent || parent === document.body || parent === document.documentElement) break;
      if (text(parent).length < text(best).length * 1.15 && linkDensity(parent) < 0.2) best = parent;
      else break;
    }
    return best;
  };

  const measure = () => {
    const el = findArticle();
    if (!el) return { el: null, chars: 0 };
    let chars = 0;
    for (const p of el.querySelectorAll("p, pre, blockquote, li")) {
      if (p.querySelector("p, pre, blockquote, li")) continue;
      chars += text(p).length;
    }
    return { el, chars };
  };

  const meta = (...names) => {
    for (const name of names) {
      const el = document.querySelector(`meta[property="${name}" i], meta[name="${name}" i]`);
      const v = el && el.getAttribute("content");
      if (v && v.trim()) return v.trim();
    }
    return "";
  };

  const abs = (url) => {
    try {
      const u = new URL(url, document.baseURI);
      return u.protocol === "http:" || u.protocol === "https:" ? u.href : "";
    } catch (_) { return ""; }
  };

  const imageSource = (img) => {
    const src = img.currentSrc || img.getAttribute("src") || img.getAttribute("data-src") ||
      img.getAttribute("data-original") || "";
    if (src.startsWith("data:image/") && !src.startsWith("data:image/svg")) return src;
    return abs(src);
  };

  // Rebuilds `node`'s content into `out` from the whitelist.
  const build = (node, out, ctx) => {
    for (const child of Array.from(node.childNodes)) {
      if (child.nodeType === Node.TEXT_NODE) {
        if (child.nodeValue) out.appendChild(document.createTextNode(child.nodeValue));
        continue;
      }
      if (child.nodeType !== Node.ELEMENT_NODE) continue;
      const tag = child.tagName;
      if (SKIP_TAGS.has(tag) || tag.includes(":")) continue;
      if (child.namespaceURI && child.namespaceURI.endsWith("svg")) continue;
      if (hidden(child)) continue;
      const name = (typeof child.className === "string" ? child.className : "") + " " + (child.id || "");
      if (NEGATIVE.test(name) && !POSITIVE.test(name) && child !== ctx.top) {
        const t = text(child);
        if (t.length < 400 || linkDensity(child) > 0.3) continue;
      }
      if (tag !== "A" && tag !== "LI" && (tag === "DIV" || tag === "SECTION" || tag === "UL" || tag === "OL")) {
        // Link-heavy lists and boxes are navigation, share bars and related links.
        const t = text(child);
        if (linkDensity(child) > 0.55 && t.length < 600) continue;
        if (!t && !child.querySelector("img, picture")) continue;
      }
      if (tag === "IMG") {
        const src = imageSource(child);
        if (!src) continue;
        const w = child.naturalWidth || parseInt(child.getAttribute("width") || "0", 10);
        const h = child.naturalHeight || parseInt(child.getAttribute("height") || "0", 10);
        if ((w && w < 80) || (h && h < 60)) continue;
        const img = document.createElement("img");
        img.src = src;
        img.alt = child.getAttribute("alt") || "";
        img.loading = "lazy";
        out.appendChild(img);
        continue;
      }
      if (tag === "PICTURE") {
        const inner = child.querySelector("img");
        if (inner) build({ childNodes: [inner] }, out, ctx);
        continue;
      }
      if (tag === "A") {
        const href = abs(child.getAttribute("href") || "");
        const holder = document.createElement(href ? "a" : "span");
        if (href) { holder.href = href; holder.rel = "noopener noreferrer"; }
        build(child, holder, ctx);
        if (holder.textContent.trim() || holder.querySelector("img")) out.appendChild(holder);
        continue;
      }
      if (KEEP.has(tag)) {
        if (tag === "H1" && ctx.title && text(child) === ctx.title) continue;
        const el = document.createElement(tag.toLowerCase());
        if ((tag === "TD" || tag === "TH") && child.colSpan > 1) el.colSpan = child.colSpan;
        if (tag === "OL" && child.start > 1) el.start = child.start;
        build(child, el, ctx);
        const empty = !el.textContent.trim() && !el.querySelector("img, hr") && tag !== "BR" && tag !== "HR";
        if (empty) continue;
        out.appendChild(el);
        continue;
      }
      // Generic container: unwrap it. One that holds only inline content stays a paragraph.
      let hasBlock = false;
      for (const c of child.children) {
        if (BLOCKS.has(c.tagName) || c.querySelector("p, ul, ol, table, blockquote, pre, h1, h2, h3, h4, h5, h6, figure")) {
          hasBlock = true;
          break;
        }
      }
      const display = getComputedStyle(child).display;
      const isBlock = display !== "inline" && display !== "contents" && display !== "inline-block";
      if (!hasBlock && isBlock && tag !== "SPAN") {
        const p = document.createElement("p");
        build(child, p, ctx);
        if (p.textContent.trim() || p.querySelector("img")) out.appendChild(p);
      } else {
        build(child, out, ctx);
      }
    }
  };

  const extract = () => {
    const { el, chars } = measure();
    if (!el || chars < MIN_CHARS) return null;
    const heading = (document.querySelector("article h1, main h1, h1") || {}).textContent;
    let title = meta("og:title", "twitter:title") || (heading || "").replace(/\s+/g, " ").trim() || document.title;
    if (heading && heading.trim() && title.length > heading.trim().length + 3 && title.includes(heading.trim())) {
      title = heading.replace(/\s+/g, " ").trim();
    }
    const site = meta("og:site_name", "application-name");
    const byline = meta("author", "article:author", "twitter:creator", "byline") ||
      text(document.querySelector('[rel="author"], [itemprop="author"], .byline, .author') || document.createElement("i"));
    let date = meta("article:published_time", "date", "datePublished", "og:updated_time");
    const time = document.querySelector("article time[datetime], time[datetime]");
    if (!date && time) date = time.getAttribute("datetime");
    let when = "";
    if (date) {
      const d = new Date(date);
      if (!isNaN(d)) when = d.toLocaleDateString(document.documentElement.lang || undefined, { dateStyle: "long" });
    }
    const body = document.createElement("div");
    build(el, body, { title: title.trim(), top: el });
    // Whitespace-only text between blocks and a leading duplicate of the title go.
    if (body.textContent.trim().length < MIN_CHARS / 2) return null;
    return {
      title: title.trim(), site, byline: byline.slice(0, 120), when, body,
      lang: document.documentElement.lang || meta("og:locale", "content-language").replace("_", "-") || "",
      dir: document.documentElement.dir || "",
    };
  };

  const CSS = `
    :root { color-scheme: light dark; }
    html { background: var(--axr-bg); }
    body { margin: 0; background: var(--axr-bg); color: var(--axr-fg); }
    .axr { box-sizing: border-box; max-width: calc(var(--axr-width) + 64px); margin: 0 auto; padding: 56px 32px 120px;
      font-family: var(--axr-font); font-size: var(--axr-size); line-height: var(--axr-lh);
      -webkit-font-smoothing: antialiased; overflow-wrap: break-word; hyphens: auto; }
    .axr header { margin-bottom: 2.2em; padding-bottom: 1.2em; border-bottom: 1px solid var(--axr-rule); }
    .axr .axr-site { font: 600 0.62em/1.2 system-ui, sans-serif; letter-spacing: .08em; text-transform: uppercase; color: var(--axr-muted); margin-bottom: 1em; }
    .axr h1.axr-title { font-size: 1.75em; line-height: 1.2; margin: 0 0 .5em; font-weight: 700; }
    .axr .axr-meta { font: 0.7em/1.4 system-ui, sans-serif; color: var(--axr-muted); }
    .axr article > :first-child { margin-top: 0; }
    .axr p, .axr ul, .axr ol, .axr dl, .axr pre, .axr blockquote, .axr table, .axr figure { margin: 0 0 1.1em; }
    .axr h2, .axr h3, .axr h4, .axr h5, .axr h6 { line-height: 1.25; margin: 1.6em 0 .5em; }
    .axr h2 { font-size: 1.4em; } .axr h3 { font-size: 1.2em; } .axr h4, .axr h5, .axr h6 { font-size: 1.05em; }
    .axr a { color: var(--axr-link); text-decoration: underline; text-decoration-color: color-mix(in srgb, var(--axr-link) 40%, transparent); text-underline-offset: 2px; }
    .axr img { display: block; max-width: 100%; height: auto; margin: 1.4em auto; border-radius: 8px; }
    .axr figure { margin-left: 0; margin-right: 0; }
    .axr figcaption { font: 0.72em/1.4 system-ui, sans-serif; color: var(--axr-muted); text-align: center; margin-top: -.6em; }
    .axr blockquote { margin-left: 0; padding: .1em 0 .1em 1.1em; border-left: 3px solid var(--axr-rule); color: var(--axr-muted); }
    .axr pre, .axr code, .axr kbd, .axr samp { font: 0.85em/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; }
    .axr pre { overflow-x: auto; padding: .9em 1em; background: var(--axr-code); border-radius: 8px; }
    .axr :not(pre) > code { padding: .1em .35em; background: var(--axr-code); border-radius: 4px; }
    .axr table { border-collapse: collapse; display: block; overflow-x: auto; font-size: .9em; }
    .axr th, .axr td { border: 1px solid var(--axr-rule); padding: .35em .6em; text-align: left; }
    .axr hr { border: 0; border-top: 1px solid var(--axr-rule); margin: 2em 0; }
    .axr li { margin: .25em 0; }
    ::selection { background: color-mix(in srgb, var(--axr-link) 30%, transparent); }
  `;

  const FONTS = {
    serif: 'Charter, "Bitstream Charter", "Sitka Text", Cambria, Georgia, "Noto Serif", serif',
    sans: 'system-ui, -apple-system, "Segoe UI", Roboto, "Noto Sans", sans-serif',
    mono: 'ui-monospace, SFMono-Regular, Menlo, Consolas, "DejaVu Sans Mono", monospace',
  };
  const THEMES = {
    light: { bg: "#ffffff", fg: "#1d1d1f", muted: "#6e6e73", link: "#0066cc", rule: "#e3e3e8", code: "#f4f4f6", scheme: "light" },
    sepia: { bg: "#f4ecd8", fg: "#4b3a2a", muted: "#8a7660", link: "#8a4b14", rule: "#decfae", code: "#eadfc4", scheme: "light" },
    dark: { bg: "#1c1c1e", fg: "#e6e6ea", muted: "#98989f", link: "#64a8ff", rule: "#3a3a3e", code: "#2a2a2d", scheme: "dark" },
  };

  const apply = (options) => {
    if (!root) return false;
    lastOptions = options;
    const o = options || {};
    const theme = THEMES[o.theme] || THEMES.light;
    const st = root.style;
    st.setProperty("--axr-bg", theme.bg);
    st.setProperty("--axr-fg", theme.fg);
    st.setProperty("--axr-muted", theme.muted);
    st.setProperty("--axr-link", theme.link);
    st.setProperty("--axr-rule", theme.rule);
    st.setProperty("--axr-code", theme.code);
    st.setProperty("--axr-font", FONTS[o.font] || FONTS.serif);
    st.setProperty("--axr-size", (o.size || 20) + "px");
    st.setProperty("--axr-lh", String(o.lineHeight || 1.6));
    st.setProperty("--axr-width", (o.width || 680) + "px");
    st.setProperty("color-scheme", theme.scheme);
    return true;
  };

  const enter = (options) => {
    if (original) return apply(options);
    const article = extract();
    if (!article) return false;
    original = document.documentElement;
    scrollBefore = window.scrollY;
    titleBefore = document.title;
    const html = document.createElement("html");
    if (article.lang) html.lang = article.lang;
    if (article.dir) html.dir = article.dir;
    const head = document.createElement("head");
    const charset = document.createElement("meta");
    charset.setAttribute("charset", "utf-8");
    const viewport = document.createElement("meta");
    viewport.name = "viewport";
    viewport.content = "width=device-width, initial-scale=1";
    const title = document.createElement("title");
    title.textContent = article.title || titleBefore;
    head.append(charset, viewport, title);
    const body = document.createElement("body");
    const main = document.createElement("main");
    main.className = "axr";
    const header = document.createElement("header");
    if (article.site) {
      const s = document.createElement("div");
      s.className = "axr-site";
      s.textContent = article.site;
      header.appendChild(s);
    }
    const h1 = document.createElement("h1");
    h1.className = "axr-title";
    h1.textContent = article.title;
    header.appendChild(h1);
    const metaLine = [article.byline, article.when].filter(Boolean).join(" · ");
    if (metaLine) {
      const m = document.createElement("div");
      m.className = "axr-meta";
      m.textContent = metaLine;
      header.appendChild(m);
    }
    const art = document.createElement("article");
    while (article.body.firstChild) art.appendChild(article.body.firstChild);
    main.append(header, art);
    body.appendChild(main);
    html.append(head, body);
    root = html;
    try {
      document.replaceChild(html, original);
      sheet = new CSSStyleSheet();
      sheet.replaceSync(CSS);
      document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
    } catch (e) {
      // Put the page back if anything went wrong.
      try { document.replaceChild(original, document.documentElement); } catch (_) {}
      original = null;
      root = null;
      return false;
    }
    apply(options);
    window.scrollTo(0, 0);
    post({ t: "state", active: true });
    return true;
  };

  const exit = () => {
    if (!original) return false;
    try {
      document.replaceChild(original, document.documentElement);
      if (sheet) document.adoptedStyleSheets = document.adoptedStyleSheets.filter((s) => s !== sheet);
    } catch (_) {}
    original = null;
    root = null;
    sheet = null;
    document.title = titleBefore;
    window.scrollTo(0, scrollBefore);
    post({ t: "state", active: false });
    schedule(0);
    return true;
  };

  let timer = 0;
  const probe = () => {
    timer = 0;
    if (original) { post({ t: "probe", readable: true, active: true }); return; }
    let readable = false;
    try { readable = measure().chars >= MIN_CHARS; } catch (_) {}
    post({ t: "probe", readable, active: false });
  };
  const schedule = (delay) => {
    clearTimeout(timer);
    timer = setTimeout(probe, delay);
  };

  Object.defineProperty(window, "__axionReader", {
    value: Object.freeze({ enter, exit, apply, probe: () => schedule(0) }),
    enumerable: false,
  });

  const start = () => schedule(DELAY);
  if (document.readyState === "complete") start();
  else window.addEventListener("load", start, { once: true });
  document.addEventListener("DOMContentLoaded", () => schedule(DELAY + 600), { once: true });
  // Single-page apps change their text without loading anything.
  for (const name of ["pushState", "replaceState"]) {
    const fn = history[name];
    history[name] = function () {
      const r = fn.apply(this, arguments);
      if (!original) schedule(DELAY + 400);
      return r;
    };
  }
  window.addEventListener("popstate", () => { if (!original) schedule(DELAY + 400); });
})();
