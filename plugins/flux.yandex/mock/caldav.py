"""The fake Yandex Calendar (CalDAV) of mock_yandex.py: the principal, the calendar home, three
calendars (two of events, one of tasks) and meetings placed relative to the moment the mock started
(or `/_mock/caldav` re-anchored them), in Moscow time:

- "Дейли" — daily, 15 min, the next one 12 min after the anchor, with a Telemost conference;
- "Ревью архитектуры" — 3 h after the anchor, an hour, in a meeting room;
- "День рождения Анны" — all day, the anchor's day;
- a cancelled meeting (1 h after) and one the user declined (2 h after) — left out by the plugin;
- "Планирование спринта" — tomorrow 10:00, a Telemost link in its description;
- "Ретро команды" — tomorrow 16:00, in the second calendar ("Команда").

Answers as Yandex's server does: `multistatus` in the DAV: namespace, iCalendar with a VTIMEZONE.
A REPORT gets the series as they are (RRULE) or, after `POST /_mock/caldav {"expand": true}`, their
instances in the time range (RECURRENCE-ID, UTC) — the two ways a server may answer.

Credentials: `OAuth <token>` (any but "expired"), `Bearer <token>`, Basic `login:password` (any but
the password "wrong"); else 401.
"""
import base64
import json
import re
import time
from datetime import datetime, timedelta, timezone

EMAIL = "egor@example.ru"
QUOTED = "egor%40example.ru"
MSK = timezone(timedelta(hours=3))

CALENDARS = [
    ("events-default", "Мои события", "VEVENT"),
    ("team", "Команда", "VEVENT"),
    ("todos-default", "Дела", "VTODO"),
]

VTIMEZONE = (
    "BEGIN:VTIMEZONE\r\nTZID:Europe/Moscow\r\nTZURL:http://tzurl.org/zoneinfo/Europe/Moscow\r\n"
    "X-LIC-LOCATION:Europe/Moscow\r\nBEGIN:STANDARD\r\nTZOFFSETFROM:+0300\r\nTZOFFSETTO:+0300\r\n"
    "TZNAME:MSK\r\nDTSTART:20141026T020000\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n"
)


class Answer:
    def __init__(self, status, body=b"", headers=None, content_type="application/xml; charset=utf-8"):
        self.status = status
        self.body = body.encode() if isinstance(body, str) else body
        self.headers = dict(headers or {})
        self.headers.setdefault("Content-Type", content_type)


