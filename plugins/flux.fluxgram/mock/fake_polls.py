"""The fake helper's polls and reactions (part 5).

- "Team polls" (supergroup 1005, chat -1000000001005): Anna's text with reactions (🔥 2, 👍 1);
  Anna's anonymous poll of one choice ("Where do we meet on Friday?"); the user's own public poll
  of several choices (Stop Poll is theirs); Dima's anonymous quiz with its explanation; Masha's
  closed poll (final results); Petya's line last. Its reactions are some: nine emoji (more than
  the row shows, so ⌄ opens the grid).
- "Anna Smirnova" (101) allows all reactions: `updateActiveEmojiReactions` at the start lists
  Telegram's (24 of them).
- `setPollAnswer` votes (none — the vote taken back; a quiz once), `stopPoll` closes a poll: the
  poll comes again in `updateMessageContent`, its shares counted as TDLib does — shown once the
  user voted or it closed, a quiz's answer and explanation with them.
- `addMessageReaction` / `removeMessageReaction` change the user's reaction: the counts come
  again in `updateMessageInteractionInfo`.
"""

from __future__ import annotations

import fake_feed
import fake_world
from fake_world import NOW, handler, on_ready, text

POLLS = -1000000001005
ANNA = 101

fake_world.SUPERGROUPS[1005] = fake_world.supergroup(1005, 8)
fake_world.CHATS[POLLS] = fake_world.chat(
    POLLS, {"@type": "chatTypeSupergroup", "supergroup_id": 1005, "is_channel": False},
    "Team polls", 4, None, 775)
fake_world.CHATS[POLLS]["available_reactions"] = {
    "@type": "chatAvailableReactionsSome", "max_reaction_count": 11,
    "reactions": [{"@type": "reactionTypeEmoji", "emoji": emoji}
                  for emoji in ["👍", "❤", "🔥", "🎉", "😁", "🤔", "😢", "👏", "🙏"]]}
fake_world.CHATS[ANNA]["available_reactions"] = {"@type": "chatAvailableReactionsAll",
                                                 "max_reaction_count": 11}

ACTIVE = ["👍", "👎", "❤", "🔥", "🥰", "👏", "😁", "🤔", "🤯", "😱", "🤬", "😢", "🎉", "🤩", "🙏",
          "👌", "🕊", "🤡", "🥱", "😍", "🐳", "💯", "🤣", "⚡"]

# The quizzes' answers, by (chat, message): (the right option, the explanation) — TDLib tells
# them only once the user answered or the quiz closed.
ANSWERS: dict[tuple[int, int], tuple[int, str]] = {}


def formatted(value: str) -> dict:
    return {"@type": "formattedText", "text": value, "entities": []}


def option(value: str, voters: int) -> dict:
    """An option: `voters` — the others' votes (the user's own is added when chosen)."""
    return {"@type": "pollOption", "text": formatted(value), "voter_count": voters, "vote_percentage": 0,
            "is_chosen": False, "is_being_chosen": False}


def poll(question: str, options: list[dict], kind: dict, anonymous: bool = True,
         closed: bool = False) -> dict:
    voters = sum(item["voter_count"] for item in options)
    return {"@type": "messagePoll", "poll": {
        "@type": "poll", "id": str(abs(hash(question)) % 10**12), "question": formatted(question),
        "options": options, "total_voter_count": voters, "recent_voter_ids": [],
        "is_anonymous": anonymous, "type": kind, "open_period": 0, "close_date": 0,
        "is_closed": closed}}


def regular(multiple: bool = False) -> dict:
    return {"@type": "pollTypeRegular", "allow_multiple_answers": multiple}


def quiz() -> dict:
    return {"@type": "pollTypeQuiz", "correct_option_id": -1, "explanation": formatted("")}


def counted(chat_id: int, obj: dict) -> None:
    """The poll's shares and a quiz's answer, as TDLib shows them: once voted or closed."""
    shown = obj["content"]["poll"]
    chosen = any(item["is_chosen"] for item in shown["options"])
    answered = chosen or shown["is_closed"]
    total = shown["total_voter_count"]
    for item in shown["options"]:
        item["vote_percentage"] = round(item["voter_count"] * 100 / total) if answered and total else 0
    if shown["type"]["@type"] == "pollTypeQuiz":
        right, why = ANSWERS.get((chat_id, obj["id"]), (-1, ""))
        shown["type"]["correct_option_id"] = right if answered else -1
        shown["type"]["explanation"] = formatted(why if answered else "")


