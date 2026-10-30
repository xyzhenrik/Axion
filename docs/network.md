# Network & VPN

Axion can send its traffic through an upstream proxy (SOCKS5, HTTP, HTTPS), a WireGuard VPN
(userspace, no root) or Tor, per account, per private tab and per site. This page describes
how it works, what it protects and what it does not. Settings: **Settings > Network & VPN**
(`[network]` in `config.toml`).

Upstream proxies, WireGuard and Tor all work.

## WireGuard

A fully userspace client: no root, no TUN device, no kernel module, no external program (the way
`wireproxy` works). Import a `.conf` in **Settings > Network & VPN > Add WireGuard** (file or
clipboard). The keys go to the password vault (hidden secret), everything else to `config.toml`.

```
Chromium -> HTTP front (login) -> local SOCKS5 (login) -> Dialer -> smoltcp TCP stream
         -> IP packets -> boringtun (Noise, timers, rekey) -> one UDP socket -> the peer
```

* `crates/axion-net/src/wireguard.rs`: the `Tunnel` (config -> keys and networks, start, status).
  `wg_net.rs`: the stack. One worker thread owns the WireGuard peers ([boringtun]) and a
  userspace TCP/IP stack ([smoltcp]) behind a virtual IP device; a reader thread empties the UDP
  socket into the worker's queue (the kernel's UDP buffer is small); the worker blocks on that
  queue with smoltcp's `poll_delay` (never a busy loop). SOCKS5 connection threads read and write
  the smoltcp sockets directly under one lock and sleep on a per-stream condition variable that
  the worker signals only when that stream can progress (backpressure is the socket buffer;
  half-close, resets and clean teardown are handled).
* **Handshake and status.** `Connecting` until the first handshake completes, then
  `Up { info: "Peer host:port, last handshake 12 s ago" }` (also in the Settings row and the chip
  popover). `Failed("No answer from ... within N s. Check the endpoint, the keys and that UDP is
  not blocked.")` when a handshake stays unanswered for `network.wireguard_handshake_timeout_secs`
  (default 20): a wrong key, a dead server and blocked UDP look the same in WireGuard, which never
  answers a stranger. A tunnel that was up and then loses its peer turns `Failed` the same way
  (the kill switch blocks), starts a new handshake every 15 s by itself and comes back to `Up`.
  Rekeying (every 2 minutes of traffic) and keepalives (`PersistentKeepalive`) are boringtun's.
* **DNS never leaves the tunnel.** A host name in a SOCKS5/CONNECT request is looked up with the
  config's `DNS =` servers, over UDP through the tunnel (A, and AAAA when the config has an IPv6
  address), cached for 30 s. Without a `DNS =` line names are refused with a clear error; the local
  resolver is never used for them. The one local lookup is the peer's `Endpoint` host name, with the
  system resolver, at start and again (and the socket re-created) when a handshake stays unanswered
  or sending keeps failing (sleep/wake, network change).
* **AllowedIPs are the kill switch of the tunnel.** Only destinations inside the AllowedIPs of some
  peer (longest prefix wins, several peers are supported) are connected; anything else is refused
  with SOCKS5 "connection not allowed" and nothing is sent to the local network. Incoming packets
  must come from an allowed address of the peer that sent them.
* IPv4 and IPv6 interface addresses (the first of each family is used), MTU from the config
  (default 1420), preshared keys, IPv6 endpoints (the UDP socket is dual stack when needed).
* Throughput on loopback with two userspace stacks: several Gbit/s in release builds (the test
  asserts more than 50 Mbit/s), 100 parallel streams of 200 kB each in about two seconds.
* Streams: `network.max_relay_connections` (shared with the other backends) bounds the open
  streams; each has 256 KiB of buffer per direction, Reno congestion control.

**Not supported / ignored.** `PreUp`, `PostUp`, `PreDown`, `PostDown`, `SaveConfig`, `Table`,
`FwMark` and search domains in `DNS =` are never run or used (the import lists them as ignored).
Only TCP is carried: SOCKS5 CONNECT has no UDP, so no QUIC/HTTP3 through the tunnel (QUIC is
switched off while routed), no WebRTC media, no ICMP. IPv6 destinations need an IPv6 `Address`
in the config. DNS goes over UDP only (no TCP fallback for truncated answers; names with huge
answers may fail). Only the first IPv4 and first IPv6 address of the interface are used. Keys
live in zeroizing buffers, but boringtun's internal copies are not wiped. This is a TCP-only
"VPN for the browser", not a system VPN: other programs on this computer are not affected.

