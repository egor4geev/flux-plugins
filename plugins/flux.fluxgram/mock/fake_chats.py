"""The fake helper's chat list (part 3, the list's agent): TDLib's requests of the list —
`loadChats`, `getChats`, `getChat`, `searchChats`, `searchChatsOnServer`, `getUser`, `getBasicGroup`,
`getSupergroup` — over `fake_world`, and the list's events: the people, groups and default muting a
client learns as it starts, and new messages coming (`FAKE_INCOMING=<seconds>`); `fluxgram.chats`
filters them as the helper does (fake_helper.py).

- `FAKE_INCOMING=<seconds>` — every that many seconds a new text comes, in turn from Anna, the
  "Backend · team" group (Petya), "Design review" (Masha) and Oleg (muted): `updateNewMessage`,
  `updateChatLastMessage` (the chat moves up), `updateChatReadInbox` (one more unread).
- `FAKE_BURST=<count>` — with FAKE_INCOMING, each tick brings that many messages at once (in turn
  through the four chats), in one write as the helper's batch: a burst, as a catch-up after sleep.
- `searchChatsOnServer` answers FAKE_SERVER_DELAY seconds later (0.3 by default), as Telegram does:
  the popup's answer grows.
"""

from __future__ import annotations

import itertools
import json
import os
import sys
import threading
import time

import fake_world as world

# The chats new messages come to (FAKE_INCOMING), with who writes and what.
INCOMING = [
    (101, 101, ["Are you around?", "Take a look at the second screen", "Thanks!"]),
    (-1000000001001, 103, ["Deploy is green", "Who reviews #142?", "Release on Friday then"]),
    (-201, 107, ["New mockups are in the folder", "Looks good to me", "One more round?"]),
    (104, 104, ["👍", "ok", "see you"]),
]


def chats_of(client) -> dict[int, dict]:
    return world.chats_of(client.user)


def order_of(chat: dict) -> int:
    positions = chat.get("positions") or []
    return int(positions[0]["order"]) if positions else 0


def in_main_list(chat: dict) -> bool:
    """Chats a left group isn't among (TDLib keeps it out of the main list)."""
    kind = chat["type"]
    if kind["@type"] == "chatTypeSupergroup":
        group = world.SUPERGROUPS.get(kind["supergroup_id"], {})
        if group.get("status", {}).get("@type") == "chatMemberStatusLeft":
            return False
    return order_of(chat) > 0


def ordered(client) -> list[dict]:
    """The main list, newest first."""
    with world.LOCK:
        chats = [chat for chat in chats_of(client).values() if in_main_list(chat)]
    return sorted(chats, key=lambda chat: (-order_of(chat), chat["id"]))


def username(chat: dict) -> str:
    kind = chat["type"]
    if kind["@type"] == "chatTypePrivate":
        user = world.USERS.get(kind["user_id"], {})
        names = user.get("usernames", {}).get("active_usernames") or []
        return names[0] if names else ""
    if kind["@type"] == "chatTypeSupergroup":
        group = world.SUPERGROUPS.get(kind["supergroup_id"], {})
        names = group.get("usernames", {}).get("active_usernames") or []
        return names[0] if names else ""
    return ""


def found(client, request: dict) -> list[int]:
    query = (request.get("query") or "").strip().lower().lstrip("@")
    limit = int(request.get("limit") or 50)
    if not query:
        return []
    with world.LOCK:
        chats = list(chats_of(client).values())
    hits = [chat["id"] for chat in sorted(chats, key=lambda chat: -order_of(chat))
            if query in chat["title"].lower() or (username(chat) and query in username(chat).lower())]
    return hits[:limit]


@world.handler("loadChats")
def load_chats(client, request: dict) -> None:
    # Everything comes at the first call; the next ones find no more (TDLib's 404).
    if getattr(client, "chats_loaded", False):
        return client.error(request, 404, "Not Found")
    client.chats_loaded = True
    for chat in ordered(client):
        client.send({"@type": "updateNewChat", "chat": world.with_last_message(chat, client.user)})
    client.ok(request)


@world.handler("getChats")
def get_chats(client, request: dict) -> None:
    limit = int(request.get("limit") or 50)
    ids = [chat["id"] for chat in ordered(client)]
    client.answer(request, {"@type": "chats", "total_count": len(ids), "chat_ids": ids[:limit]})


@world.handler("getChat")
def get_chat(client, request: dict) -> None:
    with world.LOCK:
        chat = chats_of(client).get(int(request.get("chat_id", 0)))
        if chat is None:
            return client.error(request, 400, "Chat not found")
        obj = world.with_last_message(chat, client.user)
    client.answer(request, obj)


