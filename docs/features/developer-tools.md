# Developer tools

Axion includes Chromium's own developer tools, unchanged. If you know them from Chrome, you know
them here.

## Open them

| What | How |
|---|---|
| Developer tools | **F12** or **Ctrl+Shift+I**. Press again to close. |
| Console | **Ctrl+Shift+J** opens the tools straight on the Console panel. |
| View source | **Ctrl+U** opens the page's source in a new tab. |
| Inspect an element | Right-click an element and choose **Inspect**. The tools open with that element selected. |

The tools open in a window of their own next to Axion. They have the usual panels: Elements,
Console, Sources, Network and more. The window closes when its page closes.

## Good to know

- Developer tools work in private tabs too.
- A tab with open developer tools is never frozen or unloaded, so you do not lose your session
  while you debug. See [Tabs and memory](tabs-and-memory.md).
- Shields keep working while the tools are open.
- The tools' own look and language follow Chromium, not Axion's appearance settings.
- The view-source tab does not appear in your history.

## Turn them off

**Settings > General > Developer > Developer tools** switches all of it off: the three shortcuts and
the **Inspect** entries in menus do nothing then. This is handy on a shared computer.

You can change the keys in `keymap.toml` (`devtools`, `devtools_console`, `view_source`). See
[Keyboard shortcuts](../keyboard-shortcuts.md).
