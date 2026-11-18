"""The fake helper's profiles (part 3+, agent P): what the profile dialog asks TDLib —
`getUserFullInfo` (a bio for some people), `getBasicGroupFullInfo` (the description and the
members), `getSupergroupFullInfo`, `getSupergroupMembers` (recent, administrators, a search),
`searchChatMembers`, `getUser` of the generated people, `createPrivateChat`,
`setChatNotificationSettings` (→ `updateChatNotificationSettings`).

- "Backend · team" has its five people and the account (Anna the owner, Dima an admin "backend
  lead"); "Design review" (a basic group) Anna, Masha (admin) and the account; "Flood" 240
  generated people (ids 5001–5240, every third with a photo, statuses of every kind; 5001 the
  owner, 5002–5003 admins); "Old project" (left) doesn't show its members.
- Anna's number is visible (her `getUser`), the others' aren't; Anna and Dima have a bio.
- Before a page of members, the people in it come as `updateUser`, as TDLib sends them.
- "Backend · team" gets four months of media before its history: photos and videos, files,
  links (with and without a preview), voice messages — so the gallery has several months; "Design
  review" a few photos more. They take message ids below the history's (TDLib's ids grow with
  time).
"""

from __future__ import annotations

import fake_feed
import fake_world
from fake_world import NOW, handler

BACKEND = fake_feed.BACKEND
DESIGN = fake_feed.DESIGN
FLOOD = fake_feed.FLOOD

# --- People ---

FIRST = ["Alex", "Ivan", "Olga", "Nina", "Pavel", "Vera", "Igor", "Lena", "Sergey", "Katya",
         "Max", "Yulia", "Roman", "Dasha", "Artem", "Sveta"]
LAST = ["Orlov", "Belova", "Kozlov", "Sokolova", "Novikov", "Morozova", "Volkov", "Lebedeva",
        "Egorov", "Pavlova", "Zaitsev", "Fedorova", "Gromov", "Titova", "Popov"]


def status_of(index: int) -> dict:
    kind = index % 6
    if kind == 0:
        return {"@type": "userStatusOnline", "expires": NOW + 86_400}
    if kind == 1:
        return {"@type": "userStatusOffline", "was_online": NOW - 600 * (index % 40 + 1)}
    if kind == 2:
        return {"@type": "userStatusRecently"}
    if kind == 3:
        return {"@type": "userStatusLastWeek"}
    if kind == 4:
        return {"@type": "userStatusOffline", "was_online": NOW - 86_400 * (index % 9 + 2)}
    return {"@type": "userStatusLastMonth"}


PEOPLE: dict[int, dict] = {}
for _index in range(240):
    _id = 5001 + _index
    _first = FIRST[_index % len(FIRST)]
    _last = LAST[(_index * 7) % len(LAST)]
    _name = f"{_first.lower()}_{_last.lower()}{_index}" if _index % 4 == 0 else None
    PEOPLE[_id] = fake_world.user(_id, _first, _last, _name, _index % 7, photo=_index % 3 == 0,
                                  status=status_of(_index), contact=False)

PHONES = {101: "79001234567"}
BIOS = {
    101: "Designer at Flux: mockups, icons, motion. Write about the screens here.",
    102: "Backend, Rust, coffee. On call every other week.",
}


def person(client, user_id: int) -> dict | None:
    if client.user and client.user["id"] == user_id:
        return client.user
    found = PEOPLE.get(user_id) or fake_world.USERS.get(user_id)
    if found and user_id in PHONES:
        found = dict(found, phone_number=PHONES[user_id])
    return found


_previous_get_user = fake_world.HANDLERS.get("getUser")


@handler("getUser")
def get_user(client, request: dict) -> None:
    user_id = int(request.get("user_id", 0))
    found = person(client, user_id)
    if found is None and _previous_get_user:
        return _previous_get_user(client, request)
    if found is None:
        return client.error(request, 404, "User not found")
    client.answer(request, found)


