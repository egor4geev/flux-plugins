# Yandex Tracker

Yandex Tracker in Flux: your tasks on a board, work on a task with its branch and its own tabs, and
your own Claude actions on a task.

## Signing in

**Settings → Plugins → Yandex Tracker → Sign In with Yandex…** — that's all it takes:

- The browser opens Yandex ID's page; once you allow access, it comes back to Flux on this Mac and
  says you're signed in. Flux asks for Tracker's rights and who you are, and — if you allow it — for
  the list of your organizations. Flux keeps the token.
- Flux then finds your organization by itself: one is taken, several are offered to choose from;
  if it can't find any (you declined the list, or the organization is in Yandex Cloud), it shows where
  the ID is — Tracker → Administration → Organizations — and asks for it. Then it checks the
  connection, and the page says who you are and which organization Tracker works in.
- **Change Organization…** — another organization of yours, or an ID by hand.
- While Flux waits for the browser, a notification has **Cancel**; if the browser doesn't come back
  in a while, it suggests the other ways.

**Other Ways…** — when the browser sign-in doesn't suit you:

- **OAuth token** — if you already have a Yandex ID token with `tracker:read` and `tracker:write`.
- **IAM token** — for a federated account of Yandex Cloud: `yc iam create-token` gives one; it lives
  12 hours, then Flux asks for a new one.
- **Own app** — when your organization allows only its own Yandex ID app: its ClientID, and what it
  is for. An app "for user authorization" signs in in the browser (it needs the redirect URIs
  `http://127.0.0.1:52580/callback`, `:52581/callback` and `:52582/callback`); an app "for API access
  or debugging" shows a code in the browser — paste it into Flux.

A new sign-in replaces the previous one; **Sign Out** forgets it. Flux renews the token before it
expires when Yandex allows; when it can't — or Yandex stops accepting the token — a notification
offers to sign in again.

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
