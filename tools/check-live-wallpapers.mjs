// Live-wallpaper check against a running Axion (start it with AXION_CHROMIUM_SWITCHES=remote-debugging-port=9333,
// a throwaway AXION_PROFILE_DIR/AXION_CONFIG_DIR, and axion://home/ open):
//   node tools/check-live-wallpapers.mjs 9333 clip.webm clip.mp4 anim.gif ...
// Imports each file through the start page's path field (the real importer) and prints the wallpaper
// video's currentTime/paused/readyState and any hint, so you can see whether it plays.
const port = process.argv[2];
const files = process.argv.slice(3);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function targets() {
  for (let i = 0; i < 60; i++) {
    try {
      const t = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
      const p = t.find((x) => x.type === "page" && x.url.startsWith("axion://home"));
      if (p) return p;
    } catch {}
    await sleep(500);
  }
  throw new Error("no target");
}
const t = await targets();
const ws = new WebSocket(t.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let id = 0;
const pending = new Map();
ws.onmessage = (m) => {
  const d = JSON.parse(m.data);
  if (d.id && pending.has(d.id)) { pending.get(d.id)(d); pending.delete(d.id); }
  else if (d.method === "Runtime.consoleAPICalled") console.log("CONSOLE", d.params.type, d.params.args.map((a) => a.value ?? a.description).join(" "));
  else if (d.method === "Runtime.exceptionThrown") console.log("EXC", JSON.stringify(d.params.exceptionDetails).slice(0, 400));
  else if (d.method === "Log.entryAdded") console.log("LOG", d.params.entry.level, d.params.entry.text, d.params.entry.url || "");
};
const send = (method, params = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
const ev = async (expr) => { const r = await send("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true }); return r.result?.result?.value ?? r.result; };
await send("Runtime.enable"); await send("Log.enable");
const probe = `(() => { const v = document.getElementById('wallpaper'); const b = document.getElementById('backdrop'); return JSON.stringify({ has: !!v, t: v && v.currentTime, paused: v && v.paused, rs: v && v.readyState, err: v && v.error && v.error.code, vw: v && v.videoWidth, vis: document.visibilityState, bg: b.style.backgroundImage.slice(0, 60), note: document.getElementById('bg-note').textContent, hidden: document.getElementById('bg-note').hidden, hint: document.getElementById('bg-hint').hidden ? '' : document.getElementById('bg-hint').textContent }); })()`;
for (const f of files) {
  if (f !== "-") {
    console.log("IMPORT", f);
    await ev(`(() => { const i = document.getElementById('path'); i.value = ${JSON.stringify(f)}; document.getElementById('path-form').requestSubmit(); })()`);
    await sleep(2500);
  }
  for (let k = 0; k < 3; k++) { console.log("PROBE", await ev(probe)); await sleep(700); }
}
ws.close();
process.exit(0);
