# Tabs and memory

Browsers use a lot of memory, especially with many tabs. Axion looks after background tabs for you.
You decide how aggressive it is.

## The life of a background tab

A tab you are not looking at goes through up to four states:

| State | What it means |
|---|---|
| **Active** | The tab you see. |
| **Background** | Hidden, but still running. Chromium slows it down by itself. |
| **Frozen** | Hidden and paused. No scripts, timers or network until you open it again. It wakes up instantly. |
| **Unloaded** | The page is closed to free its memory. Axion keeps the address and a picture. It reloads when you open it. |

The rules are in **Settings > Tabs & Memory**, in the **Memory** section.

| Setting | Effect | Default |
|---|---|---|
| **Freeze background tabs after** | Freezes a tab after this long in the background. Move the slider to **Never** to turn it off. | 10 min |
| **Unload background tabs after** | Unloads a tab after this long in the background. **Never** turns it off. | 2 h |
| **Unload tabs when memory runs over the budget** | Unloads background tabs while Axion uses more than the budget. | On |
| **Memory budget** | The budget for the whole browser. **Automatic** is 40 % of your computer's memory. | Automatic |
| **Keep tabs playing audio awake** | Never freeze or unload a tab that plays sound or video. | On |
| **Reopen last session's tabs at start** | Restores your tabs on the next start. They load when you first look at them. | On |

The last line of the page shows the memory in use right now and the current budget, so you can see
the effect of your choices.

### How the memory budget works

Axion measures Chromium's memory use every few seconds. When it is above the budget, Axion unloads
the background tab that has been hidden the longest, then measures again. It unloads one tab per
check, so it stops as soon as you are back under the budget.

Set a fixed amount with the **Memory budget** slider (steps of 256 MB, up to 6 GB or your computer's
memory), or move it to the left for **Automatic**. To turn the budget off completely, switch off
**Unload tabs when memory runs over the budget**.

## Tabs that are never frozen or unloaded

- A tab that is **playing audio or video** (while **Keep tabs playing audio awake** is on). This also
  covers the video in Picture in Picture.
- A tab that is **still loading**.
- A tab with **developer tools open**.
- The tab you are looking at.

## Pinned tabs

Pin a tab with **Alt+P**. A pinned tab is small and stays at the start of the tab
bar. Pinned tabs are never closed by the **Close tabs** option below. They follow the same freezing
and unloading rules as other tabs.

## Close tabs you forgot about

**Settings > Tabs & Memory > Close tabs** closes tabs you have not looked at for a day, a week or a
month. The default is **Manually**, which never closes anything. Pinned tabs and tabs playing media
are kept. This only applies to normal tabs, not private ones.

## Closing a tab

A closed tab shrinks and fades away in the tab bar, the sidebar and the tab overview while the
tabs beside it slide into the gap. The page of the new current tab shows at once. When you close
tabs with the pointer in the tab bar, the other tabs keep their widths until the pointer leaves the
bar, so the next close button stays under the pointer. Turn the motion off with **Animate closing
tabs**, the width holding with **Keep tab widths while closing**. **Reduce motion** and the flat
glass style keep only a short fade.

## Related options

- **Open new tabs in background** keeps your current page in front when you open links in new tabs.
- **Reopen last session's tabs at start** is the session restore.
- Private tabs are described in [Accounts and privacy](accounts-and-privacy.md).

For the exact keys see the [Settings reference](../settings.md#tabs--memory).
