# herdr-newtab-plus

[![ci](https://github.com/jeffarese/herdr-newtab-plus/actions/workflows/ci.yml/badge.svg)](https://github.com/jeffarese/herdr-newtab-plus/actions/workflows/ci.yml)

A new tab that asks two questions first: **which folder**, and **which agent**.

Herdr's built-in new tab opens where you already are. This replaces it with a
popup that completes real paths as you type, remembers the folders you actually
work in, and can start an agent in the new tab for you.

![herdr-newtab-plus: pick a folder, pick an agent, press enter](demo/herdr-newtab-plus.gif)

Before you type anything, it is the folders you actually use — the ones you
picked recently, the ones that already have a space open, badged with its name:

```
 ❯  folder — type a name, or a path to complete it
 ─────────────────────────────────────────────────────────────────────────────────────────────────────────
 ▌ atlas             ~/workspace                                                                     Atlas
   herdr-bar         ~/workspace                                                                    recent
   herdr-newtab-plus ~/workspace                                                                    recent
   mobile-client     ~/workspace                                                                   frecent
   notes             ~/workspace                                                                       git
   classifier        ~/workspace
   log-sink          ~/workspace                                                                       git
   webhook-relay     ~/workspace                                                                       git
 ─────────────────────────────────────────────────────────────────────────────────────────────────────────
   agent  shell · claude · codex · opencode · pi
   → new tab in Atlas
      ^T smart  a tab when that folder already has a space open, a new space when it does not
   ⇥ agent   → complete   ↑↓ folder   ^T smart   ⏎ create   esc                                ? help  1/8
```

The popup is a fixed 110×30 cells rather than a share of the terminal: the rows
are a fixed width, so scaling with the monitor only ever bought empty columns.
The size is set by the help panel rather than by the picker — the panel is a page
about what the keys and the create modes do, and a page you have to read in
halves is a page nobody reads.

The list is capped at ten suggestions however tall the popup is, because the
folder you want is one you recognise in the first few and the counter on the
right says how many more there are. The rows past the cap stay empty rather than
becoming more folders to scan.

The line above the hints always says what Enter is about to do, because the
plugin decides between a tab and a whole new space for you:

- the folder already has a space open → **a new tab in that space**
- it does not → **a new space rooted at that folder**

So one keybinding covers both, and the sidebar keeps one space per project
instead of accumulating duplicates.

`^T` is there for when you disagree: it moves that line between letting the
plugin decide, always a tab, and always a new space. The line under it says which
position you are on and what that position means, in words, all the time — and
where you leave it is remembered between opens, so it is a preference you set
once rather than a config file you go and edit.

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

`?` gives the whole popup over to what everything on screen does, and marks which
of the three positions the create toggle is on:

```
  what this popup does                                                                    ? or esc  close
 ─────────────────────────────────────────────────────────────────────────────────────────────────────────
   Type a folder, pick an agent, press ⏎. The line above the hints says what ⏎ is about to do.

   ^T   where ⏎ lands. Three positions, and it stays on the one you leave it on — popup to popup, and
        across restarts. You are on the marked one.
      ▌ smart   a tab in the space that folder already has, a new space when it has none — and it "has a
                space" when some pane in one is standing in it, symlinks followed.
        tab     always a tab: in that folder's own space when it has one, otherwise in the space you are
                looking at now.
        space   always a new space, even when the folder already has one. Duplicates are yours to make.

   ^Y         starts that agent with its permission prompts off, for this tab only — never remembered,
              unlike the agent. claude and codex have checked flags; anything else refuses rather than
              guess until skip_permission_flags in config.json teaches it one.

   the list   before you type: folders picked recently, then ones that already have a space (badged with
              its name), then zoxide's list, then the children of your roots. Type a / ~ or . and it
              completes real directories instead.

   the agent  a plain shell plus every agent Herdr can start that is installed here. Each folder
              remembers the one you last opened it with; the new one is named after the folder.
 ─────────────────────────────────────────────────────────────────────────────────────────────────────────
   ⇥     complete, or move the agent             ^W    back one path segment
   →     fill in the highlighted folder          ^U    clear the input
   ↑↓    move the folders                        ^R    rescan the filesystem
   ⇧⇥    move the agents                         click select, click again to create
   ⏎     create it                               esc   give up and close the popup
```

The prose is wrapped rather than written to fit, so a narrower popup says the
same thing in more lines instead of a shorter, vaguer version of it.

Anything at all closes it again. A folder can legitimately have a `?` in its
name, so `?` only opens the panel while the input is empty — `F1` works whatever
is in there, and the corner of the footer says which of the two is live.

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
| `ctrl+t` | where Enter lands: decide for me, always a tab, always a new space |
| `?` `F1` | the panel above — `?` while the input is empty, `F1` always |
| `ctrl+y` | start the agent with its permission prompts off |
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

### Tab, or a space of its own

`^T` moves the line above the hints through three positions; the footer says
which one it is on, and `?` explains them:

| | line above the hints | what Enter does |
| --- | --- | --- |
| `smart` | `→ new tab in Atlas` | reuses the space standing in that folder, otherwise a new one |
| `tab` | `→ new tab in Atlas   [tab]` | always a tab — in the folder's own space when it has one, in the focused space when it does not |
| `space` | `→ new space atlas   [space]` | always a new space, duplicate or not |

Three positions rather than two, because the automatic decision is the one worth
being able to get back to; a two-way switch would strand you on an override with
no way home that was not a config file. The `[tab]` and `[space]` badges only
appear on the overrides — when the plugin is deciding, the sentence is already
the whole story.

Where you leave it is written to the state directory next to the recent folders,
so it survives the popup closing, and Herdr restarting. Cancelling with `esc`
abandons the pick but keeps the setting: a toggle that quietly reverted when you
changed your mind about the folder would be one you could not trust.

Unlike the agent, it is one setting for the whole picker rather than one per
folder — it is a preference about how you like new tabs to land, and it should be
readable the moment the popup opens rather than discovered by selecting the
folder that happens to carry it. `create` in the config is the starting point for
a machine that has never touched the key.

### Agents

The agent row offers a plain shell plus every agent Herdr can start that is
actually installed on this machine. The pick is optional; `shell` is a normal
new tab.

The agent starts in the background, so the popup closes the moment the tab
exists rather than waiting for the agent to boot. It gets a name derived from
the folder, so `herdr agent focus my-project` works afterwards. Each folder
remembers the agent you last opened it with and preselects it.

### Starting without permission prompts

`ctrl+y` arms the selected agent to start with its own approval prompts turned
off — the thing you want for a tab you intend to walk away from, and the thing
you want to be sure about otherwise. When it is armed the chip gains a `!`, the
line above the hints reads `claude (skip permissions)`, and the whole row turns
yellow:

```
   agent  shell · claude! · codex · opencode · pi
   → new tab in Atlas  ·  claude (skip permissions)
      ^T smart  a tab when that folder already has a space open, a new space when it does not
   ⇥ agent   → complete   ↑↓ folder   ^T smart   ^Y prompts off   ⏎ create   esc               ? help  1/8
```

Every CLI spells this differently and most do not spell it at all, so the flags
are a table of what has been checked rather than a guess:

| agent | argument |
| --- | --- |
| `claude` | `--dangerously-skip-permissions` |
| `codex` | `--dangerously-bypass-approvals-and-sandbox` |

On any other agent `ctrl+y` refuses and says so, rather than starting it with a
flag that might mean something else. Teach it one with `skip_permission_flags`
in the config; the arguments are handed to the agent's own executable through
`herdr agent start ... -- <args>`, untouched.

Unlike the agent itself, this is never remembered per folder. It is a decision
about the tab you are opening now, and a folder that quietly kept it would be
one you could only discover by reading the chip row.

## Configuring

Optional, at `$(herdr plugin config-dir herdr-newtab-plus)/config.json`:

```json
{
  "roots": ["~/workspace", "~/code"],
  "create": "smart",
  "agents": "auto",
  "default_agent": "last",
  "skip_permission_flags": { "gemini": ["--yolo"] },
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
| `create` | `"smart"` | where the picker starts before `ctrl+t` has ever been pressed: `"smart"` reuses a space when the folder has one, `"tab"` always makes a tab, `"space"` always makes a space. The remembered position wins once there is one |
| `agents` | `"auto"` | detect what is installed, or give an explicit list like `["claude", "codex"]` |
| `default_agent` | `"last"` | `"last"` reuses the folder's remembered agent; `"none"` always starts a shell; or name a kind |
| `skip_permission_flags` | `{}` | what `ctrl+y` passes per agent kind; merged over the built-in table, so an entry adds an agent or corrects one. `[]` disables the key for that kind |
| `zoxide` | `"auto"` | on when zoxide is installed |
| `hidden` | `false` | offer dot-directories while completing |
| `mouse` | `true` | click and scroll |
| `colors` | `{}` | `accent`, `text`, `muted`, `dim`, `match`, `ghost`, `agent`, `space`, `error`, `warn` — hex, ANSI name, or 0–255 |
| `selection_background` | `"auto"` | picked from the terminal's own background |

The colors that mean something — the accent, the agent, the space name, the
warning — are plain ANSI, so they follow whatever theme your terminal and Herdr
already use. The three tiers of grey that the rest of the text sits in are not:
`muted`, `dim` and `ghost` are fixed greys picked for contrast, tuned to the
background the terminal reports. `bright_black` is the slot themes treat as
decoration — Tokyo Night and its relatives set it to a near-background navy —
and a hint label nobody can read is a hint label that is not there.

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
python3 scripts/demo.py --plain --frame --size 107x28   # render a frame, no Herdr needed
python3 scripts/demo.py --plain --create space          # ...in a given create mode
python3 scripts/demo.py --plain --panel                 # ...or the ? help panel
python3 scripts/bench.py                                # measure and score it
python3 scripts/record_demo.py                          # re-record the GIF above
pipx run ruff check .
```

`scripts/demo.py` composes a frame against a fixed session, so the layout can be
looked at and asserted on without a pty. `107x28` is what the shipped popup
actually gets: Herdr hands the pty `height - 2` rows and `width - 3` columns of
the manifest's `110x30`.

`scripts/record_demo.py` films the GIF at the top of this file. It drives the
real picker through the same key handlers a terminal would, against the fixture
catalog and a virtual filesystem, so path completion completes the same
directories on any machine and no folder ever reads as missing. Frames are
rasterised with Pillow and encoded by ffmpeg — the only two things in this
repository that are not the standard library, and both of them are optional and
only needed to re-record. `--stills` writes one PNG per scene, `--frame N` a
single one.

### Performance

`scripts/bench.py` runs the picker against a synthetic catalog and session — no
Herdr, no pty, no filesystem — and scores what it costs out of 100:

| | budget | why that budget |
| --- | --- | --- |
| cold start | 120 ms | under this the popup reads as already open |
| keystroke p95 | 3 ms | invisible beside the terminal's own frame time |
| navigation p95 | 1.5 ms | as above, for the thing you hold a key down on |
| bytes per keystroke | 400 B | one character in, a few rows re-sorted |
| bytes per navigation | 1 kB | a selection band is full-bleed, so two whole rows |
| lookups per frame | 2 | does the folder exist, and does it already have a space |

Budgets come from what the interaction needs, not from what the code currently
does — a budget read off today's numbers can only ever say "no change".

The measurements split in two, and the split is the point. Timings depend on the
machine, so they are scored and never gated: a loaded laptop is not a
regression. Bytes on the wire and questions asked of the filesystem depend on
nothing but the code, so they are the same everywhere and `--check` fails on
them — in CI, on every push. Lookups are counted as questions rather than
syscalls, because `realpath` costs one `lstat` per path component and a number
that moved with the depth of a home directory would not be a fact about the
code.

```bash
python3 scripts/bench.py --json today.json
python3 scripts/bench.py --compare today.json   # after a change
python3 scripts/bench.py --check                # what CI runs
```

`--check` also asserts the invariants an absolute ceiling cannot: that a frame's
filesystem cost grows with neither the size of the catalog nor the number of
panes already open, and that returning to a folder costs less than arriving at
it. A frame draws the dozen rows that fit and reasons from a snapshot that
cannot change while the popup is up, so the day something starts statting per
row — or re-deriving the session behind every keystroke — it says so however
fast the machine it ran on happened to be.

## License

MIT
