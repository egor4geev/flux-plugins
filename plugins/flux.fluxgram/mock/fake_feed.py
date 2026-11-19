"""The fake helper's chats (part 3, the chat's agent): a chat's history of every kind of message
(filled into `fake_world.MESSAGES`), and TDLib's requests of an open chat — `openChat`,
`closeChat`, `getChatHistory`, `getMessage`, `getMessages`, `getRepliedMessage`, `viewMessages`,
`sendMessage`, `sendMessageAlbum`, `editMessageText`, `editMessageCaption`, `deleteMessages` (one
or several), `forwardMessages` (part 5: forwarded copies; "Old project" protects its content),
`sendChatAction`, `setChatDraftMessage`, `downloadFile` (every file: media, profile and chat
photos), `cancelDownloadFile`.

- The histories: "Backend · team" has every kind of message, over four days (part 6: 150 and more,
  for Claude's pages) (text with nested entities in Cyrillic
  and emoji — TDLib's offsets are UTF-16 —, code, quotes, spoilers, links, mentions; photos with
  minithumbnails, a sticker, a document, voice, round and other videos, a GIF, a poll, a location,
  a contact, forwards, replies — one to an old message —, reactions, an edit, the chat's own lines);
  "Flood" has 10 000 messages; the people's chats a few dozen each; Saved Messages is made when the
  account first opens it.
- `getChatHistory` follows TDLib: `from_message_id` 0 — the newest; the message itself is
  included; a negative `offset` adds newer ones; the first call of a chat gives only one message,
  as TDLib does before it has the history at hand.
- Sending: a pending message with a temporary id, then `updateMessageSendSucceeded` after 0.5 s (a
  file named with "FAIL" — `updateMessageSendFailed`); in a chat with someone online, they read it
  2 s later (`updateChatReadOutbox`).
- `FAKE_DOWNLOAD_SECONDS` (1 by default): how long a large file downloads, in `updateFile` steps;
  `FAKE_TYPING=<seconds>`: someone types in the open chats that often.
"""

from __future__ import annotations

import base64
import bisect
import math
import os
import random
import re
import shutil
import struct
import threading
import time
import zlib

import fake_world
from fake_world import NOW, add, file_object, handler, message, text

# --- Pictures ---

