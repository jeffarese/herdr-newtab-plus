# Changelog

## 0.2.1

Faster, with the same screens, keys and state files: the parity suite still
holds every screen, Herdr request and state file to the Python release, now in
109 scenarios. With 4,000 folders, 200 panes and zoxide:

- The first frame shows in about 5 ms instead of 14 (and at 400 folders, 4.7
  instead of 12). zoxide's rows come after the recents and the open spaces, so
  when those fill the screen and the list is full without zoxide, the picker
  draws the first frame without waiting for it, then finishes the list before
  it reads a key. The root rows are built, and the appearance query and theme
  read, while zoxide is still answering.
- A big session's maps are set up faster: the agents and statuses come from one
  walk of the snapshot, a folder that already has a space is asked only for its
  `.git`, and following a folder beside one already followed costs one `lstat`.
  On macOS a root's `.git` checks run on one thread, since lookups in one
  directory contend in the kernel there and more threads only wait on each
  other.
- A keystroke takes 0.58 ms instead of 0.85. Each row is scored once, without
  the positions to highlight, and only the 200 rows the list shows are matched
  again for them. Typing on scores only the rows the shorter query matched,
  and deleting a character reuses what the shorter query found instead of
  scoring the whole catalog again.
- Typing a path into a big folder takes 0.23 ms a keystroke instead of 0.85,
  and the keystroke that lists it 0.31 ms instead of 2.4. The listing stops
  once the list is full, and the folder's preview is put together from the
  listing's own read rather than read a second time.
- A session uses 39 ms of CPU instead of 55, and 9 MB of memory at its peak
  instead of 13.

## 0.2.0

- The picker is a native binary, written in Rust, and no longer needs Python.
  It is the same picker: same screens, keys, config and state files, held to
  the Python release by a parity suite that drives both on a pseudo-terminal
  and compares every screen, Herdr request and state file in 106 scenarios.
- Installing runs a build step that downloads the release binary for your
  platform (macOS or Linux, x86_64 or ARM), checks it against the release's
  `SHA256SUMS` and the version it reports, and builds from source with `cargo`
  when it cannot (Rust 1.88+; `HERDR_NEWTAB_PLUS_BUILD=source` always does).
  `herdr plugin update herdr-newtab-plus` upgrades a 0.1 install.
- Much faster. With 4,000 folders, 200 panes and zoxide, the first frame
  shows in about 14 ms instead of 90, a keystroke takes under a millisecond
  instead of 35, and a session uses an eighth of the CPU and half the memory.
  zoxide, the roots, the recents and the agents on PATH are read while Herdr
  answers the snapshot request rather than after it.

- The right-hand third of the popup previews the folder under the cursor, the
  way herdr-bar previews a pane: its name and path, its branch when it is a
  checkout, the space it already has, every agent standing in it with what each
  is doing, and what is inside it, folders first, with a count of what did not
  fit. It shows by itself when the popup is wide enough -- the shipped one is --
  and `ctrl+o` hides it or asks for it; `"preview"` in the config is `"auto"`,
  `true` or `false`, as in herdr-bar. The divider is the list's scrollbar too,
  so there is one vertical line rather than two, and it meets the rules above
  and below the list.
- The preview's read of a folder is also the answer to whether the folder
  exists, so landing on a folder asks the filesystem the same two questions it
  did before, plus one small read for a checkout's branch, and coming back to a
  folder asks nothing. Creating still asks afresh. The benchmark now counts file
  reads as lookups, and builds a fixture tree for the preview to read, so the
  bytes a navigation costs are the same on every machine.
- A click under a short list, or on the preview, no longer selects a row that
  is not on screen.
- The help panel lists `^O`, and its create-mode lines no longer run past the
  edge of a very narrow popup.

- Agents look the way they do in herdr-bar. Each one is drawn in its vendor's
  colour, from the same table, and with herdr-radar's Herdr Agent Icons Max font
  installed the chips lead with the vendor's logo. `agent_icons` in the config
  is `"auto"` (logos when the font is installed here), `"font"` (always, for a
  picker reached over ssh) or `"none"`; `--doctor` says what it found. Every
  colour can be overridden through `colors`, with herdr-bar's role names
  (`agent_claude`, `agent_codex`, ...).
- Each folder row shows the agent that folder was last opened with -- its logo
  in a gutter at the front of the row, or its name in its colour beside the
  badge without the font -- so moving through the list says what Enter is about
  to start before the chip row moves to it. Only when `default_agent` is
  `"last"`, since otherwise the memory is not what Enter uses.
- A folder an agent is already standing in says so before its badge, in Herdr's
  sidebar glyphs and colours: `◉` needs you, `◐` working, `●` done, `✓` idle.
  With several agents in one folder, the one that wants you first wins.
- The agent row is a row of chips: the selected one is a pill in the selection
  colour, bold in its agent's colour, and the rest are quiet, with no dots
  between them. Without a selection colour the selected label is underlined, as
  before. A click now lands on the chip it hits even when the row has scrolled
  to keep the selection in view; it used to measure the unscrolled row.
- The line above the hints ends with the create mode as a three-way switch --
  `smart   tab   space`, with a pill on the current position -- so the two
  positions you are not on are on screen too. The line under it now starts
  with the mode's name and lines up with the sentence it explains. The `[tab]`
  and `[space]` badges are left for popups too narrow for the switch.
- The rules between the sections are a step below every tier of text, rather
  than in the dimmest of them: they divide, and are never read. The empty list
  says what happened in bold and what to do about it underneath. `esc` in the
  hints says `close`.
- The help panel draws its keys as keys, in the accent, and its topics as
  headings, and every entry's text starts in the same column -- `^T`'s used to
  start six columns left of the rest. It says what the new row decorations
  mean, and still fits the popup without shedding a row.
- The demo session has two spaces with an agent in each, one working and one
  waiting on you, and remembers an agent for three folders, so the GIF shows
  the logos, the statuses and the chip row following the selection. The
  recorder draws the logos with the icon font when the machine has it.

- `cargo run --release --features demo --example record_demo` films the
  README's GIF: the real binary on a pseudo-terminal against a fake Herdr and a
  synthetic home, typed at in real time, so it shows exactly what the plugin
  draws. ffmpeg is needed to re-record; the `demo` feature is never part of
  the plugin build.
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
- `cargo run --release --example bench` times the binary as a black box on a
  pseudo-terminal against a fake Herdr, in three workloads up to 4,000
  folders, and compares the result with a saved run (`--compare python` is
  the Python release). Bytes on the wire and questions asked of the
  filesystem per frame are the same on every machine, so the tests hold them
  fixed on every push.
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
