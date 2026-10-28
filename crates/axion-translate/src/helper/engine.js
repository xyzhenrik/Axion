// The translation engine page: runs Bergamot (WebAssembly) on the models Axion downloaded.
// Browser -> page: window.__axionEngine.job({tag, from, to, texts}) (run as a script).
// Page -> browser: console.debug of "\u0001axion:trh:" + JSON:
//   {t:"ready"}  {t:"done", tag, texts}  {t:"error", tag, message}
(() => {
  "use strict";
  const MARKER = "\u0001axion:trh:";
  const say = (message) => console.debug(MARKER + JSON.stringify(message));
  const ALIGN = { model: 256, lex: 64, vocab: 64, srcvocab: 64, trgvocab: 64 };
  const KEEP_MODELS = 3;

  let enginePromise = null;
  const models = new Map(); // "de-en" -> TranslationModel, least recently used first
  let queue = Promise.resolve();

  const bytes = async (path) => {
    const response = await fetch(path);
    if (!response.ok) throw new Error("missing " + path);
    return new Uint8Array(await response.arrayBuffer());
  };

  const engine = () => {
    if (!enginePromise) {
      enginePromise = (async () => {
        const wasmBinary = await bytes("/api/wasm");
        return await new Promise((resolve, reject) => {
          const module = loadBergamot({
            INITIAL_MEMORY: 41943040,
            print: () => {},
            onAbort: () => reject(new Error("the engine could not start")),
            onRuntimeInitialized: async () => {
              await Promise.resolve();
              resolve(module);
            },
            wasmBinary,
          });
        });
      })();
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
    const model = aligned(bergamot, await bytes(base + "model.bin"), ALIGN.model);
    const lex = manifest.lex ? aligned(bergamot, await bytes(base + "lex.bin"), ALIGN.lex) : null;
    const vocabs = new bergamot.AlignedMemoryList();
    if (manifest.vocab) {
      vocabs.push_back(aligned(bergamot, await bytes(base + "vocab.spm"), ALIGN.vocab));
    } else if (manifest.srcvocab && manifest.trgvocab) {
      vocabs.push_back(aligned(bergamot, await bytes(base + "srcvocab.spm"), ALIGN.srcvocab));
      vocabs.push_back(aligned(bergamot, await bytes(base + "trgvocab.spm"), ALIGN.trgvocab));
    } else {
      throw new Error("the model has no vocabulary");
    }
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

  const translate = async ({ from, to, texts }) => {
    const bergamot = await engine();
    const route = from === "en" || to === "en" ? [[from, to]] : [[from, "en"], ["en", to]];
    const chain = [];
    for (const [a, b] of route) chain.push(await loadModel(bergamot, a, b));
    const service = new bergamot.BlockingService({ cacheSize: 0 });
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
      try { service.delete(); } catch (_) {}
    }
    return out;
  };

  window.__axionEngine = Object.freeze({
    job(job) {
      queue = queue.then(async () => {
        try {
          say({ t: "done", tag: job.tag, texts: await translate(job) });
        } catch (e) {
          say({ t: "error", tag: job.tag, message: String((e && e.message) || e) });
        }
        // Let the page breathe between jobs.
        await new Promise((r) => setTimeout(r, 0));
      });
    },
  });
  say({ t: "ready" });
})();