# Tiny JPEGs (made once with `sips` from fake_world.png): the minithumbnails TDLib carries in
# messages, base64 as in its JSON.
MINI = {
    "blue": "/9j/4AAQSkZJRgABAQAASABIAAD/4QBMRXhpZgAATU0AKgAAAAgAAYdpAAQAAAABAAAAGgAAAAAAA6ABAAMAAAABAAEAAKACAAQAAAABAAAAKKADAAQAAAABAAAAHgAAAAD/7QA4UGhvdG9zaG9wIDMuMAA4QklNBAQAAAAAAAA4QklNBCUAAAAAABDUHYzZjwCyBOmACZjs+EJ+/8AAEQgAHgAoAwEiAAIRAQMRAf/EAB8AAAEFAQEBAQEBAAAAAAAAAAABAgMEBQYHCAkKC//EALUQAAIBAwMCBAMFBQQEAAABfQECAwAEEQUSITFBBhNRYQcicRQygZGhCCNCscEVUtHwJDNicoIJChYXGBkaJSYnKCkqNDU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6g4SFhoeIiYqSk5SVlpeYmZqio6Slpqeoqaqys7S1tre4ubrCw8TFxsfIycrS09TV1tfY2drh4uPk5ebn6Onq8fLz9PX29/j5+v/EAB8BAAMBAQEBAQEBAQEAAAAAAAABAgMEBQYHCAkKC//EALURAAIBAgQEAwQHBQQEAAECdwABAgMRBAUhMQYSQVEHYXETIjKBCBRCkaGxwQkjM1LwFWJy0QoWJDThJfEXGBkaJicoKSo1Njc4OTpDREVGR0hJSlNUVVZXWFlaY2RlZmdoaWpzdHV2d3h5eoKDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uLj5OXm5+jp6vLz9PX29/j5+v/bAEMADAwMDAwMFAwMFB0UFBQdJx0dHR0nMScnJycnMTsxMTExMTE7Ozs7Ozs7O0dHR0dHR1NTU1NTXV1dXV1dXV1dXf/bAEMBDg8PGBYYKBYWKGFCNkJhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYf/dAAQAA//aAAwDAQACEQMRAD8AyFniuRsuuH6K4/rVWe3kt22yd+hHQ1DVuC6Ma+TIN8R6j0+leucZUoq5Najb51sd8ff1GPWqdABRRRQB/9DmKKKK9c4yWGaSBt0Zx6jsfrVwxw3gLwYjkH8HY/Ss6lBIIIOCKABlZDtYEEdjSVpRSJe4hnH7zna4/Pmpf7K/6af+O/8A16AP/9k=",
    "warm": "/9j/4AAQSkZJRgABAQAASABIAAD/4QBMRXhpZgAATU0AKgAAAAgAAYdpAAQAAAABAAAAGgAAAAAAA6ABAAMAAAABAAEAAKACAAQAAAABAAAAKKADAAQAAAABAAAAHgAAAAD/7QA4UGhvdG9zaG9wIDMuMAA4QklNBAQAAAAAAAA4QklNBCUAAAAAABDUHYzZjwCyBOmACZjs+EJ+/8AAEQgAHgAoAwEiAAIRAQMRAf/EAB8AAAEFAQEBAQEBAAAAAAAAAAABAgMEBQYHCAkKC//EALUQAAIBAwMCBAMFBQQEAAABfQECAwAEEQUSITFBBhNRYQcicRQygZGhCCNCscEVUtHwJDNicoIJChYXGBkaJSYnKCkqNDU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6g4SFhoeIiYqSk5SVlpeYmZqio6Slpqeoqaqys7S1tre4ubrCw8TFxsfIycrS09TV1tfY2drh4uPk5ebn6Onq8fLz9PX29/j5+v/EAB8BAAMBAQEBAQEBAQEAAAAAAAABAgMEBQYHCAkKC//EALURAAIBAgQEAwQHBQQEAAECdwABAgMRBAUhMQYSQVEHYXETIjKBCBRCkaGxwQkjM1LwFWJy0QoWJDThJfEXGBkaJicoKSo1Njc4OTpDREVGR0hJSlNUVVZXWFlaY2RlZmdoaWpzdHV2d3h5eoKDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uLj5OXm5+jp6vLz9PX29/j5+v/bAEMADAwMDAwMFAwMFB0UFBQdJx0dHR0nMScnJycnMTsxMTExMTE7Ozs7Ozs7O0dHR0dHR1NTU1NTXV1dXV1dXV1dXf/bAEMBDg8PGBYYKBYWKGFCNkJhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYf/dAAQAA//aAAwDAQACEQMRAD8Aes8VyNl1w/RXH9arT28lu22Tv0I6GoKtwXRjXyZBviPUen0rzD2SpRV2a1G3zrY74+/qMetUqACiiigD/9ClRRRXmHsksM0kDbozj1HY/WrhjhvAXgxHIP4Ox+lZ1KCQQQcEUAKysh2sCCOxptacUiXuIZx+852uPz5qX+yv+mn/AI7/APXoA//Z",
    "green": "/9j/4AAQSkZJRgABAQAASABIAAD/4QBMRXhpZgAATU0AKgAAAAgAAYdpAAQAAAABAAAAGgAAAAAAA6ABAAMAAAABAAEAAKACAAQAAAABAAAAHqADAAQAAAABAAAAKAAAAAD/7QA4UGhvdG9zaG9wIDMuMAA4QklNBAQAAAAAAAA4QklNBCUAAAAAABDUHYzZjwCyBOmACZjs+EJ+/8AAEQgAKAAeAwEiAAIRAQMRAf/EAB8AAAEFAQEBAQEBAAAAAAAAAAABAgMEBQYHCAkKC//EALUQAAIBAwMCBAMFBQQEAAABfQECAwAEEQUSITFBBhNRYQcicRQygZGhCCNCscEVUtHwJDNicoIJChYXGBkaJSYnKCkqNDU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6g4SFhoeIiYqSk5SVlpeYmZqio6Slpqeoqaqys7S1tre4ubrCw8TFxsfIycrS09TV1tfY2drh4uPk5ebn6Onq8fLz9PX29/j5+v/EAB8BAAMBAQEBAQEBAQEAAAAAAAABAgMEBQYHCAkKC//EALURAAIBAgQEAwQHBQQEAAECdwABAgMRBAUhMQYSQVEHYXETIjKBCBRCkaGxwQkjM1LwFWJy0QoWJDThJfEXGBkaJicoKSo1Njc4OTpDREVGR0hJSlNUVVZXWFlaY2RlZmdoaWpzdHV2d3h5eoKDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uLj5OXm5+jp6vLz9PX29/j5+v/bAEMADAwMDAwMFAwMFB0UFBQdJx0dHR0nMScnJycnMTsxMTExMTE7Ozs7Ozs7O0dHR0dHR1NTU1NTXV1dXV1dXV1dXf/bAEMBDg8PGBYYKBYWKGFCNkJhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYf/dAAQAAv/aAAwDAQACEQMRAD8AbIBfQeeg/epwQO4rKqaCZoJRIvOOo9RVq8hUgXUOSj8n2JpHkmfRRRQB/9DIq7Z3AjJhk5jk4PPTP+eapUUjySxdW5t5SvO0/dPrVetO2dbqH7HLwV5U/wCf84rOdGjYo4wR1FAH/9HIooopHkgCQQQcEVseWmpIJM7JF4Pf9Kx619K/5af8B/rQB//Z",
    "violet": "/9j/4AAQSkZJRgABAQAASABIAAD/4QBMRXhpZgAATU0AKgAAAAgAAYdpAAQAAAABAAAAGgAAAAAAA6ABAAMAAAABAAEAAKACAAQAAAABAAAAKKADAAQAAAABAAAAKAAAAAD/7QA4UGhvdG9zaG9wIDMuMAA4QklNBAQAAAAAAAA4QklNBCUAAAAAABDUHYzZjwCyBOmACZjs+EJ+/8AAEQgAKAAoAwEiAAIRAQMRAf/EAB8AAAEFAQEBAQEBAAAAAAAAAAABAgMEBQYHCAkKC//EALUQAAIBAwMCBAMFBQQEAAABfQECAwAEEQUSITFBBhNRYQcicRQygZGhCCNCscEVUtHwJDNicoIJChYXGBkaJSYnKCkqNDU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6g4SFhoeIiYqSk5SVlpeYmZqio6Slpqeoqaqys7S1tre4ubrCw8TFxsfIycrS09TV1tfY2drh4uPk5ebn6Onq8fLz9PX29/j5+v/EAB8BAAMBAQEBAQEBAQEAAAAAAAABAgMEBQYHCAkKC//EALURAAIBAgQEAwQHBQQEAAECdwABAgMRBAUhMQYSQVEHYXETIjKBCBRCkaGxwQkjM1LwFWJy0QoWJDThJfEXGBkaJicoKSo1Njc4OTpDREVGR0hJSlNUVVZXWFlaY2RlZmdoaWpzdHV2d3h5eoKDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uLj5OXm5+jp6vLz9PX29/j5+v/bAEMADAwMDAwMFAwMFB0UFBQdJx0dHR0nMScnJycnMTsxMTExMTE7Ozs7Ozs7O0dHR0dHR1NTU1NTXV1dXV1dXV1dXf/bAEMBDg8PGBYYKBYWKGFCNkJhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYf/dAAQAA//aAAwDAQACEQMRAD8AT91qMXZZlH+fw/l/PIdGjYo4wR1FCO0bB0OCOhrX/dajF2WZR/n8P5fz9c1MainOjRsUcYI6im0AFFFFAH//0MWnI7RsHQ4I6Gm0V65qbP7rUYuyzKP8/h/L+eQ6NGxRxgjqKEdo2DocEdDWv+61GLssyj/P4fy/mAY1FOdGjYo4wR1FNoA//9HFooor1zUKcjtGwdDgjoabRQBs/utRi7LMo/z+H8v5wf2Xcf3l/M/4UaX/AMfDf7p/mK3aAP/Z",
    "teal": "/9j/4AAQSkZJRgABAQAASABIAAD/4QBMRXhpZgAATU0AKgAAAAgAAYdpAAQAAAABAAAAGgAAAAAAA6ABAAMAAAABAAEAAKACAAQAAAABAAAAKKADAAQAAAABAAAAFgAAAAD/7QA4UGhvdG9zaG9wIDMuMAA4QklNBAQAAAAAAAA4QklNBCUAAAAAABDUHYzZjwCyBOmACZjs+EJ+/8AAEQgAFgAoAwEiAAIRAQMRAf/EAB8AAAEFAQEBAQEBAAAAAAAAAAABAgMEBQYHCAkKC//EALUQAAIBAwMCBAMFBQQEAAABfQECAwAEEQUSITFBBhNRYQcicRQygZGhCCNCscEVUtHwJDNicoIJChYXGBkaJSYnKCkqNDU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6g4SFhoeIiYqSk5SVlpeYmZqio6Slpqeoqaqys7S1tre4ubrCw8TFxsfIycrS09TV1tfY2drh4uPk5ebn6Onq8fLz9PX29/j5+v/EAB8BAAMBAQEBAQEBAQEAAAAAAAABAgMEBQYHCAkKC//EALURAAIBAgQEAwQHBQQEAAECdwABAgMRBAUhMQYSQVEHYXETIjKBCBRCkaGxwQkjM1LwFWJy0QoWJDThJfEXGBkaJicoKSo1Njc4OTpDREVGR0hJSlNUVVZXWFlaY2RlZmdoaWpzdHV2d3h5eoKDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uLj5OXm5+jp6vLz9PX29/j5+v/bAEMADAwMDAwMFAwMFB0UFBQdJx0dHR0nMScnJycnMTsxMTExMTE7Ozs7Ozs7O0dHR0dHR1NTU1NTXV1dXV1dXV1dXf/bAEMBDg8PGBYYKBYWKGFCNkJhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYf/dAAQAA//aAAwDAQACEQMRAD8AqLerINl2gceo4P8An8qR7Pcvm2h8xP1FUKekjxNujJU+1dp440ggkEYIpK0RcQXIC3gww4Dr/Wo5rKSNfMjIkT1FAFKiiigD/9DEooortPHCpoZpIG3RnHqOx+tQ0UAa0YgvyVKeXJjOV6H/ACT/APXp39lf9NP/AB3/AOvUOl/8fDf7p/mK3aAP/9k=",
}

# Every media file the histories have: file id → what it is (written when downloaded).
FILES: dict[int, dict] = {}
_next_file = [10_000]
# The real media (part 4): small samples next to this file, copied when "downloaded".
MEDIA = os.path.join(os.path.dirname(os.path.abspath(__file__)), "media")


def media_file(name: str) -> dict:
    """A file of mock/media: TDLib's `file`, downloaded as a copy of it."""
    source = os.path.join(MEDIA, name)
    size = os.path.getsize(source) if os.path.exists(source) else 10_000
    return new_file("copy", name, size, source=source)


def poster(name: str, width: int, height: int) -> dict:
    """A video's still thumbnail: a JPEG of mock/media."""
    return {"@type": "thumbnail", "format": {"@type": "thumbnailFormatJpeg"}, "width": width, "height": height,
            "file": media_file(name)}


def waveform(seconds: int) -> str:
    """Telegram's waveform: 5-bit samples (0–31) packed LSB-first, base64 — a voice's rise and fall."""
    samples = [max(0, min(31, int(16 + 13 * math.sin(i / 2.3) * math.sin(i / 11.0)))) for i in range(max(10, seconds * 10))]
    bits = 0
    count = 0
    packed = bytearray()
    for sample in samples:
        bits |= sample << count
        count += 5
        while count >= 8:
            packed.append(bits & 0xFF)
            bits >>= 8
            count -= 8
    if count:
        packed.append(bits & 0xFF)
    return base64.b64encode(bytes(packed)).decode()


def new_file(kind: str, name: str, size: int, **spec) -> dict:
    """A file of the histories, not downloaded: TDLib's `file` (its id registered in FILES)."""
    _next_file[0] += 1
    file_id = _next_file[0]
    FILES[file_id] = dict(spec, kind=kind, name=name, size=size)
    return file_object(file_id, size=size)


