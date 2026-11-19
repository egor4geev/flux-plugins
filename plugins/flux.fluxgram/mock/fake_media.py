"""The fake helper's media (part 4): the account's stickers and GIFs for the field's panel, the
inline bot @gif, opening a voice or round message, sending stickers and GIFs, and the helper's
own `fluxgram.render`.

- `getInstalledStickerSets` — six sets (their covers: the first sticker of each), `getStickerSet`,
  `getRecentStickers`, `getFavoriteStickers`, `getStickers` (by emoji), `searchStickers` (by word:
  a sticker's emoji name), `searchInstalledStickerSets` (by a set's title);
- `getSavedAnimations` — two GIFs; `searchPublicChat("gif")` — the bot's chat;
  `getInlineQueryResults` — pages of ten results, three pages in all (`next_offset` "10", "20", "");
- `openMessageContent` — marks a voice or round message listened to and sends
  `updateMessageContentOpened`;
- sending: `inputMessageSticker` and `inputMessageAnimation` (by the remote file's id) become the
  sticker and the GIF they name; `sendInlineQueryResultMessage` sends the result's GIF;
- `fluxgram.render` — a pre-rendered FXA of mock/media (`sticker-tgs-144.fxa`,
  `sticker-webm-144.fxa`, whatever the size asked, told apart by the input's first bytes) copied to the output, else
  `fluxgram.error`, as the helper says a file it can't render.
"""

from __future__ import annotations

import os
import shutil

import fake_feed
import fake_world
from fake_feed import BLUE, GREEN, TEAL, VIOLET, WARM, animation_object, sticker_object
from fake_world import handler

# The sets: (id, title, [(emoji, color, animated)]).
SETS = [
    ("9001", "Flux Cats", [("🐱", VIOLET, "tgs"), ("😺", WARM, False), ("😹", BLUE, False), ("🙀", GREEN, False)]),
    ("9002", "Flux Dogs", [("🐶", WARM, False), ("🐕", TEAL, "webm"), ("🦴", BLUE, False)]),
    ("9003", "Office", [("☕", WARM, False), ("💻", BLUE, False), ("📎", GREEN, False)]),
    ("9004", "Weather", [("☀", WARM, False), ("🌧", BLUE, False)]),
    ("9005", "Food", [("🍕", WARM, False), ("🍣", TEAL, False), ("🍩", VIOLET, False)]),
    ("9006", "Late set", [("🌙", VIOLET, False), ("⭐", WARM, False)]),
]
STICKERS: dict[str, list[dict]] = {
    set_id: [sticker_object(emoji, color, animated, set_id) for emoji, color, animated in items]
    for set_id, _, items in SETS
}
TITLES = {set_id: title for set_id, title, _ in SETS}
NAMES = {"🐱": "cat", "😺": "cat smile", "😹": "cat laugh", "🙀": "cat scared", "🐶": "dog", "🐕": "dog",
         "🦴": "bone", "☕": "coffee", "💻": "laptop", "📎": "clip", "☀": "sun", "🌧": "rain", "🍕": "pizza",
         "🍣": "sushi", "🍩": "donut", "🌙": "moon", "⭐": "star"}
RECENT = [STICKERS["9002"][0], STICKERS["9001"][1]]
FAVORITES = [STICKERS["9001"][0]]
SAVED = [animation_object("violet"), animation_object("teal")]
# @gif: its user (and private chat) id; a page of results.
GIF_BOT = 140_001
PAGE = 10
PAGES = 3
_results: dict[str, dict] = {}


def every_sticker() -> list[dict]:
    return [sticker for stickers in STICKERS.values() for sticker in stickers]


def by_remote(remote: str) -> dict | None:
    """A sticker or a GIF by its file's remote id."""
    for sticker in every_sticker():
        if sticker["sticker"]["remote"]["id"] == remote:
            return {"@type": "messageSticker", "sticker": sticker, "is_premium": False}
    for found in SAVED + [result["animation"] for result in _results.values()]:
        if found["animation"]["remote"]["id"] == remote:
            return {"@type": "messageAnimation", "caption": fake_feed.styled(""), "animation": found,
                    "show_caption_above_media": False, "has_spoiler": False, "is_secret": False}
    return None


