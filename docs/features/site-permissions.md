# Site permissions

Websites can ask to use your **microphone**, **camera** or **screen** (calls in Discord, Google Meet,
Zoom, Teams), to **show notifications**, to **know your location** or to **read the clipboard**.
Nothing is allowed until you say so. This page explains the prompt, how decisions are remembered and
how to see and stop what a tab records.

## The permission prompt

When a site asks, a small prompt drips out of the address field:

> **discord.com** wants to: Use your microphone

It shows one line per thing the site asks for. The buttons depend on what is asked:

| Request | Buttons |
|---|---|
| Microphone, camera | **Allow While Visiting**, **Always Allow**, **Don't Allow** |
| Screen sharing | **Share**, **Don't Allow** |
| Notifications, location, clipboard and the others | **Allow**, **Don't Allow** |

- **Allow While Visiting** allows it for this tab as long as it stays on the site. Leave the site
  (or close the tab) and it asks again.
- **Always Allow** and **Allow** remember the decision for the site: it is not asked again.
- **Don't Allow** refuses and remembers that too (for screen sharing: just this once).
- **Esc** or the **×** closes the prompt without deciding. The page gets a refusal this time and may
  ask again later. Chromium stops a site from asking for a while after it was dismissed several times.

Requests of a background tab wait until you switch to it. Leaving a site closes its waiting requests.

**Screen sharing is never remembered.** Every share asks, also on sites you trust. After **Share**,
your desktop asks which screen or window to share (see below).

**Private tabs** ask separately from normal tabs and keep their decisions in memory only: they are
forgotten when Axion quits and never written to disk.

## Seeing and stopping what a tab records

While a tab uses the microphone, camera or screen, Axion shows it:

- a coloured icon in the address field: **orange microphone**, **green camera**, **red screen**
  (the most telling one if several are in use);
- a dot in the same colour on the tab in the tab bar and the sidebar.

Click the icon for a popover that lists what the tab records, with **Stop** for each and **Stop All**.
Stop ends the recording the way the page's own button would: the page is told its microphone,
camera or screen track ended, and the call itself stays open. If a page keeps recording anyway (it
asked before Axion could watch it, or copied the recording), Axion reloads it after a moment
(`permissions.stop_reload_after_ms`). Stopping also ends an **Allow While Visiting**
permission for that kind: the site asks again next time.

Tabs that record are never frozen or unloaded to save memory, whatever the memory settings say.

## Settings

**Settings > Privacy > Site Permissions** sets what happens when a site asks and you have not decided
for it yet: **Ask** (the prompt) or **Block** (refused without asking), separately for microphone,
camera, screen sharing, notifications, location, clipboard and all other permissions.

**Settings > Privacy > Remembered for Sites** lists every decision you made with **Always Allow** or
**Don't Allow**. Remove one with its cross, or all of them with **Reset All**; the site asks again.
See the [settings reference](../settings.md#site-permissions).

## Screen sharing on Linux

On a **Wayland** desktop (KDE Plasma, GNOME), Axion shares screens through the desktop's
**screen cast portal** and **PipeWire**, like every other Wayland app. After you click **Share**, the
desktop shows its own picker: choose a screen or a window, then confirm. KDE may offer to remember
the choice. Cancelling there tells the site you refused.

This needs `pipewire`, `xdg-desktop-portal` and the portal of your desktop (`xdg-desktop-portal-kde`
on KDE, `xdg-desktop-portal-gnome` on GNOME, `xdg-desktop-portal-wlr` or `-hyprland` on wlroots
compositors). See [Troubleshooting](../troubleshooting.md#microphone-camera-or-screen-sharing-do-not-work).

On **X11** (or when Axion runs under XWayland) there is no picker: **Share** shares the whole screen,
the prompt says so.

A shared screen's **sound** ("Share audio" in Discord) is your computer's whole sound output, taken
from PipeWire's (or PulseAudio's) monitor of the default output.

## What works and what does not

| Permission | Works | Notes |
|---|---|---|
| Microphone, camera | Yes | Through PipeWire / PulseAudio and V4L2, the devices your desktop has. The site picks the device; change it in the site's own settings. |
| Screen sharing | Wayland: yes, through the portal's picker. X11: the whole screen. | A window of Axion itself can be shared like any other. |
| Notifications | Allowing works (`Notification.permission` becomes `granted`) | Shown by your desktop's notification service. |
| Location | Allowing works, but finding the position does not | Axion's engine has no location service: sites get "position unavailable". |
| Clipboard reading | Allowed reads work | On Wayland the engine has no direct access to the desktop clipboard, so a page may not see what you copied in other apps. Pasting with Ctrl+V always works. |
| MIDI, local network, window placement, fonts, idle detection, pointer and keyboard lock, sensors, several downloads | Asked like the others | Grouped as **Other permissions** in Settings. |

Other rare requests (virtual reality, payment handlers, identity providers) are refused as before.
