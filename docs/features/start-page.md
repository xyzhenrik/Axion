# Start page

New windows and tabs open Axion's own start page (`axion://home/`). It is quick, works offline and
is fully yours to change.

![The start page, light](../../assets/screenshots/start-light.webp)

![The start page with a wallpaper, dark](../../assets/screenshots/start-dark.webp)

## What is on it

- **Clock** and a **Greeting** ("Good morning, name"), using your account name. The clock follows
  your system's 12 or 24 hour format. You can force **12 h** or **24 h** in the gear panel.
- **Search field** with a search engine picker. Typing a search or a web address opens it.
- **Top sites**: tiles for your favourite sites.
- **Shields statistics**: how many trackers and ads were blocked, and an estimate of the bandwidth
  and time saved.
- A **background** of your choice.

## Top sites

By default the page shows the **five most visited domains** from your history of the last 90 days.
Sites you **pin** come first and stay. Each tile can be pinned or unpinned (**Pin**, **Unpin**),
changed (**Edit**) or removed (**Remove**). **Add Site** lets you add a tile by hand. Removed sites will not come back; the page tells you
how many are removed and lets you **Restore** them.

If there are more tiles than fit, the page shows dots to switch between pages.

The number of tiles, rows and columns and the look-back period are in `config.toml`
(`home.top_sites_count`, `home.top_sites_columns`, `home.top_sites_rows`,
`home.top_sites_window_days`). See [Configuration files](../configuration.md).

## Search engines

The picker next to the search field and **Settings > General > Search engine** choose the same
thing. Available are DuckDuckGo (default), Google, Brave, Startpage and Ecosia. The address field
uses the same engine.

## Backgrounds

Pick a background in **Settings > General > Start page background**, or in the gear panel on the
page itself (the round button in the top corner, **Customize start page**).

- Choose **None** for a plain page in your theme's colour, or one of your backgrounds.
- **Add Image…** (or **Add from file** in Settings) copies a picture or video into your account.
  Up to 24 backgrounds and files up to 200 MB are accepted by default.
- **Dim** darkens the picture so text stays readable (20 % by default). **Blur** softens it.
- Private tabs never show a background.

### Live wallpapers

Backgrounds can move. Animated images (GIF, WebP, APNG) and videos (WebM, MP4) play muted and loop.
They pause when the tab is hidden.

- **Live wallpapers (animated images and videos)** turns the animation on or off. Off, you see the
  first frame.
- **Play live wallpapers with Reduce Motion** decides whether they keep moving while **Reduce
  motion** is on. It is on by default, so they do. Turn it off if you want a still picture whenever
  Reduce motion is on.
- MP4 videos need H.264 support, which the standard engine build lacks. WebM videos always play.
  See [Troubleshooting](../troubleshooting.md).

Live wallpapers do not count as media: they never show up in Now Playing, make no tab sound icon and
do not appear in your desktop's media controls.

## Choose what shows

Each part has a switch in the gear panel and in **Settings > General > Start page**: **Clock**,
**Greeting**, **Search field**, **Top sites** and **Shields statistics**. **More Settings…** in the
gear panel opens Settings.

## Make it your homepage or not

**Settings > General > Homepage** decides what opens in new windows and tabs. Leave it empty for the
start page, or enter any address. The home button (**Show home button**) opens the same page.
