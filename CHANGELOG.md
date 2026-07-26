# Changelog

## Unreleased

- `scripts/record_demo.py` films the README's GIF. It drives the real picker
  through the same key handlers a terminal would, against the fixture catalog
  and a virtual filesystem, so the completion completes the same directories on
  any machine and no fixture folder reads as missing. Pillow and ffmpeg are
  needed to re-record and nothing else; the plugin itself still has no
  dependencies.
- The fixture folders the demo, the benchmark and the tests all share are
  neutral names rather than the author's own projects.

- The popup is a fixed 110x30 cells instead of 72% x 60% of the terminal, which
  on a large display meant something like 24 suggestions and 100 columns of
  whitespace. The size is set by the help panel rather than by the picker: the
  panel is a page of prose, and a page you have to read in halves is a page
  nobody reads. The suggestion list is capped at ten however tall the popup is,
  and the rows past the cap stay empty rather than becoming more folders to
  scan. CI asserts both ends from the manifest -- that the cap holds, and that
  the panel says all of it without shedding a row.
- The help panel is a page rather than a screenful of abbreviations: what the
  popup is for, all three create modes with the rule behind `smart` spelled out,
  what `^Y` does and which agents have checked flags, where the list comes from,
  and the keys in two columns. The prose is wrapped rather than written to fit,
  so a narrower popup says the same thing in more lines instead of a shorter,
  vaguer version of it.
- The create mode is spelled out in words on its own line under the target,
  wherever the popup has the row to spare -- `^T tab -- always a tab: that
  folder's own space when it has one, otherwise the one you are in`. A badge
  saying `[tab]` says the answer was insisted on, but not what insisting means.
- The faint text is legible. `dim` and `ghost` defaulted to `bright_black`,
  which is the ANSI slot themes treat as decoration -- Tokyo Night sets it to a
  near-background navy, about 2:1 against the background, and the hint labels,
  the counter and the create-mode line were all drawn in it. They are greys from
  the 256-colour cube now, tuned to the reported background, and the tiers are
  asserted to stay in order on either. Words that had no business being in the
  bottom tier moved up as well: the chip row's own label, the plain-shell chip,
  the create-mode badge and note, the "no matches" line, and the wrapped
  continuation of any sentence in the help panel, which was striping paragraphs
  two shades.
- The key hints are back to the full set now that there are columns for them,
  with `→` and `⇧⇥` and `esc` alongside the keys that carry state.
- `ctrl+y` starts the picked agent with its own permission prompts turned off.
  The chip gains a `!` and turns yellow, and the line above the hints says
  `claude (skip permissions)`, so an armed tab is never one you have to
  remember arming. Every CLI spells the flag differently and most have none, so
  the arguments are a checked table -- `claude` and `codex` today -- and the key
  refuses, with a reason, on an agent that is not in it. `skip_permission_flags`
  in `config.json` adds or corrects an entry. Unlike the agent itself, this is
  never remembered per folder.
- `ctrl+t` moves the tab-versus-space decision through three positions: let the
  plugin decide, always a tab, always a new space. The overrides are badged
  `[tab]` and `[space]` on the line above the hints, and the footer names the
  position the key is on. Where you leave it is remembered in the state
  directory, including when you leave with `esc` -- it is a preference about how
  new tabs should land, so it is one setting for the picker rather than one per
  folder, and `create` in `config.json` is now only the starting point.
- `?` opens a panel over the popup that says what the keys and the three create
  modes do, and marks the one the picker is on with the same bar the list uses.
  Anything closes it again. A path can contain a question mark, so `?` only
  opens it while the input is empty; `F1` works either way, and the footer's
  corner says which of the two is live. F1 is now decoded in both the SS3 and
  the CSI spelling, and a function key split across two reads is no longer
  mistaken for alt+O.
- The footer keeps the keys whose meaning moves -- what `⇥` does now, which
  position `^T` is on, whether `^Y` is armed -- and leaves `→`, `⇧⇥`, `^W`,
  `^U`, `^R` and `esc` to the panel. The way into the panel is the one thing on
  the row that is never shed; it gives up its label and keeps its corner.
- The key hints close up the gaps between them before giving a hint up, and a
  hint that does go goes whole. The row could previously drop a label and leave
  its key behind, reading as a key that does nothing.
- A `"space"` target now carries the name of the space it would make, so the
  line above the hints reads `→ new space alpha` rather than `→ new space` when
  the answer was insisted on rather than inferred.
- `scripts/bench.py` measures the picker against a synthetic catalog and
  session and scores it out of 100. Timings are scored but never gated; bytes
  on the wire and questions asked of the filesystem are the same on every
  machine, so `--check` fails on those and CI runs it on every push.
- A frame no longer re-derives the session snapshot behind every keystroke.
  Following a symlink is remembered for the life of the popup, which the rescan
  key clears along with the directory listings. A frame asked the filesystem 10
  questions with 8 panes open and 34 with 32; it now asks 2 on arriving at a
  folder and 1 standing still, whatever the session looks like.
- The home directory is worked out once rather than once per folder per
  keystroke, taking about 20% off the time to filter a large catalog.

## 0.1.0

First release.

- Popup picker with two fields: a folder input and an optional agent.
- Folder input completes real paths as you type. A `/`, `~` or `.` switches to
  the filesystem; prefix matches lead, fuzzy matches follow. A trailing `/`
  offers the folder itself above its children so you can drill in and still pick
  the parent.
- Inline completion drawn after the cursor. `⇥` accepts it while it is showing
  and moves the agent row once it is not; `→` fills the input with the
  highlighted folder.
- Resting list of folders you picked recently, folders that already have a space
  (badged with its name), zoxide's frecency list, and the children of the
  configured roots.
- Agent row offering a plain shell plus every agent Herdr can start that is
  installed. Each folder remembers the agent it was last opened with.
- Smart target: a folder that already has a space gets a new tab in it, anything
  else gets a new space. `create` can pin this to `"tab"` or `"space"`.
  Symlinked and resolved spellings of a folder match the same space.
- The tab or space is created before the popup closes; the agent starts in a
  detached child that waits out the new shell's startup files, so the popup
  never blocks on it.
- Mouse: click to select, click again to create, click an agent to pick it.
