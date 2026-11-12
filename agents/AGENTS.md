# Agents

How to work in this repository, whether you are a person or an agent.

## What this is

Iago is a dictation app for Wayland desktops. The window captures nothing and
transcribes nothing: a daemon owns the recording, the clipboard, the shortcut
session and the Parakeet server that Iago downloads and runs itself.

One binary, two processes:

| command | what it is |
| --- | --- |
| `iago` | CLI + daemon + window, one executable |
| `iago ui` | the window, a hidden subcommand `iago` spawns |

Read in this order before touching anything:

1. [`DECISIONS.md`](DECISIONS.md), for the choices that are expensive to reverse.
2. [`ARCHITECTURE.md`](ARCHITECTURE.md), for the layers, the ports and the plan.
3. [`STYLE.md`](STYLE.md), if you are touching the window.

## Build and verify

```sh
make              # every target, one line each
make check        # fmt + clippy -D warnings + tests
make build        # debug build of the binary
make run          # open the window
make install      # the binary, desktop entry and icon under ~/.local
make dist         # release tarball for this machine
```

`make check` must pass before anything is committed. Clippy runs with
`-D warnings`, so a warning is a failure.

One binary, built with `cargo build -p iago`. The window runs as `iago ui`, a
hidden subcommand the daemon spawns, so a crash in it never touches a recording.

## Where things live

```
src/          library and daemon: asr, audio, autostart, input, platform, config
src/input/    clipboard, simulated input, hotkey mechanisms (portal, gnome, hold)
src/platform/ the only OS-aware layer
src/window/   the window, a module of the same crate
agents/       these documents
themes/       the two fixed themes, in gpui-kit's JSON format
packaging/    desktop entry
docs/img/     mark, icon and the images the README uses
```

## Rules that are easy to break

- Only `src/platform/` names an operating system. Choosing a shortcut mechanism is
  `Platform::hotkey_service`'s job; the daemon never learns which desktop it is on.
- The window talks to the daemon through `ipc` and reads `config`. It never links
  the portal, the clipboard or `enigo`.
- The settings page is generated from `src/window/settings.toml`. A setting needs the field
  in `src/config.rs` and the entry in the descriptor; the coverage test fails when
  one exists without the other.
- `src/window/components.rs` is the only place that builds a card, a status row,
  an artifact row or a progress bar. `src/window/theme.rs` owns type and colour.
- The status publishes `shortcut.mechanism` and nothing else about the shortcut:
  the combination and the mode are settings, and clients read them from there.
- `input::hotkey::Trigger` is the only translator of a combination. Do not turn
  `CTRL+SHIFT+d` into `<Control><Shift>d` anywhere else.
- Whether hold-to-talk can be offered is the mechanism's answer
  (`HotkeyCapabilities::reports_release`), never a per-desktop rule in the UI.
- The portal reports the release, but not always: `input/hotkey/hold.rs` decides
  when a gesture ended from the key-repeat cadence. Keep its invariants, they are
  what keeps push-to-talk from recording until the cap.
- Never show internal details in the UI: loopback address, port, API key, full
  hashes.
- Never take a Parakeet binary or models from the machine. Iago downloads both into
  its own data directory.
- Downloads must be atomic: extract into a staging path and rename. A truncated
  model that looks installed is the worst failure mode this app has.

## Testing by hand

The scripts below live in `/tmp` and are scaffolding, not code. They need `evdev`
(`python3-evdev`) and a writable `/dev/uinput`.

### The pipeline, without a microphone

PipeWire can fake an input: a loopback with a sink side and a source side, play a
file into the sink and point `audio.device` at the source.

```sh
pw-loopback -m '[ FL FR ]' \
  --capture-props='media.class=Audio/Sink node.name=iago_sink' \
  --playback-props='media.class=Audio/Source node.name=iago_mic'
ffmpeg -f lavfi -i "sine=frequency=300:duration=1.5" -ar 16000 -ac 1 /tmp/tone.wav
pw-play --target iago_sink /tmp/tone.wav &
iago ctl recording-start; sleep 2; iago ctl recording-stop; iago ctl daemon-status
```

A tone is not speech, so the transcript comes back empty, but the capture, the
silence check and the server round trip all happen. Set `audio.device = "iago_mic"`
first.

### The shortcut, without a keyboard

Hold the combination with a virtual keyboard. Keep the device alive after the
release: destroying it in the same breath makes the compositor miss the key coming
up, and you will spend an afternoon chasing a release that never arrived.

```sh
python3 - <<'EOF'
import time
from evdev import UInput, ecodes as e
ui = UInput({e.EV_KEY: [e.KEY_LEFTCTRL, e.KEY_LEFTSHIFT, e.KEY_D]}, name="iago-test")
time.sleep(1.0)
for key in (e.KEY_LEFTCTRL, e.KEY_LEFTSHIFT, e.KEY_D):
    ui.write(e.EV_KEY, key, 1); ui.syn(); time.sleep(0.03)
time.sleep(2.0)
for key in (e.KEY_D, e.KEY_LEFTSHIFT, e.KEY_LEFTCTRL):
    ui.write(e.EV_KEY, key, 0); ui.syn(); time.sleep(0.03)
time.sleep(0.5)
ui.close()
EOF
```

One hold produces exactly one `ShortcutPress` and one `ShortcutRelease` in the log.
Anything else is a bug in the portal client, not in the test.

### Delivery, without speaking

`examples/paste_probe.rs` sends a fixed string through the same `text_sink` the
daemon builds. A GTK window whose buffer is written to a file says what arrived:

```sh
python3 /tmp/paste-target.py /tmp/out.txt &   # TextView, writes on every change
cargo run --example paste_probe -- "acentos: é ñ @ #"
cat /tmp/out.txt
```

The text must carry `é`, `ñ`, `@` and `#`: they are exactly what keycode-based
typing cannot produce on a Spanish layout.

### Screenshots for the docs

`IAGO_SPLASH=1` opens the "Linux only for now" screen on Linux, and
`IAGO_PAGE=dependencies` opens the window on a page that is not the first one.

```sh
IAGO_PAGE=dependencies setsid iago ui &
sleep 4
python3 /tmp/shot.py /tmp/full.png         # XDG Screenshot portal, no dialog
python3 /tmp/crop3.py /tmp/full.png docs/img/screenshot-dependencies.png
```

`crop3.py` finds the window by its two theme backgrounds (the page's cream and the
sidebar's muted cream) and keeps the biggest rectangle, because a terminal showing
image previews has the same colours. Crop to the window: nothing of the desktop
belongs in the docs.

## Processes

The daemon kills the model server on SIGINT and SIGTERM. When you kill it with
`-9`, check for survivors; they hold the models in memory:

```sh
pgrep -af "iago/bin/parakeet"
```

## Style of the code

Apache-2.0 headers on every file. Comments explain why, not what. Errors are
`thiserror` in the library and `anyhow` in the binaries. Tests are behaviour-first
and live in the module they cover.
