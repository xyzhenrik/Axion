# Network & VPN

Axion can send its traffic through an upstream proxy (SOCKS5, HTTP, HTTPS), a WireGuard VPN
(userspace, no root) or Tor, per account, per private tab and per site. This page describes
how it works, what it protects and what it does not. Settings: **Settings > Network & VPN**
(`[network]` in `config.toml`).

WireGuard and Tor have their pages, forms and plug-in points ready, but their backends
(`crates/axion-net/src/wireguard.rs`, `tor.rs`) answer "not available in this build yet".
Upstream proxies work today.

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
* Chromium cannot log in to SOCKS5. So Chromium is pointed at `LocalServer::http_front`, an
  HTTP proxy on loopback with a per-session Basic login (answered through CEF's
  `GetAuthCredentials`) that dials through the tunnel's authenticated SOCKS5. Neither loopback
  port accepts a connection without its login.
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
`connect_timeout_secs`, `health_check_secs`, `max_relay_connections`, `connect_at_start`,
`show_chip`, `notify_changes`. The keymap action `toggle_network` (unbound) switches the current
site between its route and direct; it also appears in the command palette.

## Adding a backend

Implement `Tunnel` in `wireguard.rs` / `tor.rs`, return `LocalServer::socks5(..)` from `start()`,
set `AVAILABLE = true`. The manager restores keys from the vault (`WgConfig::with_secrets`),
fronts the tunnel with the authenticating HTTP proxy, and shows it in Settings and the chip.
