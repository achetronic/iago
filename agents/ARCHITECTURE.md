# Architecture

## What Iago is

A dictation front end for Wayland desktops. It captures audio, sends it to
Parakeet, and puts the text where the user wants it. Parakeet runs as a child
process with the models next to it: no container, no Python, no network hop.

## Layers

```
          ┌─────────────┐        ┌──────────────┐
          │  iago ui    │        │  iago ctl    │
          └──────┬──────┘        └──────┬───────┘
                 │                      │
                 │        ┌─────────────┴──────────┐
                 │        │  ipc (unix socket)     │
                 │        └─────────────┬──────────┘
                 └────────┬─────────────┘
                    ┌─────▼──────┐
                    │   daemon   │  single owner of state
                    └─────┬──────┘
                    ┌─────▼──────┐
                    │ dictation  │  capture -> wav -> transcribe -> deliver
                    └──┬───┬───┬─┘
             ┌─────────┘   │   └──────────┐
        ┌────▼───┐    ┌────▼────┐    ┌────▼───┐
        │ audio  │    │   asr   │    │ input  │   ports (traits)
        └────┬───┘    └────┬────┘    └────┬───┘
             └─────────────┼──────────────┘
                    ┌──────▼──────┐
                    │  platform   │  the only OS-aware layer
                    └─────────────┘
```

Rules:

- Only `platform` names an operating system.
- Everything else depends on `audio::AudioBackend`, `asr::Transcriber`,
  `input::TextSink` and `input::hotkey::HotkeyService`.
- `daemon` is the only owner of mutable state. Clients send requests through
  `ipc`; nothing else records or delivers.

## Why a daemon

A shortcut that runs `iago ctl` starts a new process every time. Three things must
outlive it:

1. The audio stream, so a recording can span two invocations.
2. The clipboard selection: on Wayland it is served by the process that owns it.
3. The model process, which takes seconds to load the weights.

The daemon owns all three and `iago ctl` is a one-line client. The window follows
the same rule: it renders a snapshot and posts requests, so closing it never
interrupts a dictation.

## Ports

### Audio

```rust
pub trait AudioBackend: Send + Sync {
    fn id(&self) -> &'static str;
    fn spec(&self) -> AudioSpec;
    fn open(&self) -> Result<CaptureHandle>;
}
```

Capture runs on its own thread and delivers mono `f32` frames through a channel, so
the async side never blocks on a device.

Today `audio::pw_cat` drives the official PipeWire client with
`--record --raw --format f32`, which gives node targeting and rate negotiation
without a C dependency. A `pipewire` crate backend would be local to that file, and
it is also the point where an `echo-cancel` source becomes possible.

### Recognition

```rust
pub trait Transcriber: Send + Sync {
    fn id(&self) -> &'static str;
    async fn health(&self) -> Result<()>;
    async fn transcribe(&self, wav: Vec<u8>, language: Option<&str>) -> Result<Transcript>;
}
```

`asr::parakeet` is the HTTP client (OpenAI-compatible multipart). `asr::sidecar`
owns the process: free loopback port, random API key, `/health` polling, kill on
drop. `asr::Engine` boots the sidecar on the first transcription, caches the client
and stops the process after the configured idle time.

Speaker diarization exists on the server (`diarize=true`, up to 8 speakers) and is
deliberately unused: it belongs to conversation history, not to dictation.

### Delivery

Two targets, built from the same two primitives:

| target | what happens |
| --- | --- |
| `clipboard` | the transcript is copied, nothing else happens |
| `focus` | the previous clipboard is saved, the text is copied, the paste chord is pressed into the focused window, and the clipboard is restored |

```rust
pub trait TextSink: Send + Sync { fn deliver(&self, text: &str) -> Result<Delivery>; }
pub trait Clipboard: Send + Sync { fn read(&self) -> Result<Option<String>>; fn write(&self, text: &str) -> Result<()>; }
```

Both are actors: a thread owns the handle and receives commands. The Wayland
clipboard dies with its owner process, and the simulated-input connection (libei
through the portal) must be created once per session, not once per delivery, or the
permission dialog and the remote-control indicator show up every time.

Only one combination is pressed, the one that pastes, through `enigo`, which picks
at runtime:

1. `zwp_virtual_keyboard_manager_v1` on wlroots and COSMIC.
2. `libei` through the `RemoteDesktop` portal on GNOME and KDE.

No `/dev/uinput`, no root, no `ydotool`, and no attempt to type the transcript. The
reason is in [`DECISIONS.md`](DECISIONS.md): a keycode only means something against
the keymap of the session, and half the characters of a language are not on it.

### Hotkeys

```rust
pub trait HotkeyService: Send + Sync {
    fn id(&self) -> &'static str;
    fn capabilities(&self) -> HotkeyCapabilities;
    fn spawn(&self, events: Sender<HotkeyEvent>) -> Result<()>;
}
```

Two mechanisms, chosen by `platform/` so nothing above it knows which is in use:

- `portal`: the XDG `GlobalShortcuts` portal, which reports press **and** release.
  This is what push-to-talk needs, and it is preferred wherever it exists.
