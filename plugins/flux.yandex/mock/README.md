# A fake Yandex for tests and scenarios

`mock_yandex.py` answers as Yandex ID's OAuth, Tracker's API v3 and the Calendar's CalDAV do, on
`http://127.0.0.1:<port>`, with tasks and meetings of its own (meetings relative to now). Flux sends
the plugin's requests there with `FLUX_PLUGIN_HTTP_OVERRIDE` (a scenario's or a test's
environment); `FLUX_SCENARIO_OPEN_URL=<file>` writes the sign-in's browser address to a file, and
the scenario plays the browser: `curl -s -L "$(tail -1 <file> | sed 's|https://oauth.yandex.ru|http://127.0.0.1:<port>|')"`
— the mock's `/authorize` redirects to the plugin's callback at once.

- `oauth.py` — `/authorize` (PKCE S256), `/token` (the verifier checked), `/info`;
  `POST /_mock/oauth {"deny": true}` — the next sign-in is refused.
- `tracker.py` — Tracker v3 with Russian sample data: queues TRACK (board 1) and WEB (board 2,
  sprints 7, 8 and 9), 12 tasks with links, comments, a changelog; the plugin's settings need the
  organization `123456` (Yandex 360) or `bpf-mock` (Cloud); a pasted token `y0_…` works.
  The card's editing: `PATCH /v3/issues/<key>` (an unknown field or a wrong value — 400 with
  `errors`, as Tracker), priorities, the queue's types (`?expand=issueTypesConfig`) and
  components, users (a dismissed one among them). The fields: `GET /v3/fields` (the system ones,
  named by `Accept-Language`, with categories) and `GET /v3/queues/<key>/localFields` (TRACK has
  six — a list, a line, a number, a date, a person, a text — WEB one); TRACK-142 has a value in
  most of them, and `PATCH` changes the simple ones by their metadata (a local field by its key in
  the task, `<queue's id>--<key>`). Tags and components on most TRACK tasks, for the board's
  filters.
  `POST /_mock/assign {"key"}`, `/_mock/status {"key", "status"}`, `/_mock/summon {"key"}` play
  the news polling finds; `/_mock/reject {"field", "message"}` refuses the next change of a field.
- `boards.py` — the boards (agent K); `caldav.py` — the calendar (agent G).
- `GET /_mock/log` — the requests so far; `POST /_mock/reset` — the data back to the start.
