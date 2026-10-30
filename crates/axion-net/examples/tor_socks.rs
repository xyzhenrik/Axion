//! Starts the Tor backend alone and prints its loopback SOCKS5 endpoint, for a manual check:
//!
//! ```sh
//! cargo run -p axion-net --example tor_socks --release -- /tmp/axion-tor-dir
//! # in another shell, with the printed login:
//! curl --socks5-hostname USER:PASS@127.0.0.1:PORT https://check.torproject.org/api/ip
//! ```
//!
//! The directory holds Arti's state and cache. It stays up for 10 minutes (or until Ctrl-C).

use std::time::{Duration, Instant};

use axion_net::{ServerOptions, TorConfig, TunnelOptions, TunnelStatus};

fn main() {
    let dir = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("axion-tor-example"));
    let options = TunnelOptions {
        server: ServerOptions::default(),
        connect_timeout: Duration::from_secs(15),
        data_dir: dir,
        wireguard_handshake_timeout: Duration::from_secs(20),
        background_priority: true,
        tor_worker_threads: 2,
    };
    let tunnel = match axion_net::tor::create(&TorConfig::default(), &options) {
        Ok(t) => t,
        Err(e) => return eprintln!("cannot create the tunnel: {e}"),
    };
    let endpoint = match tunnel.start() {
        Ok(e) => e,
        Err(e) => return eprintln!("cannot start: {e}"),
    };
    println!(
        "SOCKS5 login: {}:{} at {}",
        endpoint.credentials.username,
        endpoint.credentials.password.expose(),
        endpoint.addr
    );
    let end = Instant::now() + Duration::from_secs(600);
    let mut was_up = false;
    while Instant::now() < end {
        match tunnel.status() {
            TunnelStatus::Up { .. } if !was_up => {
                was_up = true;
                println!("UP");
            }
            TunnelStatus::Failed(e) => return eprintln!("failed: {e}"),
            _ => {}
        }
        if !was_up {
            println!("bootstrapping: {:?}", tunnel.progress());
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    tunnel.stop();
}
