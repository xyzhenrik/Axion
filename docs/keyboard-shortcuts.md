# Keyboard shortcuts

The idea is simple: everything about **tabs** uses **Alt** (or an F key). Browser and system actions
use **Ctrl**. Web pages keep **Ctrl+W** and **Ctrl+T**, so pages and web apps can use them.

You can see the live list in **Settings > Keyboard**. It has a search field and shows your own
changes. The names in the last column are what you use in `keymap.toml` (see
[Change a shortcut](#change-a-shortcut)).

## Tabs

| Action | Keys | Name in `keymap.toml` |
|---|---|---|
| New tab | Alt+T | `new_tab` |
| Close tab | Alt+Q | `close_tab` |
| Reopen closed tab | Alt+Shift+T | `reopen_closed_tab` |
| Next tab | Alt+Down, Alt+PageDown, Ctrl+Tab | `next_tab` |
| Previous tab | Alt+Up, Alt+PageUp, Ctrl+Shift+Tab | `previous_tab` |
| Select tab 1 to 8 | Alt+1 ... Alt+8 | `select_tab_1` ... `select_tab_8` |
| Select last tab | Alt+9 | `select_last_tab` |
| Pin or unpin tab | Alt+P | `pin_tab` |
| Tab overview | Alt+O, F9 | `tab_overview` |
| Search tabs, recently closed tabs and history | Ctrl+Shift+A | `search_tabs` |

## Navigation

| Action | Keys | Name in `keymap.toml` |
|---|---|---|
| Focus address bar | Alt+D, F6 | `focus_address_bar` |
| Back | Alt+Left, Ctrl+[ | `back` |
| Forward | Alt+Right, Ctrl+] | `forward` |
| Reload | Ctrl+R, F5 | `reload` |
| Hard reload (skip the cache) | Ctrl+Shift+R, Ctrl+F5 | `hard_reload` |

## Page

| Action | Keys | Name in `keymap.toml` |
|---|---|---|
| Find on page | Ctrl+F | `find` |
| Find next | Ctrl+G, F3 | `find_next` |
| Find previous | Ctrl+Shift+G, Shift+F3 | `find_previous` |
| Zoom in | Ctrl++, Ctrl+= | `zoom_in` |
| Zoom out | Ctrl+- | `zoom_out` |
| Reset zoom | Ctrl+0 | `zoom_reset` |
| Paste | Ctrl+V, Shift+Insert | `paste` |
| Paste as plain text | Ctrl+Shift+V | `paste_plain_text` |

## Panels

| Action | Keys | Name in `keymap.toml` |
|---|---|---|
| Bookmark this page | Ctrl+D | `bookmark_page` |
| Show bookmarks | Ctrl+B | `show_bookmarks` |
| Show history | Ctrl+H | `show_history` |
| Open the Playlist | Ctrl+Shift+P | `open_playlist` |
| Add the page's video to the Playlist | Ctrl+Shift+D | `playlist_add` |
| Open Passwords | Ctrl+P | `open_passwords` |
| Show or hide the sidebar | Ctrl+Shift+L | `toggle_sidebar` |

## Privacy

| Action | Keys | Name in `keymap.toml` |
|---|---|---|
| Lock private tabs and password filling | Ctrl+L | `lock` |

## Developer

| Action | Keys | Name in `keymap.toml` |
|---|---|---|
| Developer tools | F12, Ctrl+Shift+I | `devtools` |
| Developer console | Ctrl+Shift+J | `devtools_console` |
| View source | Ctrl+U | `view_source` |

These only work while **Developer tools** is on in **Settings > General**.

## Window

| Action | Keys | Name in `keymap.toml` |
|---|---|---|
| Settings | Ctrl+,  and Ctrl+S | `open_settings` |
| Command palette (search every action) | Ctrl+K | `command_palette` |
| Fullscreen window | F11 | `toggle_fullscreen` |
| Quit | Ctrl+Q | `quit` |

In fullscreen (F11) the toolbar, tab row, bookmarks bar and sidebar make way and the page fills
the screen edge to edge. Rest the pointer at the top edge of the screen for a moment and the bars
come back over the page (the page keeps its size); they go again when the pointer moves away.
Escape or F11 leaves fullscreen. Turn the reveal off with `layout.fullscreen_reveal_chrome = false`;
`layout.fullscreen_reveal_delay` (seconds, 0.35) and `layout.fullscreen_reveal_edge` (pixels, 3)
tune it.

## Video in fullscreen

These keys work only while a video plays fullscreen with Axion's controls, so plain keys like Space
still reach the page the rest of the time.

| Action | Keys | Name in `keymap.toml` |
|---|---|---|
| Play or pause | Space, K | `media_play_pause` |
| Seek back | Left, J | `media_seek_back` |
| Seek forward | Right, L | `media_seek_forward` |
| Volume up | Up | `media_volume_up` |
| Volume down | Down | `media_volume_down` |
| Mute | M | `media_mute` |
| Leave fullscreen | F, Escape | `media_fullscreen` |
| Captions | C | `media_captions` |
| Slower | Shift+, | `media_slower` |
| Faster | Shift+. | `media_faster` |

## Touchpad and mouse

- Two-finger swipe left or right: back and forward.
- Pinch: open the tab overview.
- Drag tabs to reorder them; drag them in the sidebar between normal and private.

## Change a shortcut

Shortcuts live in `keymap.toml` in your account's settings folder (see
[Configuration files](configuration.md#where-things-are-stored)). Create the file if it does not
exist. Axion reloads it as soon as you save it.

Each line is an action name and a list of keys:

```toml
# Replaces the defaults of this action
focus_address_bar = ["Alt+D", "Ctrl+E"]
toggle_sidebar = ["Ctrl+Shift+B"]

# An empty list removes all keys of an action
tab_overview = []
```

Rules:

- An action you list **replaces** its default keys. Actions you do not list keep theirs.
- If you give a key to an action, that key is taken away from the defaults of other actions, so an
  old file never collides with newer defaults.
- Write keys as modifiers joined by `+`, then the key: `Ctrl`, `Alt`, `Shift` and `Super` are
  understood. Examples: `Ctrl+Shift+L`, `Alt+Left`, `F11`, `Ctrl++`.
- Named keys include `Left`, `Right`, `Up`, `Down`, `Tab`, `Enter`, `Escape`, `Space`, `PageUp`,
  `PageDown`, `Insert` and the F keys. Common aliases such as `Esc` or `Return` also work.
- If two actions end up on the same key, the first one wins and Axion logs the conflict.
