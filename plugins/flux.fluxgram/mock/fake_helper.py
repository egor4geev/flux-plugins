#!/usr/bin/env python3
"""A fake fluxgram-helper for FluxGram's sign-in checks: the helper's JSON-lines protocol
(helpers/fluxgram-helper/README.md) with a TDLib that signs in without Telegram. Telegram's test
servers refuse TDLib's example credentials (PHONE_CODE_INVALID), so the flows past the code — the
password, success, Sign Out, a restart, a refusal — are checked with this. Python 3.9+, stdlib.

Run through Flux: a folder with an executable `fluxgram-helper` that runs this script, and
`FLUX_DEV_HELPERS=<that folder>`.

The rules, by the number:
- fewer than 7 digits — PHONE_NUMBER_INVALID; ending in 0000 — "Too Many Requests: retry after 300";
- the code goes as Telegram sends it to a new client: to Telegram on another device first, then
  (resendAuthenticationCode) by SMS, then by a call, then no other way — "Authentication code can't
  be resend"; each can be asked for after FAKE_CODE_TIMEOUT seconds (60 by default);
- the code 22222 is right, any other — PHONE_CODE_INVALID;
- a number ending in 1 has a cloud password "secret" (hint "the usual"), a wrong one —
  PASSWORD_HASH_INVALID; ending in 9 — no account (authorizationStateWaitRegistration);
- the QR code is confirmed by itself after FAKE_QR_CONFIRM seconds (unset — never); it is
  replaced every 30 s as TDLib does;
- a session is kept in `<database_directory>/fake-session.json` (with the database key: another
  key — "Wrong database encryption key"); a file `revoke` there ends it after the start, as a
  sign-out on another device does;
- `fluxgram.keep` and `fluxgram.drop` filter the updates as the helper does;
- FAKE_SLOW=<seconds> — the sign-in's requests (the number, the code, the password, a resend, the
  QR code) are answered that much later, as a slow network: for a look at the busy buttons.

The chats (part 3): a signed-in account sees the people, groups and messages of `fake_world.py`;
the requests this file doesn't answer go to the handlers `fake_chats.py` (the list) and
`fake_feed.py` (a chat) register there. Media (part 4): `fake_media.py` — the panel's stickers and
GIFs, @gif, opening a voice message, `fluxgram.render`.

FAKE_LOG=<path>: every request is appended there as it came (a scenario checks what was asked).
"""

from __future__ import annotations

import json
import os
import secrets
import struct
import sys
import threading
import time
import zlib

import fake_world
import fake_chats  # noqa: F401 — registers the list's handlers
import fake_feed  # noqa: F401 — registers a chat's handlers
import fake_search  # noqa: F401 — registers the search in a chat
import fake_profile  # noqa: F401 — registers the profiles: people, groups, members, muting
import fake_media  # registers the panel's stickers and GIFs, @gif, opening media; renders

LOCK = threading.Lock()
# The helper's filter: ("keep", types) — only these updates pass; ("drop", types) — all but these.
FILTER: tuple[str, set[str]] = ("drop", set())
# The helper's chat filter of each client (`fluxgram.chats`): only these chats' updates pass; a client
# without — every chat's. It goes with the client (`fluxgram.closed`).
CHAT_FILTERS: dict[str, set[int]] = {}
CLIENTS: dict[str, "Client"] = {}
# How a sign-in code comes: the first way, then each next one (resendAuthenticationCode).
CODE_WAYS = ["authenticationCodeTypeTelegramMessage", "authenticationCodeTypeSms", "authenticationCodeTypeCall"]
# The requests FAKE_SLOW delays.
SLOW = {"setAuthenticationPhoneNumber", "checkAuthenticationCode", "checkAuthenticationPassword",
        "resendAuthenticationCode", "requestQrCodeAuthentication"}


def update_chat(obj: dict) -> int | None:
    """The chat an update is about, as the helper reads it; none — an update of no chat."""
    if isinstance(obj.get("chat_id"), int):
        return obj["chat_id"]
    message = obj.get("message")
    if isinstance(message, dict) and isinstance(message.get("chat_id"), int):
        return message["chat_id"]
    if obj.get("@type") == "updateNewChat":
        return (obj.get("chat") or {}).get("id")
    if obj.get("@type") == "updateUserStatus":
        return obj.get("user_id")
    return None


