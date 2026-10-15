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
the users (one of them dismissed).

The query language as far as the plugin uses it: `Assignee: me()`, `Resolution: empty()`,
`Followers: me()`, `"Pending reply from": me()`, `Queue: KEY`, `Key: KEY`, `Summary: "words"`,
`Boards: N`, `"Sort by": Updated DESC|ASC`; other clauses are ignored (and printed).

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
        return Response(200, [public(task) for task in chunk],
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
