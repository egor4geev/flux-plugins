# fluxgram-helper

FluxGram's program: TDLib, Telegram's library for clients, for the plugin (`plugins/flux.fluxgram`).
A WebAssembly plugin can't run TDLib (native C++ with its own threads and TCP), so the plugin starts
this program through Flux (`process.helper("fluxgram-helper")` → `process.spawn`) and talks to it
in JSON lines. The program is thin: TDLib's JSON interface (`td_json_client`) behind stdin and
stdout, a TDLib client per account. Everything Telegram-specific — the sign-in, the chats, what to
show — is the plugin's.

One file per platform, TDLib and OpenSSL linked in statically (TDLib 1.8.61, the static libraries of
the `tdlib-rs` releases, pinned by SHA-256): `macos-aarch64`, `macos-x86_64`, `windows-x86_64`,
`windows-aarch64`. CI (`.github/workflows/helpers.yml`) builds them; a new version of this crate is
released as `fluxgram-helper-v<version>` with the four files gzipped; the plugin's manifest names
them with their SHA-256 (`[[helpers]]`).

## Running

```
fluxgram-helper [--log <file>] [--verbosity <0–5>] [--stats]
```

- `--log` — TDLib's own log, a file (TDLib rotates it at 10 MB); none — TDLib logs nothing.
- `--verbosity` — TDLib's log level: 1 (errors) by default; 3–4 to see the network.
- `--stats` — every write's size on stderr (`fluxgram-helper: a write of 12 lines, 5304 bytes`):
  for measuring the batches.

