#!/usr/bin/env python3
"""Serves the web fixtures on two origins (they are different sites to the browser):

  site  http://localhost:8101   the relying party (the page with the sign-in button)
  idp   http://127.0.0.1:8102   the identity provider (the sign-in popup, an iframe)

Pages report what they saw with GET /report?k=v&..., which is appended to the report file
(default: report.log next to this script) and printed. `idp` also has:

  /authorize         302 to the site's callback page (the redirect flow)
  /frame.html        sets a cookie and says whether the request carried one (third-party cookies)

Usage: serve.py [REPORT_FILE]     Stop it with SIGTERM (its PID is printed on the first line).
"""

import http.server
import os
import sys
import threading
import urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))
SITE_PORT, IDP_PORT = 8101, 8102
REPORT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "report.log")
LOCK = threading.Lock()


def note(line):
    with LOCK:
        with open(REPORT, "a") as out:
            out.write(line + "\n")
        print(line, flush=True)


class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=HERE, **kwargs)

    def log_message(self, fmt, *args):
        pass

    def do_GET(self):
        url = urllib.parse.urlparse(self.path)
        port = self.server.server_address[1]
        if url.path == "/report":
            fields = urllib.parse.parse_qsl(url.query)
            note(f"[{port}] " + " ".join(f"{k}={v}" for k, v in fields))
            self.send_response(204)
            self.send_header("Access-Control-Allow-Origin", "*")
            self.end_headers()
        elif url.path == "/authorize" and port == IDP_PORT:
            self.send_response(302)
            self.send_header("Location", f"http://localhost:{SITE_PORT}/callback.html?code=abc")
            self.end_headers()
        elif url.path == "/frame.html" and port == IDP_PORT:
            had_cookie = "sid=1" in (self.headers.get("Cookie") or "")
            note(f"[{port}] frame request carried the idp cookie: {had_cookie}")
            body = f"<!doctype html><title>idp frame</title><p>cookie sent: {had_cookie}</p>".encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            self.send_header("Content-Length", str(len(body)))
            # A third-party cookie as a sign-in provider sets one: SameSite=None.
            self.send_header("Set-Cookie", "sid=1; Path=/; SameSite=None; Secure")
            self.end_headers()
            self.wfile.write(body)
        else:
            super().do_GET()


def main():
    servers = [http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler) for port in (SITE_PORT, IDP_PORT)]
    print(os.getpid(), flush=True)
    for server in servers[1:]:
        threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        servers[0].serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
