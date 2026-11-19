"""The fake helper's links (part 5): Lena's chat (111, not in the main list — opened by its tab's
key) is a row of messages with every kind of link FluxGram follows, one per message, and the
requests that open them.

- Telegram's: a public channel the user isn't in (t.me/rustweekly), a public group the user
  isn't in and that isn't in the list ("Zig Meetup", t.me/zigmeetup), a bot's start
  (t.me/weather_bot?start=from_lena), a message of another chat (t.me/fluxnews/<id>), an
  invite ("Go · разработчики", 1 234 members — `joinChatByInviteLink` makes the user a member),
  a sticker set ("Flux Cats", 24 stickers), a proxy (not one FluxGram opens).
- The text's entities: @dimakim (a person), @fluxnews (a channel), a mention without a username
  (Petya), #release and $FLUX, /help@weather_bot, an e-mail, a phone number, a bank card's number.
- The web: a text link whose text isn't its address, a look-alike host ("аpple.com" with a
  Cyrillic "а"), a plain address, an ordinary Russian domain (пример.рф).

`getInternalLinkType` parses t.me and tg: links as TDLib does (the kinds above); any other link is
an error, as TDLib answers for a link that isn't Telegram's.
"""

from __future__ import annotations

import re
import time

import fake_world
from fake_world import NOW, add, handler, message

LENA = 111
ZIG = -1000000001007
GO = -1000000001008
NEWS = -1000000001002

fake_world.USERS[LENA] = fake_world.user(LENA, "Лена", "Орлова", "lena_orlova", 1, photo=False)
fake_world.CHATS[LENA] = fake_world.chat(LENA, fake_world.private(LENA), "Лена Орлова", 1, None, 0)
fake_world.SUPERGROUPS[1007] = fake_world.supergroup(1007, 412, username="zigmeetup",
                                                     status="chatMemberStatusLeft")
fake_world.CHATS[ZIG] = fake_world.chat(
    ZIG, {"@type": "chatTypeSupergroup", "supergroup_id": 1007, "is_channel": False},
    "Zig Meetup", 3, None, 0)


def utf16(value: str) -> int:
    return len(value.encode("utf-16-le")) // 2


def entity(value: str, piece: str, kind: dict) -> dict:
    start = value.index(piece)
    return {"@type": "textEntity", "offset": utf16(value[:start]), "length": utf16(piece), "type": kind}


def linked(value: str, *marks: tuple[str, dict]) -> dict:
    return {"@type": "messageText",
            "text": {"@type": "formattedText", "text": value,
                     "entities": [entity(value, piece, kind) for piece, kind in marks]},
            "link_preview": None}


def url(value: str, piece: str) -> dict:
    return linked(value, (piece, {"@type": "textEntityTypeUrl"}))


LINES = [
    url("Канал: https://t.me/rustweekly", "https://t.me/rustweekly"),
    url("Группа: t.me/zigmeetup", "t.me/zigmeetup"),
    url("Бот: https://t.me/weather_bot?start=from_lena", "https://t.me/weather_bot?start=from_lena"),
    url("Пост: https://t.me/fluxnews/3", "https://t.me/fluxnews/3"),
    url("Вступай: https://t.me/+GoDevsInvite", "https://t.me/+GoDevsInvite"),
    url("Стикеры: https://t.me/addstickers/FluxCats", "https://t.me/addstickers/FluxCats"),
    url("Прокси: https://t.me/proxy?server=1.2.3.4&port=443&secret=ee00", "https://t.me/proxy?server=1.2.3.4&port=443&secret=ee00"),
    linked("Спроси @dimakim или читай @fluxnews", ("@dimakim", {"@type": "textEntityTypeMention"}),
           ("@fluxnews", {"@type": "textEntityTypeMention"})),
    linked("Петя знает", ("Петя", {"@type": "textEntityTypeMentionName", "user_id": 103})),
    linked("Теги: #release и $FLUX", ("#release", {"@type": "textEntityTypeHashtag"}),
           ("$FLUX", {"@type": "textEntityTypeCashtag"})),
    linked("Команда: /help@weather_bot", ("/help@weather_bot", {"@type": "textEntityTypeBotCommand"})),
    linked("Почта: lena@example.com", ("lena@example.com", {"@type": "textEntityTypeEmailAddress"})),
    linked("Телефон: +7 900 123-45-67", ("+7 900 123-45-67", {"@type": "textEntityTypePhoneNumber"})),
    linked("Карта: 4242 4242 4242 4242", ("4242 4242 4242 4242", {"@type": "textEntityTypeBankCardNumber"})),
    linked("Читай Документацию тут", ("Документацию", {"@type": "textEntityTypeTextUrl",
                                                        "url": "https://evil.example/login"})),
    url("Вход: аpple.com/id", "аpple.com/id"),
    url("Сайт: https://example.com/docs", "https://example.com/docs"),
    url("Наш: пример.рф", "пример.рф"),
]


