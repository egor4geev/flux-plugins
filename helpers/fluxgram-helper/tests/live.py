#!/usr/bin/env python3
"""A live test of fluxgram-helper on Telegram's TEST servers (`use_test_dc`), never the real ones.

It drives the built program the way the plugin does and checks the protocol on the way:

1. `fluxgram.ready`, `getOption version`, `fluxgram.execute`, `fluxgram.drop`, a bad line;
2. account 1 signs in by phone and code (a test number `99966XYYYY`, the code is X repeated to the code's length;
   a new number is registered);
3. a second client signs in to it by a QR code, confirmed by the first one;
4. account 1 gets a 2FA password, and a third client signs in by phone, code and password, then
   logs out (`fluxgram.closed`);
5. account 2 signs in; `fluxgram.quit` closes every client (exit 0 within 5 s);
6. the program starts again with both accounts (their databases are kept): how the start's burst
   is batched (the program's `--stats`), then CPU and memory over a minute of idling;
7. both accounts log out; the end of input ends the program.

The credentials are TDLib's own example ones by default, for the test servers only; the test
servers may refuse to sign in with them (2026-11-17: PHONE_CODE_INVALID on DCs 1–3) — then steps
2–5 are skipped, the error is reported, and step 6 measures two clients that are connected but not
signed in. `--api-id`/`--api-hash` take an application's own (still on the test servers). Needs
Python 3.9+, no packages; the network.

Usage:
    python3 tests/live.py [--helper target/release/fluxgram-helper] [--dc 2] [--idle 60]
                          [--api-id N --api-hash H]
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import random
import re
import secrets
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_HELPER = HERE.parent / "target" / "release" / "fluxgram-helper"
BASE = Path("/tmp/fg2/helper-test")
# TDLib's example credentials (td/example): the test servers only.
API_ID = 94575
API_HASH = "a3406de8d171bb422bb6ddf3bbd800e2"
CREDENTIALS = {"api_id": API_ID, "api_hash": API_HASH}
PASSWORD = "flux-live-test"
NETWORK = 90  # seconds for a step that goes to Telegram
# A client connected to the test servers but not signed in (TDLib keeps a requested code a while).
UNSIGNED = ("authorizationStateWaitPhoneNumber", "authorizationStateWaitCode")


class Failure(Exception):
    pass


class Helper:
    """The program, its output read on a thread: answers by `@extra`, each client's last
    authorization state, `fluxgram.closed`, and every write's size from `--stats`."""

    def __init__(self, path: Path, log: Path):
        self.process = subprocess.Popen(
            [str(path), "--stats", "--log", str(log), "--verbosity", "2"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        assert self.process.stdin and self.process.stdout and self.process.stderr
        self.stdin, self.stdout, self.stderr_pipe = self.process.stdin, self.process.stdout, self.process.stderr
        self.cond = threading.Condition()
        self.lines: list[dict] = []
        self.answers: dict[int, dict] = {}
        self.auth: dict[str, dict] = {}
        self.closed: list[str] = []
        self.writes: list[tuple[float, int, int]] = []  # (time, lines, bytes)
        self.stderr: list[str] = []
        self.extra = 0
        threading.Thread(target=self._read_out, daemon=True).start()
        threading.Thread(target=self._read_err, daemon=True).start()

    def _read_out(self):
        for raw in self.stdout:
            line = json.loads(raw)
            with self.cond:
                self.lines.append(line)
                if "@extra" in line and isinstance(line["@extra"], int):
                    self.answers[line["@extra"]] = line
                if line.get("@type") == "updateAuthorizationState":
                    self.auth[line["@client"]] = line["authorization_state"]
                if line.get("@type") == "fluxgram.closed":
                    self.closed.append(line["@client"])
                self.cond.notify_all()
        with self.cond:
            self.cond.notify_all()

    def _read_err(self):
        for raw in self.stderr_pipe:
            text = raw.decode(errors="replace").rstrip()
            match = re.match(r"fluxgram-helper: a write of (\d+) lines, (\d+) bytes", text)
            with self.cond:
                if match:
                    self.writes.append((time.monotonic(), int(match[1]), int(match[2])))
                else:
                    self.stderr.append(text)

    def send(self, obj: dict | str):
        line = obj if isinstance(obj, str) else json.dumps(obj)
        self.stdin.write(line.encode() + b"\n")
        self.stdin.flush()

    def wait(self, check, timeout: float, what: str):
        deadline = time.monotonic() + timeout
        with self.cond:
            while True:
                result = check()
                if result:
                    return result
                left = deadline - time.monotonic()
                if self.process.poll() is not None:
                    raise Failure(f"the program ended ({self.process.returncode}) waiting for {what}")
                if left <= 0:
                    raise Failure(f"no {what} in {timeout} s")
                self.cond.wait(min(left, 0.5))

    def request(self, client: str | None, obj: dict, timeout: float = NETWORK) -> dict:
        """A request and its answer; TDLib's error raises."""
        with self.cond:
            self.extra += 1
            extra = self.extra
        obj = dict(obj, **{"@extra": extra})
        if client is not None:
            obj["@client"] = client
        self.send(obj)
        answer = self.wait(lambda: self.answers.get(extra), timeout, f"answer to {obj['@type']}")
        if answer.get("@type") == "error":
            raise Failure(f"{obj['@type']} ({client}): error {answer.get('code')} {answer.get('message')}")
        if client is not None and answer.get("@client") != client:
            raise Failure(f"the answer to {obj['@type']} came for {answer.get('@client')}, not {client}")
        return answer

    def state(self, client: str, states: tuple[str, ...], timeout: float = NETWORK) -> dict:
        def reached():
            state = self.auth.get(client)
            return state if state and state["@type"] in states else None

        return self.wait(reached, timeout, f"{client}: {' or '.join(states)}")

    def end(self, quit: bool, timeout: float = 10) -> tuple[int, float]:
        """`fluxgram.quit` or the end of input: the exit code and how long it took."""
        start = time.monotonic()
        if quit:
            self.send({"@type": "fluxgram.quit"})
        else:
            self.stdin.close()
        code = self.process.wait(timeout)
        return code, time.monotonic() - start


def step(text: str):
    print(f"[{time.strftime('%H:%M:%S')}] {text}", flush=True)


def parameters(name: str, key: str) -> dict:
    """setTdlibParameters of an account: its folders under BASE, the test servers."""
    folder = BASE / name
    return {
        "@type": "setTdlibParameters",
        "use_test_dc": True,
        "database_directory": str(folder / "db"),
        "files_directory": str(folder / "files"),
        "database_encryption_key": key,
        "use_file_database": True,
        "use_chat_info_database": True,
        "use_message_database": True,
        "use_secret_chats": False,
        "api_id": CREDENTIALS["api_id"],
        "api_hash": CREDENTIALS["api_hash"],
        "system_language_code": "en",
        "device_model": "Flux live test",
        "system_version": "",
        "application_version": "0.1.0",
    }


def account_key() -> str:
    """The database's encryption key, as the plugin keeps one in Flux's secrets."""
    return base64.b64encode(secrets.token_bytes(32)).decode()


def start_client(helper: Helper, client: str, name: str, key: str):
    helper.request(client, {"@type": "getOption", "name": "version"}, timeout=10)
    helper.state(client, ("authorizationStateWaitTdlibParameters",), timeout=10)
    helper.request(client, parameters(name, key))


def sign_in_by_phone(helper: Helper, client: str, phone: str, password: str | None = None):
    helper.state(client, ("authorizationStateWaitPhoneNumber",))
    helper.request(client, {"@type": "setAuthenticationPhoneNumber", "phone_number": phone})
    state = helper.state(client, ("authorizationStateWaitCode",))
    # A test number's code is its DC's digit, as many times as the code is long.
    code_type = state["code_info"]["type"]
    code = phone[5] * int(code_type.get("length", 5))
    step(f"   {client}: code {code} ({code_type['@type']})")
    helper.request(client, {"@type": "checkAuthenticationCode", "code": code})
    state = helper.state(
        client,
        ("authorizationStateReady", "authorizationStateWaitRegistration", "authorizationStateWaitPassword"),
    )
    if state["@type"] == "authorizationStateWaitRegistration":
        step(f"{client}: a new number — registering the test user")
        helper.request(client, {"@type": "registerUser", "first_name": "Flux", "last_name": f"Test {phone[-4:]}"})
    elif state["@type"] == "authorizationStateWaitPassword":
        if password is None:
            raise Failure(f"{client}: the account asks for a password")
        helper.request(client, {"@type": "checkAuthenticationPassword", "password": password})
    helper.state(client, ("authorizationStateReady",))


def cpu_seconds(pid: int) -> tuple[float, int]:
    """The process's CPU time so far and its RSS in KB (`ps`)."""
    out = subprocess.run(["ps", "-o", "time=,rss=", "-p", str(pid)], capture_output=True, text=True).stdout.split()
    parts = [float(part) for part in out[0].split(":")]
    seconds = 0.0
    for part in parts:
        seconds = seconds * 60 + part
    return seconds, int(out[1])


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--helper", type=Path, default=DEFAULT_HELPER)
    parser.add_argument("--dc", type=int, default=2, choices=(1, 2, 3))
    parser.add_argument("--idle", type=int, default=60, help="seconds of idling to measure")
    parser.add_argument("--api-id", type=int, default=API_ID)
    parser.add_argument("--api-hash", default=API_HASH)
    args = parser.parse_args()
    CREDENTIALS.update(api_id=args.api_id, api_hash=args.api_hash)

    if BASE.exists():
        shutil.rmtree(BASE)
    BASE.mkdir(parents=True)
    phones = [f"99966{args.dc}{random.randint(0, 9999):04d}" for _ in range(2)]
    while phones[1] == phones[0]:
        phones[1] = f"99966{args.dc}{random.randint(0, 9999):04d}"
    keys = {"one": account_key(), "two": account_key()}
    results: dict = {"phones": phones}

    helper = Helper(args.helper, BASE / "tdlib-1.log")
    try:
        step("1. ready, version, execute, drop, a bad line")
        first = helper.wait(lambda: helper.lines[:1], 10, "the first line")[0]
        if first.get("@type") != "fluxgram.ready":
            raise Failure(f"the first line isn't fluxgram.ready: {first}")
        step(f"   {json.dumps(first)}")
        version = helper.request("a", {"@type": "getOption", "name": "version"}, timeout=10)
        step(f"   getOption version → {version['value']}")
        executed = helper.request(None, {"@type": "fluxgram.execute", "query": {"@type": "getTextEntities", "text": "see https://telegram.org"}}, timeout=10)
        assert executed["@type"] == "fluxgram.executed" and executed["result"]["@type"] == "textEntities", executed
        helper.send({"@type": "fluxgram.drop", "types": ["updateUserStatus", "updateChatAction"]})
        error = helper.request(None, {"@type": "getMe"}, timeout=10)
        assert error["@type"] == "fluxgram.error", error

        signed_in = True
        try:
            step(f"2. account one: {phones[0]} (test DC {args.dc})")
            helper.state("a", ("authorizationStateWaitTdlibParameters",), timeout=10)
            helper.request("a", parameters("one", keys["one"]))
            sign_in_by_phone(helper, "a", phones[0])
            me = helper.request("a", {"@type": "getMe"})
            step(f"   a: ready — user {me['id']} {me['first_name']} {me.get('last_name', '')}")

            step("3. client b signs in to account one by a QR code, a confirms")
            start_client(helper, "b", "one-qr", account_key())
            helper.state("b", ("authorizationStateWaitPhoneNumber",))
            helper.request("b", {"@type": "requestQrCodeAuthentication", "other_user_ids": []})
            state = helper.state("b", ("authorizationStateWaitOtherDeviceConfirmation",))
            step(f"   b: link {state['link'][:24]}…")
            session = helper.request("a", {"@type": "confirmQrCodeAuthentication", "link": state["link"]})
            step(f"   a confirmed: session {session.get('@type')} {session.get('device_model', '')}")
            helper.state("b", ("authorizationStateReady",))
            step("   b: ready")

            step("4. a sets a 2FA password; client c signs in by phone, code and password, then logs out")
            helper.request("a", {"@type": "setPassword", "old_password": "", "new_password": PASSWORD, "new_hint": "flux", "set_recovery_email_address": False, "new_recovery_email_address": ""})
            start_client(helper, "c", "one-password", account_key())
            sign_in_by_phone(helper, "c", phones[0], PASSWORD)
            step("   c: ready (password checked)")
            helper.request("c", {"@type": "logOut"})
            helper.state("c", ("authorizationStateClosed",))
            helper.wait(lambda: "c" in helper.closed, 30, "fluxgram.closed for c")
            step("   c: logged out → fluxgram.closed")
            # The key is free: a new request makes a new client.
            helper.request("c", {"@type": "getOption", "name": "version"}, timeout=10)
            helper.state("c", ("authorizationStateWaitTdlibParameters",), timeout=10)
            step("   c: a new client after fluxgram.closed")

            step(f"5. account two: {phones[1]}; b closes; fluxgram.quit")
            helper.request("d", {"@type": "getOption", "name": "version"}, timeout=10)
            helper.request("d", parameters("two", keys["two"]))
            sign_in_by_phone(helper, "d", phones[1])
            step("   d: ready")
            helper.request("b", {"@type": "close"})
            helper.wait(lambda: "b" in helper.closed, 30, "fluxgram.closed for b")
        except Failure as failure:
            # The test servers refused the credentials: reported, and the rest goes on with two
            # clients that are connected but not signed in.
            signed_in = False
            results["sign_in_refused"] = str(failure)
            step(f"   SIGN-IN REFUSED: {failure}")
            for client, name in (("a", "one"), ("d", "two")):
                if client not in helper.auth:
                    helper.request(client, {"@type": "getOption", "name": "version"}, timeout=10)
                state = helper.state(client, ("authorizationStateWaitTdlibParameters", *UNSIGNED), timeout=30)
                if state["@type"] == "authorizationStateWaitTdlibParameters":
                    helper.request(client, parameters(name, keys[name]))
            for client in ("a", "d"):
                helper.state(client, UNSIGNED)
        live_clients = {key for key, state in helper.auth.items() if state["@type"] != "authorizationStateClosed"}
        exit_code, took = helper.end(quit=True)
        closed = sorted(set(helper.closed))
        step(f"   quit: exit {exit_code} in {took:.2f} s; closed: {closed}")
        if exit_code != 0 or not live_clients <= set(closed):
            raise Failure(f"quit: exit {exit_code}, closed {closed}")
        results["quit"] = {"exit": exit_code, "seconds": round(took, 2)}
    finally:
        if helper.process.poll() is None:
            helper.process.kill()
        if helper.stderr:
            step("   the program's stderr: " + " | ".join(helper.stderr[-5:]))

    waiting = ("authorizationStateReady",) if signed_in else UNSIGNED
    step(f"6. again with both accounts ({'signed in' if signed_in else 'NOT signed in'}): the start's batches, then idle")
    helper = Helper(args.helper, BASE / "tdlib-2.log")
    try:
        helper.wait(lambda: helper.lines[:1], 10, "fluxgram.ready")
        helper.send({"@type": "fluxgram.drop", "types": ["updateUserStatus", "updateChatAction"]})
        start = time.monotonic()
        for client, name in (("a", "one"), ("d", "two")):
            helper.request(client, parameters(name, keys[name]), timeout=30)
        for client in ("a", "d"):
            helper.state(client, waiting)
        ready_at = time.monotonic() - start
        time.sleep(5)
        with helper.cond:
            burst = [write for write in helper.writes if write[0] >= start]
            lines = sum(write[1] for write in burst)
        sizes = sorted(write[1] for write in burst)
        results["start"] = {
            "signed_in": signed_in,
            "ready_seconds": round(ready_at, 2),
            "writes": len(burst),
            "lines": lines,
            "bytes": sum(write[2] for write in burst),
            "lines_per_write_max": sizes[-1] if sizes else 0,
            "lines_per_write_median": sizes[len(sizes) // 2] if sizes else 0,
            "single_line_writes": sum(1 for size in sizes if size == 1),
        }
        step(f"   start: {json.dumps(results['start'])}")

        pid = helper.process.pid
        step(f"   idling {args.idle} s…")
        time.sleep(5)
        cpu0, rss0 = cpu_seconds(pid)
        writes0 = len(helper.writes)
        time.sleep(args.idle)
        cpu1, rss1 = cpu_seconds(pid)
        results["idle"] = {
            "seconds": args.idle,
            "cpu_percent": round((cpu1 - cpu0) / args.idle * 100, 3),
            "cpu_seconds": round(cpu1 - cpu0, 3),
            "rss_mb": round(rss1 / 1024, 1),
            "writes": len(helper.writes) - writes0,
        }
        step(f"   idle: {json.dumps(results['idle'])}")

        step("7. both accounts log out (signed in) or close; the end of input")
        if signed_in:
            for client in ("a", "d"):
                helper.request(client, {"@type": "logOut"})
            helper.wait(lambda: {"a", "d"} <= set(helper.closed), 30, "fluxgram.closed for a and d")
        exit_code, took = helper.end(quit=False)
        if not {"a", "d"} <= set(helper.closed):
            raise Failure(f"end of input: closed only {helper.closed}")
        results["end"] = {"exit": exit_code, "seconds": round(took, 2)}
        step(f"   end of input: exit {exit_code} in {took:.2f} s")
        if exit_code != 0:
            raise Failure(f"end of input: exit {exit_code}")
    finally:
        if helper.process.poll() is None:
            helper.process.kill()
        if helper.stderr:
            step("   the program's stderr: " + " | ".join(helper.stderr[-5:]))

    print(json.dumps(results, indent=2))
    return 0 if signed_in else 3


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Failure as failure:
        print(f"FAILED: {failure}", file=sys.stderr)
        sys.exit(1)
