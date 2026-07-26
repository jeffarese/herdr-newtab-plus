# Changelog

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
