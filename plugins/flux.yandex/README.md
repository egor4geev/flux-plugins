# Yandex Tracker & Calendar

Yandex Tracker and Yandex Calendar in Flux: your tasks on a board, work on a task with its branch and
its own tabs, and the day's meetings in the status bar.

## Signing in

**Settings → Plugins → Yandex Tracker & Calendar**:

- **Sign In with Yandex…** opens Yandex ID in the browser; after you allow access, the browser comes
  back to Flux on this Mac (`http://127.0.0.1:52580/callback`, or 52581, 52582 when it's busy) and the
  page says you're signed in. The token is kept in the macOS keychain.
- Instead: paste an **OAuth token** (with `tracker:read`, `tracker:write`, `calendar:all`), or an
  **IAM token** for a federated account of Yandex Cloud (`yc iam create-token`, it lives 12 hours).
- **Organization**: its ID (Tracker → Administration → Organizations) and its type — by the ID (only
  digits — Yandex 360), Yandex 360 or Yandex Cloud. **Check Connection** says who you are in Tracker.
- **Advanced → ClientID**: your organization's own Yandex ID app instead of Flux's (an app "for user
  authorization" with the three redirect addresses above).

## Tasks

The **Tracker** window on the right (its icon in the launchpad):

- **The list**: My Tasks (the query of the settings), Following, a queue's open tasks or a query of
  your own — the menu at the top; tasks grouped by status.
- **The card** of a task (a double click, ↵, a link): the key and the title; the status — click it for
  its transitions; type, priority, assignee, deadline, sprint, the parent task — and after them every
  other field the task has, as Tracker's own page lists them: the system fields (start and end,
  estimates and time spent, versions, the QA engineer, the project, the epic…) in Tracker's order,
  then the queue's own fields, named as Tracker names them (**Fewer Fields** keeps only the main
  ones). Click a field with ▾ to change it: a person, a date, one value of a list, a line or a number.
  Then the description; **Links** grouped by relation (subtasks, blocks, depends on, relates…), each opens its card;
  **Comments** with a field for a new one (⌘↵ sends) and **History** — who changed what, from what to
  what.
- **A button per Claude action** of the settings. An action sends Claude its prompt with the task's
  data in it: `{key}`, `{summary}`, `{description}`, `{url}`, `{status}`, `{type}`, `{priority}`,
  `{assignee}`, `{author}`, `{queue}`, `{deadline}`, `{sprint}`, `{parent}`, `{branch}`, `{links}`,
  `{comments}`. An action marked **Take the task to work first** does it in one click, as **Take to
  work** in the settings says: moves the task to In Progress, checks out its branch (an existing one
  whose name starts with the key, or a new one by the branch template), switches to the task's tabs,
  starts its timer — then Claude gets the prompt. The action marked **On the board** is the board's
  button. Two come with the plugin: Take to Work and Plan (plan mode).
- **Discuss with Claude**: a new Claude session with the task attached; you write what to do.

**The task in work** is in the status bar — `TRACK-142 · 0:42` — with a timer counted while Flux
runs; its menu: Show, the actions, Log Time…, pause, Close…, Open in Browser. While a task is in work
the commit message starts with it (the commit template of the settings). **Close** logs the time (the
timer's, editable: `1h 25m`, `1ч 25м`, `85m`, `1.5h`), moves the task, switches back to the branch
and the tabs from before the task.

**Open Task** (⌥⇧N) finds a task by its key or words; **⌥⇧B** opens the task in the browser.

Task keys of your queues — `TRACK-142` — are links in the editor (⌘-click; a hover tells the title,
the status and the assignee), in commit messages and in Claude's chat; a click opens the card.

Every few minutes (the settings say how often) the plugin checks for news: tasks newly assigned to
you, status changes of your tasks, and tasks where you're summoned — a notification with Open.

## The board

**Tracker: Open Task Board** opens the board of tasks as a tab among the editor's tabs, as boards
are in Tracker: the board's columns side by side, each with the tasks in its statuses — the key
and the title, the type and the assignee, the priority unless it's the usual one; the task you're
working on is marked ▶. A task in a status no column takes goes to a last column, Other statuses,
rather than out of sight. A scrum board shows its current sprint; a board made of a filter (a queue,
tags) shows what the filter finds.

- **The board** — the menu in the toolbar lists your boards: your favorite boards first, then all
  the others by name. ☆ next to it makes the shown board a favorite. The tab opens the board you
  chose last — at first, your first favorite, or the most recently changed board.
- **Filters**, as on Tracker's boards: **Only mine**, then **Assignee**, **Type**, **Priority**,
  **Tags** and **Components** — each lists the values the board's tasks have, with how many tasks
  show if you choose it; choose one, then another (the menu stays open, Esc closes it): several values
  of one filter show any of them, the filters together narrow each other. The field in the toolbar
  filters by words of the key or the title. A column says how many of its tasks show — "2 of 5";
  × clears the filters. Each board keeps its filter until you change it.
- **Saved filters** — save the current filter of a board under a name and choose it later from the
  same menu (or delete it there).
- **Take to Work** — the favorite Claude action of the settings (the one marked On the board; its
  name is the button's) on the selected task: it moves the task, makes its branch, opens its tabs
  and hands Claude the prompt, as the settings say.
- A task opens in the Tracker window on the right with a double click or ↵.
- The board refreshes when its tab shows, every two minutes while it shows, and when a task on it
  changes in Flux; ⟳ refreshes it now, the globe opens it in Tracker.

## Calendar

Your Yandex Calendar meetings, without leaving the code:

- **The next meeting in the status bar** — "Daily in 12 min", "Daily now · 5 min left", "Review at
  15:00"; nothing when the rest of the day is free. The tooltip has its time, place and whether it
  is on Telemost.
- **The agenda** — click the meeting in the status bar (or **Calendar: Show Today's Meetings**) for
  today's and tomorrow's meetings; choose one to join its Telemost conference (or the link in its
  place or description), or to open the day in Yandex Calendar when it has none.
- **A reminder** a few minutes before a meeting (**Remind before a meeting**, 5 by default; 0 turns
  it off), with **Join**. All-day events are in the agenda, not reminded of.
- **Calendar: Join Meeting** — the meeting going on, or the next one within the hour.

Meetings you declined and cancelled ones are left out; recurring meetings, moved and cancelled
occurrences and time zones are read as the calendar has them. **Hidden calendars** (by name) keeps
a calendar out — say, a colleague's you subscribed to.

The calendar uses the sign-in's token. If your organization's admin doesn't allow it, or your
account is a federated one, make an app password for CalDAV in Yandex ID (Security → App passwords
→ Calendar) and put your `login@domain` and the password into **CalDAV login** and **App password
for CalDAV** — a new app password starts working in a couple of hours.
