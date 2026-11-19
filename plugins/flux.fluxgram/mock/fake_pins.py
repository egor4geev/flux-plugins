"""The fake helper's pinned messages and the chat's menus (part 5): "Backend · team" has three
pinned messages (one a photo; the user is an ordinary member there — they can't unpin them), Anna's
chat one. `pinChatMessage` / `unpinChatMessage` answer with `updateMessageIsPinned` (and the
"pinned «…»" line of the chat for a pin, as Telegram adds it); `searchChatMessages` with
`searchMessagesFilterPinned` is `fake_search.py`'s. The chat's menus: `toggleChatIsMarkedAsUnread`,
`readAllChatMentions`, `readAllChatReactions` (`updateChatIsMarkedAsUnread`), `getMessageLink`,
`addFavoriteSticker` / `removeFavoriteSticker`, `addSavedAnimation`.
"""

from __future__ import annotations

import fake_feed
import fake_world
from fake_world import NOW, handler

ANNA = 101


def pin_some() -> None:
    """The pinned messages of the fake world: three in "Backend · team", one in Anna's chat."""
    backend = fake_world.MESSAGES.get(fake_feed.BACKEND) or []
    texts = {"Do we keep the old API for the mobile app until spring?",
             "Config — I'll check today: env is read before the file",
             "Reminder: the standup is at 11."}
    for obj in backend:
        content = obj.get("content") or {}
        line = (content.get("text") or content.get("caption") or {}).get("text", "")
        if line in texts or line == "The new map screen — good?":
            obj["is_pinned"] = True
    for obj in fake_world.MESSAGES.get(ANNA) or []:
        caption = ((obj.get("content") or {}).get("caption") or {}).get("text", "")
        if caption == "The mockups, first take":
            obj["is_pinned"] = True


pin_some()


def click_only_full_sizes() -> None:
    """The second album of "Design review" (the teal and the green photo): their full sizes download
    only on a click — the profile's media opens the viewer on the small picture with a spinner, then
    swaps to the full one."""
    for obj in fake_world.MESSAGES.get(fake_feed.DESIGN) or []:
        content = obj.get("content") or {}
        if obj.get("media_album_id") != "13800000000000078" or content.get("@type") != "messagePhoto":
            continue
        full = content["photo"]["sizes"][-1]["photo"]
        fake_feed.FILES[full["id"]]["click_only"] = True


click_only_full_sizes()


def set_pinned(client, request: dict, pinned: bool) -> None:
    chat_id = int(request.get("chat_id", 0))
    message_id = int(request.get("message_id", 0))
    obj = fake_feed.find(client, chat_id, message_id)
    if obj is None:
        return client.error(request, 400, "Message not found")
    obj["is_pinned"] = pinned
    client.ok(request)
    client.send({"@type": "updateMessageIsPinned", "chat_id": chat_id, "message_id": message_id,
                 "is_pinned": pinned})
    if pinned and not request.get("only_for_self"):
        line = fake_world.message(chat_id, 0, {"@type": "messagePinMessage", "message_id": message_id},
                                  NOW, outgoing=True)
        fake_world.add(chat_id, line)
        client.send({"@type": "updateNewMessage", "message": fake_feed.served(client, line)})


@handler("pinChatMessage")
def pin_chat_message(client, request: dict) -> None:
    set_pinned(client, request, True)


@handler("unpinChatMessage")
def unpin_chat_message(client, request: dict) -> None:
    set_pinned(client, request, False)


@handler("toggleChatIsMarkedAsUnread")
def toggle_chat_is_marked_as_unread(client, request: dict) -> None:
    chat_id = int(request.get("chat_id", 0))
    marked = bool(request.get("is_marked_as_unread"))
    with fake_world.LOCK:
        if chat_id in fake_world.CHATS:
            fake_world.CHATS[chat_id] = dict(fake_world.CHATS[chat_id], is_marked_as_unread=marked)
    client.ok(request)
    client.send({"@type": "updateChatIsMarkedAsUnread", "chat_id": chat_id, "is_marked_as_unread": marked})


@handler("readAllChatMentions")
def read_all_chat_mentions(client, request: dict) -> None:
    client.ok(request)


@handler("readAllChatReactions")
def read_all_chat_reactions(client, request: dict) -> None:
    client.ok(request)


@handler("getMessageLink")
def get_message_link(client, request: dict) -> None:
    chat_id = int(request.get("chat_id", 0))
    message_id = int(request.get("message_id", 0))
    if chat_id > 0 or chat_id > -1000000000000:
        return client.error(request, 400, "Public message links are available only for messages in supergroups and channel chats")
    group = -1000000000000 - chat_id
    client.answer(request, {"@type": "messageLink", "link": f"https://t.me/c/{group}/{message_id >> 20}",
                            "is_public": False})


def favorite(client, request: dict, keep: bool) -> None:
    """The sticker (by its file) to the favorites or out of them: `updateFavoriteStickers`."""
    import fake_media
    file_id = int((request.get("sticker") or {}).get("id") or 0)
    found = fake_media.by_file(file_id) if hasattr(fake_media, "by_file") else None
    with fake_world.LOCK:
        fake_media.FAVORITES[:] = [s for s in fake_media.FAVORITES if s["sticker"]["id"] != file_id]
        if keep:
            sticker = found or next((m["content"]["sticker"] for history in fake_world.MESSAGES.values()
                                     for m in history if m["content"].get("@type") == "messageSticker"
                                     and m["content"]["sticker"]["sticker"]["id"] == file_id), None)
            if sticker:
                fake_media.FAVORITES.insert(0, sticker)
        ids = [s["sticker"]["id"] for s in fake_media.FAVORITES]
    client.ok(request)
    client.send({"@type": "updateFavoriteStickers", "sticker_ids": ids})


@handler("addFavoriteSticker")
def add_favorite_sticker(client, request: dict) -> None:
    favorite(client, request, True)


@handler("removeFavoriteSticker")
def remove_favorite_sticker(client, request: dict) -> None:
    favorite(client, request, False)


@handler("addSavedAnimation")
def add_saved_animation(client, request: dict) -> None:
    client.ok(request)