@handler("getUserFullInfo")
def get_user_full_info(client, request: dict) -> None:
    user_id = int(request.get("user_id", 0))
    if person(client, user_id) is None:
        return client.error(request, 404, "User not found")
    bio = BIOS.get(user_id, "")
    client.answer(request, {"@type": "userFullInfo", "personal_photo": None, "photo": None,
                            "public_photo": None, "block_list": None, "can_be_called": True,
                            "supports_video_calls": True, "has_private_calls": False,
                            "has_private_forwards": False, "has_restricted_voice_and_video_note_messages": False,
                            "has_posted_to_profile_stories": False, "set_chat_background": False,
                            "bio": {"@type": "formattedText", "text": bio, "entities": []},
                            "birthdate": None, "personal_chat_id": 0, "group_in_common_count": 1,
                            "bot_info": None})


# --- Groups and their members ---

def member(user_id: int, status: str = "chatMemberStatusMember", title: str = "") -> dict:
    status_obj: dict = {"@type": status}
    if status == "chatMemberStatusCreator":
        status_obj.update({"custom_title": title, "is_anonymous": False, "is_member": True})
    elif status == "chatMemberStatusAdministrator":
        status_obj.update({"custom_title": title, "can_be_edited": False, "rights": {}})
    else:
        status_obj["member_until_date"] = 0
    return {"@type": "chatMember", "member_id": {"@type": "messageSenderUser", "user_id": user_id},
            "inviter_user_id": 0, "joined_chat_date": NOW - 86_400 * 30, "status": status_obj}


def members_of(client, chat_id: int) -> list[dict] | None:
    """A group's members, the account among them; none — the list isn't shown."""
    me = client.user["id"] if client.user else 0
    if chat_id == BACKEND:
        return [member(101, "chatMemberStatusCreator"),
                member(102, "chatMemberStatusAdministrator", "backend lead"),
                member(103), member(104), member(107), member(me)]
    if chat_id == DESIGN:
        return [member(101, "chatMemberStatusCreator"), member(107, "chatMemberStatusAdministrator"),
                member(me)]
    if chat_id == FLOOD:
        found = [member(5001, "chatMemberStatusCreator"), member(5002, "chatMemberStatusAdministrator"),
                 member(5003, "chatMemberStatusAdministrator", "moderator")]
        found += [member(user_id) for user_id in range(5004, 5241)]
        return found
    return None


def chat_of_supergroup(group_id: int) -> int:
    return -1_000_000_000_000 - group_id


def matching(client, members: list[dict], query: str) -> list[dict]:
    query = query.strip().lower().lstrip("@")
    if not query:
        return members
    found = []
    for item in members:
        user = person(client, item["member_id"]["user_id"]) or {}
        names = [user.get("first_name", ""), user.get("last_name", "")]
        names += (user.get("usernames") or {}).get("active_usernames") or []
        if any(name.lower().startswith(query) for name in names if name):
            found.append(item)
    return found


def answer_members(client, request: dict, members: list[dict]) -> None:
    """The people first (TDLib sends `updateUser` before an answer names them), then the page."""
    for item in members:
        user_id = item["member_id"]["user_id"]
        if user_id in PEOPLE:
            client.send({"@type": "updateUser", "user": PEOPLE[user_id]})
    client.answer(request, {"@type": "chatMembers", "total_count": len(members), "members": members})


@handler("getBasicGroupFullInfo")
def get_basic_group_full_info(client, request: dict) -> None:
    group_id = int(request.get("basic_group_id", 0))
    members = members_of(client, -group_id)
    if group_id not in fake_world.BASIC_GROUPS or members is None:
        return client.error(request, 400, "Group not found")
    client.answer(request, {"@type": "basicGroupFullInfo", "photo": None,
                            "description": "Mockups and reviews of the new screens.",
                            "creator_user_id": 101, "members": members, "can_hide_members": False,
                            "can_toggle_aggressive_anti_spam": False, "invite_link": None,
                            "bot_commands": []})


DESCRIPTIONS = {
    1001: "The backend team: releases, reviews, who is on call.",
    1003: "Anything goes. Be nice.",
    1004: "The old project, archived.",
}