@world.handler("searchChats")
def search_chats(client, request: dict) -> None:
    ids = found(client, request)
    client.answer(request, {"@type": "chats", "total_count": len(ids), "chat_ids": ids})


@world.handler("searchChatsOnServer")
def search_chats_on_server(client, request: dict) -> None:
    ids = found(client, request)
    delay = float(os.environ.get("FAKE_SERVER_DELAY", "0.3"))
    answer = {"@type": "chats", "total_count": len(ids), "chat_ids": ids}
    threading.Timer(delay, client.answer, [request, answer]).start()


@world.handler("getUser")
def get_user(client, request: dict) -> None:
    user_id = int(request.get("user_id", 0))
    user = client.user if client.user and client.user["id"] == user_id else world.USERS.get(user_id)
    if user is None:
        return client.error(request, 404, "User not found")
    client.answer(request, user)


@world.handler("getBasicGroup")
def get_basic_group(client, request: dict) -> None:
    group = world.BASIC_GROUPS.get(int(request.get("basic_group_id", 0)))
    if group is None:
        return client.error(request, 400, "Group not found")
    client.answer(request, group)


@world.handler("getSupergroup")
def get_supergroup(client, request: dict) -> None:
    group = world.SUPERGROUPS.get(int(request.get("supergroup_id", 0)))
    if group is None:
        return client.error(request, 400, "Supergroup not found")
    client.answer(request, group)


@world.on_ready
def ready(client) -> None:
    """What a client learns as it starts: the people, the groups, the default muting."""
    for user in world.USERS.values():
        client.send({"@type": "updateUser", "user": user})
    for scope in ("notificationSettingsScopePrivateChats", "notificationSettingsScopeGroupChats"):
        client.send({"@type": "updateScopeNotificationSettings", "scope": {"@type": scope},
                     "notification_settings": {"@type": "scopeNotificationSettings", "mute_for": 0}})
    for group in world.BASIC_GROUPS.values():
        client.send({"@type": "updateBasicGroup", "basic_group": group})
    for group in world.SUPERGROUPS.values():
        client.send({"@type": "updateSupergroup", "supergroup": group})
    every = float(os.environ.get("FAKE_INCOMING", "0") or 0)
    if every > 0 and not getattr(client, "incoming", None):
        client.incoming = threading.Thread(target=incoming, args=(client, every), daemon=True)
        client.incoming.start()


def incoming(client, every: float) -> None:
    """New messages, every `every` seconds, while the client lives."""
    burst = max(1, int(os.environ.get("FAKE_BURST", "1") or 1))
    turns = itertools.cycle(range(len(INCOMING)))
    count = itertools.count()
    while not client.closed:
        time.sleep(every)
        updates: list[dict] = []
        for _ in range(burst):
            chat_id, sender, lines = INCOMING[next(turns)]
            updates += arrive(client, chat_id, sender, lines[next(count) % len(lines)])
        if not client.closed:
            send_batch(client, updates)


def send_batch(client, updates: list[dict]) -> None:
    """The updates in one write, as the helper writes what TDLib has ready at once (its filters
    applied: fake_helper.py, the running script)."""
    helper = sys.modules.get("__main__")
    if not hasattr(helper, "passes"):
        for update in updates:
            client.send(update)
        return
    lines = [json.dumps(dict(update, **{"@client": client.key}), ensure_ascii=False)
             for update in updates if helper.passes(dict(update, **{"@client": client.key}))]
    if lines:
        with helper.LOCK:
            sys.stdout.write("".join(line + "\n" for line in lines))
            sys.stdout.flush()


def arrive(client, chat_id: int, sender: int, value: str) -> list[dict]:
    """A text from `sender` in `chat_id`, as TDLib tells of it."""
    now = int(time.time())
    with world.LOCK:
        obj = world.add(chat_id, world.message(chat_id, sender, world.text(value), now))
        chat = world.CHATS[chat_id]
        chat["unread_count"] = chat.get("unread_count", 0) + 1
        unread = chat["unread_count"]
        positions = chat.get("positions") or []
        if positions and not positions[0].get("is_pinned"):
            # Up the list: the newest chat's order is the biggest.
            positions[0]["order"] = str(max(order_of(c) for c in world.CHATS.values()) + 1)
        history = world.MESSAGES.get(chat_id) or []
        incoming_ids = [m["id"] for m in history if not m.get("is_outgoing")]
        last_read = incoming_ids[-unread - 1] if len(incoming_ids) > unread else 0
    return [
        {"@type": "updateNewMessage", "message": obj},
        {"@type": "updateChatLastMessage", "chat_id": chat_id, "last_message": obj,
         "positions": positions},
        {"@type": "updateChatReadInbox", "chat_id": chat_id,
         "last_read_inbox_message_id": last_read, "unread_count": unread},
    ]