def fill_lena() -> None:
    fake_world.MESSAGES[LENA] = []
    t = NOW - 3_000
    for index, content in enumerate(LINES):
        add(LENA, message(LENA, LENA if index % 3 else 0, content, t + index * 60,
                          outgoing=index % 3 == 0))


fill_lena()


# --- TDLib's requests ---

_search_public_chat = fake_world.HANDLERS.get("searchPublicChat")


def public_chat(username: str) -> dict | None:
    """The chat of a username: a person's or a bot's private chat, a group's, a channel's."""
    name = username.lower()
    for user in fake_world.USERS.values():
        if name in [u.lower() for u in user["usernames"]["active_usernames"]]:
            return fake_world.CHATS.get(user["id"]) or fake_world.chat(
                user["id"], fake_world.private(user["id"]), f"{user['first_name']} {user['last_name']}".strip(),
                user["accent_color_id"], None, 0)
    for group in fake_world.SUPERGROUPS.values():
        if name in [u.lower() for u in group["usernames"]["active_usernames"]]:
            for chat_obj in fake_world.CHATS.values():
                if (chat_obj.get("type") or {}).get("supergroup_id") == group["id"]:
                    return chat_obj
    return None


@handler("searchPublicChat")
def search_public_chat(client, request: dict) -> None:
    found = public_chat(request.get("username") or "")
    if found is not None:
        return client.answer(request, found)
    if _search_public_chat is not None:
        return _search_public_chat(client, request)
    client.error(request, 400, "USERNAME_NOT_OCCUPIED")


@handler("getInternalLinkType")
def get_internal_link_type(client, request: dict) -> None:
    link = (request.get("link") or "").strip()
    match = re.match(r"^(?:https?://)?(?:www\.)?(?:t\.me|telegram\.me|telegram\.dog)/([^?#]*)\??([^#]*)", link,
                     re.IGNORECASE)
    tg = re.match(r"^tg://resolve\?domain=([\w]+)", link, re.IGNORECASE)
    if tg:
        return client.answer(request, {"@type": "internalLinkTypePublicChat", "chat_username": tg.group(1),
                                       "draft_text": "", "open_profile": False})
    if not match:
        return client.error(request, 404, "Not Found")
    path, query = match.group(1).strip("/"), match.group(2)
    params = dict(part.split("=", 1) for part in query.split("&") if "=" in part)
    parts = path.split("/")
    if path.startswith("+") or parts[0] == "joinchat":
        return client.answer(request, {"@type": "internalLinkTypeChatInvite", "invite_link": link})
    if parts[0] == "addstickers" and len(parts) > 1:
        return client.answer(request, {"@type": "internalLinkTypeStickerSet", "sticker_set_name": parts[1],
                                       "expect_custom_emoji": False})
    if parts[0] == "proxy":
        return client.answer(request, {"@type": "internalLinkTypeProxy", "server": params.get("server", ""),
                                       "port": int(params.get("port", 0)), "type": {"@type": "proxyTypeMtproto",
                                                                                  "secret": params.get("secret", "")}})
    if len(parts) == 2 and parts[1].isdigit():
        return client.answer(request, {"@type": "internalLinkTypeMessage", "url": link})
    if len(parts) == 1 and parts[0]:
        if "start" in params:
            return client.answer(request, {"@type": "internalLinkTypeBotStart", "bot_username": parts[0],
                                           "start_parameter": params["start"], "autostart": False})
        return client.answer(request, {"@type": "internalLinkTypePublicChat", "chat_username": parts[0],
                                       "draft_text": params.get("text", ""), "open_profile": "profile" in params})
    client.answer(request, {"@type": "internalLinkTypeUnknownDeepLink", "link": link})


