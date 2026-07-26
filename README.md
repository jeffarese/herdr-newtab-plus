# herdr-newtab-plus

A new tab that asks two questions first: **which folder**, and **which agent**.

Herdr's built-in new tab opens where you already are. This replaces it with a
popup that completes real paths as you type, remembers the folders you actually
work in, and can start an agent in the new tab for you.

```
 ❯ ~/workspace/her
 ──────────────────────────────────────────────────────────────────────────
 ▌ herdr-bar         ~/workspace                                    recent
   herdr-newtab-plus ~/workspace                                    recent
   hooklistener      ~/workspace                                       git
 ──────────────────────────────────────────────────────────────────────────
   agent  shell · claude · codex · opencode · pi
   → new tab in WeekGen  ·  claude
   ⇥ complete   ↑↓ folder   ⇧⇥ agent   ⏎ create   esc              1/3
```

The line above the hints always says what Enter is about to do, because the
plugin decides between a tab and a whole new space for you:

- the folder already has a space open → **a new tab in that space**
- it does not → **a new space rooted at that folder**

So one keybinding covers both, and the sidebar keeps one space per project
instead of accumulating duplicates.

## Install

```bash
herdr plugin install jeffarese/herdr-newtab-plus
```

Or, for local development:

```bash
git clone https://github.com/jeffarese/herdr-newtab-plus
herdr plugin link ./herdr-newtab-plus
```

Needs Herdr 0.7.4+ (session-modal popup panes) and Python 3.9+. No third-party
packages, no build step.

Check it can see everything it needs:

```bash
herdr plugin action invoke herdr-newtab-plus.open   # open it
python3 run.py --doctor                             # or diagnose it
```

## Replacing the built-in new tab

Herdr's own new tab is a normal binding, so freeing its key hands it over.
Setting a built-in binding to `""` disables it. In `~/.config/herdr/config.toml`:

```toml
[keys]
# was: new_tab = "cmd+t"
new_tab = ""

[[keys.command]]
key = ["cmd+t", "prefix+t"]
type = "plugin_action"
command = "herdr-newtab-plus.open"
description = "new tab"
```

Then `herdr server reload-config`.

If you would rather keep both, leave `new_tab` alone and bind the plugin to a
different key — nothing here depends on owning the original one.

## Using it

| key | what it does |
| --- | --- |
| type | filters folders; a `/`, `~` or `.` switches to completing real paths |
| `⇥` | completes the path while there is a completion showing, otherwise moves the agent |
| `⇧⇥` | always moves the agent |
| `→` | fills the input with the highlighted folder, ready to go a level deeper |
| `↑` `↓` | move through the folders |
| `⏎` | create |
| `esc` | cancel |
| `ctrl+w` | delete back one path segment |
| `ctrl+u` | clear |
| `ctrl+r` | rescan the filesystem |
| click | first click selects, second creates; click an agent to pick it |

The completion showing after the cursor is the picker's guess, drawn dimmer than
what you typed. `⇥` only completes when that guess is on screen — otherwise it
moves along to the agent row, so Tab always does the next useful thing and you
can always see which.

### Where the folders come from

Before you type anything, the list is, in order:

1. folders you picked recently
2. folders that already have a space open, badged with the space's name
3. zoxide's frecency list, if you have zoxide
4. every immediate child of your configured roots, `git` badged where it applies

Type a path — anything with a `/`, or starting with `~` or `.` — and the list
switches to the real subdirectories of what you named, prefix matches first. A
trailing `/` offers the folder itself as the top row and its children below, so
you can drill in and still pick the parent.

### Agents

The agent row offers a plain shell plus every agent Herdr can start that is
actually installed on this machine. The pick is optional; `shell` is a normal
new tab.

The agent starts in the background, so the popup closes the moment the tab
exists rather than waiting for the agent to boot. It gets a name derived from
the folder, so `herdr agent focus my-project` works afterwards. Each folder
remembers the agent you last opened it with and preselects it.

## Configuring

Optional, at `$(herdr plugin config-dir herdr-newtab-plus)/config.json`:

```json
{
  "roots": ["~/workspace", "~/code"],
  "create": "smart",
  "agents": "auto",
  "default_agent": "last",
  "zoxide": "auto",
  "hidden": false,
  "mouse": true,
  "colors": { "accent": "#7aa2f7", "agent": "#bb9af7" },
  "selection_background": "auto"
}
```

| key | default | meaning |
| --- | --- | --- |
| `roots` | `~/workspace`, `~/projects`, `~/src`, `~/code`, `~/dev`, `~/repos` | scanned one level deep; missing ones are skipped |
| `create` | `"smart"` | `"smart"` reuses a space when the folder has one; `"tab"` always makes a tab; `"space"` always makes a space |
| `agents` | `"auto"` | detect what is installed, or give an explicit list like `["claude", "codex"]` |
| `default_agent` | `"last"` | `"last"` reuses the folder's remembered agent; `"none"` always starts a shell; or name a kind |
| `zoxide` | `"auto"` | on when zoxide is installed |
| `hidden` | `false` | offer dot-directories while completing |
| `mouse` | `true` | click and scroll |
| `colors` | `{}` | `accent`, `text`, `muted`, `dim`, `match`, `ghost`, `agent`, `space`, `error` — hex, ANSI name, or 0–255 |
| `selection_background` | `"auto"` | picked from the terminal's own background |

Colors default to plain ANSI, so the popup follows whatever theme your terminal
and Herdr already use.

## How it works

Herdr does not record a folder on a workspace — only panes carry a `cwd`. So a
space's folders are exactly the folders its panes are standing in, which makes
the tab-versus-space decision an exact rule rather than a heuristic: a folder
already has a space when some pane in that space is sitting in it. Symlinks are
compared resolved, since a shell reports its cwd with links followed, but the
folder handed to Herdr is the one you actually picked.

Everything is read from a single `session.snapshot` when the popup opens. The
tab or space is created while you are still looking at the last frame; only the
agent — which Herdr waits on until it is genuinely ready for input — is handed
to a detached child, which waits out the new shell's startup files before
starting it. Failures there land in `agent-start.log` under the plugin's state
directory.

## Development

```bash
PYTHONPATH=src python3 -m unittest discover -s tests -t .
python3 scripts/demo.py --plain --frame --size 92x20    # render a frame, no Herdr needed
pipx run ruff check .
```

`scripts/demo.py` composes a frame against a fixed session, so the layout can be
looked at and asserted on without a pty.

## License

MIT