Dependencies added (all permissive): `boringtun` 0.7 (BSD-3-Clause), `smoltcp` 0.12 (0BSD) and
their trees: `x25519-dalek`/`curve25519-dalek` (BSD-3-Clause), `ring` (ISC/OpenSSL-style),
`chacha20poly1305`, `blake2`, `hmac`, `aead` (MIT/Apache-2.0), `heapless`, `managed`,
`ip_network`, `ip_network_table`, `parking_lot`, `byteorder`, `portable-atomic` (MIT/Apache-2.0/0BSD).

**Testing without a real server.** `axion_net::wg_testing` is a WireGuard *server* on UDP
loopback (a second boringtun peer plus its own smoltcp stack that answers for any address:
forwards TCP to real local servers and answers DNS from a table). `tests/wireguard.rs` covers
handshake, name resolution, 50 MB each way with checksums, 100 parallel streams, rekey, half-close,
AllowedIPs refusal, "no DNS server", a silent server, a wrong key, a wrong preshared key, stop and
restart, and (ignored, about 20 s each) recovery after a server restart and after a dark network.
For the real browser: `cargo run --release -p axion-net --example wg_world -- /tmp/test.conf`
prints an endpoint and writes a `.conf`; on a throwaway profile
`axion --open=wg-test:/tmp/test.conf` imports it and routes the account through it, and
`http://only-in-tunnel.test/` (a name that exists only inside) loads; stop the example and the
kill-switch page appears. Note: the first `--open=wg-test` sets up the test vault, which takes
about 20 s with a TPM.

## Tor

The Tor connection runs [Arti](https://gitlab.torproject.org/tpo/core/arti), the Tor Project's
Rust client, inside Axion (`crates/axion-net/src/tor/`). **This is not Tor Browser.** It hides
your IP address from sites and your sites from your network, but Axion does not add Tor
Browser's fingerprinting resistance (no letterboxing, no uniform fonts or canvas, sites can
recognise your browser and cookies/accounts still identify you). Use Tor Browser when you need
that.

* **Bootstrapping.** The first start downloads the relay list from the Tor network itself
  (10 to 60 s); later starts reuse the cache in `<profile>/network/tor/`. The chip, its popover
  and Settings show "Bootstrapping 40%: loading relays". The connection counts as up once Arti
  can carry traffic; until then the kill switch blocks the routed requests.
* **Names are resolved by the exit relay**, never on this computer; `.onion` addresses work.
  Requests to `localhost`, loopback, private and link-local addresses are refused before they
  reach Tor (it cannot reach them, and asking would tell the exit what you tried).