def fill_polls() -> None:
    chat = POLLS
    fake_world.MESSAGES[chat] = []
    t = NOW - 86_400
    say = fake_feed.say
    say(chat, ANNA, {"@type": "messageSupergroupChatCreate", "title": "Team polls"}, t)
    say(chat, ANNA, text("Let's settle the offsite this week 🙂"), t + 600,
        interaction_info=fake_feed.reactions(("🔥", 2, False), ("👍", 1, False)))
    say(chat, ANNA, poll("Where do we meet on Friday?",
                         [option("The office", 3), option("The café by the river", 2), option("Online", 1)],
                         regular()), t + 900)
    say(chat, 0, poll("What should the offsite include?",
                      [option("Hiking", 2), option("Board games", 3), option("A talk on Rust", 4),
                       option("Karaoke", 1)], regular(multiple=True), anonymous=False), t + 1200)
    quiz_message = say(chat, 102, poll("Which keeps an idle editor at 0 % CPU?",
                                       [option("Redrawing every frame", 1),
                                        option("Frames only for what plays", 4),
                                        option("A faster graphics card", 0)], quiz()), t + 1500)
    ANSWERS[(chat, quiz_message["id"])] = (1, "Nothing on screen changes — so nothing needs drawing.")
    say(chat, 107, poll("Lunch on Thursday?", [option("Pizza", 4), option("Sushi", 3), option("Pho", 2)],
                        regular(), closed=True), t + 1800)
    say(chat, 103, text("Vote please — the bus is booked on Monday."), t + 2100)
    for obj in fake_world.MESSAGES[chat]:
        if obj["content"]["@type"] == "messagePoll":
            counted(chat, obj)


fill_polls()


@on_ready
def active_reactions(client) -> None:
    client.send({"@type": "updateActiveEmojiReactions", "emojis": ACTIVE})


def changed(client, chat_id: int, obj: dict) -> None:
    client.send({"@type": "updateMessageContent", "chat_id": chat_id, "message_id": obj["id"],
                 "new_content": obj["content"]})


@handler("setPollAnswer")
def set_poll_answer(client, request: dict) -> None:
    chat_id, message_id = int(request["chat_id"]), int(request["message_id"])
    with fake_world.LOCK:
        obj = fake_feed.find(client, chat_id, message_id)
        if obj is None or obj["content"]["@type"] != "messagePoll":
            return client.error(request, 400, "Message not found")
        shown = obj["content"]["poll"]
        chosen = [int(index) for index in request.get("option_ids") or []]
        voted = any(item["is_chosen"] for item in shown["options"])
        if shown["is_closed"]:
            return client.error(request, 400, "POLL_CLOSED")
        if shown["type"]["@type"] == "pollTypeQuiz" and voted:
            return client.error(request, 400, "Can't change a quiz answer")
        if len(chosen) > 1 and not shown["type"].get("allow_multiple_answers"):
            return client.error(request, 400, "POLL_OPTION_INVALID")
        for index, item in enumerate(shown["options"]):
            mine = index in chosen
            item["voter_count"] += int(mine) - int(item["is_chosen"])
            item["is_chosen"] = mine
        shown["total_voter_count"] += int(bool(chosen)) - int(voted)
        counted(chat_id, obj)
    client.ok(request)
    changed(client, chat_id, obj)


@handler("stopPoll")
def stop_poll(client, request: dict) -> None:
    chat_id, message_id = int(request["chat_id"]), int(request["message_id"])
    with fake_world.LOCK:
        obj = fake_feed.find(client, chat_id, message_id)
        if obj is None or obj["content"]["@type"] != "messagePoll":
            return client.error(request, 400, "Message not found")
        obj["content"]["poll"]["is_closed"] = True
        counted(chat_id, obj)
    client.ok(request)
    changed(client, chat_id, obj)


def react(client, request: dict, put: bool) -> None:
    chat_id, message_id = int(request["chat_id"]), int(request["message_id"])
    emoji = (request.get("reaction_type") or {}).get("emoji", "")
    with fake_world.LOCK:
        obj = fake_feed.find(client, chat_id, message_id)
        if obj is None or not emoji:
            return client.error(request, 400, "REACTION_INVALID")
        info = obj.get("interaction_info") or fake_feed.reactions()
        listed = info["reactions"]["reactions"]
        found = next((item for item in listed if item["type"].get("emoji") == emoji), None)
        if put and found is None:
            found = fake_feed.reactions((emoji, 0, False))["reactions"]["reactions"][0]
            listed.append(found)
        if found is not None and found["is_chosen"] != put:
            found["total_count"] += 1 if put else -1
            found["is_chosen"] = put
        info["reactions"]["reactions"] = [item for item in listed if item["total_count"] > 0]
        obj["interaction_info"] = info
    client.ok(request)
    client.send({"@type": "updateMessageInteractionInfo", "chat_id": chat_id, "message_id": message_id,
                 "interaction_info": info})


@handler("addMessageReaction")
def add_message_reaction(client, request: dict) -> None:
    react(client, request, True)


@handler("removeMessageReaction")
def remove_message_reaction(client, request: dict) -> None:
    react(client, request, False)
