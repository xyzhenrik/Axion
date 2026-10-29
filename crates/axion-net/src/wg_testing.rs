//! A WireGuard **server** for tests and the end-to-end example: no root, no kernel module.
//!
//! It is a second [`boringtun`] peer over UDP loopback with its own [`smoltcp`] stack behind
//! it. That stack answers for *any* destination address ("the internet inside the tunnel"):
//! TCP connections to the configured ports are forwarded to real local servers
//! (`forward` maps the destination the client asked for to a loopback address) and DNS
//! queries (UDP port 53, any server address) are answered from a table of names.
//! Not used by the browser itself.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use boringtun::noise::{Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use smoltcp::iface::{Config as IfaceConfig, Interface, SocketHandle, SocketSet};
use smoltcp::socket::tcp::{SocketBuffer, State};
use smoltcp::socket::{tcp, udp};
use smoltcp::storage::PacketBuffer;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, IpListenEndpoint};

use crate::wg_net::{VDevice, smol_now};

/// Maps the address a client connected to (inside the tunnel) to a real local server.
pub type Forward = Arc<dyn Fn(SocketAddr) -> Option<SocketAddr> + Send + Sync>;

/// How the test server behaves.
#[derive(Clone)]
pub struct WgTestServerConfig {
    pub private_key: [u8; 32],
    pub client_public: [u8; 32],
    pub preshared: Option<[u8; 32]>,
    /// UDP address to bind (`127.0.0.1:0` for a random port).
    pub bind: SocketAddr,
    /// TCP ports that exist "inside".
    pub tcp_ports: Vec<u16>,
    pub forward: Forward,
    /// Names the DNS responder knows.
    pub names: Vec<(String, IpAddr)>,
    /// Waiting listeners per port (how many connections may arrive at once).
    pub backlog: usize,
    pub mtu: usize,
}

impl WgTestServerConfig {
    pub fn new(private_key: [u8; 32], client_public: [u8; 32], forward: Forward) -> Self {
        Self {
            private_key,
            client_public,
            preshared: None,
            bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            tcp_ports: vec![80, 443],
            forward,
            names: Vec::new(),
            backlog: 160,
            mtu: 1420,
        }
    }
}

/// Counters of what the server saw.
#[derive(Default)]
pub struct WgTestStats {
    pub dns_queries: Mutex<Vec<String>>,
    pub tcp_accepted: AtomicUsize,
    pub datagrams: AtomicUsize,
}

/// A running test server. Dropping it stops it.
pub struct WgTestServer {
    addr: SocketAddr,
    public_key: [u8; 32],
    stop: Arc<AtomicBool>,
    blackhole: Arc<AtomicBool>,
    pub stats: Arc<WgTestStats>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// The public key for a private key.
pub fn public_key_of(private: &[u8; 32]) -> [u8; 32] {
    PublicKey::from(&StaticSecret::from(*private)).to_bytes()
}

/// A random private key (clamped by the protocol when used).
pub fn random_private_key() -> [u8; 32] {
    let mut k = [0u8; 32];
    // A test helper: an unavailable entropy source is a broken machine.
    if getrandom::fill(&mut k).is_err() {
        k = [7u8; 32];
    }
    k
}

impl WgTestServer {
    pub fn start(config: WgTestServerConfig) -> io::Result<Self> {
        let socket = UdpSocket::bind(config.bind)?;
        socket.set_read_timeout(Some(Duration::from_millis(1)))?;
        let addr = socket.local_addr()?;
        let public_key = public_key_of(&config.private_key);
        let stop = Arc::new(AtomicBool::new(false));
        let blackhole = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(WgTestStats::default());
        let thread = {
            let (stop, blackhole, stats) = (stop.clone(), blackhole.clone(), stats.clone());
            std::thread::Builder::new()
                .name("wg-test-server".into())
                .spawn(move || run(config, socket, &stop, &blackhole, &stats))?
        };
        Ok(Self { addr, public_key, stop, blackhole, stats, thread: Some(thread) })
    }

    /// The UDP address clients use as `Endpoint`.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.public_key
    }

