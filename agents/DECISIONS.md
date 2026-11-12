# Decisions

The choices that shaped the project. Only the ones that would be expensive to
reverse, or that someone will otherwise question.

## Wayland only, Rust, and gpui-kit

No X11 backend: it has a different input model, and the target is a modern desktop.
The window needs Vulkan.

The whole app is Rust. `gpui-kit` brings 75+ components, a theme registry, AccessKit
and tables, which is what the conversation history will need. We built the window
twice and kept the second one: doing the same in egui meant writing tables, virtual
scrolling and a real text input by hand. Go was ruled out for the UI: no maintained
libei or portal bindings.

## One binary, two processes

`iago` is the CLI, the daemon and the window, all in one executable. The window
runs as `iago ui`, a hidden subcommand that `iago` spawns as a separate process, so
closing it never takes a dictation down.

They were two binaries once, because the window's toolkit (gpui) and the portal
code drove different D-Bus backends and could not be linked into one process
without a "there is no reactor running" panic. The portal code now uses a
different zbus major version than the window, so both sit in the same binary
without touching. That removed the need for a second package and a second build
invocation, and left one file to ship.

## One daemon owns the state, clients speak a socket protocol

A shortcut that runs `iago ctl` is a new process each time, so the recording has to
live elsewhere. The clipboard has to as well, because on Wayland the process that
owns the selection must stay alive. The daemon owns the recording, the clipboard,
the model process and the shortcut session, and everything else talks to it over
JSON on a Unix socket in `$XDG_RUNTIME_DIR`.

## Parakeet and the models belong to Iago

Iago does not transcribe. It downloads the Parakeet server from its GitHub releases,
verifies the published digest, and runs it with the models Iago manages, over a
private loopback port with a random API key.

Two consequences that are easy to get wrong:

- The binary always comes from the release into Iago's data directory. A Parakeet
  checkout on the machine is never used, even when both projects are ours.
- The API key is a fresh UUID per sidecar process, passed by environment variable,
  never written to disk and never shown in the UI.

Models come from our own OCI images on GHCR, pulled by a small registry client, so
the user needs no Docker or `crane`. The naming and layout are in
[`ARCHITECTURE.md`](ARCHITECTURE.md).

Extraction goes into a staging directory and is renamed into place. An interrupted
download must never leave a truncated file that looks installed: that is exactly the
bug that made a model fail with "protobuf parsing failed".

## Config is TOML, and the UI is generated from a descriptor

`config.toml` is the source of truth for values, types and defaults; the Rust
structs validate it. `ui/settings.toml` only describes presentation and is embedded
in the binary. A test fails when the two disagree, or when a setting exists in the
struct and not in the descriptor, so nothing can be silently unreachable.

Settings save themselves 600 ms after the last edit, so no edit can be lost. Saving
reloads the daemon's cached handles and re-reads the file from disk, because the
daemon holds its own copy for its whole life and would otherwise keep running with
the settings it read at start-up. It never stops the model process; that is a
separate request, used after a model download.

Saving also reconciles the autostart entry, since the daemon is what gets started at
login and the setting is what says whether it should.

## Delivery pastes, it never types

The focused window delivery used to type the transcript character by character.
On Wayland that is a dead end: synthetic input is keycodes, and `enigo` finds the
keycode of a character by searching the session keymap at the unshifted and shifted
levels only. No dead keys, no AltGr.

On a Spanish layout `é` is a dead key, so it has no keycode; `enigo` answered "Key
is not mapped" and the delivery stopped at the first accent, leaving the ASCII
prefix in the box. `ñ`, `@` and `#` fail for the same reason. A partial transcript
is worse than none: it looks like the model cut it.

So the text goes to the clipboard and only `CTRL+v` is pressed. It is one keystroke
instead of two hundred, it carries any language, and the clipboard is restored
afterwards so dictation does not eat what the user had copied.

There was a third target, `pointer`: click where the mouse is, then paste. It never
worked. The click is a libei pointer event, so it lands where the *emulated* pointer
is, not where the real one is, and moving the emulated pointer needs coordinates
Wayland does not give to applications. Measured with the real pointer over a text
field: the click brought the other window forward. The target is gone, and `pointer`
in an old file loads as `focus`.

If a delivery ever stops halfway, read the method in the log: `paste` means the
whole thing arrived, `clipboard` means the chord could not be pressed and the text
is waiting in the clipboard.

## The portal is the shortcut, with three traps

