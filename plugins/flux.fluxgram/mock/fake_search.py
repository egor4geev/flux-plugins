"""The fake helper's search in a chat (coordinator): `searchChatMessages` over `fake_world.MESSAGES`
— by a query (case-insensitive, in texts and captions) and/or a filter (`searchMessagesFilter…`:
photos and videos, documents, links, voice notes…), newest first, paged by `from_message_id`
and `limit`, with `total_count`; and `getChatMessageCount` by a filter.
"""

from __future__ import annotations

import fake_world
from fake_world import handler

# A filter → the content kinds it takes.
FILTERS = {
    "searchMessagesFilterEmpty": None,
    "searchMessagesFilterPhotoAndVideo": {"messagePhoto", "messageVideo"},
    "searchMessagesFilterPhoto": {"messagePhoto"},
    "searchMessagesFilterVideo": {"messageVideo"},
    "searchMessagesFilterAnimation": {"messageAnimation"},
    "searchMessagesFilterDocument": {"messageDocument"},
    "searchMessagesFilterAudio": {"messageAudio"},
    "searchMessagesFilterVoiceNote": {"messageVoiceNote"},
    "searchMessagesFilterVideoNote": {"messageVideoNote"},
    "searchMessagesFilterVoiceAndVideoNote": {"messageVoiceNote", "messageVideoNote"},
    "searchMessagesFilterUrl": None,  # messages with a link: checked by the text below
}


def text_of(message: dict) -> str:
    content = message.get("content") or {}
    if content.get("@type") == "messageText":
        return (content.get("text") or {}).get("text", "")
    return (content.get("caption") or {}).get("text", "")


def matches(message: dict, query: str, kind: str) -> bool:
    content = message.get("content") or {}
    if kind == "searchMessagesFilterPinned":
        return bool(message.get("is_pinned"))
    kinds = FILTERS.get(kind)
    if kinds is not None and content.get("@type") not in kinds:
        return False
    if kind == "searchMessagesFilterUrl":
        entities = ((content.get("text") or content.get("caption") or {}).get("entities")) or []
        has_link = any((e.get("type") or {}).get("@type") in ("textEntityTypeUrl", "textEntityTypeTextUrl")
                       for e in entities) or "http" in text_of(message) or content.get("link_preview")
        if not has_link:
            return False
    return not query or query.lower() in text_of(message).lower()


def found(chat_id: int, query: str, kind: str) -> list[dict]:
    with fake_world.LOCK:
        history = list(fake_world.MESSAGES.get(chat_id) or [])
    return [m for m in reversed(history) if matches(m, query, kind)]


@handler("searchChatMessages")
def search_chat_messages(client, request: dict) -> None:
    chat_id = int(request.get("chat_id", 0))
    kind = ((request.get("filter") or {}).get("@type")) or "searchMessagesFilterEmpty"
    hits = found(chat_id, request.get("query") or "", kind)
    start = int(request.get("from_message_id") or 0)
    if start:
        hits_after = [m for m in hits if m["id"] < start]
    else:
        hits_after = hits
    limit = max(1, min(int(request.get("limit") or 50), 100))
    page = hits_after[:limit]
    client.answer(request, {"@type": "foundChatMessages", "total_count": len(hits),
                            "messages": page,
                            "next_from_message_id": page[-1]["id"] if len(hits_after) > limit else 0})


@handler("getChatMessageCount")
def get_chat_message_count(client, request: dict) -> None:
    chat_id = int(request.get("chat_id", 0))
    kind = ((request.get("filter") or {}).get("@type")) or "searchMessagesFilterEmpty"
    client.answer(request, {"@type": "count", "count": len(found(chat_id, "", kind))})