    /// While on, every datagram is dropped unseen (a dead network path).
    pub fn set_blackhole(&self, on: bool) {
        self.blackhole.store(on, Ordering::Release);
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for WgTestServer {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Bridge {
    handle: SocketHandle,
    tcp: TcpStream,
    up: Vec<u8>,
    down: Vec<u8>,
    tcp_eof: bool,
    tcp_shut: bool,
}

const CHUNK: usize = 64 * 1024;

fn listener(port: u16) -> tcp::Socket<'static> {
    let mut s = tcp::Socket::new(SocketBuffer::new(vec![0u8; 128 * 1024]), SocketBuffer::new(vec![0u8; 128 * 1024]));
    s.set_nagle_enabled(false);
    // A listener on port `port` for any destination address (the interface is any-ip).
    let _ = s.listen(IpListenEndpoint { addr: None, port });
    s
}

fn run(config: WgTestServerConfig, socket: UdpSocket, stop: &AtomicBool, blackhole: &AtomicBool, stats: &WgTestStats) {
    let started = Instant::now();
    let mut tunn = Tunn::new(
        StaticSecret::from(config.private_key),
        PublicKey::from(config.client_public),
        config.preshared,
        None,
        7,
        None,
    );
    let mut device = VDevice::new(config.mtu);
    let mut cfg = IfaceConfig::new(HardwareAddress::Ip);
    cfg.random_seed = 1;
    let mut iface = Interface::new(cfg, &mut device, smol_now(started));
    let v4 = Ipv4Addr::new(10, 255, 0, 1);
    let v6 = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1);
    iface.update_ip_addrs(|a| {
        let _ = a.push(IpCidr::new(IpAddress::Ipv4(v4), 8));
        let _ = a.push(IpCidr::new(IpAddress::Ipv6(v6), 64));
    });
    let _ = iface.routes_mut().add_default_ipv4_route(v4);
    let _ = iface.routes_mut().add_default_ipv6_route(v6);
    iface.set_any_ip(true);
    let mut sockets = SocketSet::new(Vec::new());
    let mut listeners: HashMap<u16, Vec<SocketHandle>> = HashMap::new();
    for port in &config.tcp_ports {
        let handles = (0..config.backlog).map(|_| sockets.add(listener(*port))).collect();
        listeners.insert(*port, handles);
    }
    let dns = {
        let mut s = udp::Socket::new(
            PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 64], vec![0u8; 64 * 512]),
            PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 64], vec![0u8; 64 * 512]),
        );
        let _ = s.bind(53);
        sockets.add(s)
    };
    let mut bridges: Vec<Bridge> = Vec::new();
    let mut dying: Vec<SocketHandle> = Vec::new();
    let mut client: Option<SocketAddr> = None;
    let mut buf = vec![0u8; 65536 + 128];
    let mut dgram = vec![0u8; 4096 + config.mtu];
    let names: HashMap<String, Vec<IpAddr>> = {
        let mut m: HashMap<String, Vec<IpAddr>> = HashMap::new();
        for (n, ip) in &config.names {
            m.entry(n.to_ascii_lowercase()).or_default().push(*ip);
        }
        m
    };
    let send = |socket: &UdpSocket, client: Option<SocketAddr>, data: &[u8]| {
        if let Some(c) = client {
            let _ = socket.send_to(data, c);
        }
    };
    while !stop.load(Ordering::Acquire) {
        // Datagrams from the client (drain what is queued after the first).
        let mut first = true;
        loop {
            let got = if first { socket.recv_from(&mut dgram) } else { nonblocking_recv(&socket, &mut dgram) };
            first = false;
            let Ok((n, from)) = got else { break };
            stats.datagrams.fetch_add(1, Ordering::Relaxed);
            if blackhole.load(Ordering::Acquire) {
                continue;
            }
            let mut input: &[u8] = &dgram[..n];
            loop {
                match tunn.decapsulate(Some(from.ip()), input, &mut buf) {
                    TunnResult::WriteToNetwork(out) => {
                        client = Some(from);
                        send(&socket, client, out);
                        input = &[];
                    }
                    TunnResult::WriteToTunnelV4(p, _) | TunnResult::WriteToTunnelV6(p, _) => {
                        client = Some(from);
                        device.rx.push_back(p.to_vec());
                        break;
                    }
                    TunnResult::Done | TunnResult::Err(_) => break,
                }
            }
        }
        if let TunnResult::WriteToNetwork(p) = tunn.update_timers(&mut buf) {
            send(&socket, client, p);
        }
        let _ = socket.set_read_timeout(Some(Duration::from_millis(1)));

        // The stack, the bridges, the stack again.
        for _ in 0..16 {
            iface.poll(smol_now(started), &mut device, &mut sockets);
            // New connections: a listener that is no longer listening got one.
            for (port, handles) in &mut listeners {
                for h in handles.iter_mut() {
                    let state = sockets.get::<tcp::Socket>(*h).state();
                    if state == State::Established {
                        let local = sockets.get::<tcp::Socket>(*h).local_endpoint();
                        let dest = local.and_then(|e| {
                            let ip = match e.addr {
                                IpAddress::Ipv4(a) => IpAddr::V4(a),
                                IpAddress::Ipv6(a) => IpAddr::V6(a),
                            };
                            Some(SocketAddr::new(ip, e.port))
                        });
                        let real = dest.and_then(|d| (config.forward)(d));
                        let accepted = real.and_then(|r| TcpStream::connect_timeout(&r, Duration::from_secs(2)).ok());
                        let old = *h;
                        *h = sockets.add(listener(*port));
                        match accepted {
                            Some(tcp_stream) => {
                                let _ = tcp_stream.set_nodelay(true);
                                let _ = tcp_stream.set_nonblocking(true);
                                stats.tcp_accepted.fetch_add(1, Ordering::Relaxed);
                                bridges.push(Bridge { handle: old, tcp: tcp_stream, up: Vec::new(), down: Vec::new(), tcp_eof: false, tcp_shut: false });
                            }
                            None => {
                                // No real server behind it: reset, and free the socket later.
                                sockets.get_mut::<tcp::Socket>(old).abort();
                                dying.push(old);
                            }
                        }
                    }
                }
            }
            dying.retain(|h| {
                let done = sockets.get::<tcp::Socket>(*h).state() == State::Closed;
                if done {
                    sockets.remove(*h);
                }
                !done
            });
            let mut progressed = false;
            bridges.retain_mut(|b| {
                let sock = sockets.get_mut::<tcp::Socket>(b.handle);
                // client -> real server
                while b.up.len() < CHUNK && sock.can_recv() {
                    let mut tmp = [0u8; 16384];
                    match sock.recv_slice(&mut tmp) {
                        Ok(n) if n > 0 => {
                            b.up.extend_from_slice(&tmp[..n]);
                            progressed = true;
                        }
                        _ => break,
                    }
                }
                if !b.up.is_empty() {
                    match b.tcp.write(&b.up) {
                        Ok(n) => {
                            b.up.drain(..n);
                            progressed = true;
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                        Err(_) => b.up.clear(),
                    }
                }
                if !sock.may_recv() && b.up.is_empty() && !b.tcp_shut {
                    let _ = b.tcp.shutdown(std::net::Shutdown::Write);
                    b.tcp_shut = true;
                }
                // real server -> client
                if !b.tcp_eof && b.down.len() < CHUNK {
                    let mut tmp = [0u8; 16384];
                    match b.tcp.read(&mut tmp) {
                        Ok(0) => b.tcp_eof = true,
                        Ok(n) => {
                            b.down.extend_from_slice(&tmp[..n]);
                            progressed = true;
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                        Err(_) => b.tcp_eof = true,
                    }
                }
                if !b.down.is_empty() && sock.can_send() {
                    if let Ok(n) = sock.send_slice(&b.down) {
                        b.down.drain(..n);
                        progressed = true;
                    }
                }
                if b.tcp_eof && b.down.is_empty() && sock.may_send() {
                    sock.close();
                }
                let done = matches!(sock.state(), State::Closed | State::TimeWait);
                if done {
                    sockets.remove(b.handle);
                }
                !done
            });
            // DNS.
            {
                let d = sockets.get_mut::<udp::Socket>(dns);
                while let Ok((n, meta)) = d.recv_slice(&mut buf) {
                    let query = buf[..n].to_vec();
                    if let Some((name, reply)) = answer(&query, &names, config.names.is_empty()) {
                        lock(&stats.dns_queries).push(name);
                        let _ = d.send_slice(&reply, meta);
                        progressed = true;
                    }
                }
            }
            // To the wire.
            let mut sent = false;
            while let Some(packet) = device.tx.pop_front() {
                sent = true;
                if let TunnResult::WriteToNetwork(p) = tunn.encapsulate(&packet, &mut buf) {
                    send(&socket, client, p);
                }
            }
            if !progressed && !sent && device.rx.is_empty() {
                break;
            }
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn nonblocking_recv(socket: &UdpSocket, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
    socket.set_nonblocking(true)?;
    let r = socket.recv_from(buf);
    socket.set_nonblocking(false)?;
    r
}

/// Parses a DNS query and builds the answer; returns the queried name.
fn answer(query: &[u8], names: &HashMap<String, Vec<IpAddr>>, answer_all: bool) -> Option<(String, Vec<u8>)> {
    if query.len() < 12 {
        return None;
    }
    let mut pos = 12;
    let mut name = String::new();
    loop {
        let len = *query.get(pos)? as usize;
        pos += 1;
        if len == 0 {
            break;
        }
        if !name.is_empty() {
            name.push('.');
        }
        name.push_str(std::str::from_utf8(query.get(pos..pos + len)?).ok()?);
        pos += len;
    }
    let qtype = u16::from_be_bytes([*query.get(pos)?, *query.get(pos + 1)?]);
    let question_end = pos + 4;
    let name = name.to_ascii_lowercase();
    let known = names.get(&name);
    let mut out = Vec::new();
    out.extend(&query[..2]); // id
    let found = known.is_some() || answer_all;
    out.extend([0x81, if found { 0x80 } else { 0x83 }]);
    out.extend([0, 1]);
    let wanted: Vec<IpAddr> = known
        .map(|v| v.iter().copied().filter(|ip| (qtype == 1 && ip.is_ipv4()) || (qtype == 28 && ip.is_ipv6())).collect())
        .unwrap_or_default();
    out.extend((wanted.len() as u16).to_be_bytes());
    out.extend([0, 0, 0, 0]);
    out.extend(query.get(12..question_end)?);
    for ip in wanted {
        out.extend([0xc0, 0x0c]);
        match ip {
            IpAddr::V4(a) => {
                out.extend([0, 1, 0, 1, 0, 0, 0, 60, 0, 4]);
                out.extend(a.octets());
            }
            IpAddr::V6(a) => {
                out.extend([0, 28, 0, 1, 0, 0, 0, 60, 0, 16]);
                out.extend(a.octets());
            }
        }
    }
    Some((name, out))
}