@handler("getSupergroupFullInfo")
def get_supergroup_full_info(client, request: dict) -> None:
    group_id = int(request.get("supergroup_id", 0))
    group = fake_world.SUPERGROUPS.get(group_id)
    if group is None:
        return client.error(request, 400, "Supergroup not found")
    members = members_of(client, chat_of_supergroup(group_id))
    count = len(members) if members is not None else group["member_count"]
    admins = len([m for m in members or [] if m["status"]["@type"] != "chatMemberStatusMember"])
    client.answer(request, {"@type": "supergroupFullInfo", "photo": None,
                            "description": DESCRIPTIONS.get(group_id, ""), "member_count": count,
                            "administrator_count": admins, "restricted_count": 0, "banned_count": 0,
                            "linked_chat_id": 0, "slow_mode_delay": 0, "can_get_members": members is not None,
                            "has_hidden_members": False, "can_hide_members": False,
                            "can_set_sticker_set": False, "can_set_location": False,
                            "can_get_statistics": False, "is_all_history_available": True,
                            "invite_link": None, "bot_commands": [], "upgraded_from_basic_group_id": 0,
                            "upgraded_from_max_message_id": 0})


@handler("getSupergroupMembers")
def get_supergroup_members(client, request: dict) -> None:
    group_id = int(request.get("supergroup_id", 0))
    members = members_of(client, chat_of_supergroup(group_id))
    if members is None:
        return client.error(request, 400, "CHAT_ADMIN_REQUIRED")
    kind = (request.get("filter") or {}).get("@type", "supergroupMembersFilterRecent")
    if kind == "supergroupMembersFilterAdministrators":
        members = [m for m in members if m["status"]["@type"] != "chatMemberStatusMember"]
    elif kind == "supergroupMembersFilterSearch":
        members = matching(client, members, (request.get("filter") or {}).get("query", ""))
    offset = int(request.get("offset") or 0)
    limit = max(1, min(200, int(request.get("limit") or 200)))
    answer_members(client, request, members[offset:offset + limit])


@handler("searchChatMembers")
def search_chat_members(client, request: dict) -> None:
    chat_id = int(request.get("chat_id", 0))
    members = members_of(client, chat_id)
    if members is None:
        return client.error(request, 400, "CHAT_ADMIN_REQUIRED")
    limit = max(1, min(200, int(request.get("limit") or 50)))
    answer_members(client, request, matching(client, members, request.get("query") or "")[:limit])


# --- A person's chat, muting ---

@handler("createPrivateChat")
def create_private_chat(client, request: dict) -> None:
    user_id = int(request.get("user_id", 0))
    me = client.user or {"id": 0}
    chats = fake_world.chats_of(me)
    if user_id in chats:
        return client.answer(request, fake_world.with_last_message(chats[user_id], me))
    user = person(client, user_id)
    if user is None:
        return client.error(request, 400, "User not found")
    name = f"{user['first_name']} {user['last_name']}".strip()
    photo = 1000 + user_id if user.get("profile_photo") else None
    obj = fake_world.chat(user_id, fake_world.private(user_id), name, user.get("accent_color_id", 0) % 7,
                          photo, 0)
    # A chat without messages isn't in the main list.
    obj["positions"] = []
    with fake_world.LOCK:
        fake_world.CHATS[user_id] = obj
    client.send({"@type": "updateNewChat", "chat": obj})
    client.answer(request, obj)


@handler("setChatNotificationSettings")
def set_chat_notification_settings(client, request: dict) -> None:
    chat_id = int(request.get("chat_id", 0))
    settings = request.get("notification_settings") or {}
    chats = fake_world.chats_of(client.user or {"id": 0})
    if chat_id not in chats:
        return client.error(request, 400, "Chat not found")
    kept = {"@type": "chatNotificationSettings",
            "use_default_mute_for": bool(settings.get("use_default_mute_for")),
            "mute_for": int(settings.get("mute_for") or 0)}
    with fake_world.LOCK:
        if chat_id in fake_world.CHATS:
            fake_world.CHATS[chat_id] = dict(fake_world.CHATS[chat_id], notification_settings=kept)
    client.ok(request)
    client.send({"@type": "updateChatNotificationSettings", "chat_id": chat_id, "notification_settings": kept})


