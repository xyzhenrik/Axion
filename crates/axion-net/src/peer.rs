//! Who is on the other end of a loopback connection: is it a process of Axion itself?
//!
//! Chromium cannot be made to answer the loopback front's login challenge reliably (CEF's
//! `GetAuthCredentials` is not called for it), and a front without any check would let every
//! program on the computer route through the user's tunnel. So the front also accepts a
//! connection whose peer is a process in Axion's **own process tree** (Chromium's network
//! service is a descendant of the browser process): the kernel's socket table tells which
//! socket the peer's source port is, `/proc/<pid>/fd` which process owns it, and the parent
//! chain whether that process descends from this one. Anyone else must send the per-session
//! login. Linux only (see [`ProcPeers`]); elsewhere nothing is trusted and the login is the
//! only way in.
//!
//! Threat model: this keeps other programs of the same user from *using* the front by
//! connecting to its port. A program that can already read this process's memory, ptrace it
//! or inject code into Chromium is not stopped by it (nothing in user space could).

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How the process table is read (a real `/proc`, or test data).
pub trait ProcSource: Send + Sync {
    /// The contents of `/proc/net/tcp` (IPv4 sockets).
    fn tcp_table(&self) -> String;
    /// Every process id.
    fn pids(&self) -> Vec<u32>;
    /// The child processes of a process, if the kernel can list them cheaply
    /// (`/proc/<pid>/task/<tid>/children`, needs `CONFIG_PROC_CHILDREN`); `None`: cannot say.
    fn children(&self, _pid: u32) -> Option<Vec<u32>> {
        None
    }
    /// The inodes of the sockets a process has open.
    fn socket_inodes(&self, pid: u32) -> Vec<u64>;
    /// The contents of `/proc/<pid>/stat`.
    fn stat(&self, pid: u32) -> Option<String>;
}

/// The real `/proc` (Linux).
pub struct ProcFs;

impl ProcSource for ProcFs {
    fn tcp_table(&self) -> String {
        std::fs::read_to_string("/proc/net/tcp").unwrap_or_default()
    }

    fn pids(&self) -> Vec<u32> {
        let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
        dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok()).collect()
    }

    fn children(&self, pid: u32) -> Option<Vec<u32>> {
        let tasks = std::fs::read_dir(format!("/proc/{pid}/task")).ok()?;
        let mut out = Vec::new();
        for task in tasks.filter_map(Result::ok) {
            let text = std::fs::read_to_string(task.path().join("children")).ok()?;
            out.extend(text.split_whitespace().filter_map(|c| c.parse::<u32>().ok()));
        }
        Some(out)
    }

    fn socket_inodes(&self, pid: u32) -> Vec<u64> {
        let Ok(dir) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { return Vec::new() };
        dir.filter_map(|e| {
            let target = std::fs::read_link(e.ok()?.path()).ok()?;
            let text = target.to_str()?;
            text.strip_prefix("socket:[")?.strip_suffix(']')?.parse().ok()
        })
        .collect()
    }

    fn stat(&self, pid: u32) -> Option<String> {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()
    }
}

/// The inode of the socket whose local address is `local` and whose peer is `remote`, from a
/// `/proc/net/tcp` table.
pub fn socket_inode(table: &str, local: SocketAddrV4Like, remote: SocketAddrV4Like) -> Option<u64> {
    table.lines().skip(1).find_map(|line| {
        let f: Vec<&str> = line.split_whitespace().collect();
        // sl local_address rem_address st tx:rx tr:when retrnsmt uid timeout inode
        if f.len() < 10 || parse_hex_addr(f[1])? != local || parse_hex_addr(f[2])? != remote {
            return None;
        }
        f[9].parse().ok()
    })
}

/// An IPv4 address and a port.
pub type SocketAddrV4Like = (Ipv4Addr, u16);

/// `0100007F:1F90` -> 127.0.0.1:8080 (the address is the kernel's little-endian word).
pub fn parse_hex_addr(s: &str) -> Option<SocketAddrV4Like> {
    let (ip, port) = s.split_once(':')?;
    let word = u32::from_str_radix(ip, 16).ok()?;
    Some((Ipv4Addr::from(word.swap_bytes()), u16::from_str_radix(port, 16).ok()?))
}

