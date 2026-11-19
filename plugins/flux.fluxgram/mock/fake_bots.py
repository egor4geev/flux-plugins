"""The fake helper's bots (part 5).

- "Weather Bot" (105): its history ends with a message with an inline keyboard — two callback
  buttons ("Now" answers a text, "Storm alert" an alert), a URL button, a switch-inline button
  ("@weather_bot Moscow"), a copy-text button, a web app's — and a one-time keyboard over the
  field ("Moscow", "Paris", "London", a location button; placeholder "City"). Sending "London"
  makes the bot ask for a reply (`replyMarkupForceReply`, "Your name"). `deleteChatReplyMarkup`
  takes the keyboard away (`updateChatReplyMarkup` with 0).
- "Quiz Bot" (110): a bot chat with no messages — «Start» (`sendBotStartMessage`: "/start" from
  the user, the bot's welcome).
- `setMessageSenderBlockList` stops and restarts a bot (`updateChatBlockList`).
"""

from __future__ import annotations

import threading
import time

import fake_feed
import fake_world
from fake_world import NOW, add, handler, message, text

WEATHER = 105
QUIZ = 110

fake_world.USERS[QUIZ] = fake_world.user(QUIZ, "Quiz", "Bot", "quiz_bot", 5, kind="userTypeBot",
                                         contact=False)
fake_world.CHATS[QUIZ] = fake_world.chat(QUIZ, fake_world.private(QUIZ), "Quiz Bot", 5, None, 826)


def inline(label: str, kind: dict) -> dict:
    return {"@type": "inlineKeyboardButton", "text": label, "icon_custom_emoji_id": "0", "style": None,
            "type": kind}


def key(label: str, kind: str = "keyboardButtonTypeText") -> dict:
    return {"@type": "keyboardButton", "text": label, "icon_custom_emoji_id": "0", "style": None,
            "type": {"@type": kind}}


INLINE = {"@type": "replyMarkupInlineKeyboard", "rows": [
    [inline("Now", {"@type": "inlineKeyboardButtonTypeCallback", "data": "bm93"}),
     inline("Storm alert", {"@type": "inlineKeyboardButtonTypeCallback", "data": "YWxlcnQ="})],
    [inline("Forecast site", {"@type": "inlineKeyboardButtonTypeUrl", "url": "https://example.com/forecast"})],
    [inline("Search a city", {"@type": "inlineKeyboardButtonTypeSwitchInline", "query": "Moscow",
                              "target_chat": {"@type": "targetChatCurrent"}}),
     inline("Copy code", {"@type": "inlineKeyboardButtonTypeCopyText", "text": "WX-2026"})],
    [inline("Open the app", {"@type": "inlineKeyboardButtonTypeWebApp", "url": "https://example.com/app"})],
]}

CITIES = {"@type": "replyMarkupShowKeyboard", "rows": [
    [key("Moscow"), key("Paris")],
    [key("London"), key("Send my location", "keyboardButtonTypeRequestLocation")],
], "is_persistent": False, "resize_keyboard": True, "one_time": True, "is_personal": False,
    "input_field_placeholder": "City"}


def fill_weather() -> None:
    fake_world.MESSAGES[WEATHER] = []
    t = NOW - 7_200
    add(WEATHER, message(WEATHER, 0, text("/start"), t, outgoing=True))
    add(WEATHER, message(WEATHER, WEATHER, text("Hi! I tell the weather. What would you like?"), t + 5,
                         reply_markup=INLINE))
    keyboard = add(WEATHER, message(WEATHER, WEATHER, text("Choose a city:"), t + 10, reply_markup=CITIES))
    fake_world.CHATS[WEATHER]["reply_markup_message_id"] = keyboard["id"]


fill_weather()
fake_world.MESSAGES.setdefault(QUIZ, [])


def set_markup(client, chat_id: int, message_id: int) -> None:
    fake_world.CHATS[chat_id]["reply_markup_message_id"] = message_id
    client.send({"@type": "updateChatReplyMarkup", "chat_id": chat_id, "reply_markup_message_id": message_id})


def bot_says(client, chat_id: int, value: str, delay: float = 0.8, **extra) -> None:
    """The bot's message a moment later."""
    def say() -> None:
        with fake_world.LOCK:
            obj = message(chat_id, chat_id, text(value), int(time.time()), **extra)
        add(chat_id, obj)
        client.send({"@type": "updateNewMessage", "message": obj})
        client.send({"@type": "updateChatLastMessage", "chat_id": chat_id, "last_message": obj,
                     "positions": fake_world.CHATS[chat_id].get("positions") or []})
        if extra.get("reply_markup", {}).get("@type") == "replyMarkupForceReply":
            set_markup(client, chat_id, obj["id"])
    threading.Timer(delay, say).start()


@handler("getCallbackQueryAnswer")
def get_callback_query_answer(client, request: dict) -> None:
    data = (request.get("payload") or {}).get("data")
    if data == "bm93":
        return client.answer(request, {"@type": "callbackQueryAnswer", "text": "Moscow: +3°, cloudy",
                                       "show_alert": False, "url": ""})
    if data == "YWxlcnQ=":
        return client.answer(request, {"@type": "callbackQueryAnswer",
                                       "text": "Storm warning for tonight: wind up to 20 m/s.",
                                       "show_alert": True, "url": ""})
    client.error(request, 400, "BOT_RESPONSE_TIMEOUT")


@handler("deleteChatReplyMarkup")
def delete_chat_reply_markup(client, request: dict) -> None:
    chat_id = int(request["chat_id"])
    client.ok(request)
    if fake_world.CHATS.get(chat_id, {}).get("reply_markup_message_id") == int(request["message_id"]):
        set_markup(client, chat_id, 0)


@handler("sendBotStartMessage")
def send_bot_start_message(client, request: dict) -> None:
    chat_id = int(request["chat_id"])
    pending = fake_feed.send_one(client, request, chat_id, {"@type": "inputMessageText",
                                                            "text": {"@type": "formattedText", "text": "/start",
                                                                     "entities": []}})
    client.answer(request, pending)
    bot_says(client, chat_id, "Welcome to the quiz! Ready for the first question?", delay=1.2)


@handler("setMessageSenderBlockList")
def set_message_sender_block_list(client, request: dict) -> None:
    user_id = int((request.get("sender_id") or {}).get("user_id", 0))
    chat_obj = fake_world.CHATS.get(user_id)
    if chat_obj is None:
        return client.error(request, 400, "USER_ID_INVALID")
    chat_obj["block_list"] = request.get("block_list")
    client.ok(request)
    client.send({"@type": "updateChatBlockList", "chat_id": user_id, "block_list": request.get("block_list")})


def answer_text(client, chat_id: int, value: str) -> None:
    """What the fake Weather Bot says to a text: "London" — it asks for a reply."""
    if chat_id != WEATHER:
        return
    if value == "London":
        bot_says(client, chat_id, "London: +9°, rain. What's your name, for the forecast?",
                 reply_markup={"@type": "replyMarkupForceReply", "is_personal": False,
                               "input_field_placeholder": "Your name"})
    elif value in ("Moscow", "Paris"):
        bot_says(client, chat_id, f"{value}: +3°, cloudy")


_send_message = fake_world.HANDLERS["sendMessage"]


@handler("sendMessage")
def send_message(client, request: dict) -> None:
    """A message sent as fake_feed sends it; the Weather Bot answers some texts."""
    _send_message(client, request)
    content = request.get("input_message_content") or {}
    if content.get("@type") == "inputMessageText":
        answer_text(client, int(request["chat_id"]), (content.get("text") or {}).get("text", ""))