- `gnome`: the desktop's own custom keybindings, which run `iago ctl`. Press only,
  and the fallback when there is no portal. It needs no window and no dialog.

A compositor binding is not code: the user writes two lines calling
`iago ctl shortcut-press` and `shortcut-release`, and the daemon receives the same
gestures.

`HotkeyCapabilities` answers one question: does this mechanism report the release?
The settings page asks it to decide whether hold-to-talk can be offered at all,
instead of guessing with desktop-specific rules.

`Trigger` owns the spelling. The setting is stored once, as `CTRL+SHIFT+d`, and
rendered where it is needed: `<Control><Shift>d` for the portal, `spec()` for
compositor configs and for humans.

A mechanism that breaks after it started sends `HotkeyEvent::Unavailable(reason)`,
so the status stops claiming a shortcut that no longer works.

Changing the combination stops the mechanism and starts a new one, and the portal
client lives in its own process (see below) precisely so that the desktop lets go of
the old grab.

Reading `/dev/input` directly is intentionally not implemented.

## The control protocol

One line of JSON out, one line back, over a Unix socket in `$XDG_RUNTIME_DIR`.
There is no version field: the socket is private and both ends ship together, so a
number nobody negotiates is only a lie with a date on it.

```json
{"type": "push-to-talk", "action": "shortcut-press", "meta": {"source": "ambassador"}}
```

- `type` names the kind of event that produced the request: `command` for a person
  or a script, `push-to-talk` for a key being held. It is free-form, so a new
  gesture such as a double tap does not need a protocol change.
- `action` is the verb, and its name is the one the command line takes:
  `recording-toggle`, `recording-start`, `recording-stop`, `shortcut-press`,
  `shortcut-release`, `daemon-status`, `daemon-alive`, `settings-reload`,
  `engine-start`, `engine-stop`, `engine-restart` and `daemon-shutdown`.
- `params` carries whatever an action needs, and `meta` says who is asking, which
  is what the log shows when something fires at the wrong time.

The daemon decides what a gesture means from its own settings: a `shortcut-press`
toggles in toggle mode and opens the microphone in hold mode, and
`shortcut-release` only ends a hold. The sender never has to know the mode.

### The answer

One envelope for everything:

```json
{"status": "ok", "state": { }, "message": "settings reloaded"}
```

- `status` is `ok` or `error`. A boolean called `ok` says nothing about what went
  wrong; a word leaves room for a third outcome later without changing the field.
- `state` is the daemon after handling the request: phase, elapsed time, engine,
  last transcript, and who is delivering the shortcut (`shortcut.mechanism`:
  `portal`, `gnome`, or absent). It is omitted when the caller has nothing to
  paint. State carries only what changes on its own; the combination and the mode
  are settings, and the client that needs them reads the settings file.
- `message` is why it failed, or what happened.

The smallest answer, what a liveness check gets, is `{"status":"ok"}`. That is
`daemon-alive`: the ambassador asks it every five seconds and only needs to know
that something answered.

## The ambassador

Some resources belong to the process that asks for them. The portal ties a grabbed
shortcut to the client that requested it and only lets go when that client is gone,
so a client living inside the daemon would make changing the combination mean
restarting Iago.

The work that needs its own process therefore lives in `iago daemon ambassador`,
which the daemon starts, watches and replaces. It reads the settings itself, on
purpose: the daemon only has to start it. Today it holds one thing, the portal
shortcut client.

It also turns the portal's stream of events into gestures: the portal reports one
`Activated` per key repeat, and the client forwards the edges only, plus the release
the portal sometimes forgets.

## The desktop entry

xdg-desktop-portal identifies a host application by matching the process against
installed `.desktop` files. Consequences:

- The entry must be installed (`make install`), otherwise GNOME answers
  `An app id is required` when the shortcut is bound.
- Its `Exec` must resolve, and point at a binary that exists: GIO refuses to build
  app info for an entry whose executable cannot be found.
- `ashpd::register_host_app` is also called before binding, which helps but does
  not replace the entry.

The Dependencies page checks for it.

## One binary, two processes

```
iago    CLI + daemon + window, all in one executable
```

The window is `iago ui`, a hidden subcommand. Running `iago` with no arguments
spawns `iago ui` as a separate process and waits for it. The split is by process,
not by binary: the window is a pure client of the daemon, and a crash in it never
takes a dictation down.

It used to be two binaries because the window's toolkit and the portal code could
not share a process. They drive different D-Bus backends, and cargo unified them
into one build that panicked with "there is no reactor running". The portal code
and the window now use different zbus major versions, so both can live in the same
binary and never touch.

One file to ship, one command to run.

## Theme

Two themes fixed in the repository: `themes/iago-clay.json` and
`themes/iago-clay-dark.json`. Same palette family, same radii, one for light and one
for dark.

Colours are not configurable. `config.toml` has one key, `theme.name`:

| value | meaning |
| --- | --- |
| `auto` | follow the desktop appearance (the default) |
| `mint` | the light theme |
| `mint-dark` | the dark theme |

