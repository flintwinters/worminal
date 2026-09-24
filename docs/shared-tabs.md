# Shared tabs: design and proof ledger

One owner process serves one executable build, user, and X11 `DISPLAY`.
Ordinary launches send a bounded, versioned request to that owner and open a
new window on a new tab. A rebuild starts a new owner; the earlier owner keeps
its windows and PTYs until they close.
The request carries the launcher options, command, working directory, and
environment. The new child uses those values without changing the owner's
lasting environment or directory. A separate display elects a separate owner.

Tabs are ordered independently of windows. A tab owns its PTY, parser, screen,
scrollback, title, palette, and terminal modes. A window owns its X resources
and selected tab. At most one window is the live painting view for a tab;
focus or input hands that role to the receiving window and resizes the PTY.
All PTYs drain even when no window selects their tab. A hidden tab retains its
last PTY size. Other views keep terminal pixels until a handoff; tab-line
changes may repaint. The tab line uses the terminal font and colors, sits above
the grid without a separator, and keeps the selected label visible on overflow.
Native scrollback position is shared by all views of a tab.

Closing a tab or its shell removes it from every window, selecting the next
tab or the previous one at the end. Closing a window leaves its tabs running
while another window exists. The final tab or final window ends the owner and
all sessions. The earlier gap, tabs with no selected window, is closed by
keeping terminal sessions independent of views and routing parser callbacks
through the tab being parsed.

## Verified proofs

`python3 manage.py proof` runs local checks and private Xvfb proofs. The
current tree passed on 2026-09-24, including local KWin X11 and Cinnamon X11
started remotely over SSH against a private local Xvfb display:

1. Shared screen catch-up and terminal pixels frozen in inactive views.
2. Live-view handoff resizes the PTY to the receiving window.
3. Separate tabs use separate PTYs and input streams within one owner.
4. Focus and XIM behavior with managed windows.
5. Every PTY drains while another view is frozen; a tab with no selecting
   window continues draining, and forwarded command, environment, directory,
   geometry, and window ID reach its shell.

These are X11 proofs on private displays. They do not establish Wayland
behavior or performance on an active desktop. The remaining unverified item
is **7: launch latency and memory measurement** after the shared-tab change;
it stays deferred until that measurement is requested.
