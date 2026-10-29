//! End-to-end test aid: a tiny web server and a SOCKS5 proxy that sends every request to it,
//! whatever name was asked for. Run it, point the browser's proxy at the printed port and open
//! `http://some-name.test:8080/`: the page only loads if the browser sent the name to the proxy.
//!
//! `cargo run -p axion-net --example mock_world -- [AUTH_USER:AUTH_PASSWORD] [LOG_FILE]`

use std::io::Write;
use std::time::Duration;

use axion_net::Credentials;
use axion_net::testing::{Upstream, http_server, mock_socks5};

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let auth = args.next().filter(|a| a.contains(':')).and_then(|a| {
        let (u, p) = a.split_once(':')?;
        Some(Credentials::new(u, p))
    });
    let log = args.next();
    let web = http_server(
        "<!doctype html><title>mock</title><h1 style='font:48px sans-serif'>PROXIED-OK</h1>",
        "0.0.0.0:0".parse().map_err(std::io::Error::other)?,
    )?;
    let proxy =
        mock_socks5(auth, Upstream { connect_all_to: Some(([127, 0, 0, 1], web.addr().port()).into()) })?;
    println!("proxy_port={} web_port={}", proxy.addr().port(), web.addr().port());
    std::io::stdout().flush()?;
    let mut seen = 0;
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let requests = proxy.requests();
        let web_requests = web.requests();
        if let Some(path) = &log {
            let text = format!("proxy:\n{}\nweb:\n{}\n", requests.join("\n"), web_requests.join("\n"));
            std::fs::write(path, text)?;
        }
        if requests.len() != seen {
            seen = requests.len();
            println!("proxy requests: {requests:?}");
        }
    }
}
