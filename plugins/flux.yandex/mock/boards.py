"""Tracker's boards for the fake Yandex (mock_yandex.py): two boards over the tracker part's tasks.

- 1 "Backend" — kanban over queue TRACK: Open [open, needInfo], In Progress [inProgress],
  Review [inReview], Done [closed, resolved].
- 2 "Web" — scrum over queue WEB: To Do, In Progress, Done, with Sprint 7 (released),
  Sprint 8 (in progress, around today) and Sprint 9 (planned, the next one).

Routes: GET /v3/boards, /v3/boards/<id>, /v3/boards/<id>/columns, /v3/boards/<id>/sprints. The tasks
are the tracker part's (`state.data["tracker"]["issues"]`: key → the task as Tracker returns it;
its `_search` filters by the task's `boards` and `sprint`). Without the tracker part (a test of the
boards alone) this part seeds a few tasks of its own and answers `_search` and `myself` itself.
"""
import datetime

from mock_yandex import Response

ORG_HEADERS = ("x-org-id", "x-cloud-org-id")


def status(key, display):
    return {"id": key, "key": key, "display": display}


def column(id_, name, statuses):
    return {
        "self": f"https://api.tracker.yandex.net/v3/boards/columns/{id_}",
        "id": id_,
        "version": 1,
        "name": name,
        "statuses": statuses,
    }


def boards():
    return {
        1: {
            "board": {"id": 1, "version": 4, "name": "Backend", "orderBy": "updated",
                      "orderAsc": False, "useRanking": False},
            "columns": [
                column(1, "Open", [status("open", "Open"), status("needInfo", "Need info")]),
                column(2, "In Progress", [status("inProgress", "In progress")]),
                column(3, "Review", [status("inReview", "In review")]),
                column(4, "Done", [status("closed", "Closed"), status("resolved", "Resolved")]),
            ],
            "sprints": [],
        },
        2: {
            "board": {"id": 2, "version": 2, "name": "Web", "orderBy": "updated",
                      "orderAsc": False, "useRanking": False},
            "columns": [
                column(5, "To Do", [status("open", "Open")]),
                column(6, "In Progress", [status("inProgress", "In progress")]),
                column(7, "Done", [status("closed", "Closed")]),
            ],
            "sprints": sprints(),
        },
    }


def sprints():
    today = datetime.date.today()
    return [
        {"id": 7, "version": 1, "name": "Sprint 7", "status": "released", "archived": False,
         "startDate": str(today - datetime.timedelta(days=21)),
         "endDate": str(today - datetime.timedelta(days=8))},
        {"id": 8, "version": 1, "name": "Sprint 8", "status": "in_progress", "archived": False,
         "startDate": str(today - datetime.timedelta(days=7)),
         "endDate": str(today + datetime.timedelta(days=7))},
        {"id": 9, "version": 1, "name": "Sprint 9", "status": "draft", "archived": False,
         "startDate": str(today + datetime.timedelta(days=8)),
         "endDate": str(today + datetime.timedelta(days=21))},
    ]


def seed_issues():
    """Tasks of both boards, as the tracker part would have them (used only without it)."""

    def issue(key, summary, status_key, status_name, assignee, priority, board, sprint=None):
        task = {
            "self": f"https://api.tracker.yandex.net/v3/issues/{key}",
            "key": key,
            "summary": summary,
            "status": status(status_key, status_name),
            "type": {"id": "1", "key": "task", "display": "Task"},
            "priority": {"id": "3", "key": priority, "display": priority.capitalize()},
            "queue": {"id": "1", "key": key.split("-")[0], "display": key.split("-")[0]},
            "boards": [{"id": board}],
            "updatedAt": "2026-10-14T09:00:00.000+0000",
        }
        if assignee:
            task["assignee"] = {"id": assignee[0], "display": assignee[1]}
        if sprint:
            task["sprint"] = [{"id": str(sprint), "display": f"Sprint {sprint}"}]
        return task

    me = ("1120000000016772", "Egor Ageev")
    anna = ("1120000000016773", "Anna Petrova")
    tasks = [
        issue("TRACK-142", "Sign-in through SSO fails", "open", "Open", me, "critical", 1),
        issue("TRACK-130", "Timeouts of the export", "needInfo", "Need info", anna, "normal", 1),
        issue("TRACK-120", "Cache the tariffs", "inProgress", "In progress", me, "normal", 1),
        issue("TRACK-99", "Logs of the importer", "inReview", "In review", anna, "minor", 1),
        issue("TRACK-90", "Drop the old API", "closed", "Closed", me, "normal", 1),
        issue("WEB-12", "The landing's header", "open", "Open", me, "normal", 2, 8),
        issue("WEB-14", "Dark theme", "inProgress", "In progress", anna, "major", 2, 8),
        issue("WEB-9", "The old banner", "closed", "Closed", me, "normal", 2, 7),
    ]
    return {task["key"]: task for task in tasks}


