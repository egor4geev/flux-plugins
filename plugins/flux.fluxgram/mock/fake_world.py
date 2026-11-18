"""The fake helper's Telegram (part 3): the people, the chats and their messages a signed-in fake
account sees, as TDLib's objects, and the registry the chats' fakes plug into.

- `fake_chats.py` (the list) answers the chat list's requests (`loadChats`, `getChats`, `getChat`,
  `searchChats`…) and plays the list's events (new messages coming, typing, muting);
- `fake_feed.py` (a chat) answers a chat's requests (`openChat`, `getChatHistory`, `sendMessage`,
  `viewMessages`, `downloadFile` of media…) and fills the chats' histories with every kind of
  message.

Both register with `handler(kind)` and `on_ready`; this module holds what they share. Ids follow
TDLib: a person's chat has the person's id, a basic group's is `-id`, a supergroup's
`-1000000000000 - id`, a secret chat's `-2000000000000 + id`. Python 3.9+, stdlib.
"""

from __future__ import annotations

import os
import struct
import threading
import time
import zlib
from typing import Callable

# --- The registry ---

# TDLib's request kind → handler(client, request): the fake helper calls it for a kind it doesn't
# know itself.
HANDLERS: dict[str, Callable] = {}
# handler(client): when a client is ready (signed in, or started again with its session).
ON_READY: list[Callable] = []
LOCK = threading.RLock()


def handler(kind: str):
    """A decorator: `@handler("getChat")` answers that request."""

    def register(function: Callable) -> Callable:
        HANDLERS[kind] = function
        return function

    return register


def on_ready(function: Callable) -> Callable:
    ON_READY.append(function)
    return function


# --- Files ---