The program ends when its input ends (Flux stopped the plugin, or quit) or on `fluxgram.quit`: it
closes every client first (TDLib's `close`, up to 5 s for all of them), so the databases are left
whole.

## Protocol

JSON Lines both ways: UTF-8, one object per line, `\n` at its end. Nothing else ever goes to stdout;
the program's own messages (start, a failure) go to stderr.

### Plugin → program

- **A TDLib request** — any object of TDLib's API with `"@client": "<account>"`: the account's key,
  the plugin's own string ("a1"). The first request with a new key creates TDLib's client for it
  (`td_create_client_id`); the program passes the object on without `@client`. `@extra` comes back
  with the answer, as TDLib does it.

  ```json
  {"@client": "a1", "@type": "getOption", "name": "version", "@extra": 1}
  ```

- **`fluxgram.execute`** — TDLib's synchronous requests (`td_execute`): `getTextEntities`,
  `parseTextEntities`, `getMarkdownText`, `setLogVerbosityLevel`…

  ```json
  {"@type": "fluxgram.execute", "query": {"@type": "getTextEntities", "text": "…"}, "@extra": 7}
  ```

  → `{"@type": "fluxgram.executed", "result": {…}, "@extra": 7}`

- **`fluxgram.keep`** — only updates of these types are passed on, for every client: what the
  plugin uses; the rest (`updateUserStatus` of every contact, `updateOption`…) would only wake it,
  and TDLib adds new ones with its versions. Only updates (`update…`) are held back — answers always
  come — and never a client's `authorizationStateClosed` (with `fluxgram.closed` after it), kept or
  not. Until the first `fluxgram.keep` or `fluxgram.drop` everything passes.

  ```json
  {"@type": "fluxgram.keep", "types": ["updateAuthorizationState", "updateUser", "updateFile"]}
  ```

- **`fluxgram.drop`** — the other way round, kept for compatibility: updates of these types aren't
  passed on, the rest are; an empty list passes everything. A new `fluxgram.keep` or
  `fluxgram.drop` replaces the one before, whichever it was.

  ```json
  {"@type": "fluxgram.drop", "types": ["updateUserStatus", "updateChatAction"]}
  ```

- **`fluxgram.chats`** — the chats of an account whose updates pass: the chats the plugin shows
  (in its list, or open). For that account's client an update about another chat is held back, so
  the rest of Telegram (a busy group, a channel) never wakes the plugin. The update's chat is its
  `chat_id`, else its message's (`updateNewMessage`, `updateMessageSendSucceeded`), else the new
  chat's (`updateNewChat`), and for `updateUserStatus` the person's (`user_id`: a person's chat has
  the person's id). Updates about no chat (`updateUser`, `updateFile`, `updateSupergroup`…) pass,
  and answers always do — `getChat` of any chat is answered. `"chats": null` lets every chat's
  updates pass again, an empty list none. It may come before the account's client is made (the
  start's burst is filtered already) and goes with the client (`fluxgram.closed`): a new client of
  the key starts without one. It works after `fluxgram.keep` or `fluxgram.drop`, not instead.

  ```json
  {"@type": "fluxgram.chats", "@client": "a1", "chats": [101, -1000000001001]}
  ```

- **`fluxgram.quit`** — closes every client and exits with 0, as the end of input does.

### Program → plugin

- **`fluxgram.ready`** — the first line, once TDLib is loaded:
  `{"@type": "fluxgram.ready", "version": "0.1.0", "tdlib": "1.8.61"}`.
- **Every answer and update of TDLib**, as TDLib gives it, with `"@client": "<account>"` in place
  of TDLib's `@client_id`:

  ```json
  {"@client": "a1", "@type": "optionValueString", "value": "1.8.61", "@extra": 1}
  {"@client": "a1", "@type": "updateAuthorizationState", "authorization_state": {"@type": "authorizationStateWaitTdlibParameters"}}
  ```

- **`fluxgram.closed`** — the client of the account is gone (after `authorizationStateClosed`: a
  `close`, a `logOut`, or the account was signed out elsewhere); a new request with the key makes a
  new client: `{"@type": "fluxgram.closed", "@client": "a1"}`. Until this line a request with the
  key still goes to the closing client (TDLib answers it with an error): wait for it.
- **`fluxgram.error`** — a line the program couldn't take (not JSON, not an object, no `@client` or
  not a non-empty string, an unknown `fluxgram.*`, a `fluxgram.execute` without `query`, a
  `fluxgram.keep` or `fluxgram.drop` without its `types`, a `fluxgram.chats` without `@client` or
  `chats`):
  `{"@type": "fluxgram.error", "message": "…", "@extra": …}` (`@extra` when the line had one). Empty
  lines are skipped.

### Batches

The program waits in `td_receive` (no timers, no polling: an idle program sleeps) and writes what
TDLib gives in batches: everything that is ready at once — a burst of updates at the start, a page
of history — in one write and one flush, up to about 1 MB. The plugin gets a burst as one
`process-output`, not a call per line.

## Files

The plugin gives TDLib its folders in `setTdlibParameters`: the account's database and files under
the plugin's data folder (`<data>/accounts/<account>/{db,files}`) — Flux shows pictures only from
there — and the database's encryption key, which the plugin keeps in Flux's secrets.

## Building

```
cargo build --release                     # this Mac's platform
cargo build --release --target x86_64-apple-darwin
cargo test --release                      # the protocol's pure parts
python3 tests/live.py                     # on Telegram's test servers (see the file)
```

The static libraries of TDLib come from the pinned archive of the `tdlib-rs` release for the target
(downloaded and checked by `build.rs` into `target/tdlib/`, once), or from `TDLIB_DIR` (a folder with
`lib/`).

- **macOS**: the program needs only the system's libraries and is built for macOS 12, as Flux
  (`.cargo/config.toml`). TDLib's prebuilt libraries are built for macOS 15, but of what they take
  from the system only libc++'s `__libcpp_verbose_abort` is newer than 12 (13.3), and the program
  defines it itself (`td.rs`) — checked by the program's imports, not yet on a macOS 12 machine.
- **Windows**: TDLib's prebuilt libraries are built for the C runtime in DLLs (`/MD`), so the program
  needs the Visual C++ runtime (`vcruntime140.dll`, `vcruntime140_1.dll`, `msvcp140.dll`) — unlike
  Flux.exe (`+crt-static`). CI lists the DLLs and warns. A program without it needs TDLib built
  with `/MT` (from source, `TDLIB_DIR`).
