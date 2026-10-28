//! The local models: which files Mozilla's Remote Settings lists for a language pair, where
//! they are kept in the profile, and the download (only ever started with the user's consent;
//! every file is checked against the SHA-256 the list names).
//!
//! Layout under the store's root:
//! `bergamot.wasm` and, per pair, `pairs/<from>-<to>/{manifest.json, model.bin, lex.bin, vocab.spm}`
//! (or `srcvocab.spm` and `trgvocab.spm`). The manifest is written last, so a pair without one
//! is incomplete.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axion_config::Translation;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::TranslateError;
use crate::lang;

/// The engine release the bundled glue script (`helper/bergamot-translator.js`) belongs to. The
/// `.wasm` must be of the same release.
pub const WASM_RELEASE: &str = "v0.6.0";

const CHUNK: usize = 64 * 1024;
const MAX_LIST_BYTES: u64 = 64 * 1024 * 1024;

/// What a file of a model is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Model,
    Lex,
    Vocab,
    SrcVocab,
    TrgVocab,
    Wasm,
}

impl Role {
    fn parse(file_type: &str) -> Option<Self> {
        Some(match file_type {
            "model" => Role::Model,
            "lex" => Role::Lex,
            "vocab" => Role::Vocab,
            "srcvocab" => Role::SrcVocab,
            "trgvocab" => Role::TrgVocab,
            _ => return None,
        })
    }

    fn local_name(self) -> &'static str {
        match self {
            Role::Model => "model.bin",
            Role::Lex => "lex.bin",
            Role::Vocab => "vocab.spm",
            Role::SrcVocab => "srcvocab.spm",
            Role::TrgVocab => "trgvocab.spm",
            Role::Wasm => "bergamot.wasm",
        }
    }
}

/// One downloadable file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSpec {
    pub role: Role,
    /// The original file name (tells the model's quantisation).
    pub name: String,
    /// Path below the attachments base URL.
    pub location: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Deserialize)]
struct RawList {
    data: Vec<RawRecord>,
}

#[derive(Deserialize)]
struct RawRecord {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "fromLang")]
    from_lang: String,
    #[serde(default, rename = "toLang")]
    to_lang: String,
    #[serde(default, rename = "fileType")]
    file_type: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    release: String,
    attachment: Option<RawAttachment>,
}

#[derive(Deserialize)]
struct RawAttachment {
    hash: String,
    size: u64,
    location: String,
    #[serde(default)]
    filename: String,
}

