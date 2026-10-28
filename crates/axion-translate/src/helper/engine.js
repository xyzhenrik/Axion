// The translation engine page: a pool of workers, each running Bergamot (WebAssembly) on the
// models Axion downloaded. The page compiles the .wasm once and shares the module.
// Browser -> page: window.__axionEngine.job({tag, from, to, texts, workers}) and
//   .prewarm({from, to, workers}) (run as scripts).
// Page -> browser: console.debug of "\u0001axion:trh:" + JSON:
//   {t:"ready"}  {t:"done", tag, texts}  {t:"error", tag, message}  {t:"log", what, ms}
(() => {
  "use strict";
  const MARKER = "\u0001axion:trh:";
  const say = (message) => console.debug(MARKER + JSON.stringify(message));
  const log = (what, started) => say({ t: "log", what, ms: Math.round(performance.now() - started) });
  const MAX_WORKERS = 4;
  // Texts of one job are only spread over several workers when there is enough of them.
  const SPLIT_CHARS = 400;

  const pageStart = performance.now();
  let wanted = 0; // workers wanted
  let modulePromise = null;
  const workers = []; // {worker, ready, busy, chunk, models:Set}
  const chunks = []; // waiting chunks {job, indices}
  const jobs = new Map(); // tag -> {job, out, left, failed}
  const warm = new Set(); // "de-en" the workers were asked to load
  let nextChunk = 1;

  const compile = () => {
    if (!modulePromise) {
      const started = performance.now();
      modulePromise = fetch("/api/wasm")
        .then((r) => { if (!r.ok) throw new Error("missing the engine"); return r.arrayBuffer(); })
        .then((b) => WebAssembly.compile(b))
        .then((m) => { log("engine compiled", started); return m; });
      modulePromise.catch(() => { modulePromise = null; });
    }
    return modulePromise;
  };

  const count = (requested) => {
    const auto = Math.floor((navigator.hardwareConcurrency || 2) / 2);
    const n = requested > 0 ? requested : auto;
    return Math.max(1, Math.min(MAX_WORKERS, n));
  };

  const fail = (job, message) => {
    if (!jobs.delete(job.tag)) return;
    for (let i = chunks.length - 1; i >= 0; i--) if (chunks[i].job === job) chunks.splice(i, 1);
    say({ t: "error", tag: job.tag, message });
  };

  const spawn = () => {
    const started = performance.now();
    const w = { worker: new Worker("/worker.js"), ready: false, busy: false, chunk: null, models: new Set() };
    w.worker.onmessage = (event) => {
      const m = event.data;
      if (m.t === "ready") {
        w.ready = true;
        log("worker started", started);
        for (const key of warm) w.worker.postMessage({ op: "model", from: key.split("-")[0], to: key.split("-")[1] });
        dispatch();
      } else if (m.t === "model") {
        w.models.add(m.from + "-" + m.to);
        log("model loaded in a worker (" + m.from + "-" + m.to + ")", started);
      } else if (m.t === "done" && w.chunk) {
        const { chunk } = w;
        w.chunk = null;
        w.busy = false;
        const entry = jobs.get(chunk.job.tag);
        if (entry) {
          chunk.indices.forEach((index, k) => { entry.out[index] = m.texts[k]; });
          if (--entry.left === 0) {
            jobs.delete(chunk.job.tag);
            log("job translated (" + chunk.job.texts.length + " pieces)", entry.started);
            say({ t: "done", tag: chunk.job.tag, texts: entry.out });
          }
        }
        dispatch();
      } else if (m.t === "error") {
        const chunk = w.chunk;
        w.chunk = null;
        w.busy = false;
        if (chunk) fail(chunk.job, m.message);
        dispatch();
      }
    };
    w.worker.onerror = (event) => {
      const chunk = w.chunk;
      w.chunk = null;
      w.busy = true; // a dead worker takes no more work
      if (chunk) fail(chunk.job, String((event && event.message) || "a translation worker stopped"));
    };
    compile().then((module) => w.worker.postMessage({ op: "init", module }), (e) => {
      for (const job of [...jobs.values()].map((j) => j.job)) fail(job, String((e && e.message) || e));
    });
    workers.push(w);
    return w;
  };

  const ensureWorkers = (n) => {
    wanted = Math.max(wanted, n);
    while (workers.length < wanted) spawn();
  };

  const dispatch = () => {
    for (const w of workers) {
      if (!chunks.length) return;
      if (!w.ready || w.busy) continue;
      const chunk = chunks.shift();
      w.busy = true;
      w.chunk = chunk;
      w.worker.postMessage({
        op: "translate", id: nextChunk++, from: chunk.job.from, to: chunk.job.to,
        texts: chunk.indices.map((i) => chunk.job.texts[i]),
      });
    }
  };

  // Splits a job's texts into balanced chunks (longest first onto the least loaded).
  const split = (texts, parts) => {
    const order = texts.map((t, i) => [t.length, i]).sort((a, b) => b[0] - a[0]);
    const loads = new Array(parts).fill(0);
    const groups = Array.from({ length: parts }, () => []);
    for (const [len, i] of order) {
      let best = 0;
      for (let p = 1; p < parts; p++) if (loads[p] < loads[best]) best = p;
      loads[best] += len + 1;
      groups[best].push(i);
    }
    return groups.filter((g) => g.length).map((g) => g.sort((a, b) => a - b));
  };

  const prewarm = ({ from, to, workers: requested }) => {
    ensureWorkers(count(requested));
    for (const [a, b] of from === "en" || to === "en" ? [[from, to]] : [[from, "en"], ["en", to]]) {
      const key = a + "-" + b;
      if (warm.has(key)) continue;
      warm.add(key);
      for (const w of workers) if (w.ready) w.worker.postMessage({ op: "model", from: a, to: b });
    }
    compile().catch(() => {});
  };

  const job = (job) => {
    const n = count(job.workers);
    prewarm({ from: job.from, to: job.to, workers: n });
    const chars = job.texts.reduce((sum, t) => sum + t.length, 0);
    const parts = chars < SPLIT_CHARS ? 1 : Math.min(n, job.texts.length);
    const groups = split(job.texts, Math.max(parts, 1));
    jobs.set(job.tag, { job, out: job.texts.slice(), left: groups.length, started: performance.now() });
    if (!groups.length) {
      jobs.delete(job.tag);
      say({ t: "done", tag: job.tag, texts: job.texts.slice() });
      return;
    }
    for (const indices of groups) chunks.push({ job, indices });
    dispatch();
  };

  window.__axionEngine = Object.freeze({
    job(j) {
      try { job(j); } catch (e) { say({ t: "error", tag: j.tag, message: String((e && e.message) || e) }); }
    },
    prewarm(p) {
      try { prewarm(p); } catch (e) { say({ t: "log", what: "prewarm failed: " + e, ms: 0 }); }
    },
  });
  say({ t: "ready" });
  log("engine page ready", pageStart);
})();
