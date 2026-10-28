"""Tracker's API v3 for the fake Yandex (mock_yandex.py), with Russian sample data: queues TRACK
(board 1 "Backend") and WEB (board 2 "Web", scrum: Sprint 7 released, 8 in progress, 9 planned).

Tasks are `state.data["tracker"]["issues"]` (key → the task as Tracker returns it, plus `_`-fields
the mock keeps for itself: followers, who is summoned) — the boards' part reads them. Routes:
myself, queues, `issues/_search` (a query — the clauses below — `keys`, or `filter` with `boards`
and `sprint`; `perPage` / `page`, `X-Total-Count`), a task, its transitions and `_execute`,
comments (read, add), links, the changelog (`perPage`, after `id`), the work log (read, add); the
task's fields changed — `PATCH /v3/issues/<key>` in Tracker's formats (unknown fields and wrong
values answer 400, as Tracker does) — and what they take: priorities (named by `Accept-Language`;
`localized=false` — by language), a queue with `expand=issueTypesConfig`, a queue's components,
the users (one of them dismissed); the fields — the system ones (`GET /v3/fields`, named by
`Accept-Language`) and a queue's own (`GET /v3/queues/<key>/localFields`: TRACK has six, WEB one) —
and TRACK-142 has a value in most of them; `PATCH` changes them by their metadata (a date, a
number, a person, one of a list, a line), a local field by its key in the task
(`<queue's id>--<key>`).

The query language as far as the plugin uses it: `Assignee: me()`, `Resolution: empty()`,
`Followers: me()`, `"Pending reply from": me()`, `Queue: KEY`, `Key: KEY`, `Summary: "words"`,
`Boards: N`, `"Sort by": Updated DESC|ASC`, `Updated: >= now()-15m` (`>=`, `<=`, `>`, `<`; `now()`
with `- N` / `+ N` in `m`, `h`, `d`, `w`, or `"2026-10-28 09:30:00"` / `"2026-10-28"` in UTC — a date
alone means its whole day; `"from".."to"` — a range); other clauses are ignored (and printed).
`_search?fields=key,summary,status` — each task has only `self`, `id`, `key` and these fields.

Auth: `Authorization: OAuth <a token oauth.py gave, or one that starts with y0_>` or `Bearer <IAM>`
(`OAuth revoked` — 401); the organization `X-Org-ID: 123456` or `X-Cloud-Org-ID: bpf-mock`.

Control: POST /_mock/assign {"key"} — the task becomes mine (TRACK-155 by default); POST
/_mock/status {"key", "status"} — someone else moves it; POST /_mock/summon {"key"} — I'm summoned;
POST /_mock/reject {"field", "message"} — the next change of the field is refused (400), as a
workflow's rule would.
"""
import datetime
import re
import sys

from mock_yandex import Response

ORGS = {"x-org-id": "123456", "x-cloud-org-id": "bpf-mock"}
API = "https://api.tracker.yandex.net/v3"

ME = {"id": "1120000000016772", "display": "Егор Агеев", "login": "egor.ageev"}
ANNA = {"id": "1120000000016773", "display": "Анна Петрова", "login": "anna.petrova"}
OLEG = {"id": "1120000000016774", "display": "Олег Смирнов", "login": "oleg.smirnov"}
IVAN = {"id": "1120000000016775", "display": "Иван Шаров", "login": "ivan.sharov",
        "dismissed": True}
PEOPLE = [ME, ANNA, OLEG, IVAN]

# Priorities: (id, key, Russian, English, order).
PRIORITIES = [(1, "trivial", "Незначительный", "Trivial", 10), (2, "minor", "Низкий", "Minor", 20),
              (3, "normal", "Средний", "Normal", 30), (4, "critical", "Критичный", "Critical", 40),
              (5, "blocker", "Блокер", "Blocker", 50)]
# A queue's issue types: (id, key, display).
TYPES = {"TRACK": [("1", "bug", "Ошибка"), ("2", "task", "Задача"), ("3", "story", "История"),
                   ("4", "epic", "Эпик")],
         "WEB": [("2", "task", "Задача"), ("3", "story", "История")]}
COMPONENTS = {"TRACK": [("11", "Авторизация"), ("12", "Платежи"), ("13", "Отчёты")],
              "WEB": []}

STATUSES = {
    "open": ("Открыт", "new"),
    "needInfo": ("Требуется информация", "paused"),
    "inProgress": ("В работе", "inProgress"),
    "inReview": ("На ревью", "inProgress"),
    "resolved": ("Решён", "done"),
    "closed": ("Закрыт", "done"),
}

# The workflow: status → its transitions (id, the button's name, the target status).
WORKFLOW = {
    "open": [("start", "В работу", "inProgress"), ("needInfo", "Требуется информация", "needInfo"),
             ("close", "Закрыть", "closed")],
    "needInfo": [("start", "В работу", "inProgress"), ("close", "Закрыть", "closed")],
    "inProgress": [("review", "На ревью", "inReview"), ("stop", "Остановить", "open"),
                   ("close", "Закрыть", "closed")],
    "inReview": [("start", "Вернуть в работу", "inProgress"), ("resolve", "Решить", "resolved"),
                 ("close", "Закрыть", "closed")],
    "resolved": [("reopen", "Переоткрыть", "open")],
    "closed": [("reopen", "Переоткрыть", "open")],
}

# Fields' categories: key → (Russian, English).
CATEGORIES = {"system": ("Системные", "System"), "agile": ("Agile", "Agile"),
              "time": ("Учёт времени", "Time tracking"), "email": ("Email", "Email")}