def register(router, state):
    data = state.data.setdefault("caldav", {})

    def reset():
        data["anchor"] = int(time.time()) // 60 * 60
        data["expand"] = False

    reset()
    state.resets.append(reset)

    def authorized(request):
        header = request.headers.get("authorization", "")
        if header.startswith("OAuth "):
            token = header[6:].strip()
            return bool(token) and token != "expired"
        if header.startswith("Bearer "):
            return bool(header[7:].strip())
        if header.startswith("Basic "):
            try:
                login, _, password = base64.b64decode(header[6:].strip()).decode().partition(":")
            except Exception:
                return False
            return bool(login) and bool(password) and password != "wrong"
        return False

    def guarded(handler):
        def run(request, match):
            if not authorized(request):
                return Answer(401, "", {"WWW-Authenticate": 'Basic realm="Yandex.Calendar"'}, "text/plain")
            return handler(request, match)
        return run

    def principal(request, match):
        return multistatus([response("/", props(
            f"<current-user-principal><href>/principals/users/{QUOTED}/</href></current-user-principal>"))])

    def home(request, match):
        return multistatus([response(f"/principals/users/{QUOTED}/", props(
            "<C:calendar-home-set xmlns:C='urn:ietf:params:xml:ns:caldav'>"
            f"<href>/calendars/{QUOTED}/</href></C:calendar-home-set>"))])

    def calendars(request, match):
        items = [response(f"/calendars/{QUOTED}/", props(
            "<resourcetype><collection/></resourcetype><displayname>egor</displayname>"))]
        for slug, name, component in CALENDARS:
            items.append(response(
                f"/calendars/{QUOTED}/{slug}/",
                props(
                    "<resourcetype><collection/><C:calendar xmlns:C='urn:ietf:params:xml:ns:caldav'/>"
                    f"</resourcetype><displayname>{name}</displayname>"
                    "<C:supported-calendar-component-set xmlns:C='urn:ietf:params:xml:ns:caldav'>"
                    f"<C:comp name='{component}'/></C:supported-calendar-component-set>"),
                missing="<calendar-color xmlns='http://apple.com/ns/ical/'/>"))
        return multistatus(items)

    def report(request, match):
        slug = match.group(1)
        body = request.body.decode(errors="replace")
        found = re.search(r'time-range start="(\d{8}T\d{6}Z)" end="(\d{8}T\d{6}Z)"', body)
        start, end = (parse_stamp(found.group(1)), parse_stamp(found.group(2))) if found else (0, 2**40)
        expand = data["expand"] and "<C:expand" in body
        items = []
        for event in events(data["anchor"], slug):
            for text in render(event, start, end, expand):
                uid = event["uid"]
                items.append(response(
                    f"/calendars/{QUOTED}/{slug}/{uid}.ics",
                    props(f'<getetag>"{uid}-1"</getetag>'
                          "<C:calendar-data xmlns:C='urn:ietf:params:xml:ns:caldav'>"
                          f"{escape(text)}</C:calendar-data>")))
        return multistatus(items)

    def control(request, match):
        settings = request.json() or {}
        if settings.get("reanchor"):
            data["anchor"] = int(time.time()) // 60 * 60
        if "expand" in settings:
            data["expand"] = bool(settings["expand"])
        if "anchor" in settings:
            data["anchor"] = int(settings["anchor"])
        return Answer(200, json.dumps({"anchor": data["anchor"], "expand": data["expand"]}),
                      content_type="application/json")

    router.add("PROPFIND", r"/", guarded(principal))
    router.add("PROPFIND", r"/principals/users/[^/]+/?", guarded(home))
    router.add("PROPFIND", r"/calendars/[^/]+/?", guarded(calendars))
    router.add("REPORT", r"/calendars/[^/]+/([^/]+)/?", guarded(report))
    router.add("POST", r"/_mock/caldav", control)


# --- Meetings ---

def events(anchor, slug):
    """The meetings of a calendar, as dictionaries."""
    local = datetime.fromtimestamp(anchor, MSK)
    day = local.replace(hour=0, minute=0, second=0)
    tomorrow = day + timedelta(days=1)
    daily_first = datetime.fromtimestamp(anchor + 12 * 60, MSK) - timedelta(days=7)
    if slug == "events-default":
        return [
            {"uid": "daily", "summary": "Дейли", "start": daily_first, "minutes": 15, "zone": True,
             "rrule": "FREQ=DAILY", "extra": "X-TELEMOST-CONFERENCE:https://telemost.yandex.ru/j/12345678901234\r\n"},
            {"uid": "review", "summary": "Ревью архитектуры",
             "start": datetime.fromtimestamp(anchor + 3 * 3600, MSK), "minutes": 60, "zone": False,
             "extra": "LOCATION:Переговорка «Нева»\r\n"},
            {"uid": "birthday", "summary": "День рождения Анны", "date": day},
            {"uid": "cancelled", "summary": "Отменённая встреча",
             "start": datetime.fromtimestamp(anchor + 3600, MSK), "minutes": 30, "zone": True,
             "extra": "STATUS:CANCELLED\r\n"},
            {"uid": "declined", "summary": "Встреча, от которой отказались",
             "start": datetime.fromtimestamp(anchor + 2 * 3600, MSK), "minutes": 30, "zone": True,
             "extra": f"ATTENDEE;PARTSTAT=DECLINED;CN=Егор:mailto:{EMAIL}\r\n"
                      "ATTENDEE;PARTSTAT=ACCEPTED;CN=Анна:mailto:anna@example.ru\r\n"},
            {"uid": "planning", "summary": "Планирование спринта",
             "start": tomorrow.replace(hour=10), "minutes": 60, "zone": True,
             "extra": "DESCRIPTION:Повестка в задаче TRACK-142.\\nТелемост: https://telemost.yandex.ru/j/99887766554433\r\n"},
        ]
    if slug == "team":
        return [
            {"uid": "retro", "summary": "Ретро команды", "start": tomorrow.replace(hour=16),
             "minutes": 60, "zone": True},
        ]
    return []


