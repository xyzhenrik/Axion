//! The userspace network behind the WireGuard backend: a WireGuard peer set
//! ([`boringtun`]) under a userspace TCP/IP stack ([`smoltcp`]), driven by one worker thread.
//!
//! ```text
//! SOCKS5 connection threads --(blocking Read/Write on WgStream)--+
//!                                                                v
//!                      Mutex<Core>: smoltcp sockets + interface + virtual device
//!                                                                ^
//! worker thread: UDP socket <-> Tunn (Noise, timers, rekey) -----+   (poll, never busy)
//! ```
//!
//! Connection threads work on the smoltcp sockets directly under the core lock (no copy
//! through intermediate queues, so backpressure is the socket buffer's), and wait on a
//! per-connection condition variable that the worker signals only when that connection can
//! make progress. The worker blocks in `recv_from` with a timeout taken from smoltcp's
//! `poll_delay` and the WireGuard timer tick; a write by a connection thread wakes it with a
//! datagram to its own UDP port (coalesced).
//!
//! Nothing here touches the host's network except the UDP socket to the peers' endpoints.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use boringtun::noise::errors::WireGuardError;
use boringtun::noise::{Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use smoltcp::iface::{Config as IfaceConfig, Interface, PollResult, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp::{CongestionControl, SocketBuffer, State};
use smoltcp::socket::{dns, tcp};
use smoltcp::time::{Duration as SDuration, Instant as SInstant};
use smoltcp::wire::{DnsQueryType, HardwareAddress, IpAddress, IpCidr, IpEndpoint, IpListenEndpoint};
use zeroize::Zeroizing;

use crate::conn::{Address, Conn, Dialer};

/// Knobs of the userspace stack. Only the handshake timeout is a user setting
/// (`network.wireguard_handshake_timeout_secs`); the rest are internal limits with sane values.
#[derive(Clone, Debug)]
pub struct WgOptions {
    /// How long a handshake may stay unanswered before the connection counts as failed.
    pub handshake_timeout: Duration,
    /// Connecting a stream inside the tunnel (and resolving its name).
    pub connect_timeout: Duration,
    /// Most TCP streams open inside the tunnel at once.
    pub max_streams: usize,
    /// Send and receive buffer of one stream (bytes); above 64 KiB enables window scaling.
    pub tcp_buffer: usize,
    /// Longest the worker sleeps (the WireGuard timers are ticked at least this often).
    pub tick: Duration,
    /// A closed stream whose peer does not finish the close is aborted after this.
    pub close_linger: Duration,
    /// How long a resolved name is remembered.
    pub dns_cache_ttl: Duration,
    /// Resolved again (outside the tunnel, with the system resolver) when a handshake has been
    /// unanswered this long and the endpoint is a name.
    pub reresolve_after: Duration,
    /// A dead tunnel starts a new handshake this often (WireGuard itself gives up after 90 s).
    pub retry_interval: Duration,
    /// Datagrams taken from the socket per round.
    pub batch: usize,
}

impl Default for WgOptions {
    fn default() -> Self {
        Self {
            handshake_timeout: Duration::from_secs(20),
            connect_timeout: Duration::from_secs(15),
            max_streams: 512,
            tcp_buffer: 256 * 1024,
            tick: Duration::from_millis(250),
            close_linger: Duration::from_secs(15),
            dns_cache_ttl: Duration::from_secs(30),
            reresolve_after: Duration::from_secs(8),
            retry_interval: Duration::from_secs(15),
            batch: 64,
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An address with a prefix length (`10.0.0.0/8`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cidr {
    pub addr: IpAddr,
    pub prefix: u8,
}

impl Cidr {
    /// `10.0.0.2/32`, `fd00::/64` or a bare address (a host).
    pub fn parse(text: &str) -> Option<Cidr> {
        let (addr, prefix) = match text.trim().split_once('/') {
            Some((a, p)) => (a.trim(), Some(p.trim())),
            None => (text.trim(), None),
        };
        let addr: IpAddr = addr.parse().ok()?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => p.parse::<u8>().ok().filter(|p| *p <= max)?,
            None => max,
        };
        Some(Cidr { addr, prefix })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = if self.prefix == 0 { 0 } else { u32::MAX << (32 - u32::from(self.prefix)) };
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = if self.prefix == 0 { 0 } else { u128::MAX << (128 - u32::from(self.prefix)) };
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

/// One peer of the config, decoded.
pub struct PeerSpec {
    pub public: [u8; 32],
    pub preshared: Option<Zeroizing<[u8; 32]>>,
    pub allowed: Vec<Cidr>,
    /// The endpoint as written (`host:port`), empty if the peer has none.
    pub endpoint_text: String,
    /// Where it was resolved to at start.
    pub endpoint_addrs: Vec<SocketAddr>,
    /// Seconds, 0 = off.
    pub keepalive: u16,
}

/// Everything the stack is built from.
pub struct StackSpec {
    pub private: Zeroizing<[u8; 32]>,
    pub addresses: Vec<Cidr>,
    pub dns: Vec<IpAddr>,
    pub mtu: usize,
    pub listen_port: u16,
    pub peers: Vec<PeerSpec>,
}

/// How the tunnel is doing (the facade turns this into a `TunnelStatus`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Connecting,
    Up { since: SystemTime },
    Failed(String),
    Stopped,
}

/// Live numbers for the UI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WgStats {
    /// Bytes of WireGuard datagrams received / sent (UDP payload, everything included).
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    /// Completed handshakes since start.
    pub handshakes: u64,
    /// TCP streams open inside the tunnel now.
    pub streams: usize,
}

// ---------------------------------------------------------------------------------------------
// The virtual device

pub(crate) struct VDevice {
    pub(crate) rx: VecDeque<Vec<u8>>,
    pub(crate) tx: VecDeque<Vec<u8>>,
    mtu: usize,
    tx_cap: usize,
}

impl VDevice {
    pub(crate) fn new(mtu: usize) -> Self {
        Self { rx: VecDeque::new(), tx: VecDeque::new(), mtu, tx_cap: 512 }
    }
}

pub(crate) struct VRx(Vec<u8>);

pub(crate) struct VTx<'a>(&'a mut VecDeque<Vec<u8>>);

impl RxToken for VRx {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

impl TxToken for VTx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut packet = vec![0u8; len];
        let result = f(&mut packet);
        self.0.push_back(packet);
        result
    }
}

impl Device for VDevice {
    type RxToken<'a>
        = VRx
    where
        Self: 'a;
    type TxToken<'a>
        = VTx<'a>
    where
        Self: 'a;

    fn receive(&mut self, _: SInstant) -> Option<(VRx, VTx<'_>)> {
        if self.tx.len() >= self.tx_cap {
            return None;
        }
        let packet = self.rx.pop_front()?;
        Some((VRx(packet), VTx(&mut self.tx)))
    }

    fn transmit(&mut self, _: SInstant) -> Option<VTx<'_>> {
        (self.tx.len() < self.tx_cap).then_some(VTx(&mut self.tx))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = self.mtu;
        caps
    }
}

// ---------------------------------------------------------------------------------------------
// Peers

struct PeerRt {
    tunn: Tunn,
    allowed: Vec<Cidr>,
    label: String,
    /// Host and port of a name endpoint (re-resolved when handshakes fail).
    host: Option<(String, u16)>,
    endpoint: Option<SocketAddr>,
    /// Age of the session at the previous look (a smaller one means a new handshake).
    last_age: Option<Duration>,
    handshake_at: Option<Instant>,
    /// When the unanswered handshake initiation was first sent.
    pending_since: Option<Instant>,
    last_retry: Instant,
    last_resolve: Option<Instant>,
    resolving: bool,
}

impl PeerRt {
    fn best_prefix(&self, ip: IpAddr) -> Option<u8> {
        self.allowed.iter().filter(|c| c.contains(ip)).map(|c| c.prefix).max()
    }

    fn allows(&self, ip: IpAddr) -> bool {
        self.allowed.iter().any(|c| c.contains(ip))
    }
}

struct ConnState {
    cond: Arc<Condvar>,
    wait_read: u32,
    wait_write: u32,
    wait_connect: bool,
    released: Option<Instant>,
    /// A FIN arrived (the peer finished sending).
    remote_closed: bool,
    /// The connection ended without a close from either side (reset or timed out).
    reset: bool,
    ever_open: bool,
    port: u16,
}

pub(crate) struct Core {
    iface: Interface,
    sockets: SocketSet<'static>,
    device: VDevice,
    peers: Vec<PeerRt>,
    conns: HashMap<SocketHandle, ConnState>,
    dns: SocketHandle,
    dns_waiters: usize,
    dns_cache: HashMap<String, (Vec<IpAddr>, Instant)>,
    ports: HashSet<u16>,
    next_port: u16,
    started: Instant,
    udp: Arc<UdpSocket>,
    udp_v6: bool,
    scratch: Vec<u8>,
    v4: Option<Ipv4Addr>,
    v6: Option<Ipv6Addr>,
    has_dns: bool,
    send_errors: u32,
    stopped: bool,
    handshake_events: u64,
}

pub(crate) struct Shared {
    core: Mutex<Core>,
    dns_cond: Condvar,
    events: SyncSender<Event>,
    readers: Mutex<Vec<(Arc<AtomicBool>, std::thread::JoinHandle<()>)>>,
    woken: AtomicBool,
    stop: AtomicBool,
    phase: Mutex<Phase>,
    phase_cond: Condvar,
    rx_bytes: AtomicU64,
    tx_bytes: AtomicU64,
    handshakes: AtomicU64,
    opts: WgOptions,
    inbox: Mutex<Vec<(usize, Vec<SocketAddr>)>>,
    rekey: AtomicBool,
    pub(crate) last_handshake: Mutex<Option<(Instant, String)>>,
}

impl Shared {
    fn core(&self) -> MutexGuard<'_, Core> {
        lock(&self.core)
    }

    /// Wakes the worker (coalesced: at most one wake-up in the queue).
    fn wake(&self) {
        if self.woken.swap(true, Ordering::AcqRel) {
            return;
        }
        // A full queue already has the worker's attention.
        let _ = self.events.try_send(Event::Wake);
    }

    fn set_phase(&self, phase: Phase) {
        let mut p = lock(&self.phase);
        if *p != phase {
            *p = phase;
            self.phase_cond.notify_all();
        }
    }

    pub(crate) fn phase(&self) -> Phase {
        lock(&self.phase).clone()
    }

    pub(crate) fn stats(&self) -> WgStats {
        let streams = self.core().conns.len();
        WgStats {
            rx_bytes: self.rx_bytes.load(Ordering::Relaxed),
            tx_bytes: self.tx_bytes.load(Ordering::Relaxed),
            handshakes: self.handshakes.load(Ordering::Relaxed),
            streams,
        }
    }

    /// Waits until the tunnel is up or failed, or `timeout` has passed.
    pub(crate) fn wait_phase(&self, timeout: Duration) -> Phase {
        let deadline = Instant::now() + timeout;
        let mut p = lock(&self.phase);
        loop {
            if !matches!(*p, Phase::Connecting) {
                return p.clone();
            }
            let now = Instant::now();
            if now >= deadline {
                return p.clone();
            }
            p = self.phase_cond.wait_timeout(p, deadline - now).unwrap_or_else(PoisonError::into_inner).0;
        }
    }

    /// Starts a new handshake with every peer at once (rekey on demand; also a test hook).
    pub(crate) fn rekey_now(&self) {
        self.rekey.store(true, Ordering::Release);
        self.wake();
    }

    pub(crate) fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        self.wake();
    }
}