/// "2.1" → (2, 1). Pre-releases ("2.1a1") and anything else are not versions we use.
fn release_version(text: &str) -> Option<(u32, u32)> {
    let (major, minor) = text.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

fn spec(record: &RawRecord, role: Role) -> Option<FileSpec> {
    let a = record.attachment.as_ref()?;
    Some(FileSpec {
        role,
        name: if a.filename.is_empty() { record.name.clone() } else { a.filename.clone() },
        location: a.location.clone(),
        sha256: a.hash.to_ascii_lowercase(),
        size: a.size,
    })
}

/// The files of the newest released model for `from` → `to` in a `translations-models` list.
pub fn pair_files(list_json: &str, from: &str, to: &str) -> Result<Vec<FileSpec>, TranslateError> {
    let list: RawList =
        serde_json::from_str(list_json).map_err(|e| TranslateError::Catalog(e.to_string()))?;
    let mut best: BTreeMap<&'static str, ((u32, u32), &RawRecord)> = BTreeMap::new();
    for record in &list.data {
        if record.from_lang != from || record.to_lang != to {
            continue;
        }
        let (Some(version), Some(role)) = (release_version(&record.version), Role::parse(&record.file_type))
        else {
            continue;
        };
        let key = role.local_name();
        if best.get(key).is_none_or(|(v, _)| version > *v) {
            best.insert(key, (version, record));
        }
    }
    let mut files = Vec::new();
    for role in [Role::Model, Role::Lex, Role::Vocab, Role::SrcVocab, Role::TrgVocab] {
        if let Some((_, record)) = best.get(role.local_name())
            && let Some(file) = spec(record, role)
        {
            files.push(file);
        }
    }
    let has = |role| files.iter().any(|f| f.role == role);
    let vocab_ok = has(Role::Vocab) || (has(Role::SrcVocab) && has(Role::TrgVocab));
    if !has(Role::Model) || !vocab_ok {
        return Err(TranslateError::NoModel { from: from.to_owned(), to: to.to_owned() });
    }
    Ok(files)
}

/// The engine's `.wasm` of [`WASM_RELEASE`] in a `translations-wasm` list.
pub fn wasm_file(list_json: &str) -> Result<FileSpec, TranslateError> {
    let list: RawList =
        serde_json::from_str(list_json).map_err(|e| TranslateError::Catalog(e.to_string()))?;
    list.data
        .iter()
        .find(|r| r.name == "bergamot-translator" && r.release == WASM_RELEASE)
        .and_then(|r| spec(r, Role::Wasm))
        .ok_or_else(|| TranslateError::Catalog(format!("no engine {WASM_RELEASE} in the list")))
}

/// What a finished pair's `manifest.json` says; the helper page reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// File name the model had originally (`...intgemm8.bin` is quantised differently).
    pub model_name: String,
    pub lex: bool,
    pub vocab: bool,
    pub srcvocab: bool,
    pub trgvocab: bool,
}

/// The downloaded models, in the profile.
#[derive(Clone, Debug)]
pub struct ModelStore {
    root: PathBuf,
}

impl ModelStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn wasm_path(&self) -> PathBuf {
        self.root.join(Role::Wasm.local_name())
    }

    pub fn pair_dir(&self, from: &str, to: &str) -> PathBuf {
        self.root.join("pairs").join(format!("{from}-{to}"))
    }

    pub fn has_wasm(&self) -> bool {
        self.wasm_path().is_file()
    }

    pub fn has_pair(&self, from: &str, to: &str) -> bool {
        self.pair_dir(from, to).join("manifest.json").is_file()
    }

    /// The pairs (and the engine) a translation `from` → `to` still lacks.
    pub fn missing(&self, from: &str, to: &str) -> Option<(bool, Vec<(String, String)>)> {
        let route = lang::route(from, to)?;
        let pairs = route.into_iter().filter(|(a, b)| !self.has_pair(a, b)).collect();
        Some((!self.has_wasm(), pairs))
    }

    /// Whether `from` → `to` can run right now.
    pub fn ready(&self, from: &str, to: &str) -> bool {
        self.missing(from, to).is_some_and(|(wasm, pairs)| !wasm && pairs.is_empty())
    }

    /// Bytes the downloaded models use.
    pub fn installed_bytes(&self) -> u64 {
        fn size(path: &Path) -> u64 {
            let Ok(meta) = std::fs::metadata(path) else { return 0 };
            if meta.is_file() {
                return meta.len();
            }
            std::fs::read_dir(path).map(|dir| dir.flatten().map(|e| size(&e.path())).sum()).unwrap_or(0)
        }
        size(&self.root)
    }

    /// Deletes every downloaded file.
    pub fn remove_all(&self) -> std::io::Result<()> {
        match std::fs::remove_dir_all(&self.root) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// Where a request for models stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    /// Looking up what is needed.
    Planning,
    /// Waiting for the user's consent to download `bytes`.
    NeedsConsent {
        bytes: u64,
    },
    Downloading {
        done: u64,
        total: u64,
    },
    Ready,
    Failed(String),
}

/// A file to fetch, where it goes, and the pair it belongs to (`None`: the engine).
type Item = (PathBuf, FileSpec, Option<(String, String)>);

struct Plan {
    files: Vec<Item>,
    pairs: Vec<(String, String, PathBuf)>,
    bytes: u64,
}