The XDG `GlobalShortcuts` portal is the mechanism: it reports press and release, so
push-to-talk works, and GNOME's portal cannot bind a shortcut for a headless daemon
anyway (it wants a window to parent a dialog), which is why the desktop's own custom
keybinding is the fallback and reports presses only.

Three things about it are not obvious:

- **Repeats.** The portal sends `Activated` on every key repeat, thirty-three times a
  second. Forwarding each one flooded the daemon and let a release queue behind
  hundreds of presses, sometimes after a press that should not have been there: the
  recording stopped, leftovers started another, and nothing was left to stop it. The
  client forwards the edge only, and drains presses before releases so a buffered
  pair keeps its order.
- **The missing release.** `Deactivated` is not guaranteed: mutter matches the
  accelerator against the modifiers still held, so letting go of `Control` or
  `Shift` before the letter closes nothing. Measured on GNOME Shell 50.1, two holds,
  one released letter-first and one modifier-first, produce exactly one
  `AcceleratorDeactivated`. The repeats stop when the key comes up, so the client
  learns the repeat cadence and takes a silence of four intervals (150–600 ms) as
  the release, waiting three seconds before any repeat has been seen.
- **A press while the gesture is open is a repeat**, never a new gesture: the first
  repeat arrives half a second after the press, and treating it as a new one stopped
  the recording the user was dictating into.

The safety cap of a recording is a timer, and a timer belongs to its recording. It
used to outlive it and stop whatever was recording when it fired, which looked like
the shortcut releasing itself. It is cancelled with the recording now.

`input/hotkey/hold.rs` holds the timing logic, with the numbers as tests.

## The combination is Iago's, the permission is the desktop's

A portal only consults the app when the shortcut is new. After the first grant, GNOME
stores the combination and ignores `preferred_trigger`, so an app that believes its
settings own the key would silently lie. Iago writes its own entry in the desktop's
store before binding, and changing the setting stops the mechanism and starts a new
one, which is why the portal client lives in its own process: the desktop lets go of
a grab when its client is gone. The dialog exists once, for the permission itself.

## One trigger, translated once

The combination is stored as `CTRL+SHIFT+d`. Every backend spells it differently, and
having each one translate produced three translators that disagreed. `Trigger` parses
once and renders per dialect.

The same reasoning applies to the mechanism: `HotkeyService` plus
`Platform::hotkey_service` means adding macOS is one file, and the daemon never
learns which desktop it runs on.

## Theme is not configurable

Two themes fixed in the repository, light and dark, following the desktop appearance
by default. `theme.name` carries a name, nothing else. Colour choices belong to the
project, not to a settings file.

## No shell extension

A GNOME Shell extension used to watch the shortcut, because only the shell could
report a key coming up. The portal reports the same pair, so the extension was
removed. If it ever comes back: the shell stage no longer emits key events, so the
keys have to come from a shell keybinding plus a grab while the key is held, and a
disabled and re-enabled extension is not reimported, so every change needs a logout.

## Diagnostics is a log file, not a checklist

Both binaries append to `~/.local/share/iago/logs/`, and "Open logs" opens the file
with the desktop's editor. The daemon also keeps the last lines of Parakeet's
stderr, so a failed start says why instead of showing an exit code.

## UI rules live in `agents/STYLE.md`

Typography, spacing, colour and copy are not taste per page: they are written there
and enforced by `ui/src/components.rs`.

## The paste permission is asked once, not per connection

The RemoteDesktop portal only persists a grant when it can name the app. A host
process is invisible until registered, so the daemon registers
`com.achetronic.Iago` before opening the input connection. With that, the paste
permission is asked once and reused; without it, every connection shows a dialog
again, which is how a handful of test runs became a wall of permission windows.

The input connection is opened once at daemon start and reused. A wedged press
replaces the worker, but there is no watcher reconnecting on its own: the only
reason to reconnect is a press that already failed.

## The model server stays up, and comes back if it dies

There used to be an `asr.idle_timeout_secs` setting that stopped the sidecar
after ten minutes without dictating, to give back the memory. It read as a crash:
the server disappeared on its own, and the reason lived in a log nobody opens.

Memory is not worth that. The default is gone, the setting is gone, and a config
file that still carries `idle_timeout_secs` loads with the key dropped (the
settings parser prunes keys this version no longer knows).

What replaces it is a watcher: the daemon looks at the sidecar every five
seconds and restarts it when it exited on its own. Only processes nobody killed
are replaced, which is why `Sidecar::exit_status` reports a status and
`shutdown` does not: stopping the server on purpose is a decision, not a death.
Restarts back off, doubling up to a minute, so a server that cannot start does
not spin.

The watcher never restarts mid-recording.