def mini(name: str, width: int = 40, height: int = 30) -> dict:
    return {"@type": "minithumbnail", "width": width, "height": height, "data": MINI[name]}


def picture_png(path: str, color: tuple, width: int, height: int) -> None:
    """A picture: a vertical gradient of `color` with a light diagonal band — rows at a time."""
    raw = bytearray()
    band = max(4, height // 10)
    for y in range(height):
        shade = tuple(min(255, int(c * (0.7 + 0.6 * y / max(height, 1)))) for c in color)
        light = tuple(min(255, c + 90) for c in shade)
        x0 = max(0, min(width, (y * width) // max(height, 1) - band))
        x1 = max(0, min(width, x0 + 2 * band))
        raw += b"\x00" + bytes(shade) * x0 + bytes(light) * (x1 - x0) + bytes(shade) * (width - x1)
    _write_png(path, width, height, 2, bytes(raw))


def sticker_png(path: str, color: tuple, side: int = 256) -> None:
    """A sticker: a disc with a lighter ring on a transparent background (RGBA). Flux tells the
    format by the content, so a sticker's `.webp` may hold a PNG."""
    raw = bytearray()
    r = side * 0.42
    c = side / 2
    for y in range(side):
        dy = y - c
        row = bytearray(b"\x00")
        if abs(dy) >= r:
            row += b"\x00\x00\x00\x00" * side
        else:
            half = (r * r - dy * dy) ** 0.5
            x0, x1 = int(c - half), int(c + half)
            inner = max(0.0, (r * 0.7) ** 2 - dy * dy) ** 0.5
            i0, i1 = int(c - inner), int(c + inner)
            light = bytes(min(255, v + 80) for v in color) + b"\xff"
            body = bytes(color) + b"\xff"
            row += b"\x00\x00\x00\x00" * x0
            if inner:
                row += light * (i0 - x0) + body * (i1 - i0) + light * (x1 - i1)
            else:
                row += light * (x1 - x0)
            row += b"\x00\x00\x00\x00" * (side - x1)
        raw += row
    _write_png(path, side, side, 6, bytes(raw))


def _write_png(path: str, width: int, height: int, color_type: int, raw: bytes) -> None:
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    header = struct.pack(">IIBBBBB", width, height, 8, color_type, 0, 0, 0)
    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(raw, 3))
    data += chunk(b"IEND", b"")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as out:
        out.write(data)


def image_size(path: str) -> tuple[int, int]:
    """A picture's size from its header: PNG, JPEG, GIF, WebP; 800×600 when unknown."""
    try:
        with open(path, "rb") as source:
            head = source.read(64 * 1024)
    except OSError:
        return 800, 600
    if head.startswith(b"\x89PNG"):
        return struct.unpack(">II", head[16:24])
    if head[:6] in (b"GIF87a", b"GIF89a"):
        return struct.unpack("<HH", head[6:10])
    if head[:4] == b"RIFF" and head[8:12] == b"WEBP":
        if head[12:16] == b"VP8X":
            return 1 + int.from_bytes(head[24:27], "little"), 1 + int.from_bytes(head[27:30], "little")
        if head[12:16] == b"VP8 ":
            w, h = struct.unpack("<HH", head[26:30])
            return w & 0x3FFF, h & 0x3FFF
    if head.startswith(b"\xff\xd8"):
        at = 2
        while at + 9 < len(head):
            if head[at] != 0xFF:
                at += 1
                continue
            marker = head[at + 1]
            length = struct.unpack(">H", head[at + 2:at + 4])[0]
            if 0xC0 <= marker <= 0xCF and marker not in (0xC4, 0xC8, 0xCC):
                h, w = struct.unpack(">HH", head[at + 5:at + 9])
                return w, h
            at += 2 + length
    return 800, 600


# --- Contents ---

def u16(value: str) -> int:
    return len(value.encode("utf-16-le")) // 2


def entity(source: str, piece: str, kind: str, after: int = 0, **extra) -> dict:
    """The entity of `piece`'s first occurrence in `source` (from the byte offset `after`), with
    TDLib's UTF-16 offset and length."""
    at = source.index(piece, after)
    return {"@type": "textEntity", "offset": u16(source[:at]), "length": u16(piece),
            "type": dict({"@type": kind}, **extra)}


def styled(value: str, *entities: dict) -> dict:
    return {"@type": "formattedText", "text": value, "entities": list(entities)}


def rich_text(value: str, *entities: dict, preview: dict | None = None) -> dict:
    content = {"@type": "messageText", "text": styled(value, *entities), "link_preview": preview}
    return content


def photo(color: tuple, mini_name: str, width: int = 1600, height: int = 1200, caption: str = "") -> dict:
    """A photo in three sizes (m, x, w): the feed takes x, the viewer w."""
    sizes = []
    for kind, scale in (("m", 0.2), ("x", 0.5), ("w", 1.0)):
        w, h = int(width * scale), int(height * scale)
        sizes.append({"@type": "photoSize", "type": kind, "width": w, "height": h,
                      "photo": new_file("picture", f"photo_{kind}.png", w * h // 4, color=color, w=w, h=h),
                      "progressive_sizes": []})
    return {"@type": "messagePhoto", "caption": styled(caption), "show_caption_above_media": False,
            "has_spoiler": False, "is_secret": False,
            "photo": {"@type": "photo", "has_stickers": False,
                      "minithumbnail": mini(mini_name, 40, max(1, 40 * height // width)), "sizes": sizes}}


def thumbnail(color: tuple, width: int, height: int) -> dict:
    return {"@type": "thumbnail", "format": {"@type": "thumbnailFormatJpeg"}, "width": width, "height": height,
            "file": new_file("picture", "thumb.png", width * height // 8, color=color, w=width, h=height)}


def sticker_object(emoji: str, color: tuple, animated: bool | str = False, set_id: str = "1") -> dict:
    """TDLib's `sticker`: a static WebP (a disc), or an animated one — TGS (`True` or "tgs") or WEBM
    ("webm") from mock/media — with a still WebP thumbnail."""
    thumb = new_file("sticker", "thumb.webp", 6_000, color=color)
    kind = "tgs" if animated is True else animated
    sticker_obj = {"@type": "sticker", "id": str(thumb["id"]), "set_id": set_id, "width": 512, "height": 512,
                   "emoji": emoji, "format": {"@type": "stickerFormatWebp"},
                   "full_type": {"@type": "stickerFullTypeRegular"},
                   "thumbnail": {"@type": "thumbnail", "format": {"@type": "thumbnailFormatWebp"},
                                 "width": 128, "height": 128, "file": thumb}}
    if kind:
        sticker_obj["format"] = {"@type": "stickerFormatTgs" if kind == "tgs" else "stickerFormatWebm"}
        sticker_obj["sticker"] = media_file(f"sticker.{kind}")
    else:
        sticker_obj["sticker"] = new_file("sticker", "sticker.webp", 18_000, color=color)
    return sticker_obj


def sticker(emoji: str, color: tuple, animated: bool | str = False) -> dict:
    return {"@type": "messageSticker", "sticker": sticker_object(emoji, color, animated), "is_premium": False}


def document(name: str, content: bytes, caption: str = "") -> dict:
    file = new_file("bytes", name, len(content), content=content)
    return {"@type": "messageDocument", "caption": styled(caption),
            "document": {"@type": "document", "file_name": name, "mime_type": "application/octet-stream",
                         "minithumbnail": None, "thumbnail": None, "document": file}}


def video(seconds: int, color: tuple, mini_name: str, caption: str = "") -> dict:
    """A video (mock/media/video.mp4, 640×360, with its poster)."""
    return {"@type": "messageVideo", "caption": styled(caption), "show_caption_above_media": False,
            "has_spoiler": False, "is_secret": False,
            "video": {"@type": "video", "duration": seconds, "width": 640, "height": 360,
                      "file_name": "clip.mp4", "mime_type": "video/mp4", "has_stickers": False,
                      "supports_streaming": True, "minithumbnail": mini(mini_name, 40, 22),
                      "thumbnail": poster("video.jpg", 640, 360), "video": media_file("video.mp4")}}


def animation_object(mini_name: str = "violet") -> dict:
    """TDLib's `animation`: mock/media/gif.mp4 (320×240, no sound) with its poster."""
    return {"@type": "animation", "duration": 3, "width": 320, "height": 240,
            "file_name": "funny.mp4", "mime_type": "video/mp4", "has_stickers": False,
            "minithumbnail": mini(mini_name, 40, 30), "thumbnail": poster("gif.jpg", 320, 240),
            "animation": media_file("gif.mp4")}


def animation(color: tuple, mini_name: str) -> dict:
    return {"@type": "messageAnimation", "caption": styled(""), "show_caption_above_media": False,
            "has_spoiler": False, "is_secret": False, "animation": animation_object(mini_name)}


def video_note(seconds: int, color: tuple, mini_name: str) -> dict:
    """A round message (mock/media/round.mp4, 384×384, with sound)."""
    return {"@type": "messageVideoNote", "is_viewed": False, "is_secret": False,
            "video_note": {"@type": "videoNote", "duration": seconds, "length": 384, "waveform": "",
                           "minithumbnail": mini(mini_name, 40, 40), "thumbnail": poster("round.jpg", 384, 384),
                           "speech_recognition_result": None, "video": media_file("round.mp4")}}


def voice(seconds: int, caption: str = "") -> dict:
    """A voice message (mock/media/voice.ogg) with its waveform."""
    return {"@type": "messageVoiceNote", "caption": styled(caption), "is_listened": False,
            "voice_note": {"@type": "voiceNote", "duration": seconds, "waveform": waveform(seconds),
                           "mime_type": "audio/ogg", "speech_recognition_result": None,
                           "voice": media_file("voice.ogg")}}


def audio(performer: str, title: str) -> dict:
    return {"@type": "messageAudio", "caption": styled(""),
            "audio": {"@type": "audio", "duration": 214, "title": title, "performer": performer,
                      "file_name": "demo.mp3", "mime_type": "audio/mpeg", "album_cover_minithumbnail": None,
                      "album_cover_thumbnail": None, "external_album_covers": [],
                      "audio": new_file("bytes", "demo.mp3", 5_100_000)}}


def link_preview(url: str, site: str, title: str, description: str, color: tuple, mini_name: str) -> dict:
    preview_photo = photo(color, mini_name, 1200, 630)["photo"]
    return {"@type": "linkPreview", "url": url, "display_url": url.split("://")[-1], "site_name": site,
            "title": title, "description": styled(description), "author": "",
            "type": {"@type": "linkPreviewTypeArticle", "photo": preview_photo},
            "has_large_media": True, "show_large_media": True, "show_media_above_description": False,
            "skip_confirmation": False, "show_above_text": False, "instant_view_version": 0}


def reactions(*items: tuple) -> dict:
    return {"@type": "messageInteractionInfo", "view_count": 0, "forward_count": 0, "reply_info": None,
            "reactions": {"@type": "messageReactions", "are_tags": False, "paid_reactors": [],
                          "can_get_added_reactions": True,
                          "reactions": [{"@type": "messageReaction",
                                         "type": {"@type": "reactionTypeEmoji", "emoji": emoji},
                                         "total_count": count, "is_chosen": chosen, "used_sender_id": None,
                                         "recent_sender_ids": []} for emoji, count, chosen in items]}}


def reply_to(chat_id: int, message_id: int) -> dict:
    return {"@type": "messageReplyToMessage", "chat_id": chat_id, "message_id": message_id, "quote": None,
            "origin": None, "origin_send_date": 0, "content": None}


def forward(origin: dict) -> dict:
    return {"@type": "messageForwardInfo", "origin": origin, "date": NOW - 86_400 * 3, "source": None,
            "public_service_announcement_type": ""}


# --- The histories ---

BACKEND = -1000000001001
DESIGN = -201
FLOOD = -1000000001003
OLD = -1000000001004
TEAM = [101, 102, 103, 104, 107]
BLUE, WARM, GREEN, VIOLET, TEAL = (60, 90, 170), (190, 110, 60), (50, 140, 100), (120, 90, 180), (40, 130, 150)
FILLER = [
    "Config — env is read before the file.",
    "Sounds good.",
    "Can we measure the scroll at 120 Hz?",
    "I'll check it tomorrow morning, there is a lot to read here and the text wraps over several lines.",
    "Pushed the fix, CI is green.",
    "Who is on call this week?",
    "Let's sync after lunch.",
    "Merged 👍",
    "Привет! Посмотрю вечером.",
    "The staging is back up.",
]


def say(chat_id: int, sender: int, content: dict, date: int, message_id: int | None = None, **extra) -> dict:
    return add(chat_id, message(chat_id, sender, content, date, message_id=message_id, outgoing=sender == 0,
                                **extra))


def fill_backend() -> None:
    chat = BACKEND
    t = NOW - 86_400 * 4
    say(chat, 101, {"@type": "messageSupergroupChatCreate", "title": "Backend · team"}, t)
    say(chat, 101, {"@type": "messageChatAddMembers", "member_user_ids": [103, 102]}, t + 60)
    old = say(chat, 103, text("Do we keep the old API for the mobile app until spring?"), t + 120)
    # Part 6: enough history for Claude's pages — 120 lines over three days (with the rest, more
    # than 150 messages), replies to the old question and a forward among them.
    for i in range(120):
        sender = 0 if i % 6 == 0 else TEAM[i % len(TEAM)]
        extra = {}
        if i % 25 == 7:
            extra["reply_to"] = {"@type": "messageReplyToMessage", "chat_id": chat,
                                 "message_id": old["id"]}
        if i % 40 == 13:
            extra["forward_info"] = {"@type": "messageForwardInfo", "date": t,
                                     "origin": {"@type": "messageOriginUser", "sender_user_id": 104}}
        say(chat, sender, text(FILLER[i % len(FILLER)]), t + 600 + i * 2400, **extra)
    t = NOW - 3 * 3600
    plan = "Release plan for Friday: Thursday is too early, secret: v2. Run cargo build, see https://example.com/release and the doc, ask @dimakim or Petya, mail team@example.com, call +7 900 123-45-67, #release /deploy@flux_bot"
    say(chat, 101, rich_text(
        plan,
        entity(plan, "Release plan", "textEntityTypeBold"),
        entity(plan, "Friday", "textEntityTypeItalic"),
        entity(plan, "Thursday", "textEntityTypeStrikethrough"),
        entity(plan, "v2", "textEntityTypeSpoiler"),
        entity(plan, "cargo build", "textEntityTypeCode"),
        entity(plan, "https://example.com/release", "textEntityTypeUrl"),
        entity(plan, "the doc", "textEntityTypeTextUrl", url="https://example.com/doc"),
        entity(plan, "@dimakim", "textEntityTypeMention"),
        entity(plan, "Petya", "textEntityTypeMentionName", user_id=103),
        entity(plan, "team@example.com", "textEntityTypeEmailAddress"),
        entity(plan, "+7 900 123-45-67", "textEntityTypePhoneNumber"),
        entity(plan, "#release", "textEntityTypeHashtag"),
        entity(plan, "/deploy@flux_bot", "textEntityTypeBotCommand"),
    ), t)
    hello = "Привет 👋 жирный и курсив 🚀 конец, ~~зачёркнуто~~ и подчёркнуто"
    say(chat, 102, rich_text(
        hello,
        entity(hello, "жирный и курсив", "textEntityTypeBold"),
        entity(hello, "и курсив 🚀 конец", "textEntityTypeItalic"),
        entity(hello, "~~зачёркнуто~~", "textEntityTypeStrikethrough"),
        entity(hello, "подчёркнуто", "textEntityTypeUnderline"),
    ), t + 60)
    fix = ("Here is the fix:\nfn main() {\n    let answer = 42;\n    println!(\"{answer}\");\n}\n"
           "And the rule:\nMeasure first, then optimize.\nAgreed?")
    code_text = "fn main() {\n    let answer = 42;\n    println!(\"{answer}\");\n}"
    say(chat, 103, rich_text(
        fix,
        entity(fix, code_text, "textEntityTypePreCode", language="rust"),
        entity(fix, "Measure first, then optimize.", "textEntityTypeBlockQuote"),
        entity(fix, "Measure first", "textEntityTypeBold"),
    ), t + 120)
    say(chat, 101, photo(BLUE, "blue", caption="The new map screen — good?"), t + 300,
        interaction_info=reactions(("🔥", 2, True), ("👍", 1, False)))
    mine = say(chat, 0, text("Config — I'll check today: env is read before the file"), t + 600)
    say(chat, 102, document("staging.env.example", (b"# Staging\nAPI_URL=https://staging.example.com\n" * 26)[:1229],
                            caption="Ok, I'll look at the PR tonight. The env example for staging:"),
        t + 900, reply_to=reply_to(chat, mine["id"]))
    say(chat, 103, sticker("🚀", WARM), t + 1000)
    say(chat, 103, voice(42, caption="Release on Friday? Or Thursday?"), t + 1100)
    say(chat, 101, video(65, TEAL, "teal", caption="The demo of the map"), t + 1300)
    say(chat, 102, animation(VIOLET, "violet"), t + 1400)
    say(chat, 107, video_note(12, GREEN, "green"), t + 1500)
    say(chat, 104, {"@type": "messagePoll", "poll": {"@type": "poll", "id": "1",
        "question": styled("Where do we go for lunch?"), "options": [], "total_voter_count": 3,
        "recent_voter_ids": [], "is_anonymous": True, "type": {"@type": "pollTypeRegular",
        "allow_multiple_answers": False}, "open_period": 0, "close_date": 0, "is_closed": False}}, t + 1600)
    say(chat, 101, {"@type": "messageLocation", "location": {"@type": "location", "latitude": 55.75,
        "longitude": 37.61, "horizontal_accuracy": 0}, "live_period": 0, "heading": 0,
        "proximity_alert_radius": 0}, t + 1700)
    say(chat, 102, {"@type": "messageVenue", "venue": {"@type": "venue", "location": {"@type": "location",
        "latitude": 55.75, "longitude": 37.61}, "title": "Coffee Bean", "address": "Main st. 1",
        "provider": "", "id": "", "type": ""}}, t + 1750)
    say(chat, 103, {"@type": "messageContact", "contact": {"@type": "contact", "phone_number": "79001234567",
        "first_name": "Ivan", "last_name": "Petrov", "vcard": "", "user_id": 0}}, t + 1800)
    say(chat, 101, text("Measure first, then optimize."), t + 1900,
        forward_info=forward({"@type": "messageOriginUser", "sender_user_id": 107}))
    say(chat, 102, rich_text("Flux 0.5 is out: chats in the editor https://example.com/news",
                             entity("Flux 0.5 is out: chats in the editor https://example.com/news",
                                    "https://example.com/news", "textEntityTypeUrl")), t + 2000,
        forward_info=forward({"@type": "messageOriginChannel", "chat_id": -1000000001002, "message_id": 1,
                              "author_signature": ""}))
    say(chat, 103, text("Don't deploy on Fridays."), t + 2100,
        forward_info=forward({"@type": "messageOriginHiddenUser", "sender_name": "Ivan Petrov"}))
    say(chat, 103, text("Back to this: still relevant?"), t + 2200, reply_to=reply_to(chat, old["id"]))
    say(chat, 101, text("Edited: release on Friday at 15:00"), t + 2300, edit_date=t + 2400)
    blog = "Release notes are up: https://example.com/blog/release"
    say(chat, 102, rich_text(blog, entity(blog, "https://example.com/blog/release", "textEntityTypeUrl"),
                             preview=link_preview("https://example.com/blog/release", "Example Blog",
                                                  "Release notes",
                                                  "What is new in this release: faster scrolling, pictures, chats.",
                                                  WARM, "warm")), t + 2500)
    say(chat, 107, audio("Masha's band", "Demo 3"), t + 2600)
    say(chat, 101, {"@type": "messageDice", "initial_state": None, "final_state": None, "emoji": "🎲",
                    "value": 4, "success_animation_frame_number": 0}, t + 2700)
    say(chat, 101, {"@type": "messagePinMessage", "message_id": mine["id"]}, t + 2800)
    say(chat, BACKEND, text("Reminder: the standup is at 11."), t + 2900)
    say(chat, 0, photo(GREEN, "green", 1200, 1600, caption="The office today"), t + 3000)
    say(chat, 0, text("Looks good to me. Shipping it 🚀"), t + 3100, interaction_info=reactions(("❤", 1, False)))
    say(chat, 102, text("Then on Friday."), t + 3300)
    say(chat, 101, text("I'll prepare the notes."), t + 3400)
    say(chat, 103, text("Release on Friday? Or Thursday?"), t + 3500)


def fill_people() -> None:
    t = NOW - 86_400 * 2
    for i in range(24):
        sender = 0 if i % 3 == 0 else 101
        say(101, sender, text(FILLER[(i + 3) % len(FILLER)]), t + i * 3000)
    say(101, 101, photo(WARM, "warm", 1200, 900, caption="The mockups, first take"), NOW - 7200)
    mine = say(101, 0, text("Great, I'll look tonight"), NOW - 7000)
    say(101, 101, text("And the second screen?"), NOW - 6000, reply_to=reply_to(101, mine["id"]))
    say(101, 0, sticker("👍", GREEN), NOW - 5000)
    say(101, 101, text("Sent you the mockups, take a look when you can"), NOW - 600)

    for i in range(18):
        sender = 0 if i % 2 == 0 else 102
        say(102, sender, text(FILLER[(i + 5) % len(FILLER)]), NOW - 86_400 * 3 + i * 4000)
    say(102, 102, document("report.pdf", b"%PDF-1.4\n% fake report\n" * 200), NOW - 9000)
    say(102, 0, text("See you at 11 tomorrow?"), NOW - 1900)

    say(104, 104, text("The meeting moved to Monday"), NOW - 86_400)
    say(104, 104, sticker("👍", BLUE, animated=True), NOW - 80_000)

    for i, line in enumerate(["hey", "are you there?", "ok, later then"]):
        say(103, 103, text(line), NOW - 30_000 + i * 600)
    say(106, 106, text("…"), NOW - 86_400 * 40)
    say(107, 107, voice(17), NOW - 20_000)
    say(107, 107, text("Thanks!"), NOW - 19_000)
    # Every kind of media to play (part 4), at the end of the chat: a voice, a round, a GIF, a
    # video, a static sticker, an animated TGS and WEBM one, an animated emoji.
    say(107, 107, text("Recorded the demo for you"), NOW - 3000)
    say(107, 107, voice(5), NOW - 2900)
    say(107, 107, video_note(6, GREEN, "green"), NOW - 2800)
    say(107, 0, animation(VIOLET, "violet"), NOW - 2700)
    say(107, 107, video(10, TEAL, "teal", caption="The walkthrough"), NOW - 2600)
    say(107, 107, sticker("👍", BLUE), NOW - 2500)
    say(107, 107, sticker("🎉", WARM, animated="tgs"), NOW - 2400)
    say(107, 0, sticker("🐱", VIOLET, animated="webm"), NOW - 2300)
    say(107, 107, {"@type": "messageAnimatedEmoji", "emoji": "❤",
                   "animated_emoji": {"@type": "animatedEmoji", "sticker": sticker_object("❤", WARM, "tgs"),
                                      "sticker_width": 512, "sticker_height": 512, "fitzpatrick_type": 0,
                                      "sound": None}}, NOW - 2200)
    say(OLD, 104, text("We're moving to the new repository."), NOW - 86_400 * 90)
    say(OLD, 104, text("Archived"), NOW - 86_400 * 89)


def fill_design() -> None:
    t = NOW - 86_400
    for i in range(20):
        say(DESIGN, [101, 107, 0][i % 3], text(FILLER[(i + 1) % len(FILLER)]), t + i * 1800)
    # Albums: items of their own sharing `media_album_id` (an int64 — a string in TDLib's JSON);
    # the second one has a video and its caption on the last item, with the reactions.
    album = "13800000000000077"
    for color, name in ((BLUE, "blue"), (WARM, "warm"), (VIOLET, "violet")):
        say(DESIGN, 101, photo(color, name), NOW - 5000, media_album_id=album)
    say(DESIGN, 107, sticker("😍", VIOLET), NOW - 4800)
    album = "13800000000000078"
    say(DESIGN, 107, photo(GREEN, "green", 1200, 1600), NOW - 4700, media_album_id=album)
    say(DESIGN, 107, photo(TEAL, "teal", 1600, 900), NOW - 4700, media_album_id=album)
    say(DESIGN, 107, video(42, WARM, "warm", caption="Moodboard v2: the last one is the walkthrough"),
        NOW - 4700, media_album_id=album, interaction_info=reactions(("🔥", 2, False)))
    for i in range(10):
        say(DESIGN, [101, 107][i % 2], text(f"Comment {i + 1}: {FILLER[i % len(FILLER)]}"), NOW - 4000 + i * 200)


def fill_flood() -> None:
    t = NOW - 10_000 * 120
    people = [101, 102, 103, 104, 107]
    first = fake_world.next_id(FLOOD)
    for i in range(10_000):
        sender = 0 if (i % 7 == 0 and i < 9_900) else people[i % len(people)]
        line = f"#{i + 1} {FILLER[i % len(FILLER)]}"
        say(FLOOD, sender, text(line), t + i * 120, message_id=first + i * fake_world.STEP)


def fill_saved(me_id: int) -> None:
    with fake_world.LOCK:
        if fake_world.MESSAGES.get(me_id):
            return
        say(me_id, 0, text("Ideas for the talk: measure, then optimize"), NOW - 86_400 * 5)
        say(me_id, 0, document("notes.txt", b"Notes for the talk.\n" * 40), NOW - 86_400 * 4)
        say(me_id, 0, text("Measure first, then optimize."), NOW - 86_400 * 3,
            forward_info=forward({"@type": "messageOriginUser", "sender_user_id": 101}))


fill_backend()
fill_people()
fill_design()
fill_flood()
fake_world.seed_texts()


# --- The requests ---

# The chats a client has open (openChat): FAKE_TYPING types there.
OPEN: set[tuple[str, int]] = set()
# The chats whose history a client asked for already: the first ask gives one message.
ASKED: set[tuple[str, int]] = set()
# Downloads in progress: (client, file id) → its timer.
DOWNLOADS: dict[tuple[str, int], threading.Timer] = {}
_temporary = [0]


def me_of(client) -> int:
    return int((client.user or {}).get("id", 0))


def history_of(client, chat_id: int) -> list[dict]:
    if chat_id == me_of(client):
        fill_saved(chat_id)
    return fake_world.MESSAGES.get(chat_id) or []


def served(client, obj: dict) -> dict:
    """A message as the account sees it: its own (sender 0) are the account's."""
    if obj["sender_id"].get("user_id") == 0:
        obj = dict(obj, sender_id={"@type": "messageSenderUser", "user_id": me_of(client)})
    return obj


def find(client, chat_id: int, message_id: int) -> dict | None:
    for obj in history_of(client, chat_id):
        if obj["id"] == message_id:
            return obj
    return None


@handler("openChat")
def open_chat(client, request: dict) -> None:
    OPEN.add((client.key, int(request["chat_id"])))
    client.ok(request)


@handler("closeChat")
def close_chat(client, request: dict) -> None:
    OPEN.discard((client.key, int(request["chat_id"])))
    client.ok(request)


@handler("getChatHistory")
def get_chat_history(client, request: dict) -> None:
    chat_id = int(request["chat_id"])
    start_from = int(request.get("from_message_id") or 0)
    offset = int(request.get("offset") or 0)
    limit = max(1, min(100, int(request.get("limit") or 50)))
    with fake_world.LOCK:
        history = list(history_of(client, chat_id))
    ids = [obj["id"] for obj in history]
    found: list[dict] = []
    if history:
        at = len(history) - 1 if start_from == 0 else bisect.bisect_right(ids, start_from) - 1
        if at >= 0 or offset < 0:
            top = min(len(history) - 1, at - offset) if offset < 0 else at
            bottom = max(0, top - limit + 1)
            found = list(reversed(history[bottom:top + 1]))
    # TDLib's first answer for a chat it hasn't loaded: what it has at hand.
    if (client.key, chat_id) not in ASKED and found:
        ASKED.add((client.key, chat_id))
        exact = [obj for obj in found if obj["id"] <= (start_from or ids[-1])]
        found = exact[:1] or found[:1]
    client.answer(request, {"@type": "messages", "total_count": len(found),
                            "messages": [served(client, obj) for obj in found]})


@handler("getMessage")
def get_message(client, request: dict) -> None:
    obj = find(client, int(request["chat_id"]), int(request["message_id"]))
    if obj is None:
        return client.error(request, 404, "Message not found")
    client.answer(request, served(client, obj))


@handler("getRepliedMessage")
def get_replied_message(client, request: dict) -> None:
    obj = find(client, int(request["chat_id"]), int(request["message_id"]))
    target = (obj or {}).get("reply_to") or {}
    replied = find(client, int(request["chat_id"]), int(target.get("message_id") or 0))
    if replied is None:
        return client.error(request, 404, "Message not found")
    client.answer(request, served(client, replied))


@handler("getMessages")
def get_messages(client, request: dict) -> None:
    chat_id = int(request["chat_id"])
    found = [find(client, chat_id, int(i)) for i in request.get("message_ids") or []]
    client.answer(request, {"@type": "messages", "total_count": len(found),
                            "messages": [served(client, obj) if obj else None for obj in found]})


@handler("viewMessages")
def view_messages(client, request: dict) -> None:
    chat_id = int(request["chat_id"])
    client.ok(request)
    chat_obj = fake_world.CHATS.get(chat_id)
    viewed = [int(i) for i in request.get("message_ids") or []]
    if not chat_obj or not viewed:
        return
    incoming = [obj["id"] for obj in history_of(client, chat_id) if not obj.get("is_outgoing")]
    unread = [i for i in incoming if i > max(viewed)]
    if len(unread) >= chat_obj.get("unread_count", 0):
        return
    chat_obj["unread_count"] = len(unread)
    read = max(i for i in incoming if i <= max(viewed)) if any(i <= max(viewed) for i in incoming) else 0
    client.send({"@type": "updateChatReadInbox", "chat_id": chat_id, "last_read_inbox_message_id": read,
                 "unread_count": len(unread)})


def content_of(client, chat_id: int, content: dict, temporary: int) -> dict:
    """A message's content from an `inputMessage…`: a sent photo or file is the local file."""
    kind = content.get("@type")
    caption = content.get("caption") or styled("")
    import fake_media  # the panel's stickers and GIFs (part 4)
    media = fake_media.content_of(content)
    if media is not None:
        return media
    if kind == "inputMessageText":
        formatted = content.get("text") or {}
        value = formatted.get("text", "")
        found = [entity(value, url, "textEntityTypeUrl") for url in re.findall(r"https?://\S+", value)]
        return rich_text(value, *(formatted.get("entities") or []), *found)
    path = ((content.get("photo") or content.get("document") or {}).get("path")) or ""
    size = os.path.getsize(path) if os.path.exists(path) else 0
    local = file_object(1_000_000 + temporary, path=path, size=size)
    # The user's own file: TDLib downloads it from where it is (a click, the feed in sight).
    FILES[local["id"]] = {"kind": "local", "name": os.path.basename(path), "size": size, "path": path}
    if kind == "inputMessagePhoto":
        width, height = image_size(path)
        return {"@type": "messagePhoto", "caption": caption, "show_caption_above_media": False,
                "has_spoiler": False, "is_secret": False,
                "photo": {"@type": "photo", "has_stickers": False, "minithumbnail": None,
                          "sizes": [{"@type": "photoSize", "type": "x", "width": width, "height": height,
                                     "photo": local, "progressive_sizes": []}]}}
    return {"@type": "messageDocument", "caption": caption,
            "document": {"@type": "document", "file_name": os.path.basename(path),
                         "mime_type": "application/octet-stream", "minithumbnail": None, "thumbnail": None,
                         "document": local}}


def send_one(client, request: dict, chat_id: int, content: dict, album: str = "0",
             sent_album: str = "0") -> dict:
    """A pending message now; the server's (or a failure) in half a second. An album's items: the
    pending ones share TDLib's local album id (negative), the sent ones the server's."""
    with fake_world.LOCK:
        history = history_of(client, chat_id)
        last = max([obj["id"] for obj in history] or [0])
        _temporary[0] += 1
        temporary = last + _temporary[0]
    pending = message(chat_id, me_of(client), content_of(client, chat_id, content, temporary), int(time.time()),
                      message_id=temporary, outgoing=True, media_album_id=album,
                      sending_state={"@type": "messageSendingStatePending", "sending_id": 0},
                      can_be_edited=False, can_be_deleted_only_for_self=True,
                      can_be_deleted_for_all_users=False)
    target = request.get("reply_to") or {}
    if target.get("message_id"):
        pending["reply_to"] = reply_to(chat_id, int(target["message_id"]))
    # A forum's topic (part 5): the message stays in it.
    if request.get("topic_id"):
        pending["topic_id"] = request["topic_id"]
    client.send({"@type": "updateNewMessage", "message": pending})
    path = ((content.get("photo") or content.get("document") or {}).get("path")) or ""

    def done() -> None:
        if "FAIL" in os.path.basename(path):
            failed = dict(pending, id=temporary + 7, sending_state={
                "@type": "messageSendingStateFailed", "error": {"@type": "error", "code": 400,
                                                                "message": "FILE_PART_INVALID"},
                "can_retry": True, "need_another_sender": False, "need_another_reply_quote": False,
                "need_drop_reply": False, "required_paid_message_star_count": 0, "retry_after": 0})
            client.send({"@type": "updateMessageSendFailed", "message": failed, "old_message_id": temporary,
                         "error": {"@type": "error", "code": 400, "message": "FILE_PART_INVALID"}})
            return
        with fake_world.LOCK:
            sent = dict(pending, id=fake_world.next_id(chat_id), sending_state=None, media_album_id=sent_album,
                        can_be_edited=True, can_be_deleted_for_all_users=True,
                        can_be_deleted_only_for_self=False, date=int(time.time()))
            sent_stored = dict(sent, sender_id={"@type": "messageSenderUser", "user_id": 0})
            add(chat_id, sent_stored)
        client.send({"@type": "updateMessageSendSucceeded", "message": sent, "old_message_id": temporary})
        client.send({"@type": "updateChatLastMessage", "chat_id": chat_id, "last_message": sent,
                     "positions": (fake_world.CHATS.get(chat_id) or {}).get("positions") or []})
        person = fake_world.USERS.get(chat_id)
        if person and person["status"].get("@type") == "userStatusOnline":
            threading.Timer(2.0, lambda: client.send({
                "@type": "updateChatReadOutbox", "chat_id": chat_id,
                "last_read_outbox_message_id": sent["id"]})).start()

    threading.Timer(0.5, done).start()
    return pending


@handler("sendMessage")
def send_message(client, request: dict) -> None:
    chat_id = int(request["chat_id"])
    pending = send_one(client, request, chat_id, request.get("input_message_content") or {})
    client.answer(request, pending)


@handler("sendMessageAlbum")
def send_message_album(client, request: dict) -> None:
    chat_id = int(request["chat_id"])
    album, sent_album = str(-random.randint(10**9, 10**10)), str(random.randint(10**9, 10**10))
    sent = [send_one(client, request, chat_id, content, album, sent_album)
            for content in request.get("input_message_contents") or []]
    client.answer(request, {"@type": "messages", "total_count": len(sent), "messages": sent})


def edited(client, request: dict, change) -> None:
    chat_id, message_id = int(request["chat_id"]), int(request["message_id"])
    obj = find(client, chat_id, message_id)
    if obj is None:
        return client.error(request, 400, "Message not found")
    change(obj)
    obj["edit_date"] = int(time.time())
    client.answer(request, served(client, obj))
    client.send({"@type": "updateMessageContent", "chat_id": chat_id, "message_id": message_id,
                 "new_content": obj["content"]})
    client.send({"@type": "updateMessageEdited", "chat_id": chat_id, "message_id": message_id,
                 "edit_date": obj["edit_date"], "reply_markup": None})


@handler("editMessageText")
def edit_message_text(client, request: dict) -> None:
    formatted = (request.get("input_message_content") or {}).get("text") or {}
    value = formatted.get("text", "")
    edited(client, request, lambda obj: obj.update(content=rich_text(value, *(formatted.get("entities") or []))))


# TDLib's Markdown (`parseMarkdown`), as far as FluxGram writes it: **bold**, __italic__,
# ~~strike~~, ||spoiler||, `code`, ```pre``` (a language on its first line), [text](url) —
# `tg://user?id=` a mention. A mark without its pair stays text.
MARKS = [("```", "textEntityTypePre"), ("**", "textEntityTypeBold"), ("__", "textEntityTypeItalic"),
         ("~~", "textEntityTypeStrikethrough"), ("||", "textEntityTypeSpoiler"), ("`", "textEntityTypeCode")]
LINK = re.compile(r"\[([^\]\n]+)\]\(([^)\s]+)\)")


def parse_markdown(source: str) -> dict:
    out: list[str] = []
    entities: list[dict] = []
    opened: dict[str, tuple[int, dict]] = {}
    at = 0  # UTF-16 units of the text so far

    def add(piece: str) -> None:
        nonlocal at
        out.append(piece)
        at += u16(piece)

    i = 0
    while i < len(source):
        link = LINK.match(source, i)
        if link and "```" not in opened and "`" not in opened:
            label, url = link.group(1), link.group(2)
            start = at
            add(label)
            user = re.fullmatch(r"tg://user\?id=(\d+)", url)
            kind = ({"@type": "textEntityTypeMentionName", "user_id": int(user.group(1))} if user
                    else {"@type": "textEntityTypeTextUrl", "url": url})
            entities.append({"@type": "textEntity", "offset": start, "length": at - start, "type": kind})
            i = link.end()
            continue
        for mark, kind in MARKS:
            if not source.startswith(mark, i):
                continue
            in_code = ("```" in opened or "`" in opened) and mark not in opened
            if in_code:
                continue
            if mark in opened:
                start, entity_type = opened.pop(mark)
                if mark == "```" and out and out[-1] == "\n":
                    out.pop()
                    at -= 1
                if at > start:
                    entities.append({"@type": "textEntity", "offset": start, "length": at - start,
                                     "type": entity_type})
                i += len(mark)
                break
            if source.find(mark, i + len(mark)) == -1:
                continue
            i += len(mark)
            entity_type = {"@type": kind}
            if mark == "```":
                language = re.match(r"([\w+-]+)\n", source[i:])
                if language:
                    entity_type = {"@type": "textEntityTypePreCode", "language": language.group(1)}
                    i += language.end()
                elif source.startswith("\n", i):
                    i += 1
            opened[mark] = (at, entity_type)
            break
        else:
            add(source[i])
            i += 1
    text = "".join(out)
    entities.sort(key=lambda e: (e["offset"], -e["length"]))
    return {"@type": "formattedText", "text": text, "entities": entities}


@handler("parseMarkdown")
def parse_markdown_request(client, request: dict) -> None:
    client.answer(request, parse_markdown(((request.get("text") or {}).get("text")) or ""))


@handler("editMessageCaption")
def edit_message_caption(client, request: dict) -> None:
    caption = request.get("caption") or styled("")
    edited(client, request, lambda obj: obj["content"].update(caption=caption))


@handler("deleteMessages")
def delete_messages(client, request: dict) -> None:
    chat_id = int(request["chat_id"])
    ids = {int(i) for i in request.get("message_ids") or []}
    with fake_world.LOCK:
        history = fake_world.MESSAGES.get(chat_id) or []
        fake_world.MESSAGES[chat_id] = [obj for obj in history if obj["id"] not in ids]
    client.ok(request)
    client.send({"@type": "updateDeleteMessages", "chat_id": chat_id, "message_ids": sorted(ids),
                 "is_permanent": True, "from_cache": False})


# Part 5: "Old project" protects its content — its messages can't be forwarded (TDLib's
# `has_protected_content`; messages no longer say `can_be_forwarded` themselves).
fake_world.CHATS[OLD]["has_protected_content"] = True


@handler("forwardMessages")
def forward_messages(client, request: dict) -> None:
    """Part 5: the messages of `from_chat_id` as new ones of `chat_id`, in the order asked,
    "Forwarded from" their senders — on the server after a text sent just before them (0.6 s:
    the text's own takes 0.5 s); a protected chat's don't go."""
    chat_id, from_chat = int(request["chat_id"]), int(request["from_chat_id"])
    if (fake_world.CHATS.get(from_chat) or {}).get("has_protected_content"):
        client.error(request, 400, "MESSAGE_PROTECTED")
        return
    originals = [obj for obj in (find(client, from_chat, int(i)) for i in request.get("message_ids") or [])
                 if obj is not None]
    client.answer(request, {"@type": "messages", "total_count": 0, "messages": []})

    def done() -> None:
        sent = []
        for original in originals:
            sender = original["sender_id"]
            origin = ({"@type": "messageOriginChannel", "chat_id": sender["chat_id"],
                       "message_id": original["id"], "author_signature": ""}
                      if sender.get("@type") == "messageSenderChat"
                      else {"@type": "messageOriginUser",
                            "sender_user_id": sender.get("user_id") or me_of(client)})
            with fake_world.LOCK:
                obj = message(chat_id, 0, original["content"], int(time.time()),
                              message_id=fake_world.next_id(chat_id), outgoing=True,
                              forward_info=dict(forward(origin), date=original["date"]),
                              can_be_edited=False, can_be_deleted_for_all_users=True,
                              can_be_deleted_only_for_self=False)
                add(chat_id, obj)
            obj = served(client, obj)
            sent.append(obj)
            client.send({"@type": "updateNewMessage", "message": obj})
        if sent:
            client.send({"@type": "updateChatLastMessage", "chat_id": chat_id, "last_message": sent[-1],
                         "positions": (fake_world.CHATS.get(chat_id) or {}).get("positions") or []})

    threading.Timer(0.6, done).start()


@handler("sendChatAction")
def send_chat_action(client, request: dict) -> None:
    client.ok(request)


@handler("setChatDraftMessage")
def set_chat_draft_message(client, request: dict) -> None:
    chat_id = int(request["chat_id"])
    draft = request.get("draft_message")
    chat_obj = fake_world.CHATS.get(chat_id)
    if chat_obj is not None:
        chat_obj["draft_message"] = draft
    client.ok(request)
    client.send({"@type": "updateChatDraftMessage", "chat_id": chat_id, "draft_message": draft,
                 "positions": (chat_obj or {}).get("positions") or []})


# --- Files ---

def file_path(client, file_id: int, spec: dict | None) -> str:
    files = client.params.get("files_directory", "/tmp")
    if spec is None:
        return os.path.join(files, "profile_photos", f"{file_id}.png")
    if spec["kind"] == "local":
        return spec["path"]
    return os.path.join(files, "fake", f"{file_id}-{spec['name']}")


def write_file(path: str, file_id: int, spec: dict | None) -> int:
    if spec is None:
        # A profile or chat photo: a color by the id.
        fake_world.png(path, ((file_id * 37) % 200 + 40, (file_id * 91) % 160 + 60, 200), 96, 96)
    elif spec["kind"] == "picture":
        picture_png(path, spec["color"], spec["w"], spec["h"])
    elif spec["kind"] == "sticker":
        sticker_png(path, spec["color"])
    elif spec["kind"] == "copy":
        os.makedirs(os.path.dirname(path), exist_ok=True)
        if os.path.exists(spec["source"]):
            shutil.copyfile(spec["source"], path)
        else:
            # A sample not made yet (the WEBM sticker until the helper's agent adds one): a few
            # bytes that no renderer takes, so the still picture stays.
            with open(path, "wb") as out:
                out.write(b"not yet a sample: " + spec["name"].encode())
    else:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as out:
            out.write(spec.get("content") or os.urandom(min(spec["size"], 256 * 1024)))
    return os.path.getsize(path)


@handler("downloadFile")
def download_file(client, request: dict) -> None:
    file_id = int(request.get("file_id", 0))
    spec = FILES.get(file_id)
    path = file_path(client, file_id, spec)
    size = spec["size"] if spec else 9_000
    if os.path.exists(path):
        done = file_object(file_id, path=path, size=os.path.getsize(path))
        client.answer(request, done)
        client.send({"@type": "updateFile", "file": done})
        return
    key = (client.key, file_id)
    # A file only a click downloads (part 5: a photo's full size in the profile's media, to see
    # the viewer's spinner and its swap): nothing for a lower priority, slowly for a click's.
    click_only = bool(spec and spec.get("click_only"))
    if click_only and int(request.get("priority") or 1) < 32:
        client.answer(request, file_object(file_id, size=size))
        return
    client.answer(request, file_object(file_id, size=size, downloading=True))
    if key in DOWNLOADS:
        return
    seconds = float(os.environ.get("FAKE_DOWNLOAD_SECONDS", "1"))
    steps = 4 if size > 100_000 else 1
    seconds = seconds if size > 100_000 else min(seconds, 0.2)
    if click_only:
        seconds, steps = 2.0, 4

    def step(n: int) -> None:
        if key not in DOWNLOADS:
            return
        if n < steps:
            client.send({"@type": "updateFile", "file": file_object(file_id, size=size, downloaded=size * n // steps,
                                                                    downloading=True)})
            DOWNLOADS[key] = threading.Timer(seconds / steps, step, [n + 1])
            DOWNLOADS[key].start()
            return
        DOWNLOADS.pop(key, None)
        real = write_file(path, file_id, spec)
        client.send({"@type": "updateFile", "file": file_object(file_id, path=path, size=real)})

    DOWNLOADS[key] = threading.Timer(seconds / steps, step, [1])
    DOWNLOADS[key].start()


@handler("cancelDownloadFile")
def cancel_download_file(client, request: dict) -> None:
    file_id = int(request.get("file_id", 0))
    timer = DOWNLOADS.pop((client.key, file_id), None)
    if timer:
        timer.cancel()
    client.ok(request)
    spec = FILES.get(file_id)
    client.send({"@type": "updateFile", "file": file_object(file_id, size=spec["size"] if spec else 9_000)})


# --- What the list's fake answers, when it doesn't (yet) ---

def _fallback(kind: str):
    def register(function):
        if kind not in fake_world.HANDLERS:
            fake_world.HANDLERS[kind] = function
        return function
    return register


@_fallback("getChat")
def get_chat(client, request: dict) -> None:
    chat_id = int(request["chat_id"])
    chats = fake_world.chats_of(client.user or {"id": 0})
    if chat_id not in chats:
        return client.error(request, 400, "Chat not found")
    if chat_id == me_of(client):
        fill_saved(chat_id)
    client.answer(request, fake_world.with_last_message(chats[chat_id], client.user or {"id": 0}))


@_fallback("getUser")
def get_user(client, request: dict) -> None:
    user_id = int(request["user_id"])
    found = fake_world.USERS.get(user_id) or (client.user if user_id == me_of(client) else None)
    if found is None:
        return client.error(request, 404, "User not found")
    client.answer(request, found)


@_fallback("getBasicGroup")
def get_basic_group(client, request: dict) -> None:
    group = fake_world.BASIC_GROUPS.get(int(request["basic_group_id"]))
    if group is None:
        return client.error(request, 400, "Group not found")
    client.answer(request, group)


@_fallback("getSupergroup")
def get_supergroup(client, request: dict) -> None:
    group = fake_world.SUPERGROUPS.get(int(request["supergroup_id"]))
    if group is None:
        return client.error(request, 400, "Supergroup not found")
    client.answer(request, group)


# --- Someone typing ---

def typing_loop(every: float) -> None:
    while True:
        time.sleep(every)
        for client_key, chat_id in list(OPEN):
            client = _clients().get(client_key)
            if client is None:
                continue
            who = chat_id if chat_id > 0 else random.choice(TEAM)
            if chat_id > 0 and chat_id not in fake_world.USERS:
                continue
            for _ in range(2):
                client.send({"@type": "updateChatAction", "chat_id": chat_id, "message_thread_id": 0,
                             "sender_id": {"@type": "messageSenderUser", "user_id": who},
                             "action": {"@type": "chatActionTyping"}})
                time.sleep(min(4.0, every / 2))
            client.send({"@type": "updateChatAction", "chat_id": chat_id, "message_thread_id": 0,
                         "sender_id": {"@type": "messageSenderUser", "user_id": who},
                         "action": {"@type": "chatActionCancel"}})


def _clients() -> dict:
    import sys
    helper = sys.modules.get("__main__")
    return getattr(helper, "CLIENTS", {})


if os.environ.get("FAKE_TYPING"):
    threading.Thread(target=typing_loop, args=(float(os.environ["FAKE_TYPING"]),), daemon=True).start()