pub(crate) fn smol_now(started: Instant) -> SInstant {
    SInstant::from_micros(i64::try_from(started.elapsed().as_micros()).unwrap_or(i64::MAX))
}

fn to_smol(ip: IpAddr) -> IpAddress {
    IpAddress::from(ip)
}

fn from_smol(ip: IpAddress) -> IpAddr {
    match ip {
        IpAddress::Ipv4(a) => IpAddr::V4(a),
        IpAddress::Ipv6(a) => IpAddr::V6(a),
    }
}

fn bind_udp(v6: bool, port: u16) -> io::Result<UdpSocket> {
    let any = if v6 {
        SocketAddr::from((Ipv6Addr::UNSPECIFIED, port))
    } else {
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))
    };
    match UdpSocket::bind(any) {
        Ok(s) => Ok(s),
        Err(e) if port != 0 => {
            tracing::debug!(error = %e, "the configured ListenPort is busy, using a random one");
            bind_udp(v6, 0)
        }
        Err(e) => Err(e),
    }
}

/// What the worker waits for: a datagram from the network, or a nudge from another thread.
enum Event {
    Datagram(Vec<u8>, SocketAddr),
    Wake,
}

/// A thread that does nothing but empty the socket into the worker's queue, so a burst never
/// overflows the kernel's small receive buffer while the worker is busy.
fn spawn_reader(shared: &Shared, socket: Arc<UdpSocket>) -> io::Result<()> {
    let alive = Arc::new(AtomicBool::new(true));
    let flag = alive.clone();
    let events = shared.events.clone();
    let handle = std::thread::Builder::new().name("wg-udp-read".into()).spawn(move || {
        let _ = socket.set_read_timeout(Some(Duration::from_millis(50)));
        let mut buf = vec![0u8; 65536];
        while flag.load(Ordering::Acquire) {
            match socket.recv_from(&mut buf) {
                Ok((n, from)) => {
                    if events.send(Event::Datagram(buf[..n].to_vec(), from)).is_err() {
                        break;
                    }
                }
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => {
                    tracing::debug!(error = %e, "WireGuard socket read failed");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    })?;
    lock(&shared.readers).push((alive, handle));
    Ok(())
}

fn random_u64() -> u64 {
    let mut b = [0u8; 8];
    // A failing entropy source only weakens TCP sequence numbers and port choice here (the
    // tunnel's own keys come from boringtun); fall back to the clock.
    if getrandom::fill(&mut b).is_err() {
        return SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
    }
    u64::from_le_bytes(b)
}

impl Core {
    fn new(spec: &StackSpec, udp: Arc<UdpSocket>, udp_v6: bool) -> io::Result<Core> {
        let started = Instant::now();
        let mut device = VDevice::new(spec.mtu);
        let mut cfg = IfaceConfig::new(HardwareAddress::Ip);
        cfg.random_seed = random_u64();
        let mut iface = Interface::new(cfg, &mut device, smol_now(started));
        // smoltcp keeps two addresses: the first IPv4 and the first IPv6 one.
        let v4 = spec.addresses.iter().find(|c| c.addr.is_ipv4());
        let v6 = spec.addresses.iter().find(|c| c.addr.is_ipv6());
        iface.update_ip_addrs(|addrs| {
            for c in [v4, v6].into_iter().flatten() {
                let _ = addrs.push(IpCidr::new(to_smol(c.addr), c.prefix));
            }
        });
        let v4_addr = v4.and_then(|c| if let IpAddr::V4(a) = c.addr { Some(a) } else { None });
        let v6_addr = v6.and_then(|c| if let IpAddr::V6(a) = c.addr { Some(a) } else { None });
        // Everything goes out through the tunnel interface: a default route per family (the
        // gateway of a point-to-point link is never looked at).
        if let Some(a) = v4_addr {
            iface.routes_mut().add_default_ipv4_route(a).map_err(|_| io::Error::other("route table full"))?;
        }
        if let Some(a) = v6_addr {
            iface.routes_mut().add_default_ipv6_route(a).map_err(|_| io::Error::other("route table full"))?;
        }
        let dns_servers: Vec<IpAddress> = spec
            .dns
            .iter()
            .filter(|ip| if ip.is_ipv4() { v4_addr.is_some() } else { v6_addr.is_some() })
            .map(|ip| to_smol(*ip))
            .collect();
        let has_dns = !dns_servers.is_empty();
        let mut sockets = SocketSet::new(Vec::new());
        let dns = sockets.add(dns::Socket::new(&dns_servers, (0..32).map(|_| None).collect::<Vec<_>>()));
        let mut peers = Vec::new();
        for (i, p) in spec.peers.iter().enumerate() {
            let index = ((random_u64() as u32) & 0x00ff_ffff) ^ (i as u32);
            let tunn = Tunn::new(
                StaticSecret::from(*spec.private),
                PublicKey::from(p.public),
                p.preshared.as_ref().map(|k| **k),
                (p.keepalive > 0).then_some(p.keepalive),
                index,
                None,
            );
            let endpoint = p.endpoint_addrs.first().copied();
            let host = if p.endpoint_text.is_empty() {
                None
            } else {
                crate::wgconf::split_endpoint(&p.endpoint_text)
                    .filter(|(h, _)| h.parse::<IpAddr>().is_err())
                    .map(|(h, port)| (h.to_owned(), port))
            };
            peers.push(PeerRt {
                tunn,
                allowed: p.allowed.clone(),
                label: p.endpoint_text.clone(),
                host,
                endpoint,
                last_age: None,
                handshake_at: None,
                pending_since: None,
                last_retry: started,
                last_resolve: None,
                resolving: false,
            });
        }
        Ok(Core {
            iface,
            sockets,
            device,
            peers,
            conns: HashMap::new(),
            dns,
            dns_waiters: 0,
            dns_cache: HashMap::new(),
            ports: HashSet::new(),
            next_port: 49152 + (random_u64() % 16000) as u16,
            started,
            udp,
            udp_v6,
            scratch: vec![0u8; 65536 + 128],
            v4: v4_addr,
            v6: v6_addr,
            has_dns,
            send_errors: 0,
            stopped: false,
            handshake_events: 0,
        })
    }

    fn peer_for(&self, ip: IpAddr) -> Option<usize> {
        let mut best: Option<(u8, usize)> = None;
        for (i, p) in self.peers.iter().enumerate() {
            if let Some(prefix) = p.best_prefix(ip)
                && best.is_none_or(|(b, _)| prefix > b)
            {
                best = Some((prefix, i));
            }
        }
        best.map(|(_, i)| i)
    }

    fn alloc_port(&mut self) -> Option<u16> {
        for _ in 0..16384 {
            let p = self.next_port;
            self.next_port = if self.next_port == 65535 { 49152 } else { self.next_port + 1 };
            if self.ports.insert(p) {
                return Some(p);
            }
        }
        None
    }

    /// Sends a WireGuard datagram to a peer's endpoint.
    fn send_to_peer(&mut self, shared: &Shared, peer: usize, datagram: &[u8]) {
        let Some(endpoint) = self.peers.get(peer).and_then(|p| p.endpoint) else { return };
        // A handshake initiation (type 1): remember when the unanswered attempt began.
        if datagram.len() == 148 && datagram[0] == 1 {
            let p = &mut self.peers[peer];
            if p.pending_since.is_none() {
                p.pending_since = Some(Instant::now());
            }
        }
        let target = match (endpoint, self.udp_v6) {
            (SocketAddr::V4(a), true) => SocketAddr::from((a.ip().to_ipv6_mapped(), a.port())),
            (SocketAddr::V6(_), false) => return,
            (a, _) => a,
        };
        match self.udp.send_to(datagram, target) {
            Ok(n) => {
                self.send_errors = 0;
                shared.tx_bytes.fetch_add(n as u64, Ordering::Relaxed);
            }
            Err(e) => {
                self.send_errors = self.send_errors.saturating_add(1);
                tracing::debug!(error = %e, "could not send a WireGuard datagram");
            }
        }
    }

    /// A datagram from the network: the peer it belongs to decrypts it.
    fn ingest(&mut self, shared: &Shared, src: SocketAddr, datagram: &[u8]) {
        let src = SocketAddr::new(src.ip().to_canonical(), src.port());
        shared.rx_bytes.fetch_add(datagram.len() as u64, Ordering::Relaxed);
        // The peer that sends from this address first, then the others (a roaming peer, or a
        // packet that only one peer's session can open).
        let mut order: Vec<usize> =
            (0..self.peers.len()).filter(|i| self.peers[*i].endpoint == Some(src)).collect();
        let rest: Vec<usize> = (0..self.peers.len()).filter(|i| !order.contains(i)).collect();
        order.extend(rest);
        let mut scratch = std::mem::take(&mut self.scratch);
        'peers: for idx in order {
            let mut input: &[u8] = datagram;
            loop {
                // Returns whether this peer accepted the datagram.
                let result = self.peers[idx].tunn.decapsulate(Some(src.ip()), input, &mut scratch);
                match result {
                    TunnResult::Done => break 'peers,
                    TunnResult::Err(e) => {
                        if input.is_empty() {
                            break 'peers; // flushing queued packets failed: nothing more
                        }
                        tracing::trace!(error = ?e, "WireGuard datagram not accepted by a peer");
                        continue 'peers;
                    }
                    TunnResult::WriteToNetwork(out) => {
                        let out = out.to_vec();
                        // Authenticated: this is where the peer is now.
                        self.peers[idx].endpoint = Some(src);
                        self.send_to_peer(shared, idx, &out);
                        input = &[];
                    }
                    TunnResult::WriteToTunnelV4(packet, from) => {
                        self.peers[idx].endpoint = Some(src);
                        if self.peers[idx].allows(IpAddr::V4(from)) {
                            self.device.rx.push_back(packet.to_vec());
                        }
                        break 'peers;
                    }
                    TunnResult::WriteToTunnelV6(packet, from) => {
                        self.peers[idx].endpoint = Some(src);
                        if self.peers[idx].allows(IpAddr::V6(from)) {
                            self.device.rx.push_back(packet.to_vec());
                        }
                        break 'peers;
                    }
                }
            }
        }
        self.scratch = scratch;
    }

    /// Ticks the WireGuard timers (keepalive, handshake retries, rekey).
    fn tick(&mut self, shared: &Shared) {
        let mut scratch = std::mem::take(&mut self.scratch);
        let now = Instant::now();
        let force = shared.rekey.swap(false, Ordering::AcqRel);
        for idx in 0..self.peers.len() {
            if force
                && self.peers[idx].endpoint.is_some()
                && let TunnResult::WriteToNetwork(p) =
                    self.peers[idx].tunn.format_handshake_initiation(&mut scratch, true)
            {
                let p = p.to_vec();
                self.send_to_peer(shared, idx, &p);
            }
            let result = self.peers[idx].tunn.update_timers(&mut scratch);
            match result {
                TunnResult::WriteToNetwork(p) => {
                    let p = p.to_vec();
                    self.send_to_peer(shared, idx, &p);
                }
                TunnResult::Err(WireGuardError::ConnectionExpired) => {
                    // WireGuard gave up (90 s without an answer). A tunnel that is not
                    // working starts over now and then, so it recovers by itself.
                    let peer = &mut self.peers[idx];
                    if peer.endpoint.is_some()
                        && now.duration_since(peer.last_retry) >= shared.opts.retry_interval
                    {
                        peer.last_retry = now;
                        if let TunnResult::WriteToNetwork(p) =
                            peer.tunn.format_handshake_initiation(&mut scratch, false)
                        {
                            let p = p.to_vec();
                            self.send_to_peer(shared, idx, &p);
                        }
                    }
                }
                TunnResult::Err(e) => tracing::debug!(error = ?e, "WireGuard timer"),
                _ => {}
            }
        }
        self.scratch = scratch;
    }

    /// Runs the TCP/IP stack and sends what it produced.
    fn pump(&mut self, shared: &Shared) {
        for _ in 0..32 {
            let now = smol_now(self.started);
            let result = self.iface.poll(now, &mut self.device, &mut self.sockets);
            let sent = self.drain_tx(shared);
            if result == PollResult::None && !sent && self.device.rx.is_empty() {
                break;
            }
        }
    }

    fn drain_tx(&mut self, shared: &Shared) -> bool {
        let mut any = false;
        let mut scratch = std::mem::take(&mut self.scratch);
        while let Some(packet) = self.device.tx.pop_front() {
            any = true;
            let Some(dst) = Tunn::dst_address(&packet) else { continue };
            let Some(idx) = self.peer_for(dst) else {
                // Not in any peer's AllowedIPs: dropped, never sent anywhere else.
                tracing::debug!("dropped a packet for a destination outside AllowedIPs");
                continue;
            };
            let result = self.peers[idx].tunn.encapsulate(&packet, &mut scratch);
            match result {
                TunnResult::WriteToNetwork(p) => {
                    let p = p.to_vec();
                    self.send_to_peer(shared, idx, &p);
                }
                TunnResult::Err(e) => tracing::debug!(error = ?e, "could not encrypt a packet"),
                _ => {}
            }
        }
        self.scratch = scratch;
        any
    }

    /// Looks at every stream: wakes the threads that can proceed, closes what was dropped.
    fn service(&mut self, shared: &Shared) {
        let now = Instant::now();
        let mut remove = Vec::new();
        for (handle, cs) in &mut self.conns {
            let sock = self.sockets.get_mut::<tcp::Socket>(*handle);
            let state = sock.state();
            if matches!(state, State::CloseWait | State::LastAck | State::Closing | State::TimeWait) {
                cs.remote_closed = true;
            }
            if !matches!(state, State::Closed | State::SynSent | State::Listen) {
                cs.ever_open = true;
            }
            if state == State::Closed && cs.ever_open && !cs.remote_closed && cs.released.is_none() {
                cs.reset = true;
            }
            let ready = (cs.wait_read > 0 && (sock.can_recv() || !sock.may_recv()))
                || (cs.wait_write > 0 && (sock.can_send() || !sock.may_send()))
                || (cs.wait_connect && state != State::SynSent);
            if ready {
                cs.cond.notify_all();
            }
            if let Some(since) = cs.released {
                if matches!(state, State::Closed | State::TimeWait) {
                    remove.push(*handle);
                } else if now.duration_since(since) > shared.opts.close_linger {
                    sock.abort();
                }
            }
        }
        for handle in remove {
            if let Some(cs) = self.conns.remove(&handle) {
                self.ports.remove(&cs.port);
            }
            self.sockets.remove(handle);
        }
        if self.dns_waiters > 0 {
            shared.dns_cond.notify_all();
        }
    }

    /// Handshake bookkeeping, endpoint re-resolution and the up/failed verdict.
    fn watch(&mut self, shared: &Arc<Shared>) {
        let now = Instant::now();
        let timeout = shared.opts.handshake_timeout;
        for idx in 0..self.peers.len() {
            let p = &mut self.peers[idx];
            let age = p.tunn.time_since_last_handshake();
            if let Some(a) = age
                && p.last_age.is_none_or(|l| a < l)
            {
                p.handshake_at = Some(now.checked_sub(a).unwrap_or(now));
                p.pending_since = None;
                p.last_resolve = None;
                self.handshake_events += 1;
                shared.handshakes.fetch_add(1, Ordering::Relaxed);
                *lock(&shared.last_handshake) = Some((now, p.label.clone()));
                tracing::debug!("WireGuard handshake completed");
            }
            p.last_age = age;
            // The endpoint's name may point elsewhere after a network change.
            if let (Some((host, port)), Some(since)) = (&p.host, p.pending_since) {
                let waited = now.duration_since(since);
                let due = p.last_resolve.is_none_or(|t| now.duration_since(t) >= shared.opts.reresolve_after);
                if waited >= shared.opts.reresolve_after && due && !p.resolving {
                    p.resolving = true;
                    p.last_resolve = Some(now);
                    spawn_resolver(shared.clone(), idx, host.clone(), *port);
                }
            }
        }
        // Verdict.
        let stalled = |p: &PeerRt| {
            p.endpoint.is_some() && p.pending_since.is_some_and(|t| now.duration_since(t) > timeout)
        };
        let usable: Vec<&PeerRt> = self.peers.iter().filter(|p| p.endpoint.is_some()).collect();
        let any_ok = usable.iter().any(|p| p.handshake_at.is_some() && !stalled(p));
        let all_stalled = !usable.is_empty() && usable.iter().all(|p| stalled(p));
        let current = shared.phase();
        if any_ok {
            if !matches!(current, Phase::Up { .. }) {
                shared.set_phase(Phase::Up { since: SystemTime::now() });
            }
        } else if all_stalled && matches!(current, Phase::Connecting | Phase::Up { .. }) {
            let label = usable.first().map(|p| p.label.clone()).unwrap_or_default();
            shared.set_phase(Phase::Failed(format!(
                "No answer from {label} within {} s. Check the endpoint, the keys and that UDP is not blocked.",
                timeout.as_secs()
            )));
        }
    }
}

fn spawn_resolver(shared: Arc<Shared>, peer: usize, host: String, port: u16) {
    let spawned = std::thread::Builder::new().name("wg-resolve".into()).spawn(move || {
        let addrs: Vec<SocketAddr> =
            (host.as_str(), port).to_socket_addrs().map(|i| i.collect()).unwrap_or_default();
        lock(&shared.inbox).push((peer, addrs));
        shared.wake();
    });
    if spawned.is_err() {
        tracing::debug!("could not start the endpoint resolver thread");
    }
}

// ---------------------------------------------------------------------------------------------
// The worker

impl Core {
    fn next_wait(&mut self, opts: &WgOptions) -> Duration {
        if !self.device.rx.is_empty() || !self.device.tx.is_empty() {
            return Duration::ZERO;
        }
        let now = smol_now(self.started);
        match self.iface.poll_delay(now, &self.sockets) {
            Some(d) => Duration::from_micros(d.total_micros()).min(opts.tick),
            None => opts.tick,
        }
    }

    fn apply_resolved(&mut self, shared: &Shared) {
        let items = std::mem::take(&mut *lock(&shared.inbox));
        for (idx, addrs) in items {
            let v6 = self.udp_v6;
            let Some(peer) = self.peers.get_mut(idx) else { continue };
            peer.resolving = false;
            let pick = addrs.iter().find(|a| v6 || a.is_ipv4()).copied();
            if let Some(a) = pick
                && peer.endpoint != Some(a)
            {
                tracing::info!("the WireGuard endpoint's name now resolves elsewhere; switching");
                peer.endpoint = Some(a);
                shared.rekey.store(true, Ordering::Release);
            }
        }
    }

    fn close_all(&mut self, shared: &Shared) {
        self.stopped = true;
        for cs in self.conns.values() {
            cs.cond.notify_all();
        }
        shared.dns_cond.notify_all();
    }
}

fn rebind(shared: &Shared, core: &mut Core) {
    match bind_udp(core.udp_v6, 0) {
        Ok(socket) => {
            let socket = Arc::new(socket);
            for (alive, _) in lock(&shared.readers).iter() {
                alive.store(false, Ordering::Release);
            }
            if let Err(e) = spawn_reader(shared, socket.clone()) {
                tracing::warn!(error = %e, "could not start the WireGuard socket reader");
                return;
            }
            core.udp = socket;
            core.send_errors = 0;
            tracing::info!("WireGuard UDP socket re-created after repeated send errors");
        }
        Err(e) => tracing::warn!(error = %e, "could not re-create the WireGuard UDP socket"),
    }
}

fn worker(shared: Arc<Shared>, events: Receiver<Event>) {
    let opts = shared.opts.clone();
    let mut got: Vec<(Vec<u8>, SocketAddr)> = Vec::with_capacity(opts.batch);
    let mut wait = Duration::ZERO;
    let mut spins = 0u32;
    let mut last_tick: Option<Instant> = None;
    while !shared.stop.load(Ordering::Acquire) {
        got.clear();
        let first = if wait.is_zero() { events.try_recv().ok() } else { events.recv_timeout(wait).ok() };
        shared.woken.store(false, Ordering::Release);
        let mut next = first;
        while let Some(event) = next {
            if let Event::Datagram(data, from) = event {
                got.push((data, from));
            }
            next = if got.len() < opts.batch { events.try_recv().ok() } else { None };
        }
        if shared.stop.load(Ordering::Acquire) {
            break;
        }
        let mut core = shared.core();
        for (data, from) in &got {
            core.ingest(&shared, *from, data);
        }
        core.apply_resolved(&shared);
        let due =
            last_tick.is_none_or(|t| t.elapsed() >= opts.tick / 2) || shared.rekey.load(Ordering::Acquire);
        if due {
            core.tick(&shared);
            last_tick = Some(Instant::now());
        }
        core.pump(&shared);
        core.watch(&shared);
        core.service(&shared);
        if core.send_errors >= 5 {
            rebind(&shared, &mut core);
        }
        wait = core.next_wait(&opts);
        // smoltcp asking for "now" again and again with nothing arriving would spin: sleep a
        // millisecond instead.
        if wait.is_zero() && got.is_empty() {
            spins += 1;
            if spins > 64 {
                wait = Duration::from_millis(1);
            }
        } else {
            spins = 0;
        }
    }
    shared.core().close_all(&shared);
    shared.set_phase(Phase::Stopped);
}

/// A running userspace WireGuard network.
pub struct Stack {
    shared: Arc<Shared>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Stack {
    /// Binds the UDP socket and starts the worker; handshakes begin at once.
    pub fn start(spec: &StackSpec, opts: WgOptions) -> io::Result<Stack> {
        let v6 = spec.peers.iter().filter_map(|p| p.endpoint_addrs.first()).any(SocketAddr::is_ipv6);
        let socket = bind_udp(v6, spec.listen_port)?;
        let socket = Arc::new(socket);
        let core = Core::new(spec, socket.clone(), v6)?;
        let (events_tx, events_rx) = std::sync::mpsc::sync_channel::<Event>(8192);
        let shared = Arc::new(Shared {
            core: Mutex::new(core),
            dns_cond: Condvar::new(),
            events: events_tx,
            readers: Mutex::new(Vec::new()),
            woken: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            phase: Mutex::new(Phase::Connecting),
            phase_cond: Condvar::new(),
            rx_bytes: AtomicU64::new(0),
            tx_bytes: AtomicU64::new(0),
            handshakes: AtomicU64::new(0),
            opts,
            inbox: Mutex::new(Vec::new()),
            rekey: AtomicBool::new(true), // the first round starts the handshakes
            last_handshake: Mutex::new(None),
        });
        spawn_reader(&shared, socket)?;
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new().name("wg-worker".into()).spawn(move || worker(shared, events_rx))?
        };
        Ok(Stack { shared, worker: Mutex::new(Some(thread)) })
    }

    pub(crate) fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// One line per open stream (state and queue sizes), for diagnosing stalls.
    #[doc(hidden)]
    pub fn debug_dump(&self) -> String {
        let core = self.shared.core();
        let mut out = String::new();
        for (h, cs) in &core.conns {
            let s = core.sockets.get::<tcp::Socket>(*h);
            out.push_str(&format!(
                "{h:?} port={} {} send_q={} recv_q={} wr={} ww={} released={} remote_closed={} reset={}\n",
                cs.port,
                s.state(),
                s.send_queue(),
                s.recv_queue(),
                cs.wait_read,
                cs.wait_write,
                cs.released.is_some(),
                cs.remote_closed,
                cs.reset
            ));
        }
        out
    }

    pub fn dialer(&self) -> Arc<dyn Dialer> {
        Arc::new(WgDialer { shared: self.shared.clone() })
    }

    pub fn stop(&self) {
        self.shared.stop();
        if let Some(t) = lock(&self.worker).take() {
            let _ = t.join();
        }
        let readers = std::mem::take(&mut *lock(&self.shared.readers));
        for (alive, _) in &readers {
            alive.store(false, Ordering::Release);
        }
        for (_, handle) in readers {
            let _ = handle.join();
        }
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------------------------
// Streams

struct StreamInner {
    shared: Arc<Shared>,
    handle: SocketHandle,
    cond: Arc<Condvar>,
}

impl Drop for StreamInner {
    fn drop(&mut self) {
        {
            let mut core = self.shared.core();
            core.release(self.handle);
        }
        self.shared.wake();
    }
}

impl Core {
    /// The caller let go of the stream: finish the close and free the socket when it is over.
    fn release(&mut self, handle: SocketHandle) {
        let Some(cs) = self.conns.get_mut(&handle) else { return };
        cs.released.get_or_insert_with(Instant::now);
        let sock = self.sockets.get_mut::<tcp::Socket>(handle);
        match sock.state() {
            State::SynSent | State::SynReceived => sock.abort(),
            State::Established | State::CloseWait => sock.close(),
            _ => {}
        }
    }
}

/// A TCP stream inside the tunnel. Reading and writing block (the worker signals progress).
pub struct WgStream {
    inner: Arc<StreamInner>,
}

#[derive(Clone, Copy)]
enum Wait {
    Read,
    Write,
}

impl StreamInner {
    fn wait<'a>(&self, mut core: MutexGuard<'a, Core>, what: Wait) -> MutexGuard<'a, Core> {
        if let Some(cs) = core.conns.get_mut(&self.handle) {
            match what {
                Wait::Read => cs.wait_read += 1,
                Wait::Write => cs.wait_write += 1,
            }
        }
        core = self.cond.wait_timeout(core, Duration::from_secs(1)).unwrap_or_else(PoisonError::into_inner).0;
        if let Some(cs) = core.conns.get_mut(&self.handle) {
            match what {
                Wait::Read => cs.wait_read -= 1,
                Wait::Write => cs.wait_write -= 1,
            }
        }
        core
    }
}

impl io::Read for WgStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let inner = &self.inner;
        let mut core = inner.shared.core();
        loop {
            let reset = core.conns.get(&inner.handle).is_some_and(|c| c.reset);
            let sock = core.sockets.get_mut::<tcp::Socket>(inner.handle);
            if sock.can_recv() {
                let n = sock
                    .recv_slice(buf)
                    .map_err(|e| io::Error::new(io::ErrorKind::ConnectionAborted, e.to_string()))?;
                drop(core);
                // The window opened: let the stack tell the peer.
                inner.shared.wake();
                return Ok(n);
            }
            if !sock.may_recv() {
                return if reset { Err(io::ErrorKind::ConnectionReset.into()) } else { Ok(0) };
            }
            if core.stopped {
                return Err(io::ErrorKind::ConnectionAborted.into());
            }
            core = inner.wait(core, Wait::Read);
        }
    }
}