# The system fields: (id, Russian, English, type, items, readonly, order, category).
FIELDS = [
    ("summary", "Задача", "Issue", "string", None, False, 1, "system"),
    ("description", "Описание", "Description", "text", None, False, 2, "system"),
    ("status", "Статус", "Status", "status", None, True, 3, "system"),
    ("resolution", "Резолюция", "Resolution", "resolution", None, True, 4, "system"),
    ("type", "Тип", "Type", "issuetype", None, False, 5, "system"),
    ("priority", "Приоритет", "Priority", "priority", None, False, 6, "system"),
    ("assignee", "Исполнитель", "Assignee", "user", None, False, 7, "system"),
    ("createdBy", "Автор", "Reporter", "user", None, True, 8, "system"),
    ("followers", "Наблюдатели", "Followers", "array", "user", False, 9, "system"),
    ("qaEngineer", "QA-инженер", "QA engineer", "user", None, False, 10, "system"),
    ("tags", "Теги", "Tags", "array", "string", False, 11, "system"),
    ("components", "Компоненты", "Components", "array", "component", False, 12, "system"),
    ("fixVersions", "Исправить в версиях", "Fix versions", "array", "version", False, 13, "system"),
    ("affectedVersions", "Затронутые версии", "Affected versions", "array", "version", False, 14,
     "system"),
    ("project", "Проект", "Project", "project", None, False, 15, "system"),
    ("epic", "Эпик", "Epic", "issue", None, False, 16, "system"),
    ("parent", "Родительская задача", "Parent issue", "issue", None, False, 17, "system"),
    ("deadline", "Дедлайн", "Deadline", "date", None, False, 18, "system"),
    ("votes", "Голоса", "Votes", "integer", None, True, 19, "system"),
    ("createdAt", "Создана", "Created", "datetime", None, True, 20, "system"),
    ("updatedAt", "Обновлена", "Updated", "datetime", None, True, 21, "system"),
    ("resolvedAt", "Дата решения", "Resolved", "datetime", None, True, 22, "system"),
    ("sprint", "Спринт", "Sprint", "array", "sprint", False, 30, "agile"),
    ("storyPoints", "Story Points", "Story Points", "float", None, False, 31, "agile"),
    ("start", "Начало", "Start date", "date", None, False, 40, "time"),
    ("end", "Конец", "End date", "date", None, False, 41, "time"),
    ("originalEstimation", "Первоначальная оценка", "Original estimate", "duration", None, False,
     42, "time"),
    ("estimation", "Оценка", "Estimate", "duration", None, False, 43, "time"),
    ("spent", "Затрачено времени", "Time spent", "duration", None, True, 44, "time"),
    ("emailFrom", "Email отправителя", "Email from", "string", None, True, 50, "email"),
]
# A queue's own fields: queue → (its id, [(key, Russian, English, type, options, order)]).
LOCAL_FIELDS = {
    "TRACK": ("64f1c0a2e4b0a1b2c3d4e5f6", [
        ("team", "Команда", "Team", "string", ["Платформа", "Платежи", "Аналитика"], 101),
        ("environment", "Окружение", "Environment", "string", None, 102),
        ("customers", "Клиентов затронуто", "Customers affected", "integer", None, 103),
        ("releaseDate", "Дата релиза", "Release date", "date", None, 104),
        ("reviewer", "Ревьюер", "Reviewer", "user", None, 105),
        ("supportNotes", "Заметки для поддержки", "Notes for support", "text", None, 106),
    ]),
    "WEB": ("64f1c0a2e4b0a1b2c3d4e5f7", [
        ("design", "Макет", "Design", "string", None, 101),
    ]),
}


def local_id(queue, key):
    return f"{LOCAL_FIELDS[queue][0]}--{key}"


LINK_TYPES = {
    # This task → the other: (the other's role seen from here, the reverse).
    "subtask": {"id": "subtask", "inward": "Подзадача", "outward": "Родительская задача"},
    "depends": {"id": "depends", "inward": "Блокирует", "outward": "Зависит от"},
    "relates": {"id": "relates", "inward": "Связана", "outward": "Связана"},
    "duplicates": {"id": "duplicates", "inward": "Дублируется", "outward": "Дублирует"},
}


def now_iso(delta_minutes=0):
    moment = datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(minutes=delta_minutes)
    return moment.strftime("%Y-%m-%dT%H:%M:%S.000+0000")


def status_ref(key):
    display, _ = STATUSES[key]
    return {"self": f"{API}/statuses/{key}", "id": key, "key": key, "display": display}


def status_type(key):
    _, kind = STATUSES[key]
    return {"id": kind, "key": kind, "display": kind}


def user(person):
    return {"self": f"{API}/users/{person['id']}", "id": person["id"], "display": person["display"]}


def component_ref(queue, id_):
    name = dict(COMPONENTS[queue])[id_]
    return {"self": f"{API}/components/{id_}", "id": id_, "display": name}


def priority_ref(key):
    id_, key, ru, en, _ = next(p for p in PRIORITIES if p[1] == key)
    return {"self": f"{API}/priorities/{id_}", "id": str(id_), "key": key, "display": ru}


UPDATED = re.compile(
    r'(?i)(?<![\w"])updated\s*:\s*(>=|<=|>|<)?\s*'
    r'(now\(\)(?:\s*[-+]\s*\d+\s*[mhdw]\b)?|"[^"]*"\.\."[^"]*"|"[^"]*"|\d{4}-\d\d-\d\d)')
UNITS = {"m": "minutes", "h": "hours", "d": "days", "w": "weeks"}


def moment_of(text):
    """`now()-15m`, `"2026-10-28 09:30:00"` or `"2026-10-28"` (UTC): the moment and whether it is a
    whole day."""
    text = text.strip().strip('"').strip()
    now = re.fullmatch(r"now\(\)(?:\s*([-+])\s*(\d+)\s*([mhdw]))?", text, re.IGNORECASE)
    if now:
        moment = datetime.datetime.now(datetime.timezone.utc)
        if now.group(1):
            delta = datetime.timedelta(**{UNITS[now.group(3).lower()]: int(now.group(2))})
            moment = moment + delta if now.group(1) == "+" else moment - delta
        return moment, False
    for pattern, day in (("%Y-%m-%d %H:%M:%S", False), ("%Y-%m-%dT%H:%M:%S", False),
                         ("%Y-%m-%d %H:%M", False), ("%Y-%m-%d", True)):
        try:
            moment = datetime.datetime.strptime(text, pattern)
        except ValueError:
            continue
        return moment.replace(tzinfo=datetime.timezone.utc), day
    raise ValueError(f"not a date: {text}")