/// The parent of a process from its `stat` line (`pid (name) S ppid …`; the name may contain
/// spaces and parentheses, so the fields are counted from the last `)`).
pub fn parent_of(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// Longest parent chain followed (a loop or a runaway table ends here).
const MAX_DEPTH: usize = 64;

/// Whether the process descends from `root` (or is it).
pub fn descends_from(src: &dyn ProcSource, mut pid: u32, root: u32) -> bool {
    for _ in 0..MAX_DEPTH {
        if pid == root {
            return true;
        }
        if pid <= 1 {
            return false;
        }
        match src.stat(pid).as_deref().and_then(parent_of) {
            Some(parent) => pid = parent,
            None => return false,
        }
    }
    false
}

/// How long a verdict about an untrusted source port is remembered.
const REFUSAL_MEMORY: Duration = Duration::from_secs(5);

/// How long the list of processes of our own tree is reused (Chromium starts few processes,
/// rarely).
const TREE_MEMORY: Duration = Duration::from_secs(2);

/// Most processes of the tree that are looked at (a runaway fork bomb ends here).
const MAX_TREE: usize = 512;

/// A connection nobody in the (older than this) list owns makes the list be made again.
const TREE_RETRY_AFTER: Duration = Duration::from_millis(250);

/// `root` and all its descendants. Uses the kernel's child lists when it has them (a few
/// small reads); otherwise one pass over every `stat` (never the `fd` directories).
pub fn own_tree(src: &dyn ProcSource, root: u32) -> Vec<u32> {
    let mut tree = vec![root];
    let mut i = 0;
    let mut listed = true;
    while i < tree.len() && tree.len() < MAX_TREE {
        match src.children(tree[i]) {
            Some(children) => {
                for c in children {
                    if !tree.contains(&c) {
                        tree.push(c);
                    }
                }
            }
            None => {
                listed = false;
                break;
            }
        }
        i += 1;
    }
    if listed {
        return tree;
    }
    // No child lists: every process whose parent chain reaches the root (stat files only).
    let mut tree = vec![root];
    for pid in src.pids() {
        if pid != root && tree.len() < MAX_TREE && descends_from(src, pid, root) {
            tree.push(pid);
        }
    }
    tree
}

/// Decides whether a loopback peer is part of this program's process tree.
pub struct PeerCheck {
    src: Box<dyn ProcSource>,
    root: u32,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Processes known to descend from the root (checked first: a browser opens many
    /// connections from few processes).
    trusted: HashSet<u32>,
    /// Source ports that were refused, and when.
    refused: HashMap<u16, Instant>,
    /// The processes of the own tree and when the list was made.
    tree: Option<(Instant, Vec<u32>)>,
}

impl PeerCheck {
    pub fn new(src: Box<dyn ProcSource>, root: u32) -> Self {
        Self { src, root, state: Mutex::new(State::default()) }
    }

    /// For this process on the real `/proc`.
    pub fn for_this_process() -> Self {
        Self::new(Box::new(ProcFs), std::process::id())
    }

    fn tree_age(&self) -> Duration {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.tree.as_ref().map_or(Duration::MAX, |(at, _)| at.elapsed())
    }

    /// Whether the connection from `peer` to `local` (both loopback IPv4) comes from a process
    /// of the tree. Anything unclear is "no".
    pub fn is_own(&self, peer: SocketAddr, local: SocketAddr) -> bool {
        let (IpAddr::V4(peer_ip), IpAddr::V4(local_ip)) = (peer.ip(), local.ip()) else { return false };
        if !peer_ip.is_loopback() {
            return false;
        }
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.refused.retain(|_, at| at.elapsed() < REFUSAL_MEMORY);
            if state.refused.contains_key(&peer.port()) {
                return false;
            }
        }
        // The peer's end of the connection: its local address is our peer address.
        let table = self.src.tcp_table();
        let Some(inode) = socket_inode(&table, (peer_ip, peer.port()), (local_ip, local.port())) else {
            return false;
        };
        // Only processes of our own tree are searched: a connection of anyone else is not
        // found, which is a refusal. (Scanning every process's descriptors for each accepted
        // connection cost a lot while the browser opened dozens of them.)
        let (trusted, tree): (Vec<u32>, Vec<u32>) = {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let trusted = state.trusted.iter().copied().collect();
            let stale = state.tree.as_ref().is_none_or(|(at, _)| at.elapsed() >= TREE_MEMORY);
            if stale {
                drop(state);
                let tree = own_tree(self.src.as_ref(), self.root);
                let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                state.tree = Some((Instant::now(), tree.clone()));
                // Processes that left the tree are no longer trusted.
                state.trusted.retain(|p| tree.contains(p));
                (state.trusted.iter().copied().collect(), tree)
            } else {
                (trusted, state.tree.as_ref().map(|(_, t)| t.clone()).unwrap_or_default())
            }
        };
        let mut owner = trusted
            .into_iter()
            .chain(tree)
            .find(|pid| self.src.socket_inodes(*pid).contains(&inode));
        if owner.is_none() && self.tree_age() >= TREE_RETRY_AFTER {
            // A process started since the list was made (Chromium restarts its network
            // service): look at the tree once more.
            let tree = own_tree(self.src.as_ref(), self.root);
            self.state.lock().unwrap_or_else(PoisonError::into_inner).tree =
                Some((Instant::now(), tree.clone()));
            owner = tree.into_iter().find(|pid| self.src.socket_inodes(*pid).contains(&inode));
        }
        let own = owner.is_some();
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        match (owner, own) {
            (Some(pid), true) => {
                state.trusted.insert(pid);
            }
            _ => {
                state.refused.insert(peer.port(), Instant::now());
            }
        }
        own
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake `/proc`: processes with a parent and sockets, and the socket table.
    #[derive(Default)]
    struct Fake {
        /// pid -> (parent, socket inodes)
        procs: HashMap<u32, (u32, Vec<u64>)>,
        table: String,
        /// Whether `children` answers (as with CONFIG_PROC_CHILDREN).
        child_lists: bool,
        /// How many descriptor directories were read.
        fd_reads: std::sync::atomic::AtomicUsize,
    }

    impl ProcSource for Fake {
        fn tcp_table(&self) -> String {
            self.table.clone()
        }
        fn pids(&self) -> Vec<u32> {
            self.procs.keys().copied().collect()
        }
        fn socket_inodes(&self, pid: u32) -> Vec<u64> {
            self.fd_reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.procs.get(&pid).map(|p| p.1.clone()).unwrap_or_default()
        }
        fn children(&self, pid: u32) -> Option<Vec<u32>> {
            self.child_lists.then(|| {
                let mut v: Vec<u32> =
                    self.procs.iter().filter(|(_, p)| p.0 == pid).map(|(k, _)| *k).collect();
                v.sort_unstable();
                v
            })
        }
        fn stat(&self, pid: u32) -> Option<String> {
            // A name with spaces and a parenthesis, like real ones.
            self.procs.get(&pid).map(|p| format!("{pid} (we ird) name) S {} 1 1 0", p.0))
        }
    }

    fn row(local: &str, remote: &str, inode: u64) -> String {
        format!(
            "   0: {local} {remote} 01 00000000:00000000 00:00000000 00000000  1000        0 {inode} 1 0 0 0\n"
        )
    }

    const HEADER: &str =
        "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n";

    #[test]
    fn hex_addresses() {
        assert_eq!(parse_hex_addr("0100007F:1F90"), Some((Ipv4Addr::LOCALHOST, 8080)));
        assert_eq!(parse_hex_addr("0101A8C0:0050"), Some((Ipv4Addr::new(192, 168, 1, 1), 80)));
        assert_eq!(parse_hex_addr("zz:1"), None);
        assert_eq!(parse_hex_addr("0100007F"), None);
    }

    #[test]
    fn parents_from_stat_lines() {
        assert_eq!(parent_of("42 (axion) S 7 42 42 0"), Some(7));
        assert_eq!(parent_of("42 (a b) c) R 9 1 1"), Some(9));
        assert_eq!(parent_of("garbage"), None);
    }

    #[test]
    fn inodes_from_the_table() {
        let table = format!(
            "{HEADER}{}{}",
            row("0100007F:C350", "0100007F:1F90", 111),
            row("0100007F:1F90", "0100007F:C350", 222)
        );
        let lo = Ipv4Addr::LOCALHOST;
        assert_eq!(socket_inode(&table, (lo, 0xC350), (lo, 0x1F90)), Some(111));
        assert_eq!(socket_inode(&table, (lo, 0x1F90), (lo, 0xC350)), Some(222));
        assert_eq!(socket_inode(&table, (lo, 1), (lo, 2)), None);
    }

    fn fake() -> Fake {
        let mut f = Fake::default();
        // 100 = axion (root), 101 = its child, 102 = grandchild with the socket;
        // 200 = an unrelated program (parent 1), 201 = child of 200 with another socket.
        f.procs.insert(100, (50, vec![]));
        f.procs.insert(101, (100, vec![]));
        f.procs.insert(102, (101, vec![111]));
        f.procs.insert(200, (1, vec![]));
        f.procs.insert(201, (200, vec![222]));
        f.table = format!(
            "{HEADER}{}{}",
            row("0100007F:C350", "0100007F:1F90", 111),
            row("0100007F:C351", "0100007F:1F90", 222)
        );
        f
    }

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    /// Lets a test look at the fake after the check owns it.
    struct Shared(std::sync::Arc<Fake>);

    impl ProcSource for Shared {
        fn tcp_table(&self) -> String {
            self.0.tcp_table()
        }
        fn pids(&self) -> Vec<u32> {
            self.0.pids()
        }
        fn children(&self, pid: u32) -> Option<Vec<u32>> {
            self.0.children(pid)
        }
        fn socket_inodes(&self, pid: u32) -> Vec<u64> {
            self.0.socket_inodes(pid)
        }
        fn stat(&self, pid: u32) -> Option<String> {
            self.0.stat(pid)
        }
    }

    #[test]
    fn only_the_own_tree_has_its_descriptors_read() {
        for child_lists in [true, false] {
            let mut f = fake();
            f.child_lists = child_lists;
            // Many unrelated programs with sockets.
            for pid in 1000..1200 {
                f.procs.insert(pid, (1, vec![5000 + u64::from(pid)]));
            }
            f.table.push_str(&row("0100007F:C360", "0100007F:1F90", 5100));
            let f = std::sync::Arc::new(f);
            let check = PeerCheck::new(Box::new(Shared(f.clone())), 100);
            assert!(check.is_own(addr(0xC350), addr(0x1F90)));
            assert!(!check.is_own(addr(0xC360), addr(0x1F90)), "a stranger is refused");
            let reads = f.fd_reads.load(std::sync::atomic::Ordering::Relaxed);
            assert!(reads < 20, "{reads} descriptor directories read (child lists: {child_lists})");
        }
    }

    #[test]
    fn the_walker_follows_parents() {
        let f = fake();
        assert!(descends_from(&f, 102, 100));
        assert!(descends_from(&f, 100, 100));
        assert!(!descends_from(&f, 201, 100));
        assert!(!descends_from(&f, 999, 100), "an unknown process is not ours");
    }

    #[test]
    fn a_loop_in_the_table_ends() {
        let mut f = fake();
        f.procs.insert(300, (301, vec![]));
        f.procs.insert(301, (300, vec![]));
        assert!(!descends_from(&f, 300, 100));
    }

    #[test]
    fn own_tree_is_trusted_and_others_are_not() {
        let check = PeerCheck::new(Box::new(fake()), 100);
        assert!(check.is_own(addr(0xC350), addr(0x1F90)), "descendant of the root");
        assert!(!check.is_own(addr(0xC351), addr(0x1F90)), "an unrelated program");
        assert!(!check.is_own(addr(0xC399), addr(0x1F90)), "a connection nobody owns");
        assert!(!check.is_own(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 0xC350), addr(0x1F90)));
        // The trusted process is found without scanning again.
        assert!(check.is_own(addr(0xC350), addr(0x1F90)));
    }

    /// The real `/proc`: a socket this very process opened is its own.
    #[cfg(target_os = "linux")]
    #[test]
    fn this_process_is_its_own_peer() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let client = std::net::TcpStream::connect(listener.local_addr().expect("addr")).expect("connect");
        let (server_side, _) = listener.accept().expect("accept");
        let check = PeerCheck::for_this_process();
        assert!(
            check.is_own(server_side.peer_addr().expect("peer"), server_side.local_addr().expect("local"))
        );
        // A root that is not us: the same peer is then foreign (our parent is not this process).
        let other = PeerCheck::new(Box::new(ProcFs), 1 << 30);
        assert!(
            !other.is_own(server_side.peer_addr().expect("peer"), server_side.local_addr().expect("local"))
        );
        drop(client);
    }
}
