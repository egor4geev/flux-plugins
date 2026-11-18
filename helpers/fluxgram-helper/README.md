# fluxgram-helper

FluxGram's program: TDLib, Telegram's library for clients, for the plugin (`plugins/flux.fluxgram`).
A WebAssembly plugin can't run TDLib (native C++ with its own threads and TCP), so the plugin starts
this program through Flux (`process.helper("fluxgram-helper")` → `process.spawn`) and talks to it
in JSON lines. The program is thin: TDLib's JSON interface (`td_json_client`) behind stdin and
stdout, a TDLib client per account. Everything Telegram-specific — the sign-in, the chats, what to
show — is the plugin's. It also renders animated stickers (TGS and WEBM) into Flux's files of frames
(`fluxgram.render`): native code the plugin can't run either.

One file per platform, TDLib and OpenSSL linked in statically (TDLib 1.8.61, the static libraries of
the `tdlib-rs` releases, pinned by SHA-256), and rlottie and libvpx's VP9 decoder built from their
sources (`native/`): `macos-aarch64`, `macos-x86_64`, `windows-x86_64`, `windows-aarch64`. CI (`.github/workflows/helpers.yml`) builds them; a new version of this crate is
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
closes every client first (TDLib's `close`) and finishes the renders asked for, up to 5 s for all of
it, so the databases are left whole and no file of frames half-written.

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

- **`fluxgram.render`** (part 4) — renders an animated sticker once into Flux's file of frames, FXA
  (the layout: `flux-media`'s `fxa` module in the Flux tree — "FXA1", the size, the frames' count
  and length, the offsets, then each frame's BGRA pixels with straight alpha as an LZ4 block): a
  TGS (Lottie in gzip — rlottie) or a WEBM (VP9 with its alpha channel — libvpx), told apart by
  their first bytes. The frames are `size` pixels on the longer side (1–2048; the sticker's
  proportions kept), at most `fps` a second (1–60, 30 by default): the sticker's own frames, every
  other one or fewer when its rate is higher (a TGS of 60 → 30), so they stay evenly paced; a
  Lottie's frames end before its out point (`op`). Renders run on threads of their own, two at a
  time and the rest in turn, never holding up TDLib's lines; one asked for again while it waits or
  runs (the same `output`) is done once and answered for each request. The output's folder is made
  if it's missing; the file is written to `<output>.tmp` and renamed, so it is whole once it is
  there. A sticker of up to 3 s renders in well under a second (on an M-series Mac: a 3 s TGS at
  320 px ~60 ms; a 3 s WEBM of 512 px to 320 ~230 ms).

  ```json
  {"@type": "fluxgram.render", "input": "/…/files/stickers/1.tgs", "output": "/…/cache/frames/AgAD-320.fxa", "size": 320, "fps": 30, "@extra": 9}
  ```

  → `{"@type": "fluxgram.rendered", "output": "/…/AgAD-320.fxa", "width": 320, "height": 320, "frames": 90, "@extra": 9}`,
  or `fluxgram.error` with the reason and the `@extra`: `Not a TGS or WEBM sticker`, `Can't read
  the sticker: …`, `A broken TGS: …`, `A broken WEBM: …` (a cut file, a codec other than VP9, more
  than 8 bits a color), `Can't write the frames: …` (the output's folder, the disk).

- **`fluxgram.quit`** — closes every client and exits with 0, as the end of input does.

### Program → plugin

- **`fluxgram.ready`** — the first line, once TDLib is loaded:
  `{"@type": "fluxgram.ready", "version": "0.2.0", "tdlib": "1.8.61"}`.
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
cargo test --release                      # the protocol's pure parts, the test stickers' renders
python3 tests/live.py                     # on Telegram's test servers (see the file)
```

The static libraries of TDLib come from the pinned archive of the `tdlib-rs` release for the target
(downloaded and checked by `build.rs` into `target/tdlib/`, once), or from `TDLIB_DIR` (a folder with
`lib/`).

- **macOS**: the program needs only the system's libraries and is built for macOS 12, as Flux
  (`.cargo/config.toml`). TDLib's prebuilt libraries are built for macOS 15, but of what they take
  from the system only libc++'s `__libcpp_verbose_abort` is newer than 12 (13.3), and the program
  defines it itself (`td.rs`) — checked by the program's imports, not yet on a macOS 12 machine.
- **Renderers** (`native/`, compiled by `build.rs` with the `cc` crate and the target's own C and C++
  compilers — no CMake, nothing from Homebrew): only the files that are compiled are there, with
  their licenses. Kept in the tree, not downloaded: the build needs no network for them, libvpx's
  configured headers belong with its sources, and what goes into the program is plain to see.
  Portable code only — no assembly or SIMD of their own — so one list of files builds for macOS
  (arm64, x86_64) and Windows (x64, ARM64, MSVC).
  - `native/rlottie` — Samsung's rlottie at commit `ea06d2f` (2026-09-30), the renderer Telegram's
    own apps use (in their fork): MIT (`COPYING` lists which part is under which file of
    `licenses/`: FreeType's rasterizer, pixman, rapidjson, stb_image, Skia's; `zip/` is MIT and
    public-domain miniz). Its `config.h` is the
    program's own: no threads (a frame is drawn on the calling thread; rlottie isn't reentrant so,
    and the program renders one Lottie at a time), no cache of parsed animations, no image plugin.
    Its NEON drawing (32-bit ARM assembly) is off: `-U__ARM_NEON__`.
  - `native/libvpx` — libvpx v1.17.0, the VP9 decoder only: BSD (`LICENSE`, `PATENTS`). Its
    `vpx_config.h`, `vp9_rtcd.h`, `vpx_dsp_rtcd.h`, `vpx_scale_rtcd.h`, `vpx_version.h` and
    `vpx_config.c` come from `configure --target=generic-gnu --disable-vp8 --disable-vp9-encoder
    --disable-examples --disable-tools --disable-docs --disable-unit-tests --disable-multithread`
    (`make` of the headers in a scratch folder), made the same for every compiler (`INLINE`, no
    `pthread.h`); the sources are `make libvpx_srcs.txt`'s C files and the headers they include.
    Without its threads, libvpx sets up its tables unguarded in a decoder's first decode: the
    program's first decodes take turns.
  - `native/vp9.c` — the program's door to the decoder (plain functions: Rust declares none of
    libvpx's structures).

  A new version: the same folders from the new sources (only what's compiled, by the compilers'
  dependency lists), the configure step again for libvpx, then the tests and the smoke test.
- **Windows**: TDLib's prebuilt libraries are built for the C runtime in DLLs (`/MD`), so the program
  needs the Visual C++ runtime (`vcruntime140.dll`, `vcruntime140_1.dll`, `msvcp140.dll`) — unlike
  Flux.exe (`+crt-static`). CI lists the DLLs and warns. A program without it needs TDLib built
  with `/MT` (from source, `TDLIB_DIR`).
