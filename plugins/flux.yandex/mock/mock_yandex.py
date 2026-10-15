#!/usr/bin/env python3
"""A fake Yandex for the plugin's tests and Flux scenarios: Yandex ID's OAuth, Tracker's API v3
(tasks, boards) and the Calendar's CalDAV on http://127.0.0.1:<port>, stdlib only.

usage: mock_yandex.py [port]   (8765 by default; the first line printed is "READY <port>")

Flux sends the plugin's requests here with
FLUX_PLUGIN_HTTP_OVERRIDE="https://oauth.yandex.ru=http://127.0.0.1:<port>,https://login.yandex.ru=http://127.0.0.1:<port>,https://api.tracker.yandex.net=http://127.0.0.1:<port>,https://caldav.yandex.ru=http://127.0.0.1:<port>".

The parts register their routes: `oauth` and `tracker` (agent F), `boards` (agent F2), `caldav`
(agent G) — modules next to this one with `register(router, state)`. A route is a method, a regex
over the path (without the query) and a handler `(request, match) -> Response`. `GET /_mock/log`
— the requests so far (JSON); `POST /_mock/reset` — the data back to the start.
"""
import importlib
import json
import re
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

PARTS = ["oauth", "tracker", "boards", "caldav"]


class Request:
    def __init__(self, method, path, query, headers, body):
        self.method = method
        self.path = path
        self.query = query  # {name: [values]}
        self.headers = headers  # lowercase names
        self.body = body  # bytes

    def json(self):
        return json.loads(self.body or b"null")

    def form(self):
        return {k: v[0] for k, v in parse_qs(self.body.decode()).items()}

    def arg(self, name, default=None):
        return self.query.get(name, [default])[0]


class Response:
    def __init__(self, status=200, body=b"", headers=None, content_type="application/json"):
        if isinstance(body, (dict, list)):
            body = json.dumps(body, ensure_ascii=False).encode()
        elif isinstance(body, str):
            body = body.encode()
        self.status = status
        self.body = body
        self.headers = dict(headers or {})
        self.headers.setdefault("Content-Type", content_type)


class Router:
    def __init__(self):
        self.routes = []

    def add(self, method, pattern, handler):
        self.routes.append((method, re.compile(pattern + r"\Z"), handler))

    def route(self, request):
        for method, pattern, handler in self.routes:
            match = pattern.match(request.path)
            if match and method in (request.method, "*"):
                return handler(request, match)
        return Response(404, {"errorMessages": [f"no route for {request.method} {request.path}"]})


class State:
    """What the parts share: the requests seen, and each part's data (`state.data[part]`)."""

    def __init__(self):
        self.lock = threading.Lock()
        self.log = []
        self.data = {}
        self.resets = []

    def reset(self):
        for reset in self.resets:
            reset()


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
    router, state = Router(), State()
    sys.path.insert(0, __file__.rsplit("/", 1)[0])
    for part in PARTS:
        try:
            module = importlib.import_module(part)
        except ModuleNotFoundError:
            continue
        module.register(router, state)
    router.add("GET", r"/_mock/log", lambda request, match: Response(200, state.log))
    router.add("POST", r"/_mock/reset", lambda request, match: (state.reset(), Response(204))[1])

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def handle_any(self):
            url = urlsplit(self.path)
            length = int(self.headers.get("Content-Length") or 0)
            body = self.rfile.read(length) if length else b""
            request = Request(
                self.command,
                url.path,
                parse_qs(url.query),
                {k.lower(): v for k, v in self.headers.items()},
                body,
            )
            with state.lock:
                if not url.path.startswith("/_mock/"):
                    state.log.append({"method": self.command, "path": self.path,
                                      "body": body.decode(errors="replace")})
                response = router.route(request)
            self.send_response(response.status)
            for name, value in response.headers.items():
                self.send_header(name, value)
            self.send_header("Content-Length", str(len(response.body)))
            self.end_headers()
            self.wfile.write(response.body)

        do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = handle_any

        def do_PROPFIND(self):
            self.handle_any()

        def do_REPORT(self):
            self.handle_any()

        def log_message(self, *args):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    print(f"READY {server.server_address[1]}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