def render(event, start, end, expand):
    """The event's iCalendar texts in [start, end): one (a series as it is, a single meeting), or
    its instances when the server expands."""
    if "date" in event:
        first = event["date"].timestamp()
        if first < end and first + 86400 > start:
            return [calendar(vevent(event))]
        return []
    if "rrule" in event and expand:
        texts = []
        first = event["start"]
        for index in range(0, 400):
            at = first + timedelta(days=index)
            moment = at.timestamp()
            if moment >= end:
                break
            if moment + event["minutes"] * 60 > start:
                instance = dict(event, start=at, recurrence=at)
                instance.pop("rrule")
                texts.append(calendar(vevent(instance, utc=True)))
        return texts
    if "rrule" in event:
        return [calendar(vevent(event))]
    moment = event["start"].timestamp()
    if moment < end and moment + event["minutes"] * 60 > start:
        return [calendar(vevent(event))]
    return []


def vevent(event, utc=False):
    lines = ["BEGIN:VEVENT", f"UID:{event['uid']}@mock.yandex", f"SUMMARY:{event['summary']}",
             "DTSTAMP:20261014T000000Z"]
    if "date" in event:
        day = event["date"]
        lines += [f"DTSTART;VALUE=DATE:{day:%Y%m%d}",
                  f"DTEND;VALUE=DATE:{day + timedelta(days=1):%Y%m%d}", "TRANSP:TRANSPARENT"]
    else:
        start, finish = event["start"], event["start"] + timedelta(minutes=event["minutes"])
        if utc or not event.get("zone"):
            lines += [f"DTSTART:{stamp(start)}", f"DTEND:{stamp(finish)}"]
        else:
            lines += [f"DTSTART;TZID=Europe/Moscow:{start:%Y%m%dT%H%M%S}",
                      f"DTEND;TZID=Europe/Moscow:{finish:%Y%m%dT%H%M%S}"]
        if "rrule" in event:
            lines.append(f"RRULE:{event['rrule']}")
        if "recurrence" in event:
            lines.append(f"RECURRENCE-ID:{stamp(event['recurrence'])}")
    text = "\r\n".join(lines) + "\r\n" + event.get("extra", "") + "END:VEVENT\r\n"
    return fold(text)


def calendar(vevent_text):
    return ("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Yandex LLC//Yandex Calendar//EN\r\n"
            + VTIMEZONE + vevent_text + "END:VCALENDAR\r\n")


def fold(text):
    """Lines longer than 75 octets folded, as RFC 5545 says (the plugin unfolds them)."""
    out = []
    for line in text.split("\r\n"):
        encoded = line.encode()
        while len(encoded) > 75:
            cut = 75
            while cut > 0 and (encoded[cut] & 0xC0) == 0x80:
                cut -= 1
            out.append(encoded[:cut].decode())
            encoded = b" " + encoded[cut:]
        out.append(encoded.decode())
    return "\r\n".join(out)


def stamp(moment):
    return moment.astimezone(timezone.utc).strftime("%Y%m%dT%H%M%SZ")


def parse_stamp(text):
    return datetime.strptime(text, "%Y%m%dT%H%M%SZ").replace(tzinfo=timezone.utc).timestamp()


# --- WebDAV ---

def props(inner):
    return f"<propstat><status>HTTP/1.1 200 OK</status><prop>{inner}</prop></propstat>"


def response(href, propstat, missing=None):
    lost = (f"<propstat><status>HTTP/1.1 404 Not Found</status><prop>{missing}</prop></propstat>"
            if missing else "")
    return f"<response><href>{href}</href>{propstat}{lost}</response>"


def multistatus(responses):
    return Answer(207, "<?xml version='1.0' encoding='UTF-8'?>\n<multistatus xmlns='DAV:'>"
                  + "".join(responses) + "</multistatus>")


def escape(text):
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