impl io::Write for WgStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let inner = &self.inner;
        let mut core = inner.shared.core();
        loop {
            let reset = core.conns.get(&inner.handle).is_some_and(|c| c.reset);
            let sock = core.sockets.get_mut::<tcp::Socket>(inner.handle);
            if !sock.may_send() {
                return Err(
                    if reset { io::ErrorKind::ConnectionReset } else { io::ErrorKind::BrokenPipe }.into()
                );
            }
            if sock.can_send() {
                let n = sock
                    .send_slice(buf)
                    .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e.to_string()))?;
                drop(core);
                inner.shared.wake();
                return Ok(n);
            }
            if core.stopped {
                return Err(io::ErrorKind::ConnectionAborted.into());
            }
            core = inner.wait(core, Wait::Write);
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Conn for WgStream {
    fn try_clone_conn(&self) -> io::Result<Box<dyn Conn>> {
        Ok(Box::new(WgStream { inner: self.inner.clone() }))
    }

    fn shutdown_write(&self) -> io::Result<()> {
        {
            let mut core = self.inner.shared.core();
            let sock = core.sockets.get_mut::<tcp::Socket>(self.inner.handle);
            if sock.may_send() {
                sock.close();
            }
        }
        self.inner.shared.wake();
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Dialing and DNS

/// Opens streams inside the tunnel. Names are looked up through the config's DNS servers
/// over the tunnel, never on this computer.
pub struct WgDialer {
    shared: Arc<Shared>,
}

impl Core {
    fn start_dns(&mut self, name: &str, ty: DnsQueryType) -> Result<dns::QueryHandle, dns::StartQueryError> {
        let Core { iface, sockets, dns, .. } = self;
        sockets.get_mut::<dns::Socket>(*dns).start_query(iface.context(), name, ty)
    }

    fn dns_result(&mut self, handle: dns::QueryHandle) -> Result<Vec<IpAddr>, dns::GetQueryResultError> {
        let found = self.sockets.get_mut::<dns::Socket>(self.dns).get_query_result(handle)?;
        Ok(found.into_iter().map(from_smol).collect())
    }

    fn dns_cancel(&mut self, handle: dns::QueryHandle) {
        self.sockets.get_mut::<dns::Socket>(self.dns).cancel_query(handle);
    }
}

fn wait_dns<'a>(shared: &'a Shared, mut core: MutexGuard<'a, Core>, max: Duration) -> MutexGuard<'a, Core> {
    core.dns_waiters += 1;
    core = shared.dns_cond.wait_timeout(core, max).unwrap_or_else(PoisonError::into_inner).0;
    core.dns_waiters -= 1;
    core
}

impl WgDialer {
    fn resolve(&self, name: &str, deadline: Instant) -> io::Result<Vec<IpAddr>> {
        let shared = &*self.shared;
        let key = name.trim_end_matches('.').to_ascii_lowercase();
        let mut core = shared.core();
        if let Some((ips, at)) = core.dns_cache.get(&key)
            && at.elapsed() < shared.opts.dns_cache_ttl
        {
            return Ok(ips.clone());
        }
        if !core.has_dns {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the WireGuard config has no DNS server (DNS = ...), so names cannot be looked up through the tunnel",
            ));
        }
        let mut types = Vec::new();
        if core.v6.is_some() {
            types.push(DnsQueryType::Aaaa);
        }
        if core.v4.is_some() {
            types.push(DnsQueryType::A);
        }
        let mut pending = Vec::new();
        for ty in types {
            loop {
                if core.stopped || Instant::now() >= deadline {
                    for h in pending {
                        core.dns_cancel(h);
                    }
                    return Err(io::ErrorKind::TimedOut.into());
                }
                match core.start_dns(&key, ty) {
                    Ok(h) => {
                        pending.push(h);
                        break;
                    }
                    Err(dns::StartQueryError::NoFreeSlot) => {
                        core = wait_dns(shared, core, Duration::from_millis(50));
                    }
                    Err(_) => {
                        for h in pending {
                            core.dns_cancel(h);
                        }
                        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a valid host name"));
                    }
                }
            }
        }
        shared.wake();
        let mut found: Vec<IpAddr> = Vec::new();
        loop {
            let mut still = Vec::new();
            for h in pending {
                match core.dns_result(h) {
                    Ok(v) => found.extend(v),
                    Err(dns::GetQueryResultError::Pending) => still.push(h),
                    Err(dns::GetQueryResultError::Failed) => {}
                }
            }
            pending = still;
            if pending.is_empty() {
                break;
            }
            let now = Instant::now();
            if now >= deadline || core.stopped {
                for h in pending {
                    core.dns_cancel(h);
                }
                if found.is_empty() {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "the DNS server did not answer"));
                }
                break;
            }
            core = wait_dns(shared, core, (deadline - now).min(Duration::from_millis(200)));
        }
        if found.is_empty() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "the name does not exist"));
        }
        found.dedup();
        if core.dns_cache.len() >= 512 {
            core.dns_cache.clear();
        }
        core.dns_cache.insert(key, (found.clone(), Instant::now()));
        Ok(found)
    }

    fn connect_ip(&self, remote: SocketAddr, deadline: Instant) -> io::Result<WgStream> {
        let shared = &self.shared;
        let mut core = shared.core();
        if core.stopped {
            return Err(io::ErrorKind::NotConnected.into());
        }
        if core.peer_for(remote.ip()).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "this address is not in the connection's AllowedIPs, so it is not sent through the tunnel (and never outside it)",
            ));
        }
        let local = match remote.ip() {
            IpAddr::V4(_) => core.v4.map(IpAddr::V4),
            IpAddr::V6(_) => core.v6.map(IpAddr::V6),
        }
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::Unsupported, "the tunnel has no address of this IP version")
        })?;
        if core.conns.len() >= shared.opts.max_streams {
            return Err(io::Error::new(io::ErrorKind::ConnectionRefused, "too many open connections"));
        }
        let port = core.alloc_port().ok_or_else(|| io::Error::other("no free local port"))?;
        let size = shared.opts.tcp_buffer;
        let mut sock =
            tcp::Socket::new(SocketBuffer::new(vec![0u8; size]), SocketBuffer::new(vec![0u8; size]));
        sock.set_nagle_enabled(false);
        sock.set_congestion_control(CongestionControl::Reno);
        sock.set_keep_alive(Some(SDuration::from_secs(30)));
        sock.set_timeout(Some(SDuration::from_secs(90)));
        let connected = {
            let Core { iface, .. } = &mut *core;
            sock.connect(
                iface.context(),
                IpEndpoint::new(to_smol(remote.ip()), remote.port()),
                IpListenEndpoint { addr: Some(to_smol(local)), port },
            )
        };
        if let Err(e) = connected {
            core.ports.remove(&port);
            return Err(io::Error::other(format!("could not open a connection: {e}")));
        }
        let handle = core.sockets.add(sock);
        let cond = Arc::new(Condvar::new());
        core.conns.insert(
            handle,
            ConnState {
                cond: cond.clone(),
                wait_read: 0,
                wait_write: 0,
                wait_connect: true,
                released: None,
                remote_closed: false,
                reset: false,
                ever_open: false,
                port,
            },
        );
        shared.wake();
        let outcome = loop {
            let state = core.sockets.get::<tcp::Socket>(handle).state();
            match state {
                State::SynSent | State::SynReceived => {}
                State::Closed => break Err(io::Error::from(io::ErrorKind::ConnectionRefused)),
                _ => break Ok(()),
            }
            let now = Instant::now();
            if now >= deadline {
                break Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "no answer from the host through the tunnel",
                ));
            }
            if core.stopped {
                break Err(io::ErrorKind::ConnectionAborted.into());
            }
            core = cond
                .wait_timeout(core, (deadline - now).min(Duration::from_secs(1)))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        };
        if let Some(cs) = core.conns.get_mut(&handle) {
            cs.wait_connect = false;
        }
        match outcome {
            Ok(()) => {
                drop(core);
                Ok(WgStream { inner: Arc::new(StreamInner { shared: shared.clone(), handle, cond }) })
            }
            Err(e) => {
                core.release(handle);
                drop(core);
                shared.wake();
                Err(e)
            }
        }
    }
}

impl Dialer for WgDialer {
    fn dial(&self, target: &Address) -> io::Result<Box<dyn Conn>> {
        let deadline = Instant::now() + self.shared.opts.connect_timeout;
        let (ips, port) = match target {
            Address::Ip(a) => (vec![a.ip()], a.port()),
            Address::Domain(name, port) => match name.parse::<IpAddr>() {
                Ok(ip) => (vec![ip], *port),
                Err(_) => (self.resolve(name, deadline)?, *port),
            },
        };
        let mut last = io::Error::new(io::ErrorKind::NotFound, "no address to connect to");
        let count = ips.len();
        for (i, ip) in ips.into_iter().enumerate() {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            // Each address but the last gets a share of the time, so one dead address does
            // not eat the whole budget.
            let attempt = if i + 1 < count { now + (deadline - now) / 2 } else { deadline };
            match self.connect_ip(SocketAddr::new(ip, port), attempt) {
                Ok(s) => return Ok(Box::new(s)),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}