The two names predate the clay palette and stayed, because renaming them would
rewrite the settings file of every user for no gain.

The window has a sun/moon toggle in the title bar. It writes `mint` or `mint-dark`,
so the choice survives a restart, and `auto` is what you get before touching it. The
UI warns about an unknown name.

A test asserts that both files define the same tokens, so switching can never leave
a surface without a colour.

## UI rules

Typography, spacing, colour and copy are in [`STYLE.md`](STYLE.md), and the
reusable pieces live in `ui/src/components.rs`.

## Settings descriptor

The settings page is generated, not written. `ui/settings.toml` declares every
option: section order, label, control, help text, bounds, options and a
`visible_when` condition.

```toml
[[section]]
title = "Recognition"

[[section.field]]
key = "asr.endpoint"
label = "Endpoint"
control = "text"
help = "only used in remote mode"
visible_when = "asr.mode == remote"
```

Rules:

- `iago::config` is the source of truth for values, types and defaults. The
  descriptor only decides presentation, and it is embedded with `include_str!`, so
  a broken file cannot ship.
- Edits go through `serde_json::Value` addressed by the dotted `key`, and the typed
  settings are rebuilt with `serde` on save. A typo in a key, or an option the enum
  does not accept, fails at save time and in the tests, never silently.
- `cargo test` enforces the link: every key must exist, every control
  must match the field type, every option must deserialise, every `visible_when`
  path must exist, and every setting in `Settings` must appear in the descriptor or
  be marked `hidden = true`.

Adding an option is a two-line change: the field in `config.rs` and the entry in
`settings.toml`.

## The server binary

Iago does not transcribe. It downloads the Parakeet server, starts it with the
models it manages, and talks to it over HTTP.

`binary.rs` reads the release list from GitHub, picks the asset for the platform
(`parakeet-linux-amd64`, `parakeet-linux-arm64`, ...), streams it to disk, checks it
against the `sha256:` digest the release publishes, marks it executable and writes
the tag into `parakeet.version` next to it. Re-running replaces the binary, which is
the repair path.

It lives in `<data dir>/bin/parakeet`, next to `<data dir>/models`. There is no
setting for it and it never picks up a Parakeet checkout from the machine: Iago owns
both the server and the models, so a developer's clone cannot leak into a user's
install.

The daemon starts it with `-models`, `-model-precision`, a loopback port, a random
API key and `-long-audio`, so a long recording is chunked instead of rejected.

## Models

The Dependencies page downloads them from our OCI images on GHCR. No Docker, no
Podman, no `crane`.

`oci.rs` is a small registry client: anonymous pull token, manifest (following the
index to `linux/amd64`), then each layer streamed through a gzip decoder into a tar
reader. Files are written under `models/`, stripping that prefix, exactly what
`tar --strip-components=1 models` does in the Parakeet Makefile. The recognition
image also ships the full-attention encoder, which the server never loads, so the
extractor skips `encoder-full*` and saves 650 MB.

| family | source | destination |
| --- | --- | --- |
| recognition | `ghcr.io/achetronic/parakeet-models-asr-<precision>` | `<models>/asr/<precision>` |
| diarization | `ghcr.io/achetronic/parakeet-models-diarization-<precision>` | `<models>/diarization/<precision>` |
| VAD | the pinned Silero release, verified against its SHA-256 | `<models>/vad` |

Each image extracts into a staging directory and is renamed into place. The VAD is
the exception: there is no image for it, upstream publishes the ONNX directly, so
Iago downloads the URL the Parakeet Makefile pins and refuses the file when the hash
does not match.

The About page shows the digest of the image each model came from, not a hash of
the file: the image digest is the release, and it is what a rebuild replaces. It is
written to `~/.config/iago/images.json` at install time, so the page can show it
without touching the network. It also reads the files under the models directory,
sums their real sizes and fingerprints the main graph, so nothing on that page is
asserted from memory.

Installing always downloads. Re-running is the repair path for a broken install and
the way a new release lands, because the tag is `latest`.

## Porting to macOS

The port is a new file in `platform/` plus, at most, new files under `audio/` and
`input/`. Nothing above the traits changes.

| Concern | Linux (done) | macOS |
| --- | --- | --- |
| Capture | `pw-cat` | `AVCaptureSession` or `cpal` |
| Clipboard | `arboard` / data-control | `NSPasteboard` |
| Paste | `enigo` (virtual-keyboard/libei) | `CGEventPost` |
| Hotkeys | `GlobalShortcuts` portal | `CGEventTap` or `RegisterEventHotKey` |
| Extras | portal dialogs | Accessibility permission, notarisation |

`enigo` and `arboard` already cover both platforms, so those two rows are
configuration rather than code.

`platform/macos.rs` exists, answers `Error::Unsupported` to everything and documents
the plan in its module comment. There the window shows the launch screen instead of
a control center full of cards that cannot work.

Only unix-like systems are targeted. There is no Windows port and no Windows
artefact: the project is built for Linux, with macOS next.