def content_of(content: dict) -> dict | None:
    """The message's content of an `inputMessageSticker` / `inputMessageAnimation`; none — not one."""
    kind = content.get("@type")
    if kind == "inputMessageSticker":
        remote = (content.get("sticker") or {}).get("id", "")
    elif kind == "inputMessageAnimation":
        remote = (content.get("animation") or {}).get("id", "")
    else:
        return None
    return by_remote(remote) or fake_feed.text("(unknown file)")


def set_info(set_id: str) -> dict:
    return {"@type": "stickerSetInfo", "id": set_id, "title": TITLES[set_id], "name": f"set{set_id}",
            "thumbnail": None, "is_installed": True, "is_archived": False, "is_official": False,
            "sticker_type": {"@type": "stickerTypeRegular"}, "size": len(STICKERS[set_id]),
            "covers": STICKERS[set_id][:1]}


@handler("getInstalledStickerSets")
def installed(client, request: dict) -> None:
    client.answer(request, {"@type": "stickerSets", "total_count": len(SETS),
                            "sets": [set_info(set_id) for set_id, _, _ in SETS]})


@handler("getStickerSet")
def sticker_set(client, request: dict) -> None:
    set_id = str(request.get("set_id"))
    if set_id not in STICKERS:
        return client.error(request, 400, "STICKERSET_INVALID")
    client.answer(request, dict(set_info(set_id), **{"@type": "stickerSet", "stickers": STICKERS[set_id],
                                                     "emojis": []}))


@handler("getRecentStickers")
def recent(client, request: dict) -> None:
    client.answer(request, {"@type": "stickers", "stickers": RECENT})


@handler("getFavoriteStickers")
def favorites(client, request: dict) -> None:
    client.answer(request, {"@type": "stickers", "stickers": FAVORITES})


@handler("getStickers")
def stickers_by_emoji(client, request: dict) -> None:
    query = request.get("query", "")
    client.answer(request, {"@type": "stickers",
                            "stickers": [s for s in every_sticker() if query and s["emoji"] in query]})


@handler("searchStickers")
def search_stickers(client, request: dict) -> None:
    query = (request.get("query") or "").lower()
    found = [s for s in every_sticker() if query and query in NAMES.get(s["emoji"], "")]
    client.answer(request, {"@type": "stickers", "stickers": found})


@handler("searchInstalledStickerSets")
def search_sets(client, request: dict) -> None:
    query = (request.get("query") or "").lower()
    sets = [set_info(set_id) for set_id, title, _ in SETS if query and query in title.lower()]
    client.answer(request, {"@type": "stickerSets", "total_count": len(sets), "sets": sets})


@handler("getSavedAnimations")
def saved(client, request: dict) -> None:
    client.answer(request, {"@type": "animations", "animations": SAVED})


@handler("searchPublicChat")
def search_public_chat(client, request: dict) -> None:
    if (request.get("username") or "").lower() != "gif":
        return client.error(request, 400, "USERNAME_NOT_OCCUPIED")
    client.answer(request, {"@type": "chat", "id": GIF_BOT, "title": "GIF Search",
                            "type": {"@type": "chatTypePrivate", "user_id": GIF_BOT}})


@handler("getInlineQueryResults")
def inline_results(client, request: dict) -> None:
    if int(request.get("bot_user_id") or 0) != GIF_BOT:
        return client.error(request, 400, "BOT_INVALID")
    query = request.get("query") or ""
    offset = int(request.get("offset") or 0)
    page = offset // PAGE
    results = []
    if query.strip() and page < PAGES:
        for index in range(offset, offset + PAGE):
            result_id = f"{query}-{index}"
            if result_id not in _results:
                _results[result_id] = {"@type": "inlineQueryResultAnimation", "id": result_id,
                                       "animation": animation_object(["violet", "teal", "warm"][index % 3]),
                                       "title": f"{query} {index + 1}"}
            results.append(_results[result_id])
    next_offset = str(offset + PAGE) if query.strip() and page + 1 < PAGES else ""
    client.answer(request, {"@type": "inlineQueryResults", "inline_query_id": str(700 + len(query)),
                            "button": None, "results": results, "next_offset": next_offset})


@handler("sendInlineQueryResultMessage")
def send_inline_result(client, request: dict) -> None:
    result = _results.get(request.get("result_id") or "")
    if result is None:
        return client.error(request, 400, "RESULT_ID_INVALID")
    content = {"@type": "inputMessageAnimation",
               "animation": {"@type": "inputFileRemote", "id": result["animation"]["animation"]["remote"]["id"]}}
    pending = fake_feed.send_one(client, request, int(request["chat_id"]), content)
    client.answer(request, pending)


