# Restoring window layouts

A window reopens with the layout it last had: how its panes are split, where each divider sits, which tabs are in
which pane (and which is selected), which pane had focus, and the window's size, position and maximized state. It
works after a window is closed by accident, a frontend crash, a daemon restart and a reboot.

## What comes back

- **Frontend closed or crashed, daemon alive:** tabs, shells and scrollback were never gone. The panes return as they
  were.
- **Daemon restarted or machine rebooted:** the daemon recovers tab directories and URLs as fresh shells (see
  [architecture](architecture.md)); this only places them. Running commands and scrollback are not restored.
- **Several windows:** each window comes back with its own layout, in the order they were opened. A running frontend
  that loses its daemon and reconnects rebuilds the last layout it had, because the recovered tabs have new ids.
- **GUI update:** the update swaps each window for one in a new process. Splits and divider positions
  travel in the window handoff ([window updates](window-updates.md)), not through this file, so they survive even
  with `restore_layout: false`. The new windows save nothing until the update commits, so a failed update never
  writes over the file.
- **A window opened beside others** (Ctrl+Shift+N, a launcher running `worminal`) starts with the split of the
  window it opened from, master tabs included; the file is only for the first window of a process. Its new shell
  joins the first pane and keeps the selection.
- **Master tabs** are not part of the saved file; their panes collapse. The daemon restores the tabs themselves.

## Recovering an older tab list

Before replacing the recovery file with fewer tabs, the daemon archives the previous tab list. It keeps up to
eight checkpoints, retaining the largest list from each hour in which tabs were removed. Ordinary startup
continues to use the latest tab list, so closed tabs stay closed until you explicitly recover them.

1. Open Worminal, then run `worminald recover list` to see the available checkpoint IDs and tab counts.
2. Run `worminald recover restore ID` with the ID you want. The daemon appends fresh shells in the saved
   directories and browser tabs at the saved URLs. Existing tabs stay open. Recovering the same list again
   creates another set of tabs.

Checkpoints are private and scoped to the daemon that owns the tabs. A master keeps its own checkpoints.
They contain tab recipes. Window layouts, running programs, and scrollback are outside this recovery history.

## How it is stored

`$XDG_STATE_HOME/worminal/layout-v<wire>-<scope>.json`, one file per workspace scope, beside the recovery file. It is
private (`0600`), bounded (64 KiB, 64 windows, 256 panes), written atomically and only after changes settle (about
half a second after the last one; a closing window flushes it). There is no polling and no timer while nothing changes.

A tab is named by its position in the daemon's tab order, never by id: ids are new after a daemon restart, while
recovery restores tabs in order. When the saved layout is applied it is fitted to the tabs that exist:

- positions that no longer exist are dropped, and a pane left with no tabs collapses into its sibling;
- a tab listed twice stays in the first pane that lists it;
- tabs the layout never mentioned (for example the fresh shell a new window opens) join the first pane;
- if nothing fits, the window is the ordinary single pane.

A file that cannot be read as a layout (damaged, unknown version, out of bounds) is reported once and **kept**: the
window starts fresh and nothing is saved over it until you remove the file.

Nothing is saved while the daemon link is down, so a lost daemon never overwrites the layout it held. A saved
position on a monitor that is no longer attached is ignored; the window manager places the window.

## Turning it off, and starting fresh

- `worminal: {restore_layout: false}` in `worminal.yaml` (default `true`): no layout is read or written, and every window
  starts as a single pane at the default size. Read at start, like the other `worminal:` settings.
- **Ctrl+Shift+Q** closes the window and forgets its layout, so the next start is fresh. An ordinary close keeps it:
  that is the point, since a close can be an accident.

## For contributors

`src/workspace/src/layout_state.rs` owns the format, bounds and fitting (tested in `src/workspace/tests/layout_state.rs`).
`src/frontend/src/app_layout.rs` owns saving and restoring; `layout.rs` keeps divider ratios in the pane tree.
`tests/test_layout_restore.py` drives real windows through a frontend crash, a daemon restart, Ctrl+Shift+Q and the
off switch.
