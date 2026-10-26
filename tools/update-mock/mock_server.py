#!/usr/bin/env python3
"""A fake release server for the updater: python http.server over ./site.

Paths below /slow/ answer after SLOW seconds (to capture the "Checking..." state).
Prints every request line so tests can see what the updater sends.
"""
import functools
import http.server
import os
import sys
import time

SLOW = 25
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 18941
ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "site")


class Handler(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        print("GET", self.path, "UA:", self.headers.get("User-Agent"), flush=True)
        if self.path.startswith("/slow/"):
            time.sleep(SLOW)
            self.path = self.path[len("/slow") :]
        if self.path.startswith("/throttle/"):
            return self.throttled(self.path[len("/throttle") :])
        super().do_GET()

    def throttled(self, path):
        full = os.path.join(ROOT, path.lstrip("/"))
        size = os.path.getsize(full)
        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(size))
        self.end_headers()
        with open(full, "rb") as f:
            while chunk := f.read(256 * 1024):
                self.wfile.write(chunk)
                self.wfile.flush()
                time.sleep(0.15)

    def guess_type(self, path):
        if path.endswith("/latest") or path.endswith(".json"):
            return "application/json"
        return super().guess_type(path)

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    handler = functools.partial(Handler, directory=ROOT)
    http.server.ThreadingHTTPServer(("127.0.0.1", PORT), handler).serve_forever()