def updated_matches(task, operator, value):
    """`Updated: <operator> <value>` on the task's `updatedAt`."""
    updated = datetime.datetime.strptime(task["updatedAt"], "%Y-%m-%dT%H:%M:%S.%f%z")
    one_day = datetime.timedelta(days=1)
    if ".." in value:
        low, high = value.split("..", 1)
        start, _ = moment_of(low)
        end, day = moment_of(high)
        return start <= updated < end + one_day if day else start <= updated <= end
    moment, day = moment_of(value)
    if day:
        # A date alone: its whole day.
        return {">=": updated >= moment, ">": updated >= moment + one_day,
                "<=": updated < moment + one_day, "<": updated < moment,
                None: moment <= updated < moment + one_day}[operator]
    return {">=": updated >= moment, ">": updated > moment, "<=": updated <= moment,
            "<": updated < moment, None: updated == moment}[operator]


def sample():
    today = datetime.date.today()

    def issue(key, summary, status, kind, priority, assignee, *, description="", deadline=None,
              sprint=None, parent=None, followers=(), minutes_ago=60):
        queue = key.split("-")[0]
        task = {
            "self": f"{API}/issues/{key}",
            "id": "mock" + key.replace("-", ""),
            "key": key,
            "version": 3,
            "summary": summary,
            "description": description,
            "status": status_ref(status),
            "statusType": status_type(status),
            "type": {"id": kind[0], "key": kind[0], "display": kind[1]},
            "priority": {"id": priority[0], "key": priority[0], "display": priority[1]},
            "queue": {"id": queue, "key": queue, "display": {"TRACK": "Трекер", "WEB": "Веб"}[queue]},
            "createdBy": user(ANNA),
            "createdAt": now_iso(-60 * 24 * 7),
            "updatedAt": now_iso(-minutes_ago),
            "boards": [{"id": 1 if queue == "TRACK" else 2}],
            "_followers": [person["id"] for person in followers],
            "_pending": [],
        }
        if assignee:
            task["assignee"] = user(assignee)
        if status in ("resolved", "closed"):
            task["resolution"] = {"id": "fixed", "key": "fixed", "display": "Решён"}
        if deadline is not None:
            task["deadline"] = str(today + datetime.timedelta(days=deadline))
        if sprint:
            task["sprint"] = [{"self": f"{API}/sprints/{sprint}", "id": str(sprint),
                               "display": f"Sprint {sprint}"}]
        if parent:
            task["parent"] = {"self": f"{API}/issues/{parent[0]}", "id": "mock" + parent[0],
                              "key": parent[0], "display": parent[1]}
        return task

    bug, task_, story = ("bug", "Ошибка"), ("task", "Задача"), ("story", "История")
    critical, major, normal, minor = (("critical", "Критичный"), ("major", "Высокий"),
                                      ("normal", "Средний"), ("minor", "Низкий"))
    tasks = [
        issue("TRACK-142", "Ошибка входа через SSO", "open", bug, critical, ME, deadline=6,
              followers=[ME, ANNA], minutes_ago=30,
              description="После перехода с ADFS вход через SSO падает с ошибкой **500** на шаге "
                          "обмена кода.\n\nШаги:\n1. Открыть `/login`\n2. Нажать «Войти через "
                          "SSO»\n3. Вернуться из ADFS\n\nОжидается: вход в кабинет.\n\n"
                          "Логи — в задаче TRACK-150."),
        issue("TRACK-150", "Логи входа в аудит", "open", task_, normal, ME,
              parent=("TRACK-142", "Ошибка входа через SSO"),
              description="Писать каждую попытку входа в аудит с причиной отказа."),
        issue("TRACK-130", "Таймауты выгрузки отчётов", "needInfo", bug, major, ANNA,
              followers=[ME], description="Выгрузка за год обрывается через 30 с."),
        issue("TRACK-120", "Кэшировать тарифы", "inProgress", task_, normal, ME, deadline=2,
              description="Тарифы читаются из базы на каждый запрос: кэш на 5 минут."),
        issue("TRACK-99", "Логи импортёра", "inReview", task_, minor, ANNA),
        issue("TRACK-90", "Убрать старое API v1", "closed", task_, normal, ME),
        issue("TRACK-155", "Повторы в клиенте платежей", "open", task_, major, None,
              description="Повторять запрос при 502/503 с экспоненциальной паузой."),
        issue("WEB-12", "Шапка лендинга", "open", story, normal, ME, sprint=8),
        issue("WEB-14", "Тёмная тема", "inProgress", story, major, ANNA, sprint=8, followers=[ME]),
        issue("WEB-15", "Форма входа: тексты ошибок", "open", task_, normal, ME, sprint=8,
              description="Показывать понятную ошибку, когда SSO недоступен."),
        issue("WEB-16", "Анимация меню", "open", task_, minor, OLEG, sprint=8),
        issue("WEB-9", "Старый баннер", "closed", task_, minor, ME, sprint=7),
    ]
    issues = {task["key"]: task for task in tasks}
    # Fields a task may have filled: tags, components, story points.
    issues["TRACK-142"]["tags"] = ["sso", "auth"]
    issues["TRACK-142"]["components"] = [component_ref("TRACK", "11")]
    issues["TRACK-142"]["storyPoints"] = 5
    issues["TRACK-120"]["tags"] = ["backend"]
    # More of them, for the board's filters.
    issues["TRACK-130"]["tags"] = ["export", "backend"]
    issues["TRACK-130"]["components"] = [component_ref("TRACK", "13")]
    issues["TRACK-155"]["tags"] = ["payments", "backend"]
    issues["TRACK-155"]["components"] = [component_ref("TRACK", "12")]
    issues["TRACK-150"]["tags"] = ["audit", "auth"]
    issues["TRACK-150"]["components"] = [component_ref("TRACK", "11")]
    issues["TRACK-99"]["tags"] = ["logs"]
    issues["WEB-12"]["tags"] = ["frontend", "landing"]
    issues["WEB-14"]["tags"] = ["frontend", "design"]
    issues["WEB-15"]["tags"] = ["frontend", "auth"]
    # Every other field it has: the system ones, the queue's own, Tracker's bookkeeping.
    issues["TRACK-142"].update({
        "start": str(today - datetime.timedelta(days=3)),
        "end": str(today + datetime.timedelta(days=11)),
        "originalEstimation": "P3D",
        "spent": "PT6H30M",
        "qaEngineer": user(OLEG),
        "fixVersions": [{"self": f"{API}/versions/21", "id": "21", "display": "2.4"}],
        "project": {"self": f"{API}/projects/5", "id": "5", "display": "Вход 2.0"},
        "epic": {"self": f"{API}/issues/TRACK-100", "id": "mockTRACK100", "key": "TRACK-100",
                 "display": "Новая авторизация"},
        "votes": 2,
        "votedBy": [user(ANNA), user(OLEG)],
        "favorite": False,
        "unique": "mock-142",
        "commentWithoutExternalMessageCount": 2,
        "lastCommentUpdatedAt": now_iso(-60 * 3),
        "statusStartTime": now_iso(-60 * 24),
        "previousStatus": {"id": "needInfo", "key": "needInfo", "display": "Требуется информация"},
        local_id("TRACK", "team"): "Платформа",
        local_id("TRACK", "environment"): "stage, prod — после перехода с ADFS на новый IdP",
        local_id("TRACK", "customers"): 120,
        local_id("TRACK", "releaseDate"): str(today + datetime.timedelta(days=14)),
        local_id("TRACK", "reviewer"): user(OLEG),
    })
    issues["TRACK-120"][local_id("TRACK", "environment")] = "prod"
    issues["WEB-14"][local_id("WEB", "design")] = "figma.com/file/dark-theme"
    links = [
        # (this, the other, type): this → the other is "outward".
        ("TRACK-150", "TRACK-142", "subtask"),
        ("TRACK-142", "WEB-15", "depends"),
        ("TRACK-142", "TRACK-130", "relates"),
        ("WEB-14", "WEB-12", "relates"),
    ]
    comments = {
        "TRACK-142": [
            (ANNA, "Воспроизводится на стенде **stage**, ошибка в `token_exchange`.", -60 * 26),
            (ME, "Похоже, ADFS отдаёт `id_token` без `nonce`. Проверю.", -60 * 3),
        ],
        "TRACK-120": [(OLEG, "Только не забудьте сбрасывать кэш при смене тарифа.", -60 * 50)],
    }
    changelog = {
        "TRACK-142": [
            (ANNA, "IssueCreated", [], -60 * 24 * 7),
            (ANNA, "IssueUpdated",
             [("priority", "Приоритет", {"key": "normal", "display": "Средний"},
               {"key": "critical", "display": "Критичный"})], -60 * 24 * 6),
            (ANNA, "IssueUpdated",
             [("assignee", "Исполнитель", None, user(ME))], -60 * 24 * 5),
            (ANNA, "IssueCommentAdded", [], -60 * 26),
            (ME, "IssueCommentAdded", [], -60 * 3),
        ],
    }
    return issues, links, comments, changelog


