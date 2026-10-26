// Reports the page's icon links (with their media queries) and theme colours to the engine,
// once the document is parsed and whenever they change (pages like GitHub swap their icon
// when the colour scheme changes). Injected into top-level documents only.
// Format (one console message): MARKER, then lines "i\t<url>\t<media>\t<sizes>\t<type>" and
// "c\t<r>,<g>,<b>\t<media>".
(() => {
  if (window.top !== window || window.__axionMeta) return;
  window.__axionMeta = true;
  const MARKER = "\u0001axion-meta:";
  const clean = (s) => String(s || "").replace(/[\t\n\r]/g, " ").trim();
  const ctx = document.createElement("canvas").getContext("2d");
  const rgb = (value) => {
    if (!ctx || !value) return null;
    ctx.fillStyle = "#010203";
    ctx.fillStyle = value;
    const c = ctx.fillStyle;
    if (c === "#010203" && value.trim().toLowerCase() !== "#010203") return null;
    let m = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(c);
    if (m) return [m[1], m[2], m[3]].map((h) => parseInt(h, 16)).join(",");
    m = /^rgba?\((\d+),\s*(\d+),\s*(\d+)(?:,\s*([\d.]+))?\)$/.exec(c);
    if (m && (m[4] === undefined || parseFloat(m[4]) > 0.5)) return [m[1], m[2], m[3]].join(",");
    return null;
  };
  let last = "";
  const report = () => {
    const lines = [];
    for (const link of document.querySelectorAll("link[rel][href]")) {
      const rel = (link.getAttribute("rel") || "").toLowerCase().split(/\s+/);
      if (!rel.includes("icon") && !rel.includes("apple-touch-icon")) continue;
      lines.push(["i", clean(link.href), clean(link.media), clean(link.getAttribute("sizes")), clean(link.getAttribute("type"))].join("\t"));
    }
    for (const meta of document.querySelectorAll('meta[name="theme-color" i][content]')) {
      const c = rgb(meta.getAttribute("content"));
      if (c) lines.push(["c", c, clean(meta.media)].join("\t"));
    }
    const text = lines.join("\n");
    if (text === last) return;
    last = text;
    console.debug(MARKER + text);
  };
  let pending = 0;
  const soon = () => {
    if (!pending) pending = setTimeout(() => { pending = 0; report(); }, 120);
  };
  const watch = () => {
    report();
    if (document.head) {
      new MutationObserver(soon).observe(document.head, {
        childList: true, subtree: true, attributes: true,
        attributeFilter: ["href", "rel", "media", "content", "sizes"],
      });
    }
    try {
      matchMedia("(prefers-color-scheme: dark)").addEventListener("change", soon);
    } catch (_) {}
  };
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", watch, { once: true });
  } else {
    watch();
  }
})();
