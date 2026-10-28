//! The optional endpoint backend: a LibreTranslate-compatible server the user runs or trusts
//! (`translation.endpoint_url`). Off by default; page text is sent to that server and nowhere
//! else.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use axion_config::Translation;
use serde::Deserialize;

use crate::{Job, Outcome, Translator};

pub struct EndpointTranslator {
    url: String,
    api_key: String,
    timeout: Duration,
    tx: Sender<Outcome>,
    rx: Receiver<Outcome>,
}

#[derive(Deserialize)]
struct Reply {
    #[serde(rename = "translatedText")]
    translated: Vec<String>,
}

impl EndpointTranslator {
    /// `None` unless `settings.endpoint_url` is a web address.
    pub fn new(settings: &Translation) -> Option<Self> {
        let url = settings.endpoint_url.trim();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return None;
        }
        let (tx, rx) = channel();
        Some(Self {
            url: url.to_owned(),
            api_key: settings.endpoint_api_key.clone(),
            timeout: Duration::from_secs(u64::from(settings.endpoint_timeout_seconds.max(1))),
            tx,
            rx,
        })
    }
}

fn request(url: &str, api_key: &str, timeout: Duration, job: &Job) -> Result<Vec<String>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .user_agent(concat!("Axion/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let mut body = serde_json::json!({
        "q": job.texts, "source": job.from, "target": job.to, "format": "text",
    });
    if !api_key.is_empty() {
        body["api_key"] = api_key.into();
    }
    let mut response = agent
        .post(url)
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .map_err(|e| e.to_string())?;
    let text = response.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let reply: Reply = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if reply.translated.len() != job.texts.len() {
        return Err("the server answered with a different number of texts".into());
    }
    Ok(reply.translated)
}

impl Translator for EndpointTranslator {
    fn name(&self) -> &'static str {
        "endpoint"
    }

    /// The server decides; any two different languages are tried.
    fn supports(&self, from: &str, to: &str) -> bool {
        from != to && !from.is_empty() && !to.is_empty()
    }

    fn submit(&mut self, job: Job) {
        let (url, key, timeout, tx) = (self.url.clone(), self.api_key.clone(), self.timeout, self.tx.clone());
        let tag = job.tag;
        let spawned = std::thread::Builder::new().name("translate-endpoint".into()).spawn(move || {
            let outcome = match request(&url, &key, timeout, &job) {
                Ok(texts) => Outcome::Done { tag, texts },
                Err(reason) => Outcome::Failed { tag, reason },
            };
            let _ = tx.send(outcome);
        });
        if let Err(e) = spawned {
            let _ = self.tx.send(Outcome::Failed { tag, reason: e.to_string() });
        }
    }

    fn poll(&mut self) -> Vec<Outcome> {
        self.rx.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn needs_a_web_address() {
        let mut s = Translation::default();
        assert!(EndpointTranslator::new(&s).is_none());
        s.endpoint_url = "ftp://x".into();
        assert!(EndpointTranslator::new(&s).is_none());
        s.endpoint_url = "http://localhost:5000/translate".into();
        assert!(EndpointTranslator::new(&s).is_some());
    }

    #[test]
    fn talks_to_a_libretranslate_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            let mut buf = vec![0u8; 8192];
            // The headers and the body may arrive in separate reads.
            while !request.contains(r#""target""#) {
                let n = stream.read(&mut buf).unwrap();
                assert!(n > 0, "the request ended early");
                request.push_str(&String::from_utf8_lossy(&buf[..n]));
            }
            let body = r#"{"translatedText":["Hello","World"]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            request
        });
        let settings = Translation {
            endpoint_url: format!("http://127.0.0.1:{port}/translate"),
            ..Translation::default()
        };
        let mut t = EndpointTranslator::new(&settings).unwrap();
        t.submit(Job {
            tag: 9,
            from: "de".into(),
            to: "en".into(),
            texts: vec!["Hallo".into(), "Welt".into()],
        });
        let mut got = Vec::new();
        for _ in 0..100 {
            got = t.poll();
            if !got.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(got, vec![Outcome::Done { tag: 9, texts: vec!["Hello".into(), "World".into()] }]);
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /translate"));
        assert!(request.contains(r#""source":"de""#));
    }
}