@handler("openMessageContent")
def open_message_content(client, request: dict) -> None:
    chat_id, message_id = int(request["chat_id"]), int(request["message_id"])
    client.ok(request)
    found = fake_feed.find(client, chat_id, message_id)
    if found is None:
        return
    content = found["content"]
    key = {"messageVoiceNote": "is_listened", "messageVideoNote": "is_viewed"}.get(content.get("@type"))
    if key and not content.get(key):
        content[key] = True
        client.send({"@type": "updateMessageContentOpened", "chat_id": chat_id, "message_id": message_id})


def render(request: dict) -> dict:
    """`fluxgram.render`: a pre-rendered FXA of mock/media copied to the output, else an error."""
    source = request.get("input") or ""
    output = request.get("output") or ""
    size = int(request.get("size") or 0)
    try:
        with open(source, "rb") as file:
            head = file.read(4)
    except OSError as err:
        return {"@type": "fluxgram.error", "message": f"Can't read the input: {err}"}
    kind = "tgs" if head[:2] == b"\x1f\x8b" else "webm" if head == b"\x1a\x45\xdf\xa3" else None
    if kind is None:
        return {"@type": "fluxgram.error", "message": "Not a TGS or WEBM sticker"}
    # One render per kind is kept (144 px — the files are large); Flux fits it to any size asked.
    ready = os.path.join(fake_feed.MEDIA, f"sticker-{kind}-144.fxa")
    if not os.path.exists(ready):
        return {"@type": "fluxgram.error", "message": f"No renderer in the fake helper (no {os.path.basename(ready)})"}
    os.makedirs(os.path.dirname(output), exist_ok=True)
    shutil.copyfile(ready, output + ".tmp")
    os.replace(output + ".tmp", output)
    return {"@type": "fluxgram.rendered", "output": output, "width": 144, "height": 144, "frames": 60}


def cache(request: dict) -> dict:
    """`fluxgram.cache` (part 5) as the helper does it: the folder's files (8 folders deep, links
    neither followed nor removed); `touch` — files of the folder set to now first; `limit` — the
    least recently used (by modification time) removed until the folder holds no more; `"limit":
    0, "clear": true` — every file removed; the answer is the size it holds then."""
    folder = request.get("dir") or ""
    limit = request.get("limit")
    clear = request.get("clear") is True
    if not os.path.isabs(folder) or ".." in folder.split(os.sep):
        return {"@type": "fluxgram.error", "message": "A cache without a whole folder"}
    if limit == 0 and not clear or clear and limit != 0:
        return {"@type": "fluxgram.error", "message": "`clear` goes with a `limit` of 0"}
    if not os.path.exists(folder):
        return {"@type": "fluxgram.cacheSize", "size": 0}
    if not os.path.isdir(folder):
        return {"@type": "fluxgram.error", "message": f"Not a folder: {folder}"}
    for path in request.get("touch") or []:
        if os.path.dirname(path).startswith(folder) and os.path.isfile(path) and not os.path.islink(path):
            os.utime(path)
    files = []
    for root, dirs, names in os.walk(folder):
        if root[len(folder):].count(os.sep) >= 8:
            dirs[:] = []
        for name in names:
            path = os.path.join(root, name)
            if os.path.islink(path) or not os.path.isfile(path):
                continue
            stat = os.stat(path)
            files.append((stat.st_mtime, path, stat.st_size))
    total = sum(size for _, _, size in files)
    if limit is not None:
        for _, path, size in sorted(files):
            if total <= limit:
                break
            try:
                os.remove(path)
                total -= size
            except OSError:
                pass
    return {"@type": "fluxgram.cacheSize", "size": total}


def storage_statistics(params: dict | None) -> dict:
    """`getStorageStatisticsFast`: the size of the client's files folder, plus FAKE_FILES_SIZE bytes
    (a cache of gigabytes for a check of the limit)."""
    size = int(os.environ.get("FAKE_FILES_SIZE", "0"))
    folder = (params or {}).get("files_directory")
    if folder and os.path.isdir(folder):
        for root, _, names in os.walk(folder):
            for name in names:
                path = os.path.join(root, name)
                if os.path.isfile(path) and not os.path.islink(path):
                    size += os.path.getsize(path)
    return {"@type": "storageStatisticsFast", "files_size": size, "file_count": 0, "database_size": 0,
            "language_pack_database_size": 0, "log_size": 0}

