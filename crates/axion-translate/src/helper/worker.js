// One translation worker: a Bergamot engine instance (WebAssembly) on its own thread.
// The engine page compiles the .wasm once and hands the module to every worker, so a worker
// only instantiates it. Messages in: {op:"init", module}, {op:"model", from, to},
// {op:"translate", id, from, to, texts}. Out: {t:"ready"}, {t:"model", from, to, ms},
// {t:"done", id, texts, ms}, {t:"error", id, message}.
importScripts("/bergamot.js");
(() => {
  "use strict";
  const ALIGN = { model: 256, lex: 64, vocab: 64, srcvocab: 64, trgvocab: 64 };
  const KEEP_MODELS = 3;

  let enginePromise = null;
  let compiled = null;
  const models = new Map(); // "de-en" -> TranslationModel, least recently used first
  let service = null;
  let queue = Promise.resolve();

  const bytes = async (path) => {
    const response = await fetch(path);
    if (!response.ok) throw new Error("missing " + path);
    return new Uint8Array(await response.arrayBuffer());
  };

  const engine = () => {
    if (!enginePromise) {
      enginePromise = new Promise((resolve, reject) => {
        const module = loadBergamot({
          INITIAL_MEMORY: 41943040,
          print: () => {},
          onAbort: () => reject(new Error("the engine could not start")),
          // The module is compiled already (once, by the page): only instantiate it.
          instantiateWasm: (imports, done) => {
            const instance = new WebAssembly.Instance(compiled, imports);
            done(instance, compiled);
            return instance.exports;
          },
          onRuntimeInitialized: async () => {
            await Promise.resolve();
            resolve(module);
          },
        });
      });
      enginePromise.catch(() => { enginePromise = null; });
    }
    return enginePromise;
  };

  const aligned = (bergamot, data, alignment) => {
    const memory = new bergamot.AlignedMemory(data.byteLength, alignment);
    memory.getByteArrayView().set(data);
    return memory;
  };

  const config = (precision) => {
    const indent = "            ";
    const entries = {
      "beam-size": "1", normalize: "1.0", "word-penalty": "0", "max-length-break": "128",
      "mini-batch-words": "1024", workspace: "128", "max-length-factor": "2.0", "skip-cost": "true",
      "cpu-threads": "0", quiet: "true", "quiet-translation": "true", "gemm-precision": precision,
      alignment: "soft",
    };
    return "\n" + Object.entries(entries).map(([k, v]) => `${indent}${k}: ${v}\n`).join("") + indent;
  };

  const loadModel = async (bergamot, from, to) => {
    const key = from + "-" + to;
    if (models.has(key)) {
      const model = models.get(key);
      models.delete(key);
      models.set(key, model);
      return model;
    }
    const base = "/api/model/" + key + "/";
    const manifest = await (await fetch(base + "manifest.json")).json();
    // The files are independent: fetch them together.
    const wantVocab = manifest.vocab ? ["vocab.spm"] : ["srcvocab.spm", "trgvocab.spm"];
    if (!manifest.vocab && !(manifest.srcvocab && manifest.trgvocab)) throw new Error("the model has no vocabulary");
    const [modelBytes, lexBytes, ...vocabBytes] = await Promise.all([
      bytes(base + "model.bin"),
      manifest.lex ? bytes(base + "lex.bin") : Promise.resolve(null),
      ...wantVocab.map((name) => bytes(base + name)),
    ]);
    const model = aligned(bergamot, modelBytes, ALIGN.model);
    const lex = lexBytes ? aligned(bergamot, lexBytes, ALIGN.lex) : null;
    const vocabs = new bergamot.AlignedMemoryList();
    vocabBytes.forEach((data, i) => {
      vocabs.push_back(aligned(bergamot, data, manifest.vocab ? ALIGN.vocab : (i ? ALIGN.trgvocab : ALIGN.srcvocab)));
    });
    const precision = (manifest.model_name || "").endsWith("intgemm8.bin") ? "int8shiftAll" : "int8shiftAlphaAll";
    const made = new bergamot.TranslationModel(from, to, config(precision), model, lex, vocabs, null);
    models.set(key, made);
    while (models.size > KEEP_MODELS) {
      const [oldKey, old] = models.entries().next().value;
      models.delete(oldKey);
      try { old.delete(); } catch (_) {}
    }
    return made;
  };

  const route = (from, to) => (from === "en" || to === "en" ? [[from, to]] : [[from, "en"], ["en", to]]);

  const load = async (from, to) => {
    const bergamot = await engine();
    const chain = [];
    for (const [a, b] of route(from, to)) chain.push(await loadModel(bergamot, a, b));
    return { bergamot, chain };
  };

  const translate = async ({ from, to, texts }) => {
    const { bergamot, chain } = await load(from, to);
    // One service for the life of the worker.
    if (!service) service = new bergamot.BlockingService({ cacheSize: 0 });
    const messages = new bergamot.VectorString();
    const options = new bergamot.VectorResponseOptions();
    const index = [];
    texts.forEach((text, i) => {
      // Empty paragraphs break the engine.
      if (text && text.trim()) {
        messages.push_back(text);
        options.push_back({ qualityScores: false, alignment: false, html: false });
        index.push(i);
      }
    });
    const out = texts.slice();
    try {
      if (index.length) {
        const responses = chain.length === 1
          ? service.translate(chain[0], messages, options)
          : service.translateViaPivoting(chain[0], chain[1], messages, options);
        for (let k = 0; k < index.length; k++) out[index[k]] = responses.get(k).getTranslatedText();
        responses.delete();
      }
    } finally {
      messages.delete();
      options.delete();
    }
    return out;
  };

  self.onmessage = (event) => {
    const m = event.data;
    if (m.op === "init") {
      compiled = m.module;
      postMessage({ t: "ready" });
      return;
    }
    queue = queue.then(async () => {
      const started = performance.now();
      try {
        if (m.op === "model") {
          await load(m.from, m.to);
          postMessage({ t: "model", from: m.from, to: m.to, ms: Math.round(performance.now() - started) });
        } else if (m.op === "translate") {
          const texts = await translate(m);
          postMessage({ t: "done", id: m.id, texts, ms: Math.round(performance.now() - started) });
        }
      } catch (e) {
        postMessage({ t: "error", id: m.id, message: String((e && e.message) || e) });
      }
    });
  };
})();
