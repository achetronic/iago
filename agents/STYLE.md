# UI style

Rules for the window. If something here is wrong, change it here first and then in
the code, so the next person does not have to guess.

## Shape of the window

Four pages: **Status**, **Settings**, **Dependencies**, **About**.

- **Status** answers "is it working": the server, the models, whether it is
  dictating. Nothing else.
- **Settings** is generated from `ui/settings.toml`. No page code per field.
- **Dependencies** holds what can be downloaded: the Parakeet binary and the models.
- **About** holds what is not state: build, platform, and the way into the logs.

Sections are separated by `theme::section_gap()` (24 px) and nothing else. Rows
inside a section are 12 px apart.

## Typography

One scale, defined in `ui/src/theme.rs`. A page never sets a font size, a weight or a
colour on its own.

| level | helper | format |
| --- | --- | --- |
| page or section title | `theme::page_title` | extra large, semibold, foreground |
| row name | `theme::row_title` | small, foreground |
| secondary line | `theme::meta` | extra small, muted |

No card titles, no bold sentences, no headings inside cards.

## Colour

Two chromas and a set of warm neutrals. That is the whole palette.

| meaning | colour | used for |
| --- | --- | --- |
| present, active, selected | clay `primary` | toggles on, slider fill, selected sidebar row, check for an installed artifact, dot for a running server |
| off, absent, idle | neutral `muted_foreground` | stopped server, missing artifact, not dictating |
| broken | `danger` | errors, failed starts, invalid settings |

There is **no green and no amber** in the interface. `success` and `warning` in the
theme files point at the accent and the danger colour, so a stray component cannot
reintroduce them.

Shape carries meaning too, so colour alone never has to:

| glyph | means |
| --- | --- |
| ● dot | runtime state (running, recording) |
| ✓ check | an artifact is installed |
| ⚠ triangle | an artifact is missing |

Never show internal details in the UI: the loopback address, the port, the API key,
full hashes. Digests are shortened to 12 hex characters.

## Components

Every reusable piece lives in `ui/src/components.rs`: `card`, `status_row`,
`action_row`, `artifact_row`, `progress_bar`, `dot`. A page that builds one of these
by hand is a bug: fix the component instead.

Controls come from gpui-kit and take their colours from the theme. Never override a
control's colours.

## Copy

- Labels in sentence case, short: "Download again", not "Click here to download".
- Section titles in one or two words.
- No filler sentences, no exclamation marks, no emoji.
- A number is followed by its unit: `878 MB`, `120s`.
- Button labels say what happens, not "OK".

## Themes

Two, fixed in `themes/`: `iago-clay` and `iago-clay-dark`. The user picks the mode
through `theme.name` (`auto`, `mint`, `mint-dark`) or the sun/moon toggle. The two
mint names are older than the palette and stayed, so settings files keep working.
Colours are not configuration.

Both files must define the same tokens; a test fails otherwise.
