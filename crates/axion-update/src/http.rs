//! The one place that talks to the network: plain `ureq`, a short timeout, `Axion/<version>`
//! as the user agent, nothing else sent (no cookies, no identifiers).

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axion_config::Updates;

use crate::UpdateError;

/// The bigger of the documents we read as text (release lists, manifests, `SHA256SUMS`).
const MAX_DOCUMENT_BYTES: u64 = 4 * 1024 * 1024;
/// A download is written in chunks of this size (and cancelled between them).
const CHUNK: usize = 64 * 1024;

pub struct Http {
    agent: ureq::Agent,
    max_download: u64,
    max_download_mb: u32,
}

impl Http {
    /// `user_agent` is `Axion/<version>`; timeouts and limits come from `[updates]`.
    pub fn new(updates: &Updates, user_agent: &str) -> Self {
        let timeout = Duration::from_secs(u64::from(updates.timeout_seconds.max(1)));
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(timeout))
            .timeout_resolve(Some(timeout))
            // A check is small; the body of a download is limited by size instead.
            .timeout_recv_response(Some(timeout))
            .timeout_send_request(Some(timeout))
            .user_agent(user_agent)
            .build()
            .into();
        Self {
            agent,
            max_download: u64::from(updates.max_download_mb) * 1024 * 1024,
            max_download_mb: updates.max_download_mb,
        }
    }

    /// GET `url` as text (JSON documents, checksum files).
    pub fn get_text(&self, url: &str) -> Result<String, UpdateError> {
        let mut response =
            self.agent.get(url).header("Accept", "application/json, text/plain, */*").call()?;
        Ok(response.body_mut().with_config().limit(MAX_DOCUMENT_BYTES).read_to_string()?)
    }

    /// GET `url` to the file at `path`, calling `progress(done, total)` after each chunk.
    /// Stops with [`UpdateError::Cancelled`] once `cancel` is set. The file is removed again
    /// on any failure.
    pub fn download(
        &self,
        url: &str,
        path: &Path,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), UpdateError> {
        let result = self.download_inner(url, path, cancel, progress);
        if result.is_err() {
            // Best effort: a half-written file is useless.
            let _ = std::fs::remove_file(path);
        }
        result
    }

    fn download_inner(
        &self,
        url: &str,
        path: &Path,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), UpdateError> {
        let mut response = self.agent.get(url).call()?;
        let total = response.body().content_length();
        if total.is_some_and(|t| t > self.max_download) {
            return Err(UpdateError::TooLarge { limit_mb: self.max_download_mb });
        }
        let mut reader = response.body_mut().as_reader();
        let mut file = std::fs::File::create(path).map_err(|e| UpdateError::io_path("creating", path, e))?;
        let mut buffer = vec![0u8; CHUNK];
        let mut done = 0u64;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(UpdateError::Cancelled);
            }
            let n = reader.read(&mut buffer).map_err(|e| UpdateError::io("downloading", e))?;
            if n == 0 {
                break;
            }
            done += n as u64;
            if done > self.max_download {
                return Err(UpdateError::TooLarge { limit_mb: self.max_download_mb });
            }
            file.write_all(&buffer[..n]).map_err(|e| UpdateError::io_path("writing", path, e))?;
            progress(done, total);
        }
        file.sync_all().map_err(|e| UpdateError::io_path("writing", path, e))?;
        Ok(())
    }
}