def png(path: str, color: tuple[int, int, int], width: int = 64, height: int = 64) -> None:
    """A picture of one color with a lighter diagonal band (so a scaled one is visibly a picture)."""
    rows = []
    for y in range(height):
        row = bytearray(b"\x00")
        for x in range(width):
            band = abs((x * height // max(width, 1)) - y) < max(2, height // 12)
            pixel = tuple(min(255, c + 70) for c in color) if band else color
            row += bytes(pixel)
        rows.append(bytes(row))
    raw = b"".join(rows)

    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
    data += chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
    if os.path.dirname(path):
        os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as file:
        file.write(data)


def file_object(file_id: int, path: str = "", size: int = 20_000, downloaded: int | None = None,
                downloading: bool = False) -> dict:
    """TDLib's `file`: downloaded when `path` is given."""
    done = bool(path)
    return {
        "@type": "file",
        "id": file_id,
        "size": size,
        "expected_size": size,
        "local": {
            "@type": "localFile",
            "path": path,
            "can_be_downloaded": True,
            "is_downloading_active": downloading,
            "is_downloading_completed": done,
            "downloaded_size": size if done else (downloaded or 0),
            "downloaded_prefix_size": size if done else (downloaded or 0),
        },
        "remote": {"@type": "remoteFile", "id": f"remote-{file_id}", "unique_id": f"u{file_id}",
                   "is_uploading_active": False, "is_uploading_completed": True, "uploaded_size": size},
    }


# File ids: a person's photo 1000 + id, a chat's photo 2000 + its index; media — fake_feed's.
def photo_of(file_id: int) -> dict:
    return {"@type": "profilePhoto", "id": str(file_id), "small": file_object(file_id),
            "big": file_object(file_id + 500_000), "has_animation": False, "is_personal": False}


def chat_photo_of(file_id: int) -> dict:
    return {"@type": "chatPhotoInfo", "small": file_object(file_id), "big": file_object(file_id + 500_000),
            "has_animation": False, "is_personal": False}


# --- People ---

def user(user_id: int, first: str, last: str = "", username: str | None = None, color: int = 0,
         photo: bool = True, kind: str = "userTypeRegular", status: dict | None = None,
         contact: bool = True) -> dict:
    user_type: dict = {"@type": kind}
    if kind == "userTypeBot":
        user_type.update({"can_join_groups": True, "can_read_all_group_messages": False})
    return {
        "@type": "user",
        "id": user_id,
        "first_name": first,
        "last_name": last,
        "usernames": {"@type": "usernames", "active_usernames": [username] if username else [],
                      "disabled_usernames": [], "editable_username": username or ""},
        "phone_number": "",
        "status": status or {"@type": "userStatusRecently"},
        "profile_photo": photo_of(1000 + user_id) if photo else None,
        "accent_color_id": color,
        "is_contact": contact,
        "is_mutual_contact": contact,
        "type": user_type,
    }


NOW = int(time.time())
USERS: dict[int, dict] = {u["id"]: u for u in [
    user(101, "Anna", "Smirnova", "anna", 6, status={"@type": "userStatusOnline", "expires": NOW + 3600}),
    user(102, "Dima", "Kim", "dimakim", 4, status={"@type": "userStatusOffline", "was_online": NOW - 1800}),
    user(103, "Petya", "Smirnov", None, 0, photo=False),
    user(104, "Oleg", "", "oleg", 1, photo=False, status={"@type": "userStatusLastWeek"}),
    user(105, "Weather", "Bot", "weather_bot", 2, kind="userTypeBot", contact=False),
    user(106, "", "", None, 3, photo=False, kind="userTypeDeleted", contact=False),
    user(107, "Masha", "Volkova", "masha", 5),
]}

# --- Groups ---

BASIC_GROUPS: dict[int, dict] = {
    201: {"@type": "basicGroup", "id": 201, "member_count": 3,
          "status": {"@type": "chatMemberStatusMember"}, "is_active": True, "upgraded_to_supergroup_id": 0},
}


def supergroup(group_id: int, members: int, channel: bool = False, username: str | None = None,
               status: str = "chatMemberStatusMember") -> dict:
    return {"@type": "supergroup", "id": group_id, "member_count": members, "is_channel": channel,
            "is_forum": False, "date": NOW - 86400 * 300,
            "usernames": {"@type": "usernames", "active_usernames": [username] if username else []},
            "status": {"@type": status}}


SUPERGROUPS: dict[int, dict] = {g["id"]: g for g in [
    supergroup(1001, 5),
    supergroup(1002, 12000, channel=True, username="fluxnews"),
    supergroup(1003, 240),
    supergroup(1004, 30, status="chatMemberStatusLeft"),
]}

# --- Chats ---

PERMISSIONS = {"@type": "chatPermissions", "can_send_basic_messages": True, "can_send_audios": True,
               "can_send_documents": True, "can_send_photos": True, "can_send_videos": True,
               "can_send_video_notes": True, "can_send_voice_notes": True, "can_send_polls": True,
               "can_send_other_messages": True, "can_add_link_previews": True,
               "can_change_info": False, "can_invite_users": True, "can_pin_messages": False,
               "can_create_topics": False}


def chat(chat_id: int, chat_type: dict, title: str, color: int, photo: int | None, order: int,
         pinned: bool = False, muted: bool = False, unread: int = 0) -> dict:
    return {
        "@type": "chat",
        "id": chat_id,
        "type": chat_type,
        "title": title,
        "photo": chat_photo_of(photo) if photo else None,
        "accent_color_id": color,
        "permissions": PERMISSIONS,
        "last_message": None,
        "positions": [{"@type": "chatPosition", "list": {"@type": "chatListMain"}, "order": str(order),
                       "is_pinned": pinned, "source": None}],
        "unread_count": unread,
        "unread_mention_count": 0,
        "last_read_inbox_message_id": 0,
        "last_read_outbox_message_id": 0,
        "is_marked_as_unread": False,
        "notification_settings": {"@type": "chatNotificationSettings", "use_default_mute_for": not muted,
                                  "mute_for": 2147483647 if muted else 0},
        "draft_message": None,
    }


def private(user_id: int) -> dict:
    return {"@type": "chatTypePrivate", "user_id": user_id}


# The chats every fake account has, newest first by `order`. The signed-in user's Saved Messages
# (their own id) is added per account (`chats_of`).
CHATS: dict[int, dict] = {c["id"]: c for c in [
    chat(-1000000001001, {"@type": "chatTypeSupergroup", "supergroup_id": 1001, "is_channel": False},
         "Backend · team", 5, 2201, 900, pinned=True, unread=3),
    chat(101, private(101), "Anna Smirnova", 6, 1101, 890, unread=1),
    chat(-201, {"@type": "chatTypeBasicGroup", "basic_group_id": 201}, "Design review", 3, 2202, 880, unread=12),
    chat(102, private(102), "Dima Kim", 4, 1102, 870),
    chat(104, private(104), "Oleg", 1, None, 860, muted=True),
    chat(-1000000001003, {"@type": "chatTypeSupergroup", "supergroup_id": 1003, "is_channel": False},
         "Flood", 2, None, 850, muted=True, unread=57),
    chat(-1000000001002, {"@type": "chatTypeSupergroup", "supergroup_id": 1002, "is_channel": True},
         "Flux News", 0, 2203, 840),
    chat(105, private(105), "Weather Bot", 2, 1105, 830),
    chat(-1999999999999, {"@type": "chatTypeSecret", "secret_chat_id": 1, "user_id": 103},
         "Petya Smirnov", 0, None, 820),
    chat(103, private(103), "Petya Smirnov", 0, None, 810),
    chat(106, private(106), "Deleted Account", 3, None, 800),
    chat(107, private(107), "Masha Volkova", 5, 1107, 790),
    chat(-1000000001004, {"@type": "chatTypeSupergroup", "supergroup_id": 1004, "is_channel": False},
         "Old project", 1, None, 780),
]}

# Messages by chat, oldest first: fake_feed fills them (every kind of message); until it does, a
# few lines of text each so the list has its last lines.
MESSAGES: dict[int, list[dict]] = {}
# The next message id of each chat (TDLib's server ids step by 1 << 20).
STEP = 1 << 20


def message(chat_id: int, sender: int, content: dict, date: int, message_id: int | None = None,
            outgoing: bool = False, me: int = 0, **extra) -> dict:
    """TDLib's `message` of `chat_id` by the user `sender` (a chat id when < 0: a channel's post)."""
    sender_id = ({"@type": "messageSenderChat", "chat_id": sender} if sender < 0
                 else {"@type": "messageSenderUser", "user_id": sender})
    obj = {
        "@type": "message",
        "id": message_id or next_id(chat_id),
        "chat_id": chat_id,
        "sender_id": sender_id,
        "is_outgoing": outgoing,
        "is_pinned": False,
        "can_be_edited": outgoing,
        "can_be_deleted_only_for_self": not outgoing,
        "can_be_deleted_for_all_users": outgoing,
        "date": date,
        "edit_date": 0,
        "forward_info": None,
        "interaction_info": None,
        "reply_to": None,
        "content": content,
    }
    obj.update(extra)
    return obj


def text(value: str, entities: list | None = None) -> dict:
    return {"@type": "messageText", "text": {"@type": "formattedText", "text": value, "entities": entities or []},
            "link_preview": None}


def next_id(chat_id: int) -> int:
    history = MESSAGES.get(chat_id) or []
    last = max((m["id"] for m in history), default=0)
    return last + STEP


def add(chat_id: int, obj: dict) -> dict:
    with LOCK:
        MESSAGES.setdefault(chat_id, []).append(obj)
    return obj


def seed_texts() -> None:
    """A few lines in every chat: the default history, when fake_feed doesn't fill one."""
    lines = {
        -1000000001001: [(103, "Release on Friday? Or Thursday?")],
        101: [(101, "Sent you the mockups, take a look when you can")],
        -201: [(101, "Photo of the new screen"), (107, "Looks great")],
        102: [(0, "See you at 11 tomorrow?")],
        104: [(104, "👍")],
        -1000000001003: [(102, "lol")],
        -1000000001002: [(-1000000001002, "Flux 0.5 is out")],
        105: [(105, "Moscow: +3°, cloudy")],
        103: [(103, "hey")],
        106: [(106, "…")],
        107: [(107, "Thanks!")],
        -1000000001004: [(104, "Archived")],
    }
    for chat_id, texts in lines.items():
        if MESSAGES.get(chat_id):
            continue
        for index, (sender, value) in enumerate(texts):
            date = NOW - 3600 * (len(texts) - index) - abs(chat_id) % 3600
            add(chat_id, message(chat_id, sender, text(value), date, outgoing=sender == 0))


def chats_of(me: dict) -> dict[int, dict]:
    """The account's chats: the shared ones and its Saved Messages."""
    saved = chat(me["id"], private(me["id"]), "Saved Messages", me.get("accent_color_id", 0) % 7, None, 700)
    return {**CHATS, me["id"]: saved}


def with_last_message(chat_obj: dict, me: dict) -> dict:
    """The chat as `getChat` gives it: its last message and its read marks."""
    obj = dict(chat_obj)
    history = MESSAGES.get(obj["id"]) or []
    if history:
        last = dict(history[-1])
        if last["sender_id"].get("user_id") == 0:
            last["sender_id"] = {"@type": "messageSenderUser", "user_id": me["id"]}
        obj["last_message"] = last
        unread = obj.get("unread_count", 0)
        incoming = [m for m in history if not m.get("is_outgoing")]
        if unread and incoming:
            obj["last_read_inbox_message_id"] = incoming[max(0, len(incoming) - unread - 1)]["id"]
        else:
            obj["last_read_inbox_message_id"] = history[-1]["id"]
        obj["last_read_outbox_message_id"] = history[-1]["id"]
    return obj
