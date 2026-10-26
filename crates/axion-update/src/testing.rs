//! A tiny HTTP server for tests: serves fixed responses by path on 127.0.0.1.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Route {
    path: String,
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

impl Route {
    pub fn json(path: &str, body: &str) -> Self {
        Self {
            path: path.into(),
            status: 200,
            content_type: "application/json",
            body: body.as_bytes().to_vec(),
        }
    }

    pub fn bytes(path: &str, body: Vec<u8>) -> Self {
        Self { path: path.into(), status: 200, content_type: "application/octet-stream", body }
    }

    pub fn text(path: &str, body: &str) -> Self {
        Self { path: path.into(), status: 200, content_type: "text/plain", body: body.as_bytes().to_vec() }
    }

    pub fn status(path: &str, status: u16, body: &str) -> Self {
        Self { path: path.into(), status, content_type: "text/plain", body: body.as_bytes().to_vec() }
    }
}

pub struct TestServer {
    port: u16,
    routes: Arc<Mutex<Vec<Route>>>,
    requests: Arc<AtomicUsize>,
    last_headers: Arc<Mutex<String>>,
    stop: Arc<AtomicBool>,
}

impl TestServer {
    pub fn start(routes: Vec<Route>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        listener.set_nonblocking(true).expect("nonblocking");
        let server = Self {
            port,
            routes: Arc::new(Mutex::new(routes)),
            requests: Arc::new(AtomicUsize::new(0)),
            last_headers: Arc::new(Mutex::new(String::new())),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let (routes, requests, headers, stop) = (
            server.routes.clone(),
            server.requests.clone(),
            server.last_headers.clone(),
            server.stop.clone(),
        );
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        handle(stream, &routes, &requests, &headers);
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
                }
            }
        });
        server
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    pub fn set(&self, routes: Vec<Route>) {
        *self.routes.lock().expect("lock") = routes;
    }

    pub fn add(&self, route: Route) {
        let mut routes = self.routes.lock().expect("lock");
        routes.retain(|r| r.path != route.path);
        routes.push(route);
    }

    pub fn requests(&self) -> usize {
        self.requests.load(Ordering::Relaxed)
    }

    pub fn last_headers(&self) -> String {
        self.last_headers.lock().expect("lock").clone()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn handle(
    mut stream: TcpStream,
    routes: &Mutex<Vec<Route>>,
    requests: &AtomicUsize,
    last_headers: &Mutex<String>,
) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    let mut request = Vec::new();
    let mut buffer = [0u8; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => request.extend_from_slice(&buffer[..n]),
        }
    }
    requests.fetch_add(1, Ordering::Relaxed);
    let text = String::from_utf8_lossy(&request).into_owned();
    *last_headers.lock().expect("lock") = text.clone();
    let target = text.split_whitespace().nth(1).unwrap_or("/");
    let path = target.split('?').next().unwrap_or("/");
    let route = routes.lock().expect("lock").iter().find(|r| r.path == path).cloned();
    let (status, content_type, body) = match route {
        Some(r) => (r.status, r.content_type, r.body),
        None => (404, "text/plain", b"not found".to_vec()),
    };
    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
}
