# Performance notes

A survey of where Axion spends time and wakes up, what was changed, and what is left. Measured
on 2026-11-04 with release builds on the development machine (RTX 3070 Ti, X11/XWayland test
window on a 60 Hz monitor, software page frames, as on AMD), with a throwaway profile seeded with
40 restored tabs, 2,040 bookmarks in 40 folders, 50,000 history entries and 500 downloads.

## How it was measured

- `AXION_PERF_LOG=1`: one line per second (redraws, presented and skipped frames, wakeups,
  paint/render/GPU time, CPU use). Note that the log's own `/proc` scan of Chromium's processes
  costs ~30 ms once a second on the main thread: switch it off when looking for stalls.
- `AXION_STALL_LOG=<ms>`: main-thread iterations longer than the threshold, with what it was doing.
- `--bench-resize` for idle, scrolling and resizing; `AXION_TYPE_PROBE=native:…` and
  `--open=type:TEXT` for the address bar; `--open=…,sweep:X1:Y1:X2:Y2:N` to move the pointer
  over a list frame by frame; `--overview` for the tab overview.
- Temporary `Instant` timers around suspect calls (removed again) to split a frame's time.
- SQLite query times on the seeded database (the same queries the store runs).
- `--capture` before/after in light, dark and flat mode (settings sheet, bookmarks panel, tab
  overview, page menu, start page), compared pixel by pixel with ImageMagick `compare`.

## What changed (ranked by what it cost)

| Finding | Measured cost before | Fix | After |
|---|---|---|---|
| Start page state re-read the top sites (a scan of the whole history) for every arriving site icon and Shields count change; the native start page model did the same | 25-45 ms main-thread stalls, repeatedly, while a start page was shown (50k history) | `HomePage` keeps the last reading for `home.top_sites_refresh_seconds` (30 s) or until the sites are edited | no stalls over 12 ms |
| Address bar: suggestions (history LIKE scan + sort) and inline completion (`lower(url) LIKE`) ran on the UI thread at every key | ~6 ms + ~12 ms per key at 50k entries (stall log: 31-37 ms frames while typing) | `suggest_worker`: a thread with its own connection, newest lookup only; the card keeps rows that still match until the answer arrives; Enter before the completion arrived completes at once | no typing stalls; same rows and completion (captures identical) |
| Hit testing and painting cloned every node's `Style` (`is_visible`) and child list; long clipped lists painted every row | bookmarks panel (2,000 rows) open, pointer moving: 5.0 ms per frame, 24 % CPU (paint 1.3 ms, each hit test ~0.8 ms) | `is_visible` borrows the style, children are iterated in place, rows more than a clip-height outside a clipped list are not painted | 0.57 ms per frame, 4.6 % CPU; pixel-identical |
| The tab overview created a new texture view per card every frame, so its scene never matched the last one | static overview: 59 presented frames/s, 6 % CPU | `texture_views::ViewCache` keeps views per texture (overview, Playlist player) | 0 presented frames/s, 1.4 % CPU |
| Software page frames (`OnPaint`, the AMD path) uploaded the whole frame for every paint | 0.57 ms per paint at ~2000×1200 for a 40 px animation, 60 times a second | only Chromium's dirty rectangles are uploaded into the texture that holds the previous frame (`hardware.partial_page_uploads`); a skipped frame forces a full upload | 0.07-0.13 ms per paint |
| An idle window ran a full frame (animate, layout check, paint, fingerprint) at every display refresh | 60 redraws and ~120 wakeups per second idle (4× that on a 240 Hz monitor) | after `ui.idle_frames_before_slowing` (30) unchanged frames, checks every `ui.idle_check_interval_ms` (50 ms); input, page frames and page events still draw at once | 20 redraws and ~60 wakeups per second (the rest is CEF's 30 Hz message-pump minimum) |
| Site icons stayed in memory (texture + pixels) forever | grows with every host shown (a scrolled 50k history: thousands) | at most `ui.max_site_icons_in_memory` (512); the least recently shown icons that are cached on disk leave memory | bounded |

## Checked and fine

- Window resize sweep: `ui_layout` 0.8 ms per step now (the earlier ~4 ms is gone after the
  label reshaping fix on main), 1.6 ms average frame.
- Idle page, scrolling: 0.1-0.3 ms per frame on our side.
- Tab overview thumbnails are the pages' live textures (no decode, no upload).
- Shutdown (~0.2-0.4 s) and startup (~0.4 s from window to first frame with 40 restored tabs; GPU
  adapter ~100 ms, compositor and UI ~125 ms, Chromium ~95 ms, session restore ~50 ms).

## Remaining hot spots and ideas not done

- **Text during scale animations.** A sheet or popover opening and a pressed button scale their
  text (`TextArea::scale`), so glyphon rasterises every glyph at a new size each frame. While the
  settings sheet or History opens, `render` takes 3-9 ms of CPU per frame (also in flat mode), on
  top of the GPU work. Quantising the animated scale would cache glyphs but changes mid-animation
  pixels; drawing the text at its final size and scaling the glyph quads needs a glyphon change.
- **`Chrome::sync_tabs` rebuilds every tab pill and sidebar row** on any tab change (title,
  address, loading state): 2.3-4.5 ms with 40 tabs, a few times per navigation. An incremental
  update of the changed rows would remove it.
- **Long non-virtualised lists** (sidebar bookmarks panel) still lay out and hit-test every row;
  painting is now cut to the visible part. Virtualising them like the History sheet would make
  them flat in cost.
- **Site icon persistence** (PNG encode and two file writes per new icon) and the first disk read
  of each host's icon run on the UI thread (~0.3 ms each); a writer thread would take them off.
- **Glass on the iGPU**: each glass layer runs its own blur and composite (scissored to its
  shapes already). Measured earlier at 29 ms for 6 layers at 2560×1440 on the Radeon 610M
  (`flat_bench`); merging layers of the same depth into one pass is the next step.
- `AXION_PERF_LOG`'s CPU sampling reads `/proc` for every Chromium process once a second on the
  main thread (~30 ms with many tabs); moving it to a thread would make the log less intrusive.
- CEF's external message pump is woken at least 30 times a second (`MAX_PUMP_DELAY`, CEF's
  recommendation) even when idle.
