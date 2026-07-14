# AthShell

The default desktop shell. Familiar enough to switch from Windows or Mac in 10
minutes. Swappable — the OS doesn't care which shell you run.

## Goals

- Login screen, lock screen, desktop, taskbar/dock, system tray, notification
  center, settings — every "Windows-as-OS shell" surface, native and fast
- A search box that returns sub-100ms local-first indexed results
- One unified Settings (no Settings vs. Control Panel split, ever)
- Modern File Manager: tabs, split panes, fuzzy search, batch rename, sane
  archive handling
- Swappable window manager surface: tile (i3-style), stack (macOS-style), float,
  hybrid — first-class API, not a hack
- Widget surface (Rainmeter-class, fast and sandboxed)

## Non-goals

- Being the only shell. Alternative shells must be possible from day one.
- Replicating every Windows quirk for the sake of it.

## Layering

- **athshell-login**: greeter, multi-user select.
- **athshell-desktop**: wallpaper / live wallpaper host, icon layer, right-click menus.
- **athshell-taskbar**: app pinning, running indicators, system tray.
- **athshell-notif**: notification center, focus modes.
- **athshell-settings**: every setting, one app, fuzzy-searchable.
- **athshell-files**: file manager.
- **athshell-search**: tantivy/Lucene-class local index.
- **athshell-wm**: pluggable window manager backends.

## Open design questions

- Default WM style for first-time users (float-with-snap is the most familiar)
- How aggressive the search index is by default (full-text vs. metadata-only)
- Workspace / virtual desktop model — pure Mac spaces, GNOME activities, or
  Windows virtual desktops?