@handler("getMessageLinkInfo")
def get_message_link_info(client, request: dict) -> None:
    match = re.search(r"t\.me/(\w+)/(\d+)", request.get("url") or "")
    chat_obj = public_chat(match.group(1)) if match else None
    history = fake_world.MESSAGES.get(chat_obj["id"], []) if chat_obj else []
    if match is None or chat_obj is None:
        return client.answer(request, {"@type": "messageLinkInfo", "is_public": True, "chat_id": 0,
                                       "message_thread_id": 0, "message": None, "media_timestamp": 0,
                                       "for_album": False})
    index = min(int(match.group(2)), len(history)) - 1
    found = history[index] if history and index >= 0 else None
    client.answer(request, {"@type": "messageLinkInfo", "is_public": True, "chat_id": chat_obj["id"],
                            "message_thread_id": 0, "message": found, "media_timestamp": 0, "for_album": False})


INVITE_TITLE = "Go · разработчики"


@handler("checkChatInviteLink")
def check_chat_invite_link(client, request: dict) -> None:
    if "GoDevs" not in (request.get("invite_link") or ""):
        return client.error(request, 400, "INVITE_HASH_EXPIRED")
    joined = GO in fake_world.CHATS
    client.answer(request, {"@type": "chatInviteLinkInfo", "chat_id": GO if joined else 0,
                            "accessible_for": 0, "type": {"@type": "inviteLinkChatTypeSupergroup"},
                            "title": INVITE_TITLE, "photo": None, "accent_color_id": 3, "description": "",
                            "member_count": 1234, "member_user_ids": [], "subscription_info": None,
                            "creates_join_request": False, "is_public": False, "verification_status": None})


@handler("joinChatByInviteLink")
def join_chat_by_invite_link(client, request: dict) -> None:
    if "GoDevs" not in (request.get("invite_link") or ""):
        return client.error(request, 400, "INVITE_HASH_EXPIRED")
    fake_world.SUPERGROUPS[1008] = fake_world.supergroup(1008, 1235, username=None)
    obj = fake_world.chat(GO, {"@type": "chatTypeSupergroup", "supergroup_id": 1008, "is_channel": False},
                          INVITE_TITLE, 3, None, 0)
    with fake_world.LOCK:
        fake_world.CHATS[GO] = obj
        fake_world.MESSAGES[GO] = []
        add(GO, message(GO, 102, fake_world.text("Добро пожаловать в Go · разработчики!"), int(time.time()) - 60))
    client.send({"@type": "updateSupergroup", "supergroup": fake_world.SUPERGROUPS[1008]})
    client.send({"@type": "updateNewChat", "chat": obj})
    client.answer(request, obj)


STICKER_SET = "7770001112223334445"


@handler("searchStickerSet")
def search_sticker_set(client, request: dict) -> None:
    if (request.get("name") or "").lower() != "fluxcats":
        return client.error(request, 400, "STICKERSET_INVALID")
    client.answer(request, {"@type": "stickerSet", "id": STICKER_SET, "title": "Flux Cats", "name": "FluxCats",
                            "thumbnail": None, "thumbnail_outline": None, "is_owned": False,
                            "is_installed": False, "is_archived": False, "is_official": False,
                            "sticker_type": {"@type": "stickerTypeRegular"}, "needs_repainting": False,
                            "is_allowed_as_chat_emoji_status": False, "is_viewed": True,
                            "stickers": [{"@type": "sticker", "id": str(index)} for index in range(24)],
                            "emojis": []})


@handler("changeStickerSet")
def change_sticker_set(client, request: dict) -> None:
    if str(request.get("set_id")) != STICKER_SET:
        return client.error(request, 400, "STICKERSET_INVALID")
    client.ok(request)
