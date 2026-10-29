//! End-to-end test aid for the WireGuard backend: a loopback WireGuard server whose "inside"
//! knows one name (`only-in-tunnel.test`, in no real DNS) and serves one web page there. It
//! writes a client `.conf` for it; import that in the browser (or `--open=wg-test:PATH` on a
//! throwaway profile) and open `http://only-in-tunnel.test/`: it only loads through the tunnel.
//! Stop this program and the page must turn into the kill-switch page.
//!
//! `cargo run -p axion-net --example wg_world -- CONF_PATH [UDP_PORT]`

use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axion_net::testing::http_server;
use axion_net::wg_testing::{WgTestServer, WgTestServerConfig, public_key_of, random_private_key};
use base64::Engine as _;

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let conf_path = args.next().ok_or_else(|| std::io::Error::other("usage: wg_world CONF_PATH [UDP_PORT]"))?;
    let port: u16 = args.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let web = http_server(
        "<!doctype html><title>wg</title><h1 style='font:48px sans-serif'>WG-TUNNEL-OK</h1>",
        "127.0.0.1:0".parse().map_err(std::io::Error::other)?,
    )?;
    let web_addr: SocketAddr = ([127, 0, 0, 1], web.addr().port()).into();
    let client_private = random_private_key();
    let server_private = random_private_key();
    let forward: axion_net::wg_testing::Forward = Arc::new(move |_| Some(web_addr));
    let mut config = WgTestServerConfig::new(server_private, public_key_of(&client_private), forward);
    config.bind = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    config.tcp_ports = vec![(80, 32)];
    config.names = vec![("only-in-tunnel.test".into(), IpAddr::V4(Ipv4Addr::new(10, 77, 0, 9)))];
    let server = WgTestServer::start(config)?;
    let b64 = |k: &[u8; 32]| base64::engine::general_purpose::STANDARD.encode(k);
    let conf = format!(
        "[Interface]\nPrivateKey = {}\nAddress = 10.9.0.2/32\nDNS = 10.9.0.1\n\n[Peer]\nPublicKey = {}\nAllowedIPs = 0.0.0.0/0\nEndpoint = {}\nPersistentKeepalive = 25\n",
        b64(&client_private),
        b64(&server.public_key()),
        server.addr()
    );
    std::fs::write(&conf_path, conf)?;
    println!("wg_endpoint={} conf={conf_path}", server.addr());
    std::io::stdout().flush()?;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let q = server.stats.dns_queries.lock().map(|q| q.len()).unwrap_or(0);
        eprintln!(
            "datagrams={} dns_queries={q}",
            server.stats.datagrams.load(std::sync::atomic::Ordering::Relaxed)
        );
    }
}