def register(router, state):
    def reset():
        state.data["boards"] = boards()
        tracker = state.data.setdefault("tracker", {})
        if not tracker.get("issues"):
            tracker["issues"] = seed_issues()
            state.data["boards-seeded"] = True

    reset()
    state.resets.append(reset)

    def denied(request):
        if not request.headers.get("authorization"):
            return Response(401, {"errorMessages": ["Authorization required"], "statusCode": 401})
        if not any(request.headers.get(name) for name in ORG_HEADERS):
            return Response(403, {"errorMessages": ["The organization isn't given"],
                                  "statusCode": 403})
        return None

    def board_of(match):
        return state.data["boards"].get(int(match.group(1)))

    def list_boards(request, match):
        return denied(request) or Response(200, [
            {**entry["board"], "self": f"https://api.tracker.yandex.net/v3/boards/{id_}",
             "columns": [{"id": str(c["id"]), "display": c["name"]} for c in entry["columns"]]}
            for id_, entry in state.data["boards"].items()
        ])

    def get_board(request, match):
        entry = board_of(match)
        if not entry:
            return Response(404, {"errorMessages": ["No such board"], "statusCode": 404})
        return denied(request) or Response(200, entry["board"])

    def columns(request, match):
        entry = board_of(match)
        if not entry:
            return Response(404, {"errorMessages": ["No such board"], "statusCode": 404})
        return denied(request) or Response(200, entry["columns"])

    def board_sprints(request, match):
        entry = board_of(match)
        if not entry:
            return Response(404, {"errorMessages": ["No such board"], "statusCode": 404})
        return denied(request) or Response(200, entry["sprints"])

    router.add("GET", r"/v3/boards", list_boards)
    router.add("GET", r"/v3/boards/(\d+)", get_board)
    router.add("GET", r"/v3/boards/(\d+)/columns", columns)
    router.add("GET", r"/v3/boards/(\d+)/sprints", board_sprints)

    # Without the tracker part: its search and `myself`, enough for the board (routes registered
    # after the tracker part's never answer when it's there).
    def search(request, match):
        problem = denied(request)
        if problem:
            return problem
        body = request.json() or {}
        wanted = body.get("filter") or {}
        found = []
        for task in state.data["tracker"]["issues"].values():
            board = wanted.get("boards")
            if board is not None and not any(
                str(b.get("id")) == str(board) for b in task.get("boards", [])
            ):
                continue
            sprint = wanted.get("sprint")
            if sprint is not None and not any(
                str(s.get("id")) == str(sprint) for s in task.get("sprint", [])
            ):
                continue
            found.append(task)
        per_page = int(request.arg("perPage", "50"))
        page = int(request.arg("page", "1"))
        pages = max(1, -(-len(found) // per_page))
        chunk = found[(page - 1) * per_page: page * per_page]
        return Response(200, chunk, headers={"X-Total-Count": str(len(found)),
                                             "X-Total-Pages": str(pages)})

    def myself(request, match):
        return denied(request) or Response(200, {
            "uid": 1120000000016772, "login": "egor.ageev", "trackerUid": 1120000000016772,
            "passportUid": 1120000000016772, "display": "Egor Ageev",
            "email": "egor.ageev@example.ru",
        })

    router.add("POST", r"/v3/issues/_search", search)
    router.add("GET", r"/v3/myself", myself)