def passes(obj: dict) -> bool:
    kind = obj.get("@type", "")
    if not kind.startswith("update"):
        return True
    if kind == "updateAuthorizationState" and obj["authorization_state"]["@type"] == "authorizationStateClosed":
        return True
    mode, types = FILTER
    if (kind in types) != (mode == "keep"):
        return False
    listed = CHAT_FILTERS.get(obj.get("@client", ""))
    chat = update_chat(obj)
    return listed is None or chat is None or chat in listed


def emit(obj: dict) -> None:
    if not passes(obj):
        return
    with LOCK:
        sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
        sys.stdout.flush()
    if obj.get("@type") == "fluxgram.closed":
        CHAT_FILTERS.pop(obj.get("@client", ""), None)


def png(path: str, color: tuple[int, int, int]) -> None:
    """A 64×64 picture of one color."""
    width = height = 64
    row = b"\x00" + bytes(color) * width
    raw = row * height

    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
    data += chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as file:
        file.write(data)


class Client:
    def __init__(self, key: str):
        self.key = key
        self.state = "authorizationStateWaitTdlibParameters"
        self.params: dict = {}
        self.phone = ""
        self.user: dict | None = None
        self.qr_timer: threading.Timer | None = None
        self.code_way = 0
        self.closed = False

    # --- what TDLib sends ---

    def send(self, obj: dict) -> None:
        emit(dict(obj, **{"@client": self.key}))

    def set_state(self, state: str, **fields) -> None:
        self.state = state
        self.send({"@type": "updateAuthorizationState", "authorization_state": dict({"@type": state}, **fields)})

    def answer(self, request: dict, obj: dict) -> None:
        if "@extra" in request:
            obj = dict(obj, **{"@extra": request["@extra"]})
        self.send(obj)

    def ok(self, request: dict) -> None:
        self.answer(request, {"@type": "ok"})

    def error(self, request: dict, code: int, message: str) -> None:
        self.answer(request, {"@type": "error", "code": code, "message": message})

    # --- the session on disk ---

    def session_file(self) -> str:
        return os.path.join(self.params["database_directory"], "fake-session.json")

    def ready(self) -> None:
        assert self.user is not None
        self.set_state("authorizationStateReady")
        # What a real client sends at the start; FluxGram's keep list holds most of it back.
        self.send({"@type": "updateOption", "name": "my_id", "value": {"@type": "optionValueInteger", "value": str(self.user["id"])}})
        self.send({"@type": "updateUserStatus", "user_id": 42, "status": {"@type": "userStatusOnline", "expires": 0}})
        self.send({"@type": "updateUser", "user": self.user})
        self.send({"@type": "updateConnectionState", "state": {"@type": "connectionStateReady"}})
        for hook in fake_world.ON_READY:
            hook(self)
        revoke = os.path.join(self.params["database_directory"], "revoke")
        if os.path.exists(revoke):
            threading.Timer(2.0, self.revoked).start()

    def revoked(self) -> None:
        try:
            os.remove(self.session_file())
        except OSError:
            pass
        self.set_state("authorizationStateLoggingOut")
        self.close_now()

    def close_now(self) -> None:
        if self.closed:
            return
        self.closed = True
        if self.qr_timer:
            self.qr_timer.cancel()
        self.set_state("authorizationStateClosing")
        self.set_state("authorizationStateClosed")
        emit({"@type": "fluxgram.closed", "@client": self.key})
        CLIENTS.pop(self.key, None)

    def make_user(self) -> dict:
        digits = "".join(c for c in self.phone if c.isdigit())
        user_id = int(digits[-9:] or "1") + 1000
        names = {"1": ("Anna", "Password"), "2": ("Egor", "Ageev")}
        first, last = names.get(digits[-1:], ("Test", f"User {digits[-4:]}"))
        return {
            "@type": "user",
            "id": user_id,
            "first_name": first,
            "last_name": last,
            "usernames": {"@type": "usernames", "active_usernames": [f"{first.lower()}{digits[-2:]}"]},
            "phone_number": digits,
            "accent_color_id": user_id % 7,
            "profile_photo": {"@type": "profilePhoto", "id": "1", "small": self.file(user_id, False)},
        }

    def file(self, file_id: int, done: bool) -> dict:
        path = os.path.join(self.params.get("files_directory", "/tmp"), "profile_photos", f"{file_id}.png")
        return {
            "@type": "file",
            "id": file_id,
            "size": 1000,
            "local": {"@type": "localFile", "path": path if done else "", "is_downloading_completed": done},
        }

    def qr(self) -> None:
        """A new login link; replaced in 30 s, confirmed by itself if FAKE_QR_CONFIRM says so."""
        if self.state not in ("authorizationStateWaitPhoneNumber", "authorizationStateWaitOtherDeviceConfirmation"):
            return
        self.set_state("authorizationStateWaitOtherDeviceConfirmation", link=f"tg://login?token={secrets.token_urlsafe(24)}")
        self.qr_timer = threading.Timer(30.0, self.qr)
        self.qr_timer.daemon = True
        self.qr_timer.start()

    def qr_confirmed(self) -> None:
        if self.state != "authorizationStateWaitOtherDeviceConfirmation":
            return
        if self.qr_timer:
            self.qr_timer.cancel()
        self.phone = "79005550102"
        self.signed_in()

    def signed_in(self) -> None:
        self.user = self.make_user()
        with open(self.session_file(), "w") as file:
            json.dump({"key": self.params.get("database_encryption_key"), "user": self.user}, file)
        self.ready()

    # --- requests ---

    def handle(self, request: dict) -> None:
        kind = request.get("@type")
        if kind == "getOption":
            self.answer(request, {"@type": "optionValueString", "value": "1.8.61"})
        elif kind == "setTdlibParameters":
            self.params = request
            os.makedirs(request["database_directory"], exist_ok=True)
            os.makedirs(request["files_directory"], exist_ok=True)
            session = None
            if os.path.exists(self.session_file()):
                with open(self.session_file()) as file:
                    session = json.load(file)
                if session.get("key") != request.get("database_encryption_key"):
                    return self.error(request, 401, "Wrong database encryption key")
            self.ok(request)
            if session:
                self.user = session["user"]
                self.ready()
            else:
                self.set_state("authorizationStateWaitPhoneNumber")
        elif kind == "requestQrCodeAuthentication":
            self.ok(request)
            self.qr()
            delay = os.environ.get("FAKE_QR_CONFIRM")
            if delay:
                threading.Timer(float(delay), self.qr_confirmed).start()
        elif kind == "setAuthenticationPhoneNumber":
            digits = "".join(c for c in request.get("phone_number", "") if c.isdigit())
            if len(digits) < 7:
                return self.error(request, 400, "PHONE_NUMBER_INVALID")
            if digits.endswith("0000"):
                return self.error(request, 429, "Too Many Requests: retry after 300")
            if self.qr_timer:
                self.qr_timer.cancel()
            self.phone = digits
            self.code_way = 0
            self.ok(request)
            self.code_state()
        elif kind == "resendAuthenticationCode":
            if self.state != "authorizationStateWaitCode":
                return self.error(request, 400, "resendAuthenticationCode unexpected")
            if self.code_way + 1 >= len(CODE_WAYS):
                return self.error(request, 400, "Authentication code can't be resend")
            self.code_way += 1
            self.ok(request)
            self.code_state()
        elif kind == "checkAuthenticationCode":
            if request.get("code") != "22222":
                return self.error(request, 400, "PHONE_CODE_INVALID")
            self.ok(request)
            if self.phone.endswith("1"):
                self.set_state("authorizationStateWaitPassword", password_hint="the usual",
                               has_recovery_email_address=False, has_passport_data=False,
                               recovery_email_address_pattern="")
            elif self.phone.endswith("9"):
                self.set_state("authorizationStateWaitRegistration",
                               terms_of_service={"@type": "termsOfService", "show_popup": False})
            else:
                self.signed_in()
        elif kind == "checkAuthenticationPassword":
            if request.get("password") != "secret":
                return self.error(request, 400, "PASSWORD_HASH_INVALID")
            self.ok(request)
            self.signed_in()
        elif kind == "getMe":
            if not self.user:
                return self.error(request, 401, "Unauthorized")
            self.answer(request, self.user)
        elif kind == "downloadFile" and "downloadFile" in fake_world.HANDLERS:
            # The chats' media: fake_feed downloads them (with progress), the photos too.
            fake_world.HANDLERS["downloadFile"](self, request)
        elif kind == "downloadFile":
            file_id = int(request.get("file_id", 0))
            done = self.file(file_id, True)
            png(done["local"]["path"], ((file_id * 37) % 200 + 40, 120, 200))
            self.answer(request, done)
            self.send({"@type": "updateFile", "file": done})
        elif kind == "logOut":
            self.ok(request)
            try:
                os.remove(self.session_file())
            except OSError:
                pass
            self.set_state("authorizationStateLoggingOut")
            self.close_now()
        elif kind == "close":
            self.ok(request)
            self.close_now()
        elif kind in fake_world.HANDLERS:
            fake_world.HANDLERS[kind](self, request)
        else:
            self.error(request, 400, f"The fake helper doesn't know {kind}")

    def code_state(self) -> None:
        """The code sent the current way; the next one after the timeout, while there is one."""
        kind = CODE_WAYS[self.code_way]
        next_kind = CODE_WAYS[self.code_way + 1] if self.code_way + 1 < len(CODE_WAYS) else None
        timeout = int(os.environ.get("FAKE_CODE_TIMEOUT", "60"))
        info = {
            "@type": "authenticationCodeInfo",
            "phone_number": self.phone,
            "type": {"@type": kind, "length": 5},
            "next_type": {"@type": next_kind, "length": 5} if next_kind else None,
            "timeout": timeout if next_kind else 0,
        }
        self.set_state("authorizationStateWaitCode", code_info=info)