struct Shared {
    phase: Phase,
    plan: Option<Plan>,
    /// Bumped when a request is replaced, so a stale thread can tell.
    generation: u64,
}

/// Plans, asks and downloads the models for one translation at a time.
pub struct ModelManager {
    store: ModelStore,
    shared: Arc<Mutex<Shared>>,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicU64>,
    settings: Translation,
}

impl ModelManager {
    pub fn new(store: ModelStore, settings: Translation) -> Self {
        Self {
            store,
            shared: Arc::new(Mutex::new(Shared { phase: Phase::Idle, plan: None, generation: 0 })),
            cancel: Arc::new(AtomicBool::new(false)),
            done: Arc::new(AtomicU64::new(0)),
            settings,
        }
    }

    pub fn store(&self) -> &ModelStore {
        &self.store
    }

    pub fn set_settings(&mut self, settings: Translation) {
        self.settings = settings;
    }

    pub fn phase(&self) -> Phase {
        let phase = self.shared.lock().unwrap_or_else(PoisonError::into_inner).phase.clone();
        match phase {
            Phase::Downloading { total, .. } => {
                Phase::Downloading { done: self.done.load(Ordering::Relaxed).min(total), total }
            }
            other => other,
        }
    }

    /// Starts working out what `from` → `to` needs; the phase becomes `Ready` at once when the
    /// files are there, else `Planning` and then `NeedsConsent`.
    pub fn request(&mut self, from: &str, to: &str) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        let generation = {
            let mut s = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
            s.generation += 1;
            s.plan = None;
            s.phase = if self.store.ready(from, to) { Phase::Ready } else { Phase::Planning };
            s.generation
        };
        if self.store.ready(from, to) {
            return;
        }
        let Some((need_wasm, pairs)) = self.store.missing(from, to) else {
            self.fail(generation, TranslateError::Unsupported.to_string());
            return;
        };
        let (shared, store, settings) = (self.shared.clone(), self.store.clone(), self.settings.clone());
        let spawned = std::thread::Builder::new().name("translate-plan".into()).spawn(move || {
            let result = make_plan(&store, &settings, need_wasm, &pairs);
            let mut s = shared.lock().unwrap_or_else(PoisonError::into_inner);
            if s.generation != generation {
                return;
            }
            match result {
                Ok(plan) => {
                    s.phase = Phase::NeedsConsent { bytes: plan.bytes };
                    s.plan = Some(plan);
                }
                Err(e) => {
                    tracing::warn!("planning the translation download failed: {e}");
                    s.phase = Phase::Failed(e.to_string());
                }
            }
        });
        if let Err(e) = spawned {
            self.fail(generation, e.to_string());
        }
    }

    fn fail(&self, generation: u64, reason: String) {
        let mut s = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if s.generation == generation {
            s.phase = Phase::Failed(reason);
        }
    }

    /// The user agreed to the download that `NeedsConsent` named.
    pub fn consent(&mut self) {
        let (plan, generation) = {
            let mut s = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(plan) = s.plan.take() else { return };
            s.phase = Phase::Downloading { done: 0, total: plan.bytes };
            (plan, s.generation)
        };
        self.done.store(0, Ordering::Relaxed);
        let (shared, cancel, done) = (self.shared.clone(), self.cancel.clone(), self.done.clone());
        let timeout = Duration::from_secs(u64::from(self.settings.endpoint_timeout_seconds.max(5)));
        let base = self.settings.models_attachments_url.clone();
        let spawned = std::thread::Builder::new().name("translate-download".into()).spawn(move || {
            let result = download_plan(&plan, &base, timeout, &cancel, &done);
            let mut s = shared.lock().unwrap_or_else(PoisonError::into_inner);
            if s.generation != generation {
                return;
            }
            s.phase = match result {
                Ok(()) => Phase::Ready,
                Err(TranslateError::Cancelled) => Phase::Idle,
                Err(e) => {
                    tracing::warn!("downloading translation models failed: {e}");
                    Phase::Failed(e.to_string())
                }
            };
        });
        if let Err(e) = spawned {
            self.fail(generation, e.to_string());
        }
    }

    /// Stops a download or forgets a request.
    pub fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        let mut s = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        s.generation += 1;
        s.plan = None;
        s.phase = Phase::Idle;
    }
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(timeout))
        .timeout_resolve(Some(timeout))
        .timeout_recv_response(Some(timeout))
        .timeout_send_request(Some(timeout))
        .user_agent(concat!("Axion/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn get_list(agent: &ureq::Agent, base: &str, collection: &str) -> Result<String, TranslateError> {
    let url = format!("{}/{collection}/records?_limit=5000", base.trim_end_matches('/'));
    let mut response = agent.get(&url).header("Accept", "application/json").call()?;
    response
        .body_mut()
        .with_config()
        .limit(MAX_LIST_BYTES)
        .read_to_string()
        .map_err(|e| TranslateError::Catalog(e.to_string()))
}

fn make_plan(
    store: &ModelStore,
    settings: &Translation,
    need_wasm: bool,
    pairs: &[(String, String)],
) -> Result<Plan, TranslateError> {
    let agent = agent(Duration::from_secs(u64::from(settings.endpoint_timeout_seconds.max(5))));
    let mut files = Vec::new();
    if need_wasm {
        let list = get_list(&agent, &settings.models_records_url, "translations-wasm")?;
        files.push((store.wasm_path(), wasm_file(&list)?, None));
    }
    if !pairs.is_empty() {
        let list = get_list(&agent, &settings.models_records_url, "translations-models")?;
        for (from, to) in pairs {
            for file in pair_files(&list, from, to)? {
                let path = store.pair_dir(from, to).join(file.role.local_name());
                files.push((path, file, Some((from.clone(), to.clone()))));
            }
        }
    }
    let bytes = files.iter().map(|(_, f, _)| f.size).sum();
    let pairs = pairs.iter().map(|(a, b)| (a.clone(), b.clone(), store.pair_dir(a, b))).collect();
    Ok(Plan { files, pairs, bytes })
}

fn download_plan(
    plan: &Plan,
    base: &str,
    timeout: Duration,
    cancel: &AtomicBool,
    done: &AtomicU64,
) -> Result<(), TranslateError> {
    let agent = agent(timeout);
    let base = base.trim_end_matches('/');
    for (path, file, _) in &plan.files {
        download_file(&agent, &format!("{base}/{}", file.location), path, file, cancel, done)?;
    }
    for (from, to, dir) in &plan.pairs {
        let find = |role| {
            plan.files
                .iter()
                .find(|(_, f, p)| f.role == role && p.as_ref().is_some_and(|(a, b)| a == from && b == to))
        };
        let Some((_, model, _)) = find(Role::Model) else { continue };
        let manifest = Manifest {
            model_name: model.name.clone(),
            lex: find(Role::Lex).is_some(),
            vocab: find(Role::Vocab).is_some(),
            srcvocab: find(Role::SrcVocab).is_some(),
            trgvocab: find(Role::TrgVocab).is_some(),
        };
        let json = serde_json::to_vec(&manifest).map_err(|e| TranslateError::Catalog(e.to_string()))?;
        std::fs::write(dir.join("manifest.json"), json)?;
    }
    Ok(())
}

fn download_file(
    agent: &ureq::Agent,
    url: &str,
    path: &Path,
    file: &FileSpec,
    cancel: &AtomicBool,
    done: &AtomicU64,
) -> Result<(), TranslateError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = path.with_extension("part");
    let result = (|| {
        let mut response = agent.get(url).call()?;
        let mut reader = response.body_mut().as_reader();
        let mut out = std::fs::File::create(&part)?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; CHUNK];
        let mut total = 0u64;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(TranslateError::Cancelled);
            }
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > file.size.saturating_add(1024) {
                return Err(TranslateError::Corrupt(file.name.clone()));
            }
            hasher.update(&buffer[..n]);
            out.write_all(&buffer[..n])?;
            done.fetch_add(n as u64, Ordering::Relaxed);
        }
        out.sync_all()?;
        let actual: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if actual != file.sha256 {
            return Err(TranslateError::Corrupt(file.name.clone()));
        }
        std::fs::rename(&part, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"{"data":[
      {"name":"model.deen.intgemm.alphas.bin","fromLang":"de","toLang":"en","fileType":"model","version":"1.0",
       "attachment":{"hash":"AA","size":10,"location":"a/old.bin","filename":"model.deen.intgemm.alphas.bin"}},
      {"name":"model.deen.intgemm.alphas.bin","fromLang":"de","toLang":"en","fileType":"model","version":"2.0",
       "attachment":{"hash":"BB","size":20,"location":"a/new.bin","filename":"model.deen.intgemm.alphas.bin"}},
      {"name":"model.deen.intgemm.alphas.bin","fromLang":"de","toLang":"en","fileType":"model","version":"2.1a1",
       "attachment":{"hash":"CC","size":30,"location":"a/alpha.bin","filename":"model.deen.intgemm.alphas.bin"}},
      {"name":"lex.deen","fromLang":"de","toLang":"en","fileType":"lex","version":"2.0",
       "attachment":{"hash":"DD","size":5,"location":"a/lex.bin","filename":"lex.deen"}},
      {"name":"vocab.deen.spm","fromLang":"de","toLang":"en","fileType":"vocab","version":"2.0",
       "attachment":{"hash":"EE","size":2,"location":"a/vocab.spm","filename":"vocab.deen.spm"}},
      {"name":"model.enfr.bin","fromLang":"en","toLang":"fr","fileType":"model","version":"2.0",
       "attachment":{"hash":"FF","size":9,"location":"b/m.bin","filename":"model.enfr.bin"}}
    ]}"#;

    #[test]
    fn picks_the_newest_release() {
        let files = pair_files(LIST, "de", "en").unwrap();
        let model = files.iter().find(|f| f.role == Role::Model).unwrap();
        assert_eq!(model.location, "a/new.bin");
        assert_eq!(model.sha256, "bb");
        assert_eq!(files.len(), 3);
    }

    #[test]
    fn incomplete_pairs_are_refused() {
        assert!(matches!(pair_files(LIST, "en", "fr"), Err(TranslateError::NoModel { .. })));
        assert!(matches!(pair_files(LIST, "en", "es"), Err(TranslateError::NoModel { .. })));
    }

    #[test]
    fn wasm_must_match_the_glue() {
        let list = r#"{"data":[
          {"name":"bergamot-translator","release":"v0.5.0","attachment":{"hash":"1","size":1,"location":"x"}},
          {"name":"bergamot-translator","release":"v0.6.0","attachment":{"hash":"2","size":5,"location":"y","filename":"bergamot-translator.wasm"}}]}"#;
        assert_eq!(wasm_file(list).unwrap().location, "y");
        assert!(wasm_file(r#"{"data":[]}"#).is_err());
    }

    #[test]
    fn store_knows_what_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let store = ModelStore::new(dir.path().to_path_buf());
        assert_eq!(store.missing("de", "fr").unwrap().1.len(), 2);
        assert!(!store.ready("de", "en"));
        std::fs::write(store.wasm_path(), b"wasm").unwrap();
        std::fs::create_dir_all(store.pair_dir("de", "en")).unwrap();
        std::fs::write(store.pair_dir("de", "en").join("manifest.json"), b"{}").unwrap();
        assert!(store.ready("de", "en"));
        assert_eq!(store.missing("de", "fr").unwrap(), (false, vec![("en".into(), "fr".into())]));
        assert!(store.installed_bytes() > 0);
        store.remove_all().unwrap();
        assert!(!store.ready("de", "en"));
    }
}
