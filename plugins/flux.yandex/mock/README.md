# A fake Yandex for tests and scenarios

`mock_yandex.py` answers as Yandex ID's OAuth and Tracker's API v3 do, on
`http://127.0.0.1:<port>`, with tasks of its own. Flux sends
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
  The query language also takes `Updated: >= now()-15m` (`>=` `<=` `>` `<`; `now()` ± N `m` `h`
  `d` `w`, or `"2026-10-28 09:30:00"` / `"2026-10-28"` in UTC, a date alone — its whole day;
  `"from".."to"`), by the task's `updatedAt` (the controls above change it).
  `POST /v3/issues/_search?fields=key,summary,status` — each task has only `self`, `id`, `key`
  and the fields named, as Tracker does: a field the client forgets isn't there.
- `boards.py` — the boards (agent K).
- `GET /_mock/log` — the requests so far: `method`, `path` (with the query), `body`, `t` (seconds
  since the fake started, at the arrival), `wall` (Unix time), `ms` (time to answer), `status`,
  `bytes` (the answer's body); `DELETE /_mock/log` or `POST /_mock/log/clear` — empty it.
  `POST /_mock/reset` — the data and the controls below back to the start (the log stays).
- Closer to the real Tracker (`GET /_mock/controls` — their state):
  - `POST /_mock/latency {"ms": 150, "searchMs": 400}` — each answer waits (a `_search` —
    `searchMs`, `ms` when not given); requests wait in parallel, outside the server's lock;
  - `POST /_mock/strict {"search": true}` — a `_search` that arrives while another one is in
    flight answers 429 (Tracker allows one search at a time per user);
  - `POST /_mock/throttle {"count": 2, "retryAfter": 3, "path": "_search"}` — the next `count`
    requests (whose path with its query matches the regex `path`, when given) answer 429
    `{"errorMessages": ["Too many requests"]}`, with `Retry-After` when `retryAfter` is given.