def main() -> int:
    global FILTER
    sys.stderr.write("fluxgram-helper (fake) 0.1.0\n")
    sys.stderr.flush()
    emit({"@type": "fluxgram.ready", "version": "0.1.0-fake", "tdlib": "1.8.61"})
    for raw in sys.stdin:
        raw = raw.strip()
        if not raw:
            continue
        try:
            request = json.loads(raw)
        except json.JSONDecodeError:
            emit({"@type": "fluxgram.error", "message": "not JSON"})
            continue
        log = os.environ.get("FAKE_LOG")
        if log:
            with open(log, "a") as out:
                out.write(raw + "\n")
        kind = request.get("@type", "")
        if kind == "fluxgram.render":
            answer = fake_media.render(request)
            if "@extra" in request:
                answer["@extra"] = request["@extra"]
            emit(answer)
            continue
        if kind in ("fluxgram.keep", "fluxgram.drop"):
            FILTER = (kind.removeprefix("fluxgram."), set(request.get("types", [])))
            continue
        if kind == "fluxgram.chats":
            chats = request.get("chats")
            if chats is None:
                CHAT_FILTERS.pop(request.get("@client", ""), None)
            else:
                CHAT_FILTERS[request.get("@client", "")] = set(chats)
            continue
        if kind == "fluxgram.quit":
            break
        key = request.get("@client")
        if not isinstance(key, str):
            emit({"@type": "fluxgram.error", "message": "no @client"})
            continue
        client = CLIENTS.get(key)
        if client is None:
            client = CLIENTS[key] = Client(key)
            client.send({"@type": "updateAuthorizationState",
                         "authorization_state": {"@type": "authorizationStateWaitTdlibParameters"}})
        request.pop("@client", None)
        slow = float(os.environ.get("FAKE_SLOW", "0"))
        if slow and kind in SLOW:
            threading.Timer(slow, client.handle, [request]).start()
        else:
            client.handle(request)
    for client in list(CLIENTS.values()):
        client.close_now()
    return 0


if __name__ == "__main__":
    sys.exit(main())
