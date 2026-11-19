# Flux Remote

Your Claude sessions from the phone, through your own Telegram bot: a topic per session, Claude's
answer as it is written, permissions and questions by buttons, a new session in any project — or from
a task of your tracker. No server: Flux talks to Telegram itself, and the bot answers only you.

## Setting up

1. In Telegram, make a bot with **@BotFather** (`/newbot`) and turn on its **topics** there (the bot's
   settings in @BotFather's app).
2. **Settings → Plugins → Flux Remote → Bot Token** — paste the token @BotFather gave.
3. The **Binding** block shows a link and its QR code: open it on your phone and press **Start**. Whoever
   opens it first is the owner; the bot answers nobody else. The link lives 15 minutes (**New Link**
   makes another one).
4. The bot opens **General** — its console. Pin it to the top of the list (only you can).

## In Telegram

- **A new chat** — write outside the topics: the bot asks for the project (the ones Flux knows), and —
  when a tracker plugin gives tasks — whether to start from a task: **My tasks**, **Find…** (a key or
  words), or a key right in your message. A task offers the tracker's own Claude actions (Take to Work
  moves it, makes its branch and hands Claude its prompt — asking nothing; if local changes keep the
  branch from being checked out, the branch is left alone and the topic says so) and **Discuss** — a
  session in plan mode with the task attached.
- **A session's topic** — write, and it goes to the session; Claude's answer comes as it is written
  with **Stop**; the steps fold into one block; permissions, Claude's questions and the plan come as
  buttons. A finished turn shows its files: **Diff** sends the turn's changes as a file, **Roll back**
  asks first, then puts the files back. Photos and files go to the session.
- **General** — every session's state in one message; while your computer is locked, every session's
  questions come here. It also tells when the computer sleeps and wakes, and when Claude isn't
  available from your country (with **Check Again**).
- **Commands** (the «/» menu): `/sessions` — every session (a press brings its topic to the top, or
  makes one with **Continue here**); `/rename` — the session's name in Flux and the topic's;
  `/delete` — closes the session in Flux (it stays among the recent ones) and deletes the topic;
  `/mode` — the permission mode (**No checks** asks first); `/usage` — Claude's limits; `/settings`.
- Messages written while Flux was closed or the computer slept wait for your word: **Run** or
  **Don't run** (or always run, or never — a setting).

## In Flux

- **Continue in Telegram** at a session in the Claude console (⌘Esc) gives it a topic with its last
  answer.
- While remote sessions work, Flux keeps the computer awake; **Don't sleep on the power adapter while
  the bot is on** keeps it awake all the time on the adapter. A closed lid still puts a laptop to sleep.
- The settings: the token, the connection's state, the binding (**Unbind**), the messages that came
  while Flux was closed.