* **Circuits per site.** `Circuits` in the connection form (`isolation` in `config.toml`):
  `site` (default: every registrable domain gets its own circuits, so no exit relay sees two
  sites), `context` (one set of circuits for everything the connection carries) or `none`
  (Arti's default). Isolation is by the *destination* site, because the loopback front cannot
  tell which page a request belongs to (Tor Browser isolates by the first-party page); normal
  and private tabs that use the same Tor connection share it.
* **Circuit display.** Pages that go through Tor get a route button in the address field. Its
  popover shows the circuit of the site: This browser, guard, middle relay, exit relay (for
  onion services: guard, middle, rendezvous point, onion service) with a country badge and
  IP address per relay, refreshed every second. The path comes from Arti's circuit of the
  newest stream to the site (`DataStream::client_stream_ctrl`, `tor-proto`'s `stream-ctrl`
  feature); countries come from the GeoIP database compiled into the program (no lookups over
  the network). Hops are never invented: a site without a known circuit shows "Connecting…".
  *New Tor circuit for this site* retires the site's isolation token, closes its open
  connections and reloads the tab (your guard relay may stay the same). *New Tor identity*
  does that for every site and reloads all tabs that use the connection.
* **Bridges.** Plain bridges (`ip:port fingerprint`, with or without a leading `Bridge`) are
  supported. Pluggable transports (obfs4, snowflake, meek) need separate programs this build
  does not ship; such lines are refused with a message.
* **Exit country** (`exit_country`, e.g. `de`) uses Arti's GeoIP exit filter; it narrows the
  crowd you blend into.
* **Limits.** TCP only: no UDP, QUIC or WebRTC (QUIC is switched off while routed). Tor is slow
  compared to a direct connection. One Tor connection per profile is active at a time (Arti
  locks its state directory). Tor is legal in most countries; what you do with it is your
  responsibility, and some networks and sites block it.
* **Logs** never contain destination hosts at info level (only at debug).

Added dependencies (all Apache-2.0 or MIT): `arti-client`, `tor-rtcompat`, `tor-proto`,
`tor-linkspec`, `tor-geoip` (+ `tor-geoip-db`, the embedded country data), their `tor-*`
siblings, `tokio`, `futures`, `rustls` (with `ring`, ISC/Apache-2.0), `addr`. The OpenSSL
backend of Arti is not used (rustls). `rusqlite` moved from 0.41 to 0.40 because Arti's
directory cache requires `rusqlite < 0.41` (one `libsqlite3-sys` per program).

Manual checks (they need the Tor network): `cargo test -p axion-net --lib -- --ignored real_tor
--nocapture`, or `cargo run -p axion-net --example tor_socks` and
`curl --socks5-hostname USER:PASS@127.0.0.1:PORT https://check.torproject.org/api/ip`
(`"IsTor":true`).

## Architecture

```
crates/axion-net   no UI, no engine: connections, routing, PAC, loopback servers, manager
crates/axion-engine  applies a proxy + leak preferences per request context (ProxySetting)
crates/axion-app   network.rs (state, vault cache, kill-switch page), settings/network.rs,
                   network_popover.rs, browser/network*.rs (glue, chip, requests)
```

* `ConnectionKind { Direct, Proxy, WireGuard, Tor }`, saved in `config.toml` **without
  secrets**. Proxy passwords and WireGuard keys are stored in the password vault as hidden
  vault secrets (`Vault::set_secret`, name `net:<connection id>`), never listed among the items.
  If the vault is locked, a connection that needs secrets is *Blocked* ("vault is locked") and
  its traffic is blocked, never sent directly. Unlocking Passwords starts it. Connections
  without secrets (SOCKS5 without login, Tor) do not need the vault.
* `trait Tunnel { start() -> LocalEndpoint; status(); stop() }`: a backend runs on its own
  threads and returns a SOCKS5 server on `127.0.0.1:<random>` that demands a random
  per-session login. `LocalServer::socks5(credentials, dialer, options)` builds that server for
  a backend from a `Dialer` (opens a stream by *name*; the far end resolves it).
* `NetManager` owns the named connections, starts them lazily on worker threads, polls their
  status, and turns the routing rules into a `Plan` for the engine.
* **Nothing here blocks the UI thread.** Starting, stopping and restarting a connection happen on
  `net-start` / `net-stop` threads (a restart waits for the previous run to finish stopping, so Tor's
  state directory is free); `ensure_started`, `plan`, `poll`, `status` and `stop` only take a short lock
  (regression tests in `manager.rs` use backends that start and stop in 3 s). Tor's runtime runs
  `tor_worker_threads` (default 2) threads at nice 15 while `background_priority` is on, so the
  directory download and its verification never compete with the browser for CPU. Tor's bootstrap
  progress reaches the UI at most four times a second, and the front's peer check only searches the
  own process tree (child lists from `/proc/<pid>/task/*/children`, cached 2 s) instead of every
  process's descriptors.
* Chromium cannot log in to SOCKS5. So Chromium is pointed at `LocalServer::http_front`, an
  HTTP proxy on loopback with a per-session Basic login (answered through CEF's
  `GetAuthCredentials`) that dials through the tunnel's authenticated SOCKS5. Neither loopback
  port accepts a connection without its login, with one exception described next.
* **Peer-process authentication (the front).** Chromium never answers the front's 407
  challenge (CEF does not call `GetAuthCredentials` for it; the net-log shows the challenge and
  the connection being dropped), so the front also lets in a loopback connection whose *peer
  process descends from Axion's own process*: the source port is looked up in
  `/proc/net/tcp`, the owning pid in `/proc/<pid>/fd`, and the parent chain (`/proc/<pid>/stat`)
  must reach this process (`crates/axion-net/src/peer.rs`, injectable `/proc` for tests,
  verdicts for known pids are cached). Anyone else (another program of the same user, a
  sandboxed app sharing the network) still has to send the per-session Basic login, which only
  this process knows. Where `/proc` is unavailable (not Linux) nothing is trusted and the
  login is the only way in. Switch it off with `network.front_trust_own_processes = false`.
  Threat model: it stops other programs from *using* the loopback port; a program that can
  read this process's memory or inject code into Chromium is not stopped (nothing in user space
  could), and descendants of Axion (extension helper processes, anything Chromium launches) are
  trusted like Chromium.
* Direct exposure: HTTP/HTTPS proxies and SOCKS5 proxies without a login are handed to Chromium
  as they are (`ProxyMonitor` only watches them). HTTP(S) proxy logins are answered through
  `GetAuthCredentials` (with a retry limit). A SOCKS5 proxy **with** a login goes through
  `ProxyTunnel` (loopback relay, RFC 1928/1929, names sent as domain names).

### Routing

Precedence, strongest first: per-tab override, per-site rule, private default, account default
(`RoutingConfig::resolve`). Site rules cover subdomains (`example.com` includes `www.example.com`).