def register(router, state):
    def reset():
        issues, links, comments, changelog = sample()
        data = state.data.setdefault("tracker", {})
        data["issues"] = issues
        data["links"] = links
        data["comments"] = {
            key: [{"id": index + 1, "longId": f"c{index + 1}", "text": text_, "createdBy": user(who),
                   "createdAt": now_iso(ago), "updatedAt": now_iso(ago)}
                  for index, (who, text_, ago) in enumerate(entries)]
            for key, entries in comments.items()
        }
        data["changelog"] = {}
        for key, entries in changelog.items():
            for who, kind, fields, ago in entries:
                record(data, key, who, kind, fields, ago)
        data["worklog"] = []
        data["next_id"] = 100

    def record(data, key, who, kind, fields, ago=0):
        entries = data["changelog"].setdefault(key, [])
        entries.append({
            "id": f"ch{len(entries) + 1:03d}",
            "self": f"{API}/issues/{key}/changelog/ch{len(entries) + 1:03d}",
            "issue": {"key": key},
            "updatedAt": now_iso(ago),
            "updatedBy": user(who),
            "type": kind,
            "transport": "front",
            "fields": [{"field": {"self": f"{API}/fields/{field}", "id": field, "display": name},
                        "from": before, "to": after}
                       for field, name, before, after in fields],
        })

    reset()
    state.resets.append(reset)
    data = lambda: state.data["tracker"]  # noqa: E731

    def denied(request):
        auth = request.headers.get("authorization", "")
        tokens = state.data.get("oauth", {}).get("tokens", set())
        if auth.startswith("OAuth "):
            token = auth[len("OAuth "):]
            if token == "revoked" or not (token in tokens or token.startswith("y0_")):
                return Response(401, {"errorMessages": ["Не авторизован"], "statusCode": 401})
        elif not auth.startswith("Bearer ") or auth == "Bearer expired":
            return Response(401, {"errorMessages": ["Не авторизован"], "statusCode": 401})
        if not any(request.headers.get(name) == value for name, value in ORGS.items()):
            return Response(403, {"errorMessages": ["Организация не найдена или нет доступа"],
                                  "statusCode": 403})
        return None

    def public(task):
        shown = {name: value for name, value in task.items() if not name.startswith("_")}
        # Followers as Tracker gives them: references to the people.
        ids = task.get("_followers", [])
        followers = [user(person) for person in PEOPLE if person["id"] in ids]
        if followers:
            shown["followers"] = followers
        return shown

    def task_of(match):
        return data()["issues"].get(match.group(1).upper())

    def missing(key):
        return Response(404, {"errorMessages": [f"Задача {key} не найдена"], "statusCode": 404})

    def touch(task):
        task["updatedAt"] = now_iso()
        task["version"] = task.get("version", 1) + 1

    def matches(task, query):
        # `Updated: >= now() - 15m` first: its value has spaces and operators the clauses below
        # don't take.
        for operator, value in UPDATED.findall(query):
            try:
                if not updated_matches(task, operator or None, value):
                    return False
            except ValueError as err:
                print(f"tracker mock: Updated: {err}", file=sys.stderr)
        query = UPDATED.sub(" ", query)
        rest = query
        for clause in re.findall(r'("[^"]+"|[\w.]+):\s*("[^"]*"|[^\s"]+(?:\(\))?)', query):
            name, value = clause[0].strip('"').lower(), clause[1].strip('"')
            rest = rest.replace(f"{clause[0]}:", "", 1)
            if name == "sort by":
                continue
            if name == "assignee":
                if value == "me()" and task.get("assignee", {}).get("id") != ME["id"]:
                    return False
                if value == "empty()" and task.get("assignee"):
                    return False
            elif name == "resolution":
                if value == "empty()" and task.get("resolution"):
                    return False
            elif name == "followers":
                if value == "me()" and ME["id"] not in task["_followers"]:
                    return False
            elif name == "pending reply from":
                if value == "me()" and ME["id"] not in task["_pending"]:
                    return False
            elif name == "queue":
                if task["queue"]["key"] != value.upper():
                    return False
            elif name == "key":
                if task["key"] != value.upper():
                    return False
            elif name == "summary":
                if value.lower() not in task["summary"].lower():
                    return False
            elif name == "boards":
                if not any(str(board["id"]) == value for board in task.get("boards", [])):
                    return False
            else:
                print(f"tracker mock: ignored clause {name}: {value}", file=sys.stderr)
        return True

    def search(request, match):
        problem = denied(request)
        if problem:
            return problem
        body = request.json() or {}
        found = list(data()["issues"].values())
        if body.get("keys"):
            keys = [key.upper() for key in body["keys"]]
            found = [task for task in found if task["key"] in keys]
        if body.get("query"):
            found = [task for task in found if matches(task, body["query"])]
        wanted = body.get("filter") or {}
        if wanted.get("boards") is not None:
            board = str(wanted["boards"])
            found = [task for task in found
                     if any(str(b.get("id")) == board for b in task.get("boards", []))]
        if wanted.get("sprint") is not None:
            sprint = str(wanted["sprint"])
            found = [task for task in found
                     if any(str(s.get("id")) == sprint for s in task.get("sprint", []))]
        if wanted.get("queue"):
            found = [task for task in found if task["queue"]["key"] == str(wanted["queue"]).upper()]
        ascending = '"sort by": updated asc' in (body.get("query") or "").lower()
        found.sort(key=lambda task: task["updatedAt"], reverse=not ascending)
        per_page = int(request.arg("perPage", "50"))
        page = int(request.arg("page", "1"))
        chunk = found[(page - 1) * per_page: page * per_page]
        pages = max(1, -(-len(found) // per_page))
        shown = [public(task) for task in chunk]
        # `fields=key,summary`: only these fields (and `self`, `id`, `key`) of each task.
        fields = [name.strip() for value in request.query.get("fields", [])
                  for name in value.split(",") if name.strip()]
        if fields:
            keep = {"self", "id", "key", *fields}
            shown = [{name: value for name, value in task.items() if name in keep}
                     for task in shown]
        return Response(200, shown,
                        headers={"X-Total-Count": str(len(found)), "X-Total-Pages": str(pages)})

    def myself(request, match):
        return denied(request) or Response(200, {
            "self": f"{API}/users/{ME['id']}", "uid": int(ME["id"]), "login": ME["login"],
            "trackerUid": int(ME["id"]), "passportUid": int(ME["id"]), "display": ME["display"],
            "firstName": "Егор", "lastName": "Агеев", "email": "egor.ageev@example.ru",
            "hasLicense": True,
        })

    def queues(request, match):
        return denied(request) or Response(200, [
            {"self": f"{API}/queues/TRACK", "id": 1, "key": "TRACK", "name": "Трекер",
             "lead": user(ANNA)},
            {"self": f"{API}/queues/WEB", "id": 2, "key": "WEB", "name": "Веб", "lead": user(OLEG)},
        ])

    def get_issue(request, match):
        problem = denied(request)
        if problem:
            return problem
        task = task_of(match)
        return Response(200, public(task)) if task else missing(match.group(1))

    def transitions_of(task):
        return [{"self": f"{API}/issues/{task['key']}/transitions/{id_}", "id": id_,
                 "display": name, "to": status_ref(target)}
                for id_, name, target in WORKFLOW[task["status"]["key"]]]

    def transitions(request, match):
        problem = denied(request)
        if problem:
            return problem
        task = task_of(match)
        return Response(200, transitions_of(task)) if task else missing(match.group(1))

    def move(task, target, who):
        before = task["status"]
        task["status"] = status_ref(target)
        task["statusType"] = status_type(target)
        if target in ("resolved", "closed"):
            task["resolution"] = {"id": "fixed", "key": "fixed", "display": "Решён"}
        else:
            task.pop("resolution", None)
        touch(task)
        record(data(), task["key"], who, "IssueWorkflow",
               [("status", "Статус", {"key": before["key"], "display": before["display"]},
                 {"key": target, "display": STATUSES[target][0]})])

    def execute(request, match):
        problem = denied(request)
        if problem:
            return problem
        task = task_of(match)
        if not task:
            return missing(match.group(1))
        step = next((step for step in WORKFLOW[task["status"]["key"]]
                     if step[0] == match.group(2)), None)
        if not step:
            return Response(422, {"errorMessages": [
                f"Переход {match.group(2)} недоступен из статуса «{task['status']['display']}»"],
                "statusCode": 422})
        move(task, step[2], ME)
        return Response(200, transitions_of(task))

    def comments(request, match):
        problem = denied(request)
        if problem:
            return problem
        task = task_of(match)
        if not task:
            return missing(match.group(1))
        entries = data()["comments"].setdefault(task["key"], [])
        if request.method == "GET":
            return Response(200, entries)
        body = request.json() or {}
        text_ = (body.get("text") or "").strip()
        if not text_:
            return Response(422, {"errorMessages": ["Пустой комментарий"], "statusCode": 422})
        data()["next_id"] += 1
        comment = {"id": data()["next_id"], "longId": f"c{data()['next_id']}", "text": text_,
                   "createdBy": user(ME), "createdAt": now_iso(), "updatedAt": now_iso()}
        entries.append(comment)
        touch(task)
        record(data(), task["key"], ME, "IssueCommentAdded", [])
        return Response(201, comment)

    def links(request, match):
        problem = denied(request)
        if problem:
            return problem
        task = task_of(match)
        if not task:
            return missing(match.group(1))
        out = []
        for index, (this, other, kind) in enumerate(data()["links"]):
            if task["key"] not in (this, other):
                continue
            direction = "outward" if task["key"] == this else "inward"
            target = data()["issues"][other if direction == "outward" else this]
            out.append({"self": f"{API}/issues/{task['key']}/links/{index + 1}", "id": index + 1,
                        "type": LINK_TYPES[kind], "direction": direction,
                        "object": {"self": target["self"], "id": target["id"],
                                   "key": target["key"], "display": target["summary"]},
                        "status": public(target)["status"],
                        **({"assignee": target["assignee"]} if target.get("assignee") else {})})
        return Response(200, out)

    def changelog(request, match):
        problem = denied(request)
        if problem:
            return problem
        task = task_of(match)
        if not task:
            return missing(match.group(1))
        entries = data()["changelog"].get(task["key"], [])
        after = request.arg("id")
        if after:
            ids = [entry["id"] for entry in entries]
            entries = entries[ids.index(after) + 1:] if after in ids else []
        return Response(200, entries[: int(request.arg("perPage", "50"))])

    def worklog(request, match):
        problem = denied(request)
        if problem:
            return problem
        task = task_of(match)
        if not task:
            return missing(match.group(1))
        if request.method == "GET":
            return Response(200, [entry for entry in data()["worklog"]
                                  if entry["issue"]["key"] == task["key"]])
        body = request.json() or {}
        if not re.match(r"^P(\d+W)?(\d+D)?(T(\d+H)?(\d+M)?)?$", body.get("duration", "")) \
                or not re.match(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d+)?[+-]\d{4}$",
                                body.get("start", "")):
            return Response(422, {"errorMessages": ["Неверное время или длительность"],
                                  "statusCode": 422})
        data()["next_id"] += 1
        entry = {"id": data()["next_id"], "issue": {"key": task["key"]},
                 "comment": body.get("comment", ""), "createdBy": user(ME),
                 "createdAt": now_iso(), "start": body["start"], "duration": body["duration"]}
        data()["worklog"].append(entry)
        record(data(), task["key"], ME, "IssueWorklogAdded", [])
        return Response(201, entry)

    def control_assign(request, match):
        body = request.json() or {}
        task = data()["issues"].get((body.get("key") or "TRACK-155").upper())
        if not task:
            return Response(404)
        task["assignee"] = user(ME)
        touch(task)
        record(data(), task["key"], ANNA, "IssueUpdated",
               [("assignee", "Исполнитель", None, user(ME))])
        return Response(204)

    def control_reject(request, match):
        body = request.json() or {}
        data().setdefault("rejected", {})[body.get("field") or "type"] = (
            body.get("message") or "Изменение запрещено правилами очереди")
        return Response(204)

    def control_status(request, match):
        body = request.json() or {}
        task = data()["issues"].get((body.get("key") or "").upper())
        if not task or body.get("status") not in STATUSES:
            return Response(404)
        move(task, body["status"], ANNA)
        return Response(204)

    def control_summon(request, match):
        body = request.json() or {}
        task = data()["issues"].get((body.get("key") or "").upper())
        if not task:
            return Response(404)
        task["_pending"].append(ME["id"])
        data()["next_id"] += 1
        data()["comments"].setdefault(task["key"], []).append({
            "id": data()["next_id"], "text": f"@{ME['login']}, посмотри, пожалуйста.",
            "createdBy": user(ANNA), "createdAt": now_iso(), "summonees": [user(ME)]})
        touch(task)
        return Response(204)

    def bad(field, why):
        return Response(400, {"errors": {field: why}, "errorMessages": [], "statusCode": 400})

    def ref_key(value):
        """A reference as Tracker takes it: "key", or {"key"/"id": …}."""
        if isinstance(value, str):
            return value
        if isinstance(value, dict):
            return str(value.get("key") or value.get("id") or "")
        return None

    def person(handle):
        return next((p for p in PEOPLE if handle in (p["login"], p["id"])), None)

    def list_op(current, value, field):
        """A list set (`[…]`) or changed (`{"add": […], "remove": […]}`)."""
        if isinstance(value, list):
            return list(value), None
        if isinstance(value, dict) and set(value) <= {"add", "remove"}:
            out = list(current)
            out += [item for item in value.get("add", []) if item not in out]
            out = [item for item in out if item not in value.get("remove", [])]
            return out, None
        return None, bad(field, "Ожидается список или {add, remove}")

    def other_field(field, queue):
        """A field PATCH changes by its metadata: (type, Russian name, options or None)."""
        editable = {"qaEngineer", "start", "end"}
        for id_, name_ru, _, kind, _, readonly, _, _ in FIELDS:
            if id_ == field and id_ in editable and not readonly:
                return kind, name_ru, None
        if queue in LOCAL_FIELDS:
            queue_id, fields = LOCAL_FIELDS[queue]
            for key_, name_ru, _, kind, options, _ in fields:
                if field == f"{queue_id}--{key_}":
                    return kind, name_ru, options
        return None

    def patch_issue(request, match):
        problem = denied(request)
        if problem:
            return problem
        task = task_of(match)
        if not task:
            return missing(match.group(1))
        body = request.json()
        if not isinstance(body, dict) or not body:
            return bad("body", "Пустое изменение")
        rejected = data().get("rejected", {})
        for field in body:
            if field in rejected:
                return bad(field, rejected.pop(field))
        queue = task["queue"]["key"]
        changes = []

        def change(field, name, before, after):
            changes.append((field, name, before, after))

        for field, value in body.items():
            if field == "summary":
                if not isinstance(value, str) or not value.strip():
                    return bad(field, "Название не может быть пустым")
                change(field, "Название", task["summary"], value)
                task["summary"] = value
            elif field == "description":
                if value is not None and not isinstance(value, str):
                    return bad(field, "Ожидается строка")
                change(field, "Описание", task.get("description"), value)
                task["description"] = value or ""
            elif field == "assignee":
                before = task.get("assignee")
                if value is None:
                    task.pop("assignee", None)
                else:
                    who = person(str(value))
                    if not who or who.get("dismissed"):
                        return bad(field, f"Пользователь {value} не найден")
                    task["assignee"] = user(who)
                change(field, "Исполнитель", before, task.get("assignee"))
            elif field == "priority":
                key_ = ref_key(value)
                if key_ not in [p[1] for p in PRIORITIES]:
                    return bad(field, f"Нет приоритета {key_}")
                change(field, "Приоритет", task.get("priority"), priority_ref(key_))
                task["priority"] = priority_ref(key_)
            elif field == "type":
                key_ = ref_key(value)
                found = next((t for t in TYPES[queue] if key_ in (t[0], t[1])), None)
                if not found:
                    return bad(field, f"Тип {key_} недоступен в очереди {queue}")
                after = {"self": f"{API}/issuetypes/{found[0]}", "id": found[0], "key": found[1],
                         "display": found[2]}
                change(field, "Тип", task.get("type"), after)
                task["type"] = after
            elif field == "deadline":
                if value is not None and not (isinstance(value, str)
                                              and re.match(r"^\d{4}-\d\d-\d\d$", value)):
                    return bad(field, "Ожидается дата ГГГГ-ММ-ДД")
                change(field, "Дедлайн", task.get("deadline"), value)
                if value is None:
                    task.pop("deadline", None)
                else:
                    task["deadline"] = value
            elif field == "storyPoints":
                if value is not None and not isinstance(value, (int, float)):
                    return bad(field, "Ожидается число")
                change(field, "Story Points", task.get("storyPoints"), value)
                if value is None:
                    task.pop("storyPoints", None)
                else:
                    task["storyPoints"] = value
            elif field == "tags":
                tags, problem = list_op(task.get("tags", []), value, field)
                if problem:
                    return problem
                if not all(isinstance(tag, str) and tag.strip() for tag in tags):
                    return bad(field, "Теги — строки")
                change(field, "Теги", task.get("tags", []), tags)
                task["tags"] = tags
            elif field == "components":
                ids = [str(c["id"]) for c in task.get("components", [])]
                wanted = value
                if isinstance(value, dict):
                    wanted = {op: [ref_key(item) for item in items] for op, items in value.items()}
                elif isinstance(value, list):
                    wanted = [ref_key(item) for item in value]
                ids, problem = list_op(ids, wanted, field)
                if problem:
                    return problem
                known = dict(COMPONENTS[queue])
                if any(id_ not in known for id_ in ids):
                    return bad(field, f"Нет такого компонента в очереди {queue}")
                after = [component_ref(queue, id_) for id_ in ids]
                change(field, "Компоненты", task.get("components", []), after)
                task["components"] = after
            elif field == "sprint":
                sprints = [] if value is None else value
                if not isinstance(sprints, list):
                    return bad(field, "Ожидается список спринтов")
                ids = [ref_key(item) for item in sprints]
                after = [{"self": f"{API}/sprints/{id_}", "id": id_, "display": f"Sprint {id_}"}
                         for id_ in ids]
                change(field, "Спринт", task.get("sprint", []), after)
                if after:
                    task["sprint"] = after
                else:
                    task.pop("sprint", None)
            elif field == "followers":
                ids, problem = list_op(task["_followers"],
                                       {op: [(person(str(h)) or {}).get("id") for h in items]
                                        for op, items in value.items()}
                                       if isinstance(value, dict) else value, field)
                if problem:
                    return problem
                task["_followers"] = [id_ for id_ in ids if id_]
            elif other_field(field, queue):
                kind, name, options = other_field(field, queue)
                before = task.get(field)
                if value is None:
                    task.pop(field, None)
                    after = None
                elif options is not None:
                    if value not in options:
                        return bad(field, f"Нет значения «{value}» в списке поля «{name}»")
                    after = value
                elif kind == "date":
                    if not (isinstance(value, str) and re.match(r"^\d{4}-\d\d-\d\d$", value)):
                        return bad(field, "Ожидается дата ГГГГ-ММ-ДД")
                    after = value
                elif kind in ("integer", "float"):
                    if isinstance(value, bool) or not isinstance(value, (int, float)):
                        return bad(field, "Ожидается число")
                    if kind == "integer" and value != int(value):
                        return bad(field, "Ожидается целое число")
                    after = int(value) if kind == "integer" else value
                elif kind == "user":
                    who = person(str(value))
                    if not who or who.get("dismissed"):
                        return bad(field, f"Пользователь {value} не найден")
                    after = user(who)
                elif kind in ("string", "text"):
                    if not isinstance(value, str):
                        return bad(field, "Ожидается строка")
                    after = value
                else:
                    return bad(field, f"Поле «{name}» так не меняется")
                if after is not None:
                    task[field] = after
                change(field, name, before, after)
            else:
                return bad(field, f"Неизвестное поле {field}")
        touch(task)
        record(data(), task["key"], ME, "IssueUpdated", changes)
        return Response(200, public(task))

    def priorities(request, match):
        problem = denied(request)
        if problem:
            return problem
        localized = request.arg("localized", "true") != "false"
        russian = (request.headers.get("accept-language") or "ru").lower().startswith("ru")
        return Response(200, [
            {"self": f"{API}/priorities/{id_}", "id": id_, "key": key_, "version": 1,
             "name": (ru if russian else en) if localized else {"en": en, "ru": ru},
             "order": order}
            for id_, key_, ru, en, order in PRIORITIES])

    def queue(request, match):
        problem = denied(request)
        if problem:
            return problem
        key_ = match.group(1).upper()
        if key_ not in TYPES:
            return Response(404, {"errorMessages": [f"Очередь {key_} не найдена"],
                                  "statusCode": 404})
        out = {"self": f"{API}/queues/{key_}", "key": key_,
               "name": {"TRACK": "Трекер", "WEB": "Веб"}[key_]}
        if "issueTypesConfig" in (request.arg("expand") or ""):
            out["issueTypesConfig"] = [
                {"issueType": {"self": f"{API}/issuetypes/{id_}", "id": id_, "key": type_key,
                               "display": display},
                 "workflow": {"id": "dev"}, "resolutions": []}
                for id_, type_key, display in TYPES[key_]]
        return Response(200, out)

    def queue_components(request, match):
        problem = denied(request)
        if problem:
            return problem
        key_ = match.group(1).upper()
        return Response(200, [{"self": f"{API}/components/{id_}", "id": int(id_), "name": name,
                               "queue": {"key": key_}}
                              for id_, name in COMPONENTS.get(key_, [])])

    def russian(request):
        return (request.headers.get("accept-language") or "ru").lower().startswith("ru")

    def field_meta(request):
        ru = russian(request)
        out = []
        for id_, name_ru, name_en, kind, items, readonly, order, category in FIELDS:
            schema = {"type": kind, "required": id_ == "summary"}
            if items:
                schema["items"] = items
            out.append({"self": f"{API}/fields/{id_}", "id": id_, "name": name_ru if ru else name_en,
                        "version": 1, "schema": schema, "readonly": readonly, "options": False,
                        "suggest": False, "order": order,
                        "category": {"self": f"{API}/fields/categories/{category}",
                                     "id": category,
                                     "display": CATEGORIES[category][0 if ru else 1]}})
        return out

    def local_meta(request, queue):
        ru = russian(request)
        if queue not in LOCAL_FIELDS:
            return []
        queue_id, fields = LOCAL_FIELDS[queue]
        out = []
        for key_, name_ru, name_en, kind, options, order in fields:
            entry = {"self": f"{API}/queues/{queue}/localFields/{key_}",
                     "id": f"{queue_id}--{key_}", "key": key_, "type": "local",
                     "name": name_ru if ru else name_en, "version": 1,
                     "schema": {"type": kind, "required": False}, "readonly": False,
                     "options": bool(options), "suggest": False, "order": order,
                     "queue": {"self": f"{API}/queues/{queue}", "key": queue}}
            if options:
                entry["optionsProvider"] = {"type": "FixedListOptionsProvider",
                                            "needValidation": True, "values": options}
            out.append(entry)
        return out

    def fields_route(request, match):
        return denied(request) or Response(200, field_meta(request))

    def local_fields_route(request, match):
        problem = denied(request)
        if problem:
            return problem
        queue = match.group(1).upper()
        if queue not in TYPES:
            return Response(404, {"errorMessages": [f"Очередь {queue} не найдена"],
                                  "statusCode": 404})
        return Response(200, local_meta(request, queue))

    def users(request, match):
        problem = denied(request)
        if problem:
            return problem
        per_page = int(request.arg("perPage", "50"))
        page = int(request.arg("page", "1"))
        everyone = [{"self": f"{API}/users/{p['id']}", "uid": int(p["id"]), "login": p["login"],
                     "display": p["display"], "dismissed": bool(p.get("dismissed")),
                     "external": False}
                    for p in PEOPLE]
        chunk = everyone[(page - 1) * per_page: page * per_page]
        return Response(200, chunk, headers={"X-Total-Count": str(len(everyone))})

    key = r"([A-Za-z][A-Za-z0-9_]*-\d+)"
    router.add("PATCH", rf"/v3/issues/{key}", patch_issue)
    router.add("GET", r"/v3/priorities", priorities)
    router.add("GET", r"/v3/queues/([A-Za-z][A-Za-z0-9_]*)", queue)
    router.add("GET", r"/v3/queues/([A-Za-z][A-Za-z0-9_]*)/components", queue_components)
    router.add("GET", r"/v3/users", users)
    router.add("GET", r"/v3/fields", fields_route)
    router.add("GET", r"/v3/queues/([A-Za-z][A-Za-z0-9_]*)/localFields", local_fields_route)
    router.add("GET", r"/v3/myself", myself)
    router.add("GET", r"/v3/queues/?", queues)
    router.add("POST", r"/v3/issues/_search", search)
    router.add("GET", rf"/v3/issues/{key}", get_issue)
    router.add("GET", rf"/v3/issues/{key}/transitions", transitions)
    router.add("POST", rf"/v3/issues/{key}/transitions/([\w-]+)/_execute", execute)
    router.add("*", rf"/v3/issues/{key}/comments", comments)
    router.add("GET", rf"/v3/issues/{key}/links", links)
    router.add("GET", rf"/v3/issues/{key}/changelog", changelog)
    router.add("*", rf"/v3/issues/{key}/worklog", worklog)
    router.add("POST", r"/_mock/assign", control_assign)
    router.add("POST", r"/_mock/status", control_status)
    router.add("POST", r"/_mock/summon", control_summon)
    router.add("POST", r"/_mock/reject", control_reject)
