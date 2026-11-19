"""The fake helper's channels, bots and forums (part 5).

- "Flux News" (the user a member): 34 posts by the channel with their views, one signed, one a
  photo, the last ten unread; `getChatSponsoredMessages` gives two advertisements — the first with a photo and
  `additional_info` (shown), the second a recommendation; `clickChatSponsoredMessage` is answered,
  `reportChatSponsoredMessage` asks for an option (three), then says done.
- "Rust Weekly · новости языка и экосистемы": a public channel the user isn't in (found by Add
  Chat's search); `joinChat` makes them a member (`updateSupergroup`).
- "Flux · community": a forum — General and three topics; its messages carry `topic_id`, and a
  message sent with one keeps it (`fake_feed.send_one` copies it).
- "Weather Bot" is listable (part 3 kept bots out).
"""

from __future__ import annotations

import fake_feed
import fake_world
from fake_feed import BLUE, TEAL, VIOLET, photo
from fake_world import NOW, add, handler, message, text

NEWS = -1000000001002
RUST = -1000000001005
FORUM = -1000000001006

# --- The world's additions ---

fake_world.SUPERGROUPS[1005] = fake_world.supergroup(1005, 48_210, channel=True, username="rustweekly",
                                                     status="chatMemberStatusLeft")
fake_world.SUPERGROUPS[1006] = dict(fake_world.supergroup(1006, 1_342, username="flux_community"),
                                    is_forum=True)
fake_world.SUPERGROUPS[1002]["sign_messages"] = True
# Ten posts unread: the feed opens above them, the advertisement waits for the scroll to the end.
fake_world.CHATS[NEWS]["unread_count"] = 10
fake_world.CHATS[RUST] = fake_world.chat(
    RUST, {"@type": "chatTypeSupergroup", "supergroup_id": 1005, "is_channel": True},
    "Rust Weekly · новости языка и экосистемы", 1, None, 0)
fake_world.CHATS[FORUM] = fake_world.chat(
    FORUM, {"@type": "chatTypeSupergroup", "supergroup_id": 1006, "is_channel": False},
    "Flux · community", 4, None, 835, unread=2)

# The forum's topics: (id, name, icon color).
TOPICS = [(1, "General", 0x6FB9F0), (2, "Releases", 0x8EEE98), (3, "Design and UI", 0xFF93B2),
          (4, "Bugs", 0xFB6F5F)]

POSTS = [
    "Flux 0.5 is out: plugins get conversations, galleries and media.",
    "How we keep the editor at 0 % CPU when nothing moves — a long read.",
    "Windows builds are signed now.",
    "The plugin catalog has 40 plugins.",
    "Themes: three new dark ones.",
    "Release notes for 0.5.1: fixes for the terminal and the git log.",
    "Tip: ⌘⇧A finds any action.",
    "We are hiring a Rust developer.",
]


def channel_post(chat_id: int, value: dict, date: int, views: int, signature: str = "") -> dict:
    """A channel's post: sent by the channel, with its views."""
    return add(chat_id, message(chat_id, chat_id, value, date, is_channel_post=True,
                                interaction_info={"@type": "messageInteractionInfo", "view_count": views,
                                                  "forward_count": views // 50, "reply_info": None,
                                                  "reactions": None},
                                author_signature=signature))


def fill_news() -> None:
    fake_world.MESSAGES[NEWS] = []
    t = NOW - 86_400 * 6
    for i in range(34):
        views = 950 + i * 3_517 if i != 33 else 12_480
        value = text(POSTS[i % len(POSTS)])
        signature = ""
        if i == 30:
            value = photo(BLUE, "blue", 1600, 1000, caption="The new welcome screen.")
        if i == 32:
            signature = "Егор Агеев"
            value = text("Thanks to everyone who tested the Windows build — the installer is per-user now.")
        channel_post(NEWS, value, t + i * 14_400, views, signature)


def fill_rust() -> None:
    fake_world.MESSAGES[RUST] = []
    t = NOW - 86_400 * 3
    for i, line in enumerate(["Rust 1.95 is out.", "This week in Rust #612.", "async closures, explained."]):
        channel_post(RUST, text(line), t + i * 20_000, 20_000 + i * 1_000)


def fill_forum() -> None:
    fake_world.MESSAGES[FORUM] = []
    t = NOW - 86_400
    lines = [
        (1, 101, "Welcome to the community!"),
        (2, 102, "0.5.1 is out — the changelog is pinned."),
        (3, 107, "Here is the new sidebar mockup."),
        (4, 103, "The terminal freezes after sleep on my Mac."),
        (3, 101, "Looks great, maybe less padding?"),
        (4, 104, "Same here, 0.5.0 on Sonoma."),
        (1, 109, "Hi all"),
        (2, 101, "Windows ARM64 builds coming next week."),
        (4, 102, "Fixed in 0.5.2, can you check?"),
    ]
    for i, (topic, sender, line) in enumerate(lines):
        add(FORUM, message(FORUM, sender, text(line), t + i * 3_000,
                           topic_id={"@type": "messageTopicForum", "forum_topic_id": topic}))