# --- Four months of media ---

COLORS = [(fake_feed.BLUE, "blue"), (fake_feed.WARM, "warm"), (fake_feed.GREEN, "green"),
          (fake_feed.VIOLET, "violet"), (fake_feed.TEAL, "teal")]


def document_of(name: str, size: int) -> dict:
    file = fake_feed.new_file("bytes", name, size)
    return {"@type": "messageDocument", "caption": fake_feed.styled(""),
            "document": {"@type": "document", "file_name": name, "mime_type": "application/octet-stream",
                         "minithumbnail": None, "thumbnail": None, "document": file}}


def link_text(value: str, url: str) -> dict:
    return fake_feed.rich_text(value, fake_feed.entity(value, url, "textEntityTypeUrl"))


def prepend(chat_id: int, items: list[tuple[int, dict, int]]) -> None:
    """Messages before the chat's history: (sender, content, date), oldest first; their ids are
    below the history's."""
    with fake_world.LOCK:
        history = fake_world.MESSAGES.get(chat_id) or []
        first = history[0]["id"] if history else fake_world.STEP
        step = max(1, first // (len(items) + 1))
        made = [fake_world.message(chat_id, sender, content, date, message_id=step * (index + 1),
                                   outgoing=sender == 0)
                for index, (sender, content, date) in enumerate(sorted(items, key=lambda item: item[2]))]
        fake_world.MESSAGES[chat_id] = made + history


def fill_backend_media() -> None:
    team = fake_feed.TEAM
    items: list[tuple[int, dict, int]] = []
    for index in range(30):
        color, mini = COLORS[index % len(COLORS)]
        date = NOW - 86_400 * (124 - index * 4) - 3_600 * (index % 5)
        sender = team[index % len(team)]
        if index % 5 == 4:
            items.append((sender, fake_feed.video(20 + index * 7, color, mini), date))
        else:
            wide = index % 3 != 1
            items.append((sender, fake_feed.photo(color, mini, 1600 if wide else 1200, 1200 if wide else 1600),
                          date))
    files = [("design-spec.pdf", 2_400_000), ("budget.xlsx", 48_000), ("release-notes.md", 3_200),
             ("schema.sql", 18_500), ("logo.svg", 9_100), ("backup.tar.gz", 310_000_000),
             ("contract.docx", 120_000), ("slides.key", 14_800_000)]
    for index, (name, size) in enumerate(files):
        items.append((team[(index + 1) % len(team)], document_of(name, size),
                      NOW - 86_400 * (110 - index * 13) + 1_800))
    links = [
        ("The new map provider: https://maps.example.com/pricing", "https://maps.example.com/pricing", None),
        ("Benchmarks https://bench.example.org/flux", "https://bench.example.org/flux", None),
        ("Read this before the release https://example.com/blog/checklist", "https://example.com/blog/checklist",
         ("Example Blog", "The release checklist", "Ten things to check before you ship.")),
        ("https://github.com/example/flux/pull/142", "https://github.com/example/flux/pull/142",
         ("GitHub", "Faster scrolling by Dima · Pull Request #142", "Measure first, then optimize.")),
    ]
    for index, (value, url, preview) in enumerate(links):
        content = link_text(value, url)
        if preview:
            site, title, description = preview
            content["link_preview"] = fake_feed.link_preview(url, site, title, description, fake_feed.WARM, "warm")
        items.append((team[index % len(team)], content, NOW - 86_400 * (100 - index * 21) + 600))
    for index, seconds in enumerate([12, 47, 5, 95, 31]):
        items.append((team[(index + 2) % len(team)], fake_feed.voice(seconds), NOW - 86_400 * (90 - index * 17)))
    prepend(BACKEND, items)


def fill_design_media() -> None:
    items = []
    for index in range(8):
        color, mini = COLORS[(index + 2) % len(COLORS)]
        items.append(([101, 107][index % 2], fake_feed.photo(color, mini), NOW - 86_400 * (40 - index * 3)))
    prepend(DESIGN, items)


fill_backend_media()
fill_design_media()