Per context (CEF 156): the proxy is a preference (`proxy`) of a request context. The account's
tabs share the global context, private tabs the in-memory one. With site rules a context gets a
PAC script (`mode: pac_script`, `pac_mandatory: true`, as a `data:` URL) that only compares
host names (no `dnsResolve`, never a `DIRECT` fallback behind a proxy answer). Without rules
it gets `fixed_servers`. The popover's "Direct / Use X for this site" adds a site rule.

**Limit:** a per-tab override would need a third request context per route (and with it its own
cookie jar and cache). That is not implemented. TODO: `RequestContext` per connection for
"this tab only" overrides; until then the per-site rule is the override.

### Kill switch (`network.kill_switch`, default on)

When a route's connection is not up (starting, failed, vault locked, removed) it is routed to
the *sink*, a loopback SOCKS5 server that refuses everything. Chromium reports a proxy error,
the engine emits `ProxyFailed`, and the browser shows `axion://network/blocked` ("Connection to
X is down. Axion blocked this request to protect you.") with **Retry** (restarts it, and the
page loads by itself once it is up), **Use direct connection** (asks to confirm, then adds a
visible site rule "direct" for the site) and **Open network settings**. Nothing falls back to
direct on its own. With the switch off, a down connection's traffic goes direct (an explicit
opt-in, and the toast says so).

If a proxy looked up but a page failed through it, it is checked at once; the failing page then
shows the blocked page. Otherwise a proxy is checked every `network.health_check_secs` (two
failures mark it down).

### Leak protection (`network.leak_protection`, default on)

Per routed context (preferences): `webrtc.ip_handling_policy = disable_non_proxied_udp`,
no multiple routes / non-proxied UDP, DNS prefetch and preconnect off, secure DNS off (it
would resolve names itself), alternate error pages, Safe Browsing and usage-statistics off.
Names are sent to the proxy (SOCKS5 and HTTP CONNECT carry the host name), so this computer does
not resolve them; the only name resolved locally is the proxy's own.

Process-wide switches, applied at start when a route exists (a switch cannot change later):
`disable-background-networking`, `disable-component-update`, `disable-sync`,
`disable-domain-reliability`, `no-pings`, `disable-field-trial-config`,
`disable-client-side-phishing-detection`; `disable-quic` (`network.disable_quic_when_routed`;
QUIC does not run over TCP proxies anyway, this also covers sites that go direct while others
are routed); and with `network.strict_dns` (off by default), when every request is routed,
`--host-resolver-rules="MAP * ~NOTFOUND, EXCLUDE localhost, EXCLUDE <proxy hosts>"`.

Logs never contain secrets: `Secret` and `Credentials` print as redacted, the PAC script and
the visited hosts are not logged.

## Threat model

Protects against: sites and the network between you and the exit seeing your IP address;
your resolver / ISP seeing which names you look up; WebRTC and background services revealing
the real address; silent fallback to a direct connection when the connection drops; other
programs (or sandboxed apps sharing the loopback) using the local relays.

Does not protect against: fingerprinting (this is not Tor Browser: window size, fonts, canvas,
timezone, language and extensions are not normalised); a malicious or logging proxy / VPN
operator; correlation by cookies and accounts you sign in with; traffic to localhost and the
local network (Chromium never proxies loopback / link-local, deliberately); the proxy's own host
name resolution; downloads and extensions' own network use outside Chromium's network stack
(Axion's update checks, filter-list downloads and favicon fetches by the app itself are made by
Axion, not by Chromium, and do not use the route yet).

## Settings

`[network]`: `connections`, `default_connection` (`"direct"` or an id), `private_connection`
(unset = same as the account), `site_rules` (`host`, `connection`), `kill_switch`,
`leak_protection`, `disable_quic_when_routed`, `strict_dns`, `check_ip_url` (empty: no
check; Axion never calls it on its own, the button opens it in a tab through the tab's route),
`connect_timeout_secs`, `health_check_secs`, `max_relay_connections`,
`wireguard_handshake_timeout_secs`, `background_priority`, `tor_worker_threads`, `connect_at_start`,
`show_chip`, `notify_changes`. The keymap action `toggle_network` (unbound) switches the current
site between its route and direct; it also appears in the command palette.

## Adding a backend

Implement `Tunnel` in `wireguard.rs` (`tor/` is the worked example), return `LocalServer::socks5(..)` from `start()`,
set `AVAILABLE = true`. The manager restores keys from the vault (`WgConfig::with_secrets`),
fronts the tunnel with the authenticating HTTP proxy, and shows it in Settings and the chip.