fill_news()
fill_rust()
fill_forum()


def topic_info(topic: int, name: str, color: int) -> dict:
    return {"@type": "forumTopicInfo", "chat_id": FORUM, "forum_topic_id": topic, "name": name,
            "icon": {"@type": "forumTopicIcon", "color": color, "custom_emoji_id": "0"},
            "creation_date": NOW - 86_400 * 30, "creator_id": {"@type": "messageSenderUser", "user_id": 101},
            "is_general": topic == 1, "is_outgoing": False, "is_closed": False, "is_hidden": False,
            "is_name_implicit": False}


@handler("getForumTopics")
def get_forum_topics(client, request: dict) -> None:
    if int(request.get("chat_id", 0)) != FORUM:
        return client.error(request, 400, "CHANNEL_FORUM_MISSING")
    topics = [{"@type": "forumTopic", "info": topic_info(*topic), "last_message": None, "order": "0",
               "is_pinned": False, "unread_count": 0, "last_read_inbox_message_id": 0,
               "last_read_outbox_message_id": 0, "unread_mention_count": 0, "unread_reaction_count": 0,
               "notification_settings": None, "draft_message": None} for topic in TOPICS]
    client.answer(request, {"@type": "forumTopics", "total_count": len(topics), "topics": topics,
                            "next_offset_date": 0, "next_offset_message_id": 0, "next_offset_forum_topic_id": 0})


# --- Advertisements ---

AD_PHOTO = photo(TEAL, "teal", 1200, 675)
ADS = [
    {"@type": "sponsoredMessage", "message_id": 9_000_000_001, "is_recommended": False, "can_be_reported": True,
     "content": dict(AD_PHOTO, caption=fake_feed.styled(
         "Rocket VPN — fast and private. 7 days free, no card needed.")),
     "sponsor": {"@type": "advertisementSponsor", "url": "https://rocket-vpn.example/flux",
                 "photo": photo(VIOLET, "violet", 320, 320)["photo"], "info": "ООО «Ракета», ИНН 7700000000"},
     "title": "Rocket VPN", "button_text": "Попробовать", "accent_color_id": 5,
     "background_custom_emoji_id": "0", "additional_info": "erid: 2VtzqwXyZ1"},
    {"@type": "sponsoredMessage", "message_id": 9_000_000_002, "is_recommended": True, "can_be_reported": False,
     "content": text("Rust Weekly: the language's news every Monday."),
     "sponsor": {"@type": "advertisementSponsor", "url": "https://t.me/rustweekly", "photo": None, "info": ""},
     "title": "Rust Weekly", "button_text": "Открыть канал", "accent_color_id": 1,
     "background_custom_emoji_id": "0", "additional_info": ""},
]

REPORT_OPTIONS = [("AQ==", "Мне не нравится эта реклама"), ("Ag==", "Незаконный товар или услуга"),
                  ("Aw==", "Мошенничество")]


@handler("getChatSponsoredMessages")
def get_chat_sponsored_messages(client, request: dict) -> None:
    chat_id = int(request.get("chat_id", 0))
    ads = ADS if chat_id in (NEWS, 105) else []
    client.answer(request, {"@type": "sponsoredMessages", "messages": ads, "messages_between": 0})


@handler("clickChatSponsoredMessage")
def click_chat_sponsored_message(client, request: dict) -> None:
    client.ok(request)


@handler("reportChatSponsoredMessage")
def report_chat_sponsored_message(client, request: dict) -> None:
    if not request.get("option_id"):
        return client.answer(request, {
            "@type": "reportSponsoredResultOptionRequired", "title": "Почему вы жалуетесь на эту рекламу?",
            "options": [{"@type": "reportOption", "id": key, "text": label} for key, label in REPORT_OPTIONS]})
    client.answer(request, {"@type": "reportSponsoredResultOk"})


# --- Joining ---

@handler("joinChat")
def join_chat(client, request: dict) -> None:
    chat_id = int(request.get("chat_id", 0))
    chat_obj = fake_world.CHATS.get(chat_id) or {}
    group_id = (chat_obj.get("type") or {}).get("supergroup_id")
    group = fake_world.SUPERGROUPS.get(group_id) if group_id else None
    if group is None:
        return client.error(request, 400, "CHANNEL_INVALID")
    group["status"] = {"@type": "chatMemberStatusMember"}
    group["member_count"] += 1
    client.ok(request)
    client.send({"@type": "updateSupergroup", "supergroup": group})
