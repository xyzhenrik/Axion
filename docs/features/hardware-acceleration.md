# Hardware acceleration

Axion uses your graphics card for its own interface and, by default, for web pages and video too.
**Settings > Hardware** lets you change how. Most people never need to touch it.

Changes on this page apply after a restart. When something differs from what is running, a
**Restart Axion to Apply** button appears and restarts Axion for you.

## Settings

| Setting | What it does |
|---|---|
| **Use hardware acceleration** | The master switch. Off: Chromium draws pages, video and canvas in software, page pictures go through system memory, and video decoding on the graphics card is off. Axion's own interface still uses the graphics card, because it cannot run without one. |
| **Hardware video decoding** | Decodes video on the graphics card (VA-API). Saves power and keeps playback smooth. Needs the master switch. The same switch is on the Media page. |
| **Smooth WebGL, canvas and GPU rasterization** | Lets Chromium draw web graphics on the graphics card. Turning it off forces software drawing for these. Leaving it on does not force anything: Chromium still skips cards and drivers it knows to be unreliable. |
| **Page frames** | How finished page pictures travel from Chromium to Axion (see below). |
| **Graphics card** | Which card Axion draws with, and Chromium is tied to. |

### Page frames

| Choice | Meaning |
|---|---|
| **Automatic** | Shares pictures directly in graphics memory only where that is known to work well (NVIDIA on Wayland). Everywhere else pictures are copied through system memory. |
| **Shared GPU memory** | Always shares directly. Fastest, but see the warning for AMD below. |
| **Software** | Always copies through system memory. Slightly more work for the processor, but the safe choice. |

### Graphics card

| Choice | Meaning |
|---|---|
| **Automatic** | Prefers the dedicated card if there is one. |
| **Integrated** | The card inside the processor. Saves power. |
| **Dedicated** | The most powerful card. Fastest. |

The page lists the cards it found: **In use** and **Also available**.

## GPU status and diagnostics

The **GPU status** block shows what is really active: the page frames mode, whether video decoding
was requested, and the Chromium version. **Copy Diagnostics** copies all of that, plus the card and
driver, as plain text to the clipboard. Paste it into a bug report. See
[Troubleshooting](../troubleshooting.md#report-a-bug).

## AMD graphics (RADV)

> On AMD graphics with the open-source RADV driver, **Shared GPU memory** can crash the graphics
> driver (a "GPU fault"), and your screen may freeze while it recovers. Keep **Page frames** on **Automatic** (which uses the
> safe path on AMD).

If you do choose **Shared GPU memory** on an AMD card, Axion asks you to confirm first and keeps a
warning visible on the page.

## Integrated GPU tips

On a weaker integrated graphics card, the biggest win is
**Settings > Appearance > Glass > Flat**. It skips all glass drawing. See
[Appearance and Liquid Glass](appearance-and-glass.md).

## Environment variables

Two variables override the settings for one run:

| Variable | Effect |
|---|---|
| `AXION_GPU=low` / `high` / a name | Picks the integrated card, the strongest card, or a card whose name contains the text (for example `AXION_GPU=radeon`). |
| `AXION_OSR=software` / `gpu` | Forces page frames to system memory or shared GPU memory. |

When one of them is set, the **GPU status** block says so. Do not set `AXION_OSR=gpu` on AMD
graphics. More variables are listed in [Configuration files](../configuration.md#environment-variables).

## Config keys

```toml
[hardware]
enabled = true              # master switch
page_frames = "auto"        # "auto" | "gpu" | "software"
gpu = "auto"                # "auto" | "low" | "high"
gpu_rasterization = true

[media]
hardware_decoding = true
```
