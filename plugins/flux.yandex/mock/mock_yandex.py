#!/usr/bin/env python3
"""A fake Yandex for the plugin's tests and Flux scenarios: Yandex ID's OAuth and Tracker's API v3
(tasks, boards) on http://127.0.0.1:<port>, stdlib only.

usage: mock_yandex.py [port]   (8765 by default; the first line printed is "READY <port>")

Flux sends the plugin's requests here with
FLUX_PLUGIN_HTTP_OVERRIDE="https://oauth.yandex.ru=http://127.0.0.1:<port>,https://login.yandex.ru=http://127.0.0.1:<port>,https://api.tracker.yandex.net=http://127.0.0.1:<port>".

The parts register their routes: `oauth` and `tracker` (agent F), `boards` (agent F2) — modules
next to this one with `register(router, state)`. A route is a method, a regex
over the path (without the query) and a handler `(request, match) -> Response`. `GET /_mock/log`
— the requests so far (JSON: method, path, body, `t` — seconds since the start at arrival, `wall`
— the Unix time then, `ms` — the time to answer, `status`, `bytes` — the answer's body length);
`DELETE /_mock/log` (or `POST /_mock/log/clear`) — empties it; `POST /_mock/reset` — the data and
the controls below back to the start.

Controls closer to the real Tracker (`GET /_mock/controls` shows them):
- `POST /_mock/latency {"ms": N, "searchMs": M}` — every answer waits N ms (`_search` M ms, N when
  not given), outside the server's lock: concurrent requests overlap;
- `POST /_mock/strict {"search": true}` — a `POST /v3/issues/_search` that comes while another one
  is in flight answers 429 (Tracker allows one search at a time per user);
- `POST /_mock/throttle {"count": N, "retryAfter": S, "path": "<regex>"}` — the next N requests (that
  the regex finds in the path with its query, when given) answer 429
  `{"errorMessages": ["Too many requests"]}`, with `Retry-After: S` when given.
"""
import importlib
import json
import re
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

PARTS = ["oauth", "tracker", "boards"]


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


SEARCH = re.compile(r"/v3/issues/_search/?\Z")


class State:
    """What the parts share: the requests seen, and each part's data (`state.data[part]`)."""

    def __init__(self):
        self.lock = threading.Lock()
        self.started = time.monotonic()
        self.log = []
        self.data = {}
        self.resets = [self.reset_controls]

    def reset_controls(self):
        self.latency = {"ms": 0, "searchMs": None}
        self.strict = {"search": False}
        self.throttle = {"count": 0, "retryAfter": None, "path": None}
        self.searching = 0

    def reset(self):
        for reset in self.resets:
            reset()

    def controls(self):
        return {"latency": self.latency, "strict": self.strict, "throttle": self.throttle,
                "searching": self.searching}


def too_many(retry_after=None):
    headers = {"Retry-After": str(retry_after)} if retry_after is not None else {}
    return Response(429, {"errorMessages": ["Too many requests"], "statusCode": 429},
                    headers=headers)


def register_controls(router, state):
    def clear_log(request, match):
        state.log.clear()
        return Response(204)

    def latency(request, match):
        body = request.json() or {}
        state.latency = {"ms": max(0, int(body.get("ms") or 0)),
                         "searchMs": None if body.get("searchMs") is None
                         else max(0, int(body["searchMs"]))}
        return Response(200, state.controls())

    def strict(request, match):
        body = request.json() or {}
        state.strict = {"search": bool(body.get("search"))}
        return Response(200, state.controls())

    def throttle(request, match):
        body = request.json() or {}
        path = body.get("path")
        if path:
            try:
                re.compile(path)
            except re.error as err:
                return Response(400, {"errorMessages": [f"bad path regex: {err}"]})
        state.throttle = {"count": max(0, int(body.get("count") or 0)),
                          "retryAfter": body.get("retryAfter"), "path": path or None}
        return Response(200, state.controls())

    router.add("GET", r"/_mock/log", lambda request, match: Response(200, state.log))
    router.add("DELETE", r"/_mock/log", clear_log)
    router.add("POST", r"/_mock/log/clear", clear_log)
    router.add("POST", r"/_mock/reset", lambda request, match: (state.reset(), Response(204))[1])
    router.add("GET", r"/_mock/controls", lambda request, match: Response(200, state.controls()))
    router.add("POST", r"/_mock/latency", latency)
    router.add("POST", r"/_mock/strict", strict)
    router.add("POST", r"/_mock/throttle", throttle)


def admit(state, method, path, full_path):
    """Before a request is answered (under the lock): a 429 of the throttle or of the strict
    search, or None; and whether it is a search in flight now."""
    throttle = state.throttle
    if throttle["count"] > 0 and (not throttle["path"]
                                  or re.search(throttle["path"], full_path)):
        throttle["count"] -= 1
        return too_many(throttle["retryAfter"]), False
    if method == "POST" and SEARCH.match(path):
        if state.strict["search"] and state.searching > 0:
            return too_many(), False
        state.searching += 1
        return None, True
    return None, False


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
    router, state = Router(), State()
    state.reset_controls()
    sys.path.insert(0, __file__.rsplit("/", 1)[0])
    for part in PARTS:
        try:
            module = importlib.import_module(part)
        except ModuleNotFoundError:
            continue
        module.register(router, state)
    register_controls(router, state)

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
            if url.path.startswith("/_mock/"):
                with state.lock:
                    response = router.route(request)
                self.answer(response)
                return
            arrived = time.monotonic()
            entry = {"method": self.command, "path": self.path,
                     "body": body.decode(errors="replace"),
                     "t": round(arrived - state.started, 3), "wall": round(time.time(), 3),
                     "status": None, "bytes": None}
            searching = False
            try:
                with state.lock:
                    state.log.append(entry)
                    response, searching = admit(state, self.command, url.path, self.path)
                    latency = state.latency
                # The latency outside the lock: concurrent requests overlap, as on the network.
                # A 429 of the controls comes at once (the base latency); a search takes longer.
                wait = latency["ms"]
                if searching and latency["searchMs"] is not None:
                    wait = latency["searchMs"]
                if wait:
                    time.sleep(wait / 1000)
                if response is None:
                    with state.lock:
                        response = router.route(request)
                with state.lock:
                    entry["status"] = response.status
                    entry["bytes"] = len(response.body)
                    entry["ms"] = round((time.monotonic() - arrived) * 1000)
                self.answer(response)
            finally:
                if searching:
                    with state.lock:
                        state.searching -= 1

        def answer(self, response):
            self.send_response(response.status)
            for name, value in response.headers.items():
                self.send_header(name, value)
            self.send_header("Content-Length", str(len(response.body)))
            self.end_headers()
            self.wfile.write(response.body)

        do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = handle_any

        # WebDAV's methods (an older plugin's CalDAV calls): answered 404 and logged.
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
